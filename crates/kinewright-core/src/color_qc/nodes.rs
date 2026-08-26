//! CC6 §3.7: per-node clipping attribution by removal.
//!
//! **The §3 purity claim does not extend to this module.** Everything here
//! renders through an [`Analysis`] backend and applies an [`Operation`] to a
//! *cloned* document. It is optional, bounded, single-frame, and never on
//! [`super::measure_color_qc`]'s path.
//!
//! **Method: removal, not bypass.** Each candidate node is attributed by
//! removing its effect from a scratch clone with
//! [`Operation::RemoveEffect`] — the same operation the inspector's Remove
//! button sends — and re-measuring. Removal is used because
//! `primary_correction` has no `bypass` parameter, so a bypass-based method
//! could not attribute the most common colour node in the tree, and adding the
//! parameter is forbidden. [`super::ColorQcNodeContributions::attribution`] is
//! therefore always [`super::NODE_ATTRIBUTION_REMOVED`].
//!
//! **Cost, stated rather than hidden.** At most
//! [`super::MAX_QC_NODE_CONTRIBUTIONS`] scratch renders plus one baseline —
//! seventeen full-resolution renders — plus up to sixteen `Arc<Document>` deep
//! clones, which is the real memory cost.
//!
//! **The live document is never touched.** Every mutation happens on a clone.

use std::sync::Arc;

use crate::{
    Analysis, ClipId, Document, EffectId, MediaError, Operation, TimeCode, classify_color_node,
    color_node_inactive_reason,
};

use super::{
    ColorNodeQcContribution, ColorQcNodeContributions, ColorQcReport, ColorQcRequest,
    MAX_QC_NODE_CONTRIBUTIONS, NODE_ATTRIBUTION_REMOVED, measure_color_qc, validate_node_budget,
};

/// One candidate colour node in the core-owned document order.
#[derive(Debug, Clone)]
struct Candidate {
    clip: ClipId,
    effect: EffectId,
    node_kind: String,
    active: bool,
    inactive_reason: Option<String>,
}

/// Attribute the region's clipping to the colour nodes that cause it.
///
/// **Ordering**, normative and core-owned: document track order, then clip
/// order within a track, then effect-chain order within a clip. Core cannot
/// depend on `kinewright-media`, so it must not reach for `visual_layers_at`; a
/// media fixture asserts this ordering agrees with production z-order on a
/// multi-track document, so the two cannot drift.
///
/// Candidates are the colour nodes on the clip that is on screen on each video
/// track at `at`. Every candidate is measured, including inactive ones:
/// removing something already inactive must produce a zero delta, and that is
/// asserted rather than assumed.
///
/// Beyond `request.max_nodes` candidates the list is truncated in the stated
/// order, with `truncated` and `considered_node_count` reported.
/// [`super::attach_node_contributions`] turns that into the
/// `qc_per_node_truncated` exception.
///
/// # Errors
///
/// Returns a media error when the node budget is out of range, a working proof
/// cannot be rendered, a scratch removal is rejected, or a measurement is
/// refused. Every typed [`super::ColorQcError`] travels as
/// [`MediaError::ColorQc`], so a caller recovers `code`, `field`, `observed`,
/// `allowed_values`, and `recovery_action` structurally rather than by parsing
/// the rendered message. That includes the scratch [`Operation::RemoveEffect`]
/// rejection: it is a document-model failure rather than a measurement refusal,
/// so it carries its own
/// [`super::ColorQcError::NodeRemovalRejected`] code and says so in its own
/// words, rather than being flattened into a [`MediaError::Backend`] string an
/// agent surface could only report as an unavailable working proof.
// `Arc<Document>` is taken by value to match
// `Analysis::working_proof_for_document`'s own signature, so the caller's
// reference-count bump is explicit at the call site rather than hidden here.
#[allow(clippy::needless_pass_by_value)]
pub fn measure_node_contributions(
    analysis: &dyn Analysis,
    document: Arc<Document>,
    at: TimeCode,
    request: &ColorQcRequest,
) -> Result<ColorQcNodeContributions, MediaError> {
    contributions_with_baseline(analysis, document, at, request).map(|(_, nodes)| nodes)
}

/// The baseline report and the attribution, so a caller that wants both pays
/// for exactly one baseline render rather than two.
#[allow(clippy::needless_pass_by_value)]
fn contributions_with_baseline(
    analysis: &dyn Analysis,
    document: Arc<Document>,
    at: TimeCode,
    request: &ColorQcRequest,
) -> Result<(ColorQcReport, ColorQcNodeContributions), MediaError> {
    validate_node_budget(request.max_nodes).map_err(MediaError::ColorQc)?;
    let baseline_proof = analysis.working_proof_for_document(Arc::clone(&document), at)?;
    let baseline = measure_color_qc(&baseline_proof, request).map_err(MediaError::ColorQc)?;
    let baseline_range = baseline.range.clamped_basis_points;
    let baseline_gamut = baseline.gamut.out_of_gamut_basis_points;

    let candidates = candidates_at(&document, at);
    let considered = u32::try_from(candidates.len()).unwrap_or(u32::MAX);
    let limit = usize::from(request.max_nodes).min(MAX_QC_NODE_CONTRIBUTIONS);
    let truncated = candidates.len() > limit;

    let mut nodes = Vec::with_capacity(limit.min(candidates.len()));
    for candidate in candidates.into_iter().take(limit) {
        let mut scratch = document.as_ref().clone();
        Operation::RemoveEffect {
            clip: candidate.clip,
            effect: candidate.effect,
        }
        .apply(&mut scratch)
        .map_err(|error| {
            MediaError::ColorQc(super::ColorQcError::NodeRemovalRejected {
                clip: candidate.clip,
                effect: candidate.effect,
                reason: error.to_string(),
            })
        })?;
        let removed_proof = analysis.working_proof_for_document(Arc::new(scratch), at)?;
        let removed = measure_color_qc(&removed_proof, request).map_err(MediaError::ColorQc)?;
        nodes.push(ColorNodeQcContribution {
            clip: candidate.clip,
            effect: candidate.effect,
            node_kind: candidate.node_kind,
            active: candidate.active,
            inactive_reason: candidate.inactive_reason,
            range_basis_points_delta: delta(baseline_range, removed.range.clamped_basis_points),
            gamut_basis_points_delta: delta(
                baseline_gamut,
                removed.gamut.out_of_gamut_basis_points,
            ),
        });
    }

    Ok((
        baseline,
        ColorQcNodeContributions {
            baseline_range_basis_points: baseline_range,
            baseline_gamut_basis_points: baseline_gamut,
            considered_node_count: considered,
            truncated,
            attribution: NODE_ATTRIBUTION_REMOVED.to_owned(),
            nodes,
        },
    ))
}

/// Measure a report and its per-node attribution in one call.
///
/// Convenience for the agent and app surfaces so the truncation exception and
/// the report assembly stay in one place.
///
/// # Errors
///
/// Returns a media error for the same reasons as
/// [`measure_node_contributions`].
#[allow(clippy::needless_pass_by_value)]
pub fn measure_color_qc_with_nodes(
    analysis: &dyn Analysis,
    document: Arc<Document>,
    at: TimeCode,
    request: &ColorQcRequest,
) -> Result<ColorQcReport, MediaError> {
    let (mut report, contributions) = contributions_with_baseline(analysis, document, at, request)?;
    super::attach_node_contributions(&mut report, contributions);
    Ok(report)
}

/// `with-all` minus `with-this-node-removed`, bounded by basis points.
fn delta(baseline: u32, removed: u32) -> i32 {
    let baseline = i64::from(baseline);
    let removed = i64::from(removed);
    i32::try_from(baseline - removed).unwrap_or(i32::MAX)
}

/// Every colour node on screen at `at`, in track, clip, then effect order.
///
/// **The sorted invariant, and why the search does not lean on it.** The
/// editing operations keep each track's `clips` ordered by `timeline_start`
/// with no overlap, so at most one clip on a video track contains `at` and a
/// scan may stop at the first clip that starts after it. That invariant is a
/// property of the operations, not of the type: nothing in [`Document`]
/// enforces it, and a document that arrives by deserialization, by a fixture,
/// or by a future operation can present a track in another order. An early
/// `break` would then silently attribute *no* node on that track — a missing
/// measurement reported as a clean one — so the on-screen clip is found with
/// `find` over the whole track instead. On a sorted track the result is
/// identical; the cost is a scan of a clip list, which is negligible beside
/// the full-resolution render each candidate already pays for.
fn candidates_at(document: &Document, at: TimeCode) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for track in document
        .tracks
        .iter()
        .filter(|track| track.kind == crate::TrackKind::Video)
    {
        let on_screen = track.clips.iter().find(|clip| {
            if at < clip.timeline_start {
                return false;
            }
            let Ok(duration) = document.clip_duration(clip) else {
                return false;
            };
            let Some(end) = clip.timeline_start.checked_add(duration) else {
                return false;
            };
            at < end
        });
        if let Some(clip) = on_screen {
            let local = at
                .checked_sub(clip.timeline_start)
                .unwrap_or(TimeCode::ZERO);
            for effect in &clip.effects {
                // CC3 §3.3: keyframes resolve first, then inactivity is tested
                // on the stored integers.
                let evaluated = effect.evaluated_at(local);
                let Some(kind) = classify_color_node(&evaluated) else {
                    continue;
                };
                let reason = color_node_inactive_reason(&evaluated);
                candidates.push(Candidate {
                    clip: clip.id,
                    effect: effect.id,
                    node_kind: kind.effect_name().to_owned(),
                    active: reason.is_none(),
                    inactive_reason: reason.map(|reason| reason.as_str().to_owned()),
                });
            }
        }
    }
    candidates
}
