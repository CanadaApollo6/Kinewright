//! CC7 media gates for `docs/CC7-WORKFLOW-EVALUATION.md` §4, §4.1, §4.2 and
//! §11.2 items 12b and 19–29.
//!
//! These fixtures live in the media crate for the reason every earlier
//! `ccN_fixtures.rs` does: the managed decode, the `Rgba16Float` working
//! surface, the production compositor, the LUT store, and the delivery export
//! are internal seams, and the evidence has to exercise the real path rather
//! than a public re-implementation of it.
//!
//! What this file owns:
//!
//! * the §11.1 documents — every scenario's **canonical** document, built by
//!   applying `kinewright_core::cc7_scenarios`' canonical operations to a
//!   document over `kinewright_media::cc7_sources`' rasters;
//! * §4(a) — the post-match neutral spread and chart luma mean, the skin band
//!   that survives the match, and the compositor-layer render parity case;
//! * §4(b) — the C1 and C2 proposals, the temperature clamp, and the single
//!   `delivery_range_excursion` Warning with its per-node attribution;
//! * §4(c) — the log inverse landing inside `CC7_LOG_INVERSE_MAX_CODE`, and
//!   the lattice-size sweep;
//! * §4(d) — qualifier containment, media containment, skin-hue stability, and
//!   the (d2) window-only feather band;
//! * §4(e) — the warm look's exact `deep_shadow` out-of-gamut count;
//! * §4(f) — the source-side containment of the analytic square by the 1.5×
//!   window at the smoothed keyframe centres;
//! * §4(g) — the production export and `verify_delivery_output` at **both**
//!   delivery depths for every scenario, and the starved-encode failing
//!   direction;
//! * §11.2.12b — the cross-check that keeps `cc7_scenarios`' own `f64`
//!   transcriptions honest against `color_pipeline`'s real functions;
//! * the constant half of `cc7_manifest_declares_every_required_fixture_and_constant`.
//!
//! Everything else CC7 declares is owned by the file that owns the code it
//! measures: `crates/kinewright-core/tests/cc7_core.rs` (§11.2 items 1–11),
//! `cc7_sources.rs` (§3.5's seven non-vacuity fixtures, including §11.2.13's
//! A1 guard), `crates/kinewright-agent/tests/mcp_server.rs` (§5), the app
//! (§6), and the eval binary (§7).
//!
//! # Rule 11.0.1
//!
//! No expected value in this file is obtained by calling `measure_color_qc`,
//! `match_parameters`, `bt709_limited_ycbcr`, `encode_bt709`,
//! `decode_display709`, `grade709_decode`, `matte_coverage_statistics`, the
//! compositor, or swscale. Every expectation is a `cc7_scenarios` constant or
//! an independent `f64` transcription written here with the module and
//! contract section that owns it named in a comment. The planner replica in
//! [`cc7_match_proposal`] is such a transcription: `match_parameters` lives in
//! `crates/kinewright-agent/src/color_scopes.rs`, which the media crate cannot
//! see at all, and CC7 §5.1(4) states that the canonical planner values are
//! regression pins from exactly this replica.
//!
//! # Rule 11.0.6
//!
//! Every GPU fixture here runs `fallback_gpu()` in the default lane, with an
//! `#[ignore]` `hardware_gpu()` twin for the parity gate.
//! `fixture_gpu_or_skip` and `KINEWRIGHT_GPU_TESTS_MAY_SKIP` never appear.

#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::excessive_precision)]
#![allow(clippy::float_cmp)]
#![allow(clippy::items_after_statements)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::uninlined_format_args)]

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use half::f16;
use kinewright_core::{
    Analysis, AssetId, Clip, ClipContent, ClipId, ColorBitDepth, ColorMatrix, ColorPrimaries,
    ColorQcCheck, ColorQcReport, ColorQcRequest, ColorRange, ColorTransfer, DeliveryEncodeDepth,
    DeliveryProfile, DeliveryVerification, DeliveryVerificationRequest, Document, Effect, EffectId,
    ExportCancellation, LutAsset, LutAssetId, Operation, ParamValue, QaSeverity, Rational,
    RgbaImage, TimeCode, Track, TrackId, TrackKind, apply_batch,
    cc7_scenarios::{
        CC7_A_OPERATIONS, CC7_B1_OPERATIONS, CC7_B2_OPERATIONS, CC7_CANDIDATE_CLIP_ID,
        CC7_CHART_BAND_ROI, CC7_CHART_PATCHES, CC7_D2_CONTINUOUS_AREA_WRONG_MODEL_PIXELS_TENTHS,
        CC7_D2_FEATHER_COUNTS_PIXELS, CC7_D2_WINDOW_CENTRE_BASIS_POINTS,
        CC7_D2_WINDOW_HALF_EXTENTS_BASIS_POINTS, CC7_DEEP_SHADOW_ROI,
        CC7_DELIVERY_ALLOWED_INFO_CODES, CC7_FEATHER_BASIS_POINTS,
        CC7_FEATHER_PARTIAL_TOLERANCE_PIXELS, CC7_LOG_CUBE_BYTES_REPORTED, CC7_LOG_CUBE_SIZE,
        CC7_LOG_CUBE_SIZE_LADDER, CC7_LOG_CUBE_TITLE, CC7_LOG_IDENTITY_CUBE_REPORTED_CODE,
        CC7_LOG_INVERSE_MAX_CODE, CC7_LOOK_DEEP_SHADOW_OUT_OF_GAMUT_PIXELS,
        CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS, CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE,
        CC7_MATCH_PROPOSAL_B, CC7_MATCH_PROPOSAL_C1, CC7_MATCH_PROPOSAL_C2,
        CC7_MATTE_OUTSIDE_CHANGED_PIXELS_MAX, CC7_MEASURED_CORRECTED_C2_LUMA_MEAN_CODE_MILLIONTHS,
        CC7_MEASURED_FEATHER_MODEL_ERROR_PIXELS, CC7_MEASURED_MATCH_LUMA_MEAN_CODE_MILLIONTHS,
        CC7_MEASURED_MATCH_NEUTRAL_SPREAD_CODE, CC7_MEASURED_UNMATCHED_B_LUMA_MEAN_CODE_MILLIONTHS,
        CC7_MEASURED_UNMATCHED_B_SPREAD_CODE, CC7_PRIMARY_PATCHES, CC7_PRODUCT_PATCH_PIXEL_COUNT,
        CC7_PRODUCT_RED_ROI, CC7_ROW_PATCHES, CC7_SCENARIOS, CC7_SCOPE_SIXTEEN_BIT_SCALE,
        CC7_SINGLE_CLIP_ID, CC7_SKIN_BAND_ROI, CC7_SKIN_IN_BAND_EXACT_BASIS_POINTS, CC7_SOURCE_FPS,
        CC7_SOURCE_FRAMES, CC7_SOURCE_HEIGHT, CC7_SOURCE_WIDTH,
        CC7_TRACK_ANALYTIC_CENTRES_BASIS_POINTS,
        CC7_TRACK_CONTAINMENT_REQUIRED_HALF_HEIGHT_PIXELS_REPORTED,
        CC7_TRACK_CONTAINMENT_REQUIRED_HALF_WIDTH_PIXELS_REPORTED,
        CC7_TRACK_CONTAINMENT_WORST_MARGIN_X_PIXELS_HUNDREDTHS,
        CC7_TRACK_CONTAINMENT_WORST_MARGIN_Y_PIXELS_HUNDREDTHS, CC7_TRACK_LAGGING_FINAL_KEYFRAME,
        CC7_TRACK_SEEDED_WINDOW_HALF_HEIGHT_BASIS_POINTS,
        CC7_TRACK_SEEDED_WINDOW_HALF_WIDTH_BASIS_POINTS, CC7_TRACK_SQUARE_SIZE,
        CC7_TRACK_SURVIVING_SAMPLE_FRAMES, CC7_TRACK_WINDOW_HALF_HEIGHT_BASIS_POINTS,
        CC7_TRACK_WINDOW_HALF_WIDTH_BASIS_POINTS, CC7_UNRECOVERABLE_RESIDUAL_SPREAD_REPORTED_CODE,
        Cc7Camera, Cc7Operation, Cc7Patch, Cc7PixelRect, Cc7Scenario, cc7_b1_canonical_operations,
        cc7_canonical_operations, cc7_d2_canonical_operations, cc7_decode_display709,
        cc7_encode_bt709, cc7_grade709_decode, cc7_log_lut_asset,
        cc7_lut_backed_canonical_operations, cc7_spec, cc7_track_keyframe_centres,
    },
    cc7_scenarios::{
        CC7_B1_RESIDUAL_SPREAD_MAX_CODE, CC7_C2_OVER_RANGE_BASIS_POINTS_REPORTED,
        CC7_C2_OVER_RANGE_PIXELS_REPORTED, CC7_LOG_BLACK_PATCH_REPORTED_CODE,
        CC7_LOG_PRIMARY_REPORTED_CODE, CC7_LOOK_BLUE_ZERO_CROSSING_DISPLAY709_MILLIONTHS,
        CC7_LOOK_MIX_BASIS_POINTS, CC7_MEASURED_B1_RESIDUAL_SPREAD_CODE,
        CC7_MEASURED_LOG_INVERSE_CODE, CC7_MEASURED_UNCORRECTED_C1_SPREAD_CODE,
        CC7_SECONDARY_SATURATION_PERCENT, CC7_WARM_WHOLE_RASTER_OUT_OF_GAMUT_BASIS_POINTS,
        CC7_WARM_WHOLE_RASTER_OUT_OF_GAMUT_PIXELS_REPORTED,
    },
    matte_coverage_statistics, measure_color_qc, measure_scopes,
};
use serde_json::{Value, json};

use crate::{
    Compositor, CompositorLayer,
    cc1_fixtures::{
        FixtureGpu, assert_linear_parity, backend_metadata, fallback_gpu, git_revision,
        hardware_gpu, linear_parity_metrics, working_frame, write_evidence_artefact,
    },
    cc7_sources::{Cc7SourceKind, cc7_source, write_identity_cube, write_log_like_inverse_cube},
    color_pipeline::{decode_display709, encode_bt709, grade709_decode},
    decode::probe_path,
    frame::WorkingFrame,
    initialize_ffmpeg,
    lut_store::{LUT_MAX_FILE_BYTES, LutLibrary, LutStore},
    test_support::TempDirectory,
    timeline::TransitionRenderParams,
};

/// The contract token recorded on every CC7 evidence payload and asserted
/// against the manifest.
pub(crate) const CC7_CONTRACT: &str = "cc7_workflow_evaluation";

/// The 2-pixel inset §4 mandates for every patch statistic, so a patch edge
/// never enters a mean.
const CC7_PATCH_INSET_PIXELS: u32 = 2;

// ===========================================================================
// §11.1: the rasters, the documents, and the committed canonical documents.
// ===========================================================================

/// One scenario's generated rasters plus the document they sit in.
///
/// The `GeneratedMedia` values are held for the lifetime of the scene because
/// they delete their `.mkv` on `Drop`.
struct Cc7Scene {
    /// Held only for its `Drop`, which deletes the generated `.mkv`.
    _media: Vec<crate::test_support::GeneratedMedia>,
    document: Document,
}

impl Cc7Scene {
    /// Build a document over one raster per `kind`, in timeline order.
    fn over(label: &str, kinds: &[Cc7SourceKind]) -> Self {
        initialize_ffmpeg().expect("FFmpeg must initialize for a CC7 media fixture");
        let media = kinds
            .iter()
            .map(|kind| cc7_source(*kind))
            .collect::<Vec<_>>();
        let mut assets = Vec::with_capacity(media.len());
        let mut clips = Vec::with_capacity(media.len());
        let mut timeline_start = 0_i64;
        for (index, (generated, kind)) in media.iter().zip(kinds).enumerate() {
            let id = AssetId(index as u64 + 1);
            let asset = probe_path(generated.path(), id)
                .unwrap_or_else(|error| panic!("{label}: the CC7 source should probe: {error}"));
            // CC1 refuses an untagged source, so the mux recipe's tags are
            // asserted rather than assumed (CC7 §3.2).
            assert_eq!(asset.color_description.primaries, ColorPrimaries::Bt709);
            assert_eq!(asset.color_description.transfer, ColorTransfer::Bt709);
            assert_eq!(asset.color_description.matrix, ColorMatrix::Bt709);
            assert_eq!(asset.color_description.range, ColorRange::Limited);
            assert_eq!(asset.color_description.bit_depth, ColorBitDepth::Eight);
            assert_eq!(
                asset.resolution,
                Some((CC7_SOURCE_WIDTH, CC7_SOURCE_HEIGHT)),
                "{label}: CC7 §2.3.1 pins 320 x 180"
            );
            let frames = i64::from(kind.frames());
            assert_eq!(asset.duration, TimeCode(frames));
            clips.push(Clip {
                id: ClipId(index as u64 + 1),
                asset: id,
                source_range: TimeCode::ZERO..TimeCode(frames),
                content: ClipContent::Media,
                timeline_start: TimeCode(timeline_start),
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            });
            assets.push(asset);
            timeline_start += frames;
        }
        let document = Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips,
            }],
            media_pool: assets,
            fps: Rational::new(CC7_SOURCE_FPS, 1).expect("25 fps"),
            resolution: (CC7_SOURCE_WIDTH, CC7_SOURCE_HEIGHT),
            duration: TimeCode(timeline_start),
            ..Document::default()
        };
        document
            .validate()
            .unwrap_or_else(|error| panic!("{label}: the CC7 document should validate: {error}"));
        Self {
            _media: media,
            document,
        }
    }

    /// The uncommitted document: no effect anywhere.
    fn bare(&self) -> Arc<Document> {
        Arc::new(self.document.clone())
    }

    /// Apply one batch through core and return the committed document.
    fn commit(&self, label: &str, operations: &[Operation]) -> Arc<Document> {
        let mut document = self.document.clone();
        apply_batch(&mut document, operations)
            .unwrap_or_else(|error| panic!("{label}: core must accept the batch: {error}"));
        document.validate().unwrap_or_else(|error| {
            panic!("{label}: the committed document must validate: {error}")
        });
        Arc::new(document)
    }
}

/// The two-clip (a)/(b) scene: camera A as clip 1, `candidate` as clip 2.
fn cc7_two_clip_scene(label: &str, candidate: Cc7Camera) -> Cc7Scene {
    Cc7Scene::over(
        label,
        &[
            Cc7SourceKind::Camera(Cc7Camera::A),
            Cc7SourceKind::Camera(candidate),
        ],
    )
}

/// The single-clip camera-A scene (d), (d2), (e), (g).
fn cc7_camera_a_scene(label: &str) -> Cc7Scene {
    Cc7Scene::over(label, &[Cc7SourceKind::Camera(Cc7Camera::A)])
}

/// The project frame a scenario's canonical node is measured at: clip 2's
/// first frame on the two-clip documents, project frame 0 otherwise.
fn cc7_target_frame(scenario: Cc7Scenario) -> TimeCode {
    match scenario {
        Cc7Scenario::MixedCamera | Cc7Scenario::WhiteBalance => {
            TimeCode(i64::from(CC7_SOURCE_FRAMES))
        }
        _ => TimeCode::ZERO,
    }
}

/// The production media engine on one lane.
fn cc7_engine(gpu: &FixtureGpu) -> crate::engine::FfmpegMediaEngine {
    crate::engine::FfmpegMediaEngine::new_with_gpu(gpu.context())
        .expect("the production media engine should start on the fixture adapter")
}

/// One full-resolution managed monitor proof, asserted full-raster.
fn cc7_monitor_raster(
    engine: &crate::engine::FfmpegMediaEngine,
    document: &Arc<Document>,
    at: TimeCode,
    label: &str,
) -> RgbaImage {
    let proof = engine
        .monitor_proof_for_document(Arc::clone(document), at)
        .unwrap_or_else(|error| panic!("{label}: the CC7 monitor proof should render: {error}"));
    assert!(
        proof.metadata.full_resolution,
        "{label}: a proof that is not full-raster cannot carry a CC7 claim"
    );
    assert_eq!(
        (proof.image.width, proof.image.height),
        (CC7_SOURCE_WIDTH, CC7_SOURCE_HEIGHT)
    );
    proof.image
}

/// One full-resolution scene-linear working proof.
fn cc7_working_proof(
    engine: &crate::engine::FfmpegMediaEngine,
    document: &Arc<Document>,
    at: TimeCode,
    label: &str,
) -> kinewright_core::WorkingProof {
    let proof = engine
        .working_proof_for_document(Arc::clone(document), at)
        .unwrap_or_else(|error| panic!("{label}: the CC7 working proof should render: {error}"));
    assert!(proof.metadata.render.full_resolution, "{label}");
    proof
}

// ===========================================================================
// §4: raster statistics. Every one is taken on the 2-pixel inset §4 mandates.
// ===========================================================================

/// The half-open pixel rect `rect` inset by [`CC7_PATCH_INSET_PIXELS`] on
/// every side.
fn cc7_inset(rect: Cc7PixelRect) -> Cc7PixelRect {
    let inset = CC7_PATCH_INSET_PIXELS;
    assert!(
        rect.width > 2 * inset && rect.height > 2 * inset,
        "a CC7 patch must survive its own 2-pixel inset: {rect:?}"
    );
    Cc7PixelRect::new(
        rect.x + inset,
        rect.y + inset,
        rect.width - 2 * inset,
        rect.height - 2 * inset,
    )
}

/// The per-channel mean of one rect on an RGBA8 raster, in display codes.
fn cc7_rect_mean_codes(image: &RgbaImage, rect: Cc7PixelRect) -> [f64; 3] {
    let mut sums = [0.0_f64; 3];
    let mut count = 0_u64;
    for y in rect.y..rect.y + rect.height {
        for x in rect.x..rect.x + rect.width {
            let index = ((y * image.width + x) * 4) as usize;
            for (channel, sum) in sums.iter_mut().enumerate() {
                *sum += f64::from(image.pixels[index + channel]);
            }
            count += 1;
        }
    }
    assert!(count > 0, "an empty CC7 rect measures nothing: {rect:?}");
    sums.map(|sum| sum / count as f64)
}

/// CC7 §10.1's rounding rule, half away from zero.
fn cc7_round(value: f64) -> i64 {
    if value >= 0.0 {
        (value + 0.5).floor() as i64
    } else {
        (value - 0.5).ceil() as i64
    }
}

/// One patch's mean, rounded to display codes.
fn cc7_patch_codes(image: &RgbaImage, patch: &Cc7Patch) -> [i64; 3] {
    cc7_rect_mean_codes(image, cc7_inset(patch.rect)).map(cc7_round)
}

/// §4(a)(2)'s statistic: `max over the twelve achromatic patches of
/// max(|R − G|, |G − B|)`, with the worst patch's index.
fn cc7_chart_neutral_spread(image: &RgbaImage) -> (i64, usize) {
    let mut worst = (0_i64, 0_usize);
    for (index, patch) in CC7_CHART_PATCHES.iter().enumerate() {
        let codes = cc7_patch_codes(image, patch);
        let spread = (codes[0] - codes[1]).abs().max((codes[1] - codes[2]).abs());
        if spread > worst.0 {
            worst = (spread, index);
        }
    }
    worst
}

/// §4(a)(3)'s statistic: the BT.709-weighted mean of the chart band's display
/// codes, in code millionths.
///
/// The BT.709 luma weights are transcribed here rather than imported: they are
/// the same three numbers `crates/kinewright-agent/src/color_scopes.rs:70-72`
/// carries, and the media crate cannot see that module (CC7 §11.0.1).
const CC7_BT709_LUMA_WEIGHTS: [f64; 3] = [0.212_6, 0.715_2, 0.072_2];

fn cc7_chart_band_luma_mean_codes(image: &RgbaImage) -> f64 {
    // §2.1: the chart band's rect is derived from `cc7_scenarios`' own ROI,
    // never restated as a literal here (R4-m1).
    let means = cc7_rect_mean_codes(image, cc7_chart_band_rect());
    CC7_BT709_LUMA_WEIGHTS[0] * means[0]
        + CC7_BT709_LUMA_WEIGHTS[1] * means[1]
        + CC7_BT709_LUMA_WEIGHTS[2] * means[2]
}

/// `mean(luma_candidate) − mean(luma_reference)` in code millionths.
fn cc7_chart_luma_mean_delta_millionths(reference: &RgbaImage, candidate: &RgbaImage) -> i64 {
    cc7_round(
        (cc7_chart_band_luma_mean_codes(candidate) - cc7_chart_band_luma_mean_codes(reference))
            * 1_000_000.0,
    )
}

/// The per-channel chroma spread of the skin row: `max − min` over the four
/// skin patches' channel means, in display codes (§4(a)(4)).
fn cc7_skin_row_chroma_spread(image: &RgbaImage) -> f64 {
    let mut spread = 0.0_f64;
    for patch in CC7_ROW_PATCHES.iter().take(4) {
        let means = cc7_rect_mean_codes(image, cc7_inset(patch.rect));
        let high = means[0].max(means[1]).max(means[2]);
        let low = means[0].min(means[1]).min(means[2]);
        spread += high - low;
    }
    spread
}

// ---------------------------------------------------------------------------
// The planner replica (CC7 §5.1(4), R-M8).
// ---------------------------------------------------------------------------

/// `bt709_eotf`, transcribed independently from
/// `crates/kinewright-agent/src/color_scopes.rs:62-103`, which the media crate
/// cannot depend on. This is the decode the planner applies to its **display
/// code means**; it is deliberately the agent's rounded-constant form and not
/// `color_pipeline::decode_display709`.
fn cc7_planner_eotf(encoded: f64) -> f64 {
    let encoded = encoded.clamp(0.0, 1.0);
    if encoded < 0.081 {
        encoded / 4.5
    } else {
        ((encoded + 0.099) / 1.099).powf(1.0 / 0.45)
    }
}

/// `CC1_WHITE_BALANCE_GAIN_PER_PERCENT` (`color_scopes.rs:86`).
const CC7_WHITE_BALANCE_GAIN_PER_PERCENT: f64 = 0.001;

/// The planner's `ShotStats` over one ROI of one monitor raster.
#[derive(Debug, Clone, Copy)]
struct Cc7ShotStats {
    /// The planner's own field, kept so the transcription is the same shape as
    /// `ShotStats`; the proposal reads `linear` and `linear_luma` only.
    #[allow(dead_code)]
    means: [f64; 4],
    linear: [f64; 3],
    linear_luma: f64,
}

fn cc7_shot_stats(image: &RgbaImage, rect: Cc7PixelRect) -> Cc7ShotStats {
    let means = cc7_rect_mean_codes(image, rect);
    let luma = CC7_BT709_LUMA_WEIGHTS[0] * means[0]
        + CC7_BT709_LUMA_WEIGHTS[1] * means[1]
        + CC7_BT709_LUMA_WEIGHTS[2] * means[2];
    let linear = means.map(|value| cc7_planner_eotf(value / 255.0));
    let linear_luma = CC7_BT709_LUMA_WEIGHTS[0] * linear[0]
        + CC7_BT709_LUMA_WEIGHTS[1] * linear[1]
        + CC7_BT709_LUMA_WEIGHTS[2] * linear[2];
    Cc7ShotStats {
        means: [means[0], means[1], means[2], luma],
        linear,
        linear_luma,
    }
}

/// One control of a replicated `plan_shot_match` proposal.
#[derive(Debug, Clone, Copy)]
struct Cc7ProposedControl {
    /// The planner's own field. With `existing = None` it equals `requested`,
    /// which the (b2) clamp gate reads; it is kept so the transcription is the
    /// same shape as `match_parameters`' published control.
    #[allow(dead_code)]
    delta: i64,
    requested: i64,
    value: i64,
    clamped: bool,
    minimum: i64,
    maximum: i64,
    unrounded_delta: f64,
}

/// The replicated proposal: `exposure_milli_stops`, `temperature_percent`,
/// `tint_percent`, each present only when its rounded delta is non-zero
/// (R-M19: an absent key means *not proposed*, never *zero*).
#[derive(Debug, Clone, Default)]
struct Cc7Proposal {
    controls: BTreeMap<&'static str, Cc7ProposedControl>,
}

impl Cc7Proposal {
    fn get(&self, name: &str) -> Cc7ProposedControl {
        *self
            .controls
            .get(name)
            .unwrap_or_else(|| panic!("the replicated proposal must carry {name}: {self:?}"))
    }

    fn value(&self, name: &str) -> i64 {
        self.get(name).value
    }

    /// The proposal as the `primary_correction` parameter list a commit lands.
    fn parameters(&self) -> Vec<(String, i64)> {
        self.controls
            .iter()
            .map(|(name, control)| ((*name).to_owned(), control.value))
            .collect()
    }
}

/// `match_parameters`, transcribed verbatim from
/// `crates/kinewright-agent/src/color_scopes.rs:1866-1962`, with `existing =
/// None` (CC7's scripts always propose onto an ungraded candidate).
fn cc7_match_proposal(reference: Cc7ShotStats, candidate: Cc7ShotStats) -> Cc7Proposal {
    let ratio = |reference: f64, candidate: f64| -> Option<f64> {
        (reference > 0.0 && candidate > 0.0 && reference.is_finite() && candidate.is_finite())
            .then(|| reference / candidate)
    };
    let exposure =
        ratio(reference.linear_luma, candidate.linear_luma).map(|value| value.log2() * 1_000.0);
    let temperature = match (
        ratio(reference.linear[0], reference.linear[2]),
        ratio(candidate.linear[0], candidate.linear[2]),
    ) {
        (Some(reference_ratio), Some(candidate_ratio)) if candidate_ratio > 0.0 => Some(
            (reference_ratio / candidate_ratio - 1.0) / (2.0 * CC7_WHITE_BALANCE_GAIN_PER_PERCENT),
        ),
        _ => None,
    };
    let green_over_mid = |stats: &Cc7ShotStats| {
        let mid = f64::midpoint(stats.linear[0], stats.linear[2]);
        ratio(stats.linear[1], mid)
    };
    let tint = match (green_over_mid(&reference), green_over_mid(&candidate)) {
        (Some(reference_ratio), Some(candidate_ratio)) if candidate_ratio > 0.0 => {
            Some((1.0 - reference_ratio / candidate_ratio) / CC7_WHITE_BALANCE_GAIN_PER_PERCENT)
        }
        _ => None,
    };

    let mut proposal = Cc7Proposal::default();
    for (name, raw) in [
        ("exposure_milli_stops", exposure),
        ("temperature_percent", temperature),
        ("tint_percent", tint),
    ] {
        let Some(unrounded_delta) = raw.filter(|value| value.is_finite()) else {
            continue;
        };
        let delta = cc7_round(unrounded_delta);
        if delta == 0 {
            continue;
        }
        // `existing = None`, so `current = 0` and `requested = delta`.
        let requested = delta;
        let (minimum, maximum) = cc7_primary_bounds(name);
        let value = requested.clamp(minimum, maximum);
        proposal.controls.insert(
            name,
            Cc7ProposedControl {
                delta,
                requested,
                value,
                clamped: value != requested,
                minimum,
                maximum,
                unrounded_delta,
            },
        );
    }
    proposal
}

/// `primary_parameter_bounds` (`color_scopes.rs:1786-1794`), read from the
/// **core** descriptor exactly as the planner reads it, so the clamp CC7
/// asserts is the range core will validate against.
fn cc7_primary_bounds(name: &str) -> (i64, i64) {
    kinewright_core::effect_descriptor("primary_correction")
        .and_then(|descriptor| {
            descriptor
                .parameter(name)
                .map(|parameter| (parameter.min, parameter.max))
        })
        .unwrap_or_else(|| panic!("primary_correction must declare {name}"))
}

// ===========================================================================
// The canonical document of each scenario (§2.5), built over §3's rasters.
// ===========================================================================

/// One scenario's canonical document, the LUT library it needs, and a live
/// engine already bound to that library.
struct Cc7CanonicalPlan {
    /// Held so the generated `.mkv` files outlive the plan.
    _scene: Cc7Scene,
    /// Held so the imported `.cube` and its store outlive the plan.
    _store: Option<TempDirectory>,
    document: Arc<Document>,
    library: Arc<LutLibrary>,
    engine: crate::engine::FfmpegMediaEngine,
}

/// The raster each scenario's canonical document sits on (§2.3.4-§2.3.6).
fn cc7_scenario_source_kinds(scenario: Cc7Scenario) -> Vec<Cc7SourceKind> {
    match scenario {
        Cc7Scenario::MixedCamera => vec![
            Cc7SourceKind::Camera(Cc7Camera::A),
            Cc7SourceKind::Camera(Cc7Camera::B),
        ],
        // §2.5: the committed (b) document is (b2)'s, so clip 2 is C2.
        Cc7Scenario::WhiteBalance => vec![
            Cc7SourceKind::Camera(Cc7Camera::A),
            Cc7SourceKind::Camera(Cc7Camera::C2),
        ],
        Cc7Scenario::LogLike => vec![Cc7SourceKind::Log],
        Cc7Scenario::ProductAndSkin | Cc7Scenario::CreativeLook => {
            vec![Cc7SourceKind::Camera(Cc7Camera::A)]
        }
        Cc7Scenario::TrackedSecondary => vec![Cc7SourceKind::Tracked],
    }
}

/// Build one scenario's **canonical** document by applying
/// `cc7_canonical_operations` — or, for (c) and (e),
/// `cc7_lut_backed_canonical_operations` with the real imported or built-in
/// asset — to a document over that scenario's rasters.
fn cc7_canonical_plan(gpu: &FixtureGpu, scenario: Cc7Scenario, label: &str) -> Cc7CanonicalPlan {
    let spec = cc7_spec(scenario);
    let scene = Cc7Scene::over(
        &format!("cc7-{label}-{}", spec.id),
        &cc7_scenario_source_kinds(scenario),
    );
    let engine = cc7_engine(gpu);
    let mut store = None;
    let (operations, library) = match scenario {
        Cc7Scenario::LogLike => {
            // §4(c)(5): the (c) asset is a real import into a real project
            // store, so the six-decimal quantisation `import_lut_asset`
            // applies is inside every (c) measurement.
            let (directory, asset, library) = cc7_import_log_inverse_cube(CC7_LOG_CUBE_SIZE, false);
            engine.set_lut_library(Arc::clone(&library));
            store = Some(directory);
            (
                cc7_lut_backed_canonical_operations(scenario, asset),
                library,
            )
        }
        Cc7Scenario::CreativeLook => {
            let (asset, library) = cc7_warm_look_asset();
            engine.set_lut_library(Arc::clone(&library));
            (
                cc7_lut_backed_canonical_operations(scenario, asset),
                library,
            )
        }
        _ => (
            cc7_canonical_operations(scenario),
            Arc::new(LutLibrary::default()),
        ),
    };
    let document = scene.commit(label, &operations);
    Cc7CanonicalPlan {
        _scene: scene,
        _store: store,
        document,
        library,
        engine,
    }
}

/// Import one CC7 `.cube` into a real project store and build the verified
/// library the renderer consumes, exactly as `cc4_fixtures.rs:619-651` does.
fn cc7_import_log_inverse_cube(
    size: u32,
    identity: bool,
) -> (TempDirectory, LutAsset, Arc<LutLibrary>) {
    let directory = TempDirectory::new("cc7-cube-store");
    let store = LutStore::for_project(&directory.path("project.kinewright"))
        .expect("a saved project derives a store root");
    let source = if identity {
        write_identity_cube(directory.root(), size)
    } else {
        write_log_like_inverse_cube(directory.root(), size)
    };
    let import = store
        .import_lut_asset(&source)
        .expect("the CC7 cube must import into the project store");
    let asset = import.into_lut_asset(LutAssetId(1));
    if !identity && size == CC7_LOG_CUBE_SIZE {
        // §2.2's `cc7_log_lut_asset` pins `CC7_LOG_CUBE_SIZE`, so the record
        // comparison applies at the canonical size only. §2.2's is the record the (c) commit carries; the
        // digest and byte length are properties of the file core cannot read,
        // so the fixture proves the two agree rather than assuming it.
        assert_eq!(
            asset,
            cc7_log_lut_asset(&asset.sha256, asset.byte_len, &source.display().to_string()),
            "the imported record must be `cc7_scenarios::cc7_log_lut_asset`"
        );
        assert_eq!(asset.title, CC7_LOG_CUBE_TITLE);
        assert_eq!(asset.size, size);
    }
    let (library, statuses) = LutLibrary::build(std::slice::from_ref(&asset), Some(&store));
    for (id, status) in &statuses {
        assert_eq!(
            status.kind,
            kinewright_core::LutAvailabilityKind::Verified,
            "CC7 fixture asset {id:?} was not verified: {status:?}"
        );
    }
    (directory, asset, Arc::new(library))
}

/// The built-in `warm` look bound as (e)'s asset (§4(e)(2), probe-2 M4).
fn cc7_warm_look_asset() -> (LutAsset, Arc<LutLibrary>) {
    let warm = crate::builtin_looks::BuiltinLook::Warm.to_lut_asset(LutAssetId(1));
    let (library, statuses) = LutLibrary::build(std::slice::from_ref(&warm), None);
    for (id, status) in &statuses {
        assert_eq!(
            status.kind,
            kinewright_core::LutAvailabilityKind::Verified,
            "the built-in warm look {id:?} was not verified: {status:?}"
        );
    }
    (warm, Arc::new(library))
}

/// One `primary_correction` `InsertEffect` at CC7's canonical index and id.
fn cc7_primary_insert(clip: ClipId, parameters: &[(String, i64)]) -> Operation {
    Operation::InsertEffect {
        clip,
        index: 0,
        effect: Effect {
            id: EffectId(1),
            name: "primary_correction".to_owned(),
            parameters: parameters
                .iter()
                .map(|(name, value)| (name.clone(), ParamValue::Integer(*value)))
                .collect::<BTreeMap<_, _>>(),
            keyframes: BTreeMap::new(),
        },
    }
}

/// A canonical operation's parameter list with `overrides` applied, so a
/// failing direction differs from the canonical node in exactly the named
/// controls and in nothing else.
fn cc7_operation_with(operation: &Cc7Operation, overrides: &[(&str, i64)]) -> Vec<(String, i64)> {
    let mut parameters = operation
        .parameters
        .iter()
        .map(|(name, value)| ((*name).to_owned(), *value))
        .collect::<BTreeMap<_, _>>();
    for (name, value) in overrides {
        assert!(
            parameters.contains_key(*name),
            "{name} is not a parameter of the canonical node; a failing direction must vary a \
             control the canonical document actually carries"
        );
        parameters.insert((*name).to_owned(), *value);
    }
    parameters.into_iter().collect()
}

// ===========================================================================
// §4(a) Mixed-camera interview.
// ===========================================================================

/// Everything one (a)/(b) match measures, so the passing gate, its two
/// failing directions, and the evidence payload read the same numbers.
struct Cc7MatchMeasurement {
    reference: RgbaImage,
    unmatched: RgbaImage,
    matched: RgbaImage,
    proposal: Cc7Proposal,
    unmatched_spread: (i64, usize),
    matched_spread: (i64, usize),
    unmatched_luma_millionths: i64,
    matched_luma_millionths: i64,
}

/// Plan and commit one candidate camera's match against camera A, over the
/// twelve-patch achromatic chart band (§4(a)(2), R-M19's single shared ROI).
fn cc7_measure_match(
    gpu: &FixtureGpu,
    candidate: Cc7Camera,
    label: &str,
    committed_operations: Option<Vec<Operation>>,
) -> Cc7MatchMeasurement {
    let scene = cc7_two_clip_scene(label, candidate);
    let engine = cc7_engine(gpu);
    let bare = scene.bare();
    let reference_frame = TimeCode::ZERO;
    let candidate_frame = TimeCode(i64::from(CC7_SOURCE_FRAMES));
    let reference = cc7_monitor_raster(&engine, &bare, reference_frame, label);
    let unmatched = cc7_monitor_raster(&engine, &bare, candidate_frame, label);
    let chart = cc7_chart_band_rect();
    let proposal = cc7_match_proposal(
        cc7_shot_stats(&reference, chart),
        cc7_shot_stats(&unmatched, chart),
    );
    let operations = committed_operations.unwrap_or_else(|| {
        vec![cc7_primary_insert(
            CC7_CANDIDATE_CLIP_ID,
            &proposal.parameters(),
        )]
    });
    let committed = scene.commit(label, &operations);
    let matched = cc7_monitor_raster(&engine, &committed, candidate_frame, label);
    Cc7MatchMeasurement {
        unmatched_spread: cc7_chart_neutral_spread(&unmatched),
        matched_spread: cc7_chart_neutral_spread(&matched),
        unmatched_luma_millionths: cc7_chart_luma_mean_delta_millionths(&reference, &unmatched),
        matched_luma_millionths: cc7_chart_luma_mean_delta_millionths(&reference, &matched),
        reference,
        unmatched,
        matched,
        proposal,
    }
}

/// The chart band's pixel rect, from `cc7_scenarios`' own ROI rather than a
/// literal (§2.1: a number in that module is not restated here).
fn cc7_chart_band_rect() -> Cc7PixelRect {
    let resolved = CC7_CHART_BAND_ROI
        .to_pixels(CC7_SOURCE_WIDTH, CC7_SOURCE_HEIGHT)
        .expect("the chart band ROI resolves");
    Cc7PixelRect::new(resolved.x, resolved.y, resolved.width, resolved.height)
}

/// CC7 §11.2.19 — the (a) exit gate. §4(a)(2) and §4(a)(3): the matched
/// candidate's twelve achromatic patches are neutral to within
/// `CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE`, and its chart-band luma mean is within
/// `CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS` of the reference's.
///
/// *Fails:* `cc7_a_the_unmatched_candidate_exceeds_the_neutral_spread_budget`,
/// `cc7_a_the_unrecoverable_candidate_exceeds_the_neutral_spread_budget` and
/// `cc7_a_the_unmatched_candidate_exceeds_the_luma_mean_budget`.
#[test]
fn cc7_mixed_camera_match_meets_the_neutral_spread_and_luma_budgets() {
    let gpu = fallback_gpu();
    let measurement = cc7_measure_match(&gpu, Cc7Camera::B, "cc7-a-match", None);

    // --- the replica reproduces the canonical proposal (R-M8) ------------
    // §2.5's (a) row is a regression pin from exactly this transcription, so
    // the fixture asserts the pin rather than re-deriving it.
    assert_eq!(
        measurement.proposal.value("exposure_milli_stops"),
        CC7_MATCH_PROPOSAL_B.exposure_milli_stops
    );
    assert_eq!(
        measurement.proposal.value("temperature_percent"),
        CC7_MATCH_PROPOSAL_B.temperature_percent
    );
    assert_eq!(
        measurement.proposal.value("tint_percent"),
        CC7_MATCH_PROPOSAL_B.tint_percent
    );
    for (name, control) in &measurement.proposal.controls {
        assert!(
            !control.clamped,
            "cam B's {name} is inside the planner's authority and must not clamp: {control:?}"
        );
    }
    // The committed node is exactly §2.5's (a) row.
    assert_eq!(
        measurement.proposal.parameters(),
        CC7_A_OPERATIONS[0]
            .parameters
            .iter()
            .map(|(name, value)| ((*name).to_owned(), *value))
            .collect::<Vec<_>>(),
        "the replicated proposal must equal `CC7_A_OPERATIONS`"
    );
    // §4(a)(4): `saturation_percent` is never proposed, so the intentional
    // difference is not corrected away.
    assert!(
        !measurement
            .proposal
            .controls
            .contains_key("saturation_percent"),
        "the planner must not propose a saturation change"
    );

    // --- §4(a)(2) the spread, with its margin ----------------------------
    let (spread, patch) = measurement.matched_spread;
    assert!(
        spread <= CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE,
        "the matched candidate's worst neutral spread is {spread} codes at chart patch {patch}, \
         over the budget of {CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE}"
    );
    assert_eq!(
        spread, CC7_MEASURED_MATCH_NEUTRAL_SPREAD_CODE,
        "§4.1 pins the measured spread; a move is a re-baseline, not a pass"
    );
    assert!(
        CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE >= 2 * spread,
        "rule 11.0.5: the spread budget must keep a 2x margin"
    );
    // Camera A's own chart band is exactly neutral, which is what makes the
    // statistic meaningful (A1).
    assert_eq!(cc7_chart_neutral_spread(&measurement.reference).0, 0);

    // --- §4(a)(3) the luma mean, with its margin -------------------------
    let luma = measurement.matched_luma_millionths;
    assert!(
        luma.abs() <= CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS,
        "the matched candidate's chart luma mean delta is {luma} code millionths, over the budget \
         of {CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS}"
    );
    assert_eq!(luma, CC7_MEASURED_MATCH_LUMA_MEAN_CODE_MILLIONTHS);
    assert!(CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS >= 2 * luma.abs());

    emit_cc7_evidence(
        "cc7_mixed_camera_match_meets_the_neutral_spread_and_luma_budgets",
        &gpu,
        json!({
            "candidate": "B",
            "roi": CC7_CHART_BAND_ROI,
            "proposal": {
                "exposure_milli_stops": measurement.proposal.value("exposure_milli_stops"),
                "temperature_percent": measurement.proposal.value("temperature_percent"),
                "tint_percent": measurement.proposal.value("tint_percent"),
            },
        }),
        json!({
            "neutral_spread_code": spread,
            "neutral_spread_worst_patch": patch,
            "neutral_spread_budget": CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE,
            "chart_luma_mean_delta_millionths": luma,
            "chart_luma_mean_budget_millionths": CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS,
            "unmatched_spread_code": measurement.unmatched_spread.0,
            "unmatched_luma_millionths": measurement.unmatched_luma_millionths,
            "skin_row_chroma_spread_codes": {
                "reference": cc7_skin_row_chroma_spread(&measurement.reference),
                "matched": cc7_skin_row_chroma_spread(&measurement.matched),
                "unmatched": cc7_skin_row_chroma_spread(&measurement.unmatched),
            },
        }),
    );
}

/// CC7 §11.2.20 and §4.2. The **unmatched** cam B measures exactly **6**
/// codes — the measurement that forced the budget from 6 to 5 (A15) — so a
/// `<= 6` gate would have passed its own failing-direction fixture.
#[test]
fn cc7_a_the_unmatched_candidate_exceeds_the_neutral_spread_budget() {
    let gpu = fallback_gpu();
    let measurement = cc7_measure_match(&gpu, Cc7Camera::B, "cc7-a-unmatched", None);
    let (spread, patch) = measurement.unmatched_spread;
    assert_eq!(
        spread, CC7_MEASURED_UNMATCHED_B_SPREAD_CODE,
        "unmatched cam B's worst neutral spread (patch {patch})"
    );
    assert!(
        spread > CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE,
        "the unmatched candidate must fail the gate the matched one passes; a budget of \
         {CC7_MEASURED_UNMATCHED_B_SPREAD_CODE} would not have"
    );
}

/// CC7 §11.2.20b and §4.2. The **corrected C2** — the candidate beyond the
/// planner's authority — measures `CC7_UNRECOVERABLE_RESIDUAL_SPREAD_REPORTED_CODE`,
/// which is the compromise §4(b)(2) asks the human about.
#[test]
fn cc7_a_the_unrecoverable_candidate_exceeds_the_neutral_spread_budget() {
    let gpu = fallback_gpu();
    let measurement = cc7_measure_match(&gpu, Cc7Camera::C2, "cc7-a-unrecoverable", None);
    let (spread, patch) = measurement.matched_spread;
    assert_eq!(
        spread, CC7_UNRECOVERABLE_RESIDUAL_SPREAD_REPORTED_CODE,
        "corrected C2's residual spread (patch {patch})"
    );
    assert!(spread > CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE);
    // R-M10: corrected C2 *passes* the luma term, which is why the spread and
    // the luma mean are two gates and not one.
    assert_eq!(
        measurement.matched_luma_millionths,
        CC7_MEASURED_CORRECTED_C2_LUMA_MEAN_CODE_MILLIONTHS
    );
    assert!(
        measurement.matched_luma_millionths.abs() <= CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS,
        "corrected C2 must pass the luma gate it is not the failing direction for"
    );
}

/// CC7 §11.2.21 and §4.2. Unmatched cam B's chart luma mean is 3.98x over the
/// budget, and corrected C2 is asserted to pass the same term.
#[test]
fn cc7_a_the_unmatched_candidate_exceeds_the_luma_mean_budget() {
    let gpu = fallback_gpu();
    let measurement = cc7_measure_match(&gpu, Cc7Camera::B, "cc7-a-luma-fail", None);
    let luma = measurement.unmatched_luma_millionths;
    assert_eq!(luma, CC7_MEASURED_UNMATCHED_B_LUMA_MEAN_CODE_MILLIONTHS);
    assert!(
        luma.abs() > CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS,
        "the unmatched candidate must fail the luma gate the matched one passes"
    );
}

/// CC7 §4(a)(4). The intentional difference survives the match: the skin band
/// reports `in_band_basis_points == CC7_SKIN_IN_BAND_EXACT_BASIS_POINTS` on
/// both the reference and the matched candidate, and the skin row's chroma
/// spread is **smaller** on the matched candidate — the desaturation the
/// planner was told not to correct.
///
/// *Fails:* `cc7_a_skin_band_rejects_the_product_row`.
#[test]
fn cc7_a_the_intentional_difference_survives_the_match() {
    let gpu = fallback_gpu();
    let scene = cc7_two_clip_scene("cc7-a-skin", Cc7Camera::B);
    let engine = cc7_engine(&gpu);
    let bare = scene.bare();
    let committed = scene.commit(
        "cc7-a-skin",
        &cc7_canonical_operations(Cc7Scenario::MixedCamera),
    );
    let candidate_frame = TimeCode(i64::from(CC7_SOURCE_FRAMES));

    let mut rates = Vec::new();
    for (which, document, at) in [
        ("reference", &bare, TimeCode::ZERO),
        ("matched", &committed, candidate_frame),
    ] {
        let proof = cc7_working_proof(&engine, document, at, which);
        let report = cc7_skin_report(&proof, at.0);
        let skin = report.skin.as_ref().expect("the skin diagnostic");
        assert_eq!(
            u32::try_from(CC7_SKIN_IN_BAND_EXACT_BASIS_POINTS).expect("a rate"),
            skin.in_band_basis_points,
            "{which}: the four skin patches must be entirely inside the band"
        );
        // R-M5: the rate is over the **considered** (chromatic) pixels, and
        // the population it did not measure is reported beside it.
        assert!(skin.mean_hue_centidegrees.is_some(), "{which}: {skin:?}");
        assert_eq!(skin.excluded_achromatic_pixel_count, 0, "{which}");
        assert_eq!(
            skin.considered_pixel_count,
            u64::from(CC7_ROW_PATCHES[0].rect.pixels()) * 4,
            "{which}: the four skin patches"
        );
        assert!(
            !report
                .exceptions
                .iter()
                .any(|exception| exception.code == "skin_region_outside_band"),
            "{which}: an in-band skin region must raise no Info exception"
        );
        rates.push(skin.in_band_basis_points);
    }

    // The intentional desaturation: cam B's saturation is 0.85, and the match
    // moves exposure and white balance only, so the matched skin row is still
    // less saturated than the reference's.
    let reference = cc7_monitor_raster(&engine, &bare, TimeCode::ZERO, "reference");
    let matched = cc7_monitor_raster(&engine, &committed, candidate_frame, "matched");
    let reference_chroma = cc7_skin_row_chroma_spread(&reference);
    let matched_chroma = cc7_skin_row_chroma_spread(&matched);
    assert!(
        matched_chroma < reference_chroma,
        "the matched candidate's skin-row chroma spread ({matched_chroma}) must stay below the \
         reference's ({reference_chroma}), or the intentional difference was corrected away"
    );
    println!(
        "CC7_A_SKIN in_band={rates:?} chroma_reference={reference_chroma} chroma_matched={matched_chroma}"
    );
}

/// CC7 §4(a)(4)'s failing direction. The same measurement over the
/// `product_red` patch reports `in_band_basis_points == 0` and a
/// `skin_region_outside_band` **Info** exception, so the 10 000 above is a
/// measurement rather than a property of the statistic.
#[test]
fn cc7_a_skin_band_rejects_the_product_row() {
    let gpu = fallback_gpu();
    let engine = cc7_engine(&gpu);
    let scene = cc7_camera_a_scene("cc7-a-product-row");
    let proof = cc7_working_proof(&engine, &scene.bare(), TimeCode::ZERO, "product row");
    let report = measure_color_qc(
        &proof,
        &ColorQcRequest {
            roi: Some(CC7_PRODUCT_RED_ROI),
            checks: vec![ColorQcCheck::Skin],
            ..ColorQcRequest::default()
        },
    )
    .expect("the product-row skin diagnostic measures");
    let skin = report.skin.expect("the skin diagnostic");
    assert_eq!(skin.in_band_basis_points, 0);
    assert_eq!(
        skin.considered_pixel_count,
        u64::from(CC7_PRODUCT_PATCH_PIXEL_COUNT)
    );
    let exception = report
        .exceptions
        .iter()
        .find(|exception| exception.code == "skin_region_outside_band")
        .expect("an out-of-band skin region raises the Info exception");
    assert_eq!(exception.severity, QaSeverity::Info);
    assert!(
        report.technical_pass,
        "an Info exception never clears technical_pass"
    );
}

/// `get_color_qc`'s skin check over CC7's four-patch skin ROI.
fn cc7_skin_report(proof: &kinewright_core::WorkingProof, project_frame: i64) -> ColorQcReport {
    measure_color_qc(
        proof,
        &ColorQcRequest {
            roi: Some(CC7_SKIN_BAND_ROI),
            checks: vec![ColorQcCheck::Skin],
            project_frame,
            ..ColorQcRequest::default()
        },
    )
    .expect("the CC7 skin diagnostic measures")
}

// ===========================================================================
// Evidence.
// ===========================================================================

/// Every fixture in this file that emits a `CC7_EVIDENCE` payload.
///
/// Declared rather than free-form so a payload cannot appear under a name the
/// manifest does not list, exactly as CC5 and CC6 do.
const CC7_EVIDENCE_FIXTURES: [&str; 8] = [
    "cc7_mixed_camera_match_meets_the_neutral_spread_and_luma_budgets",
    "cc7_wrong_balance_clamps_temperature_and_raises_one_range_warning",
    "cc7_log_inverse_lands_every_patch_inside_the_budget",
    "cc7_product_qualifier_covers_exactly_its_patch_and_changes_nothing_outside",
    "cc7_warm_look_out_of_gamut_count_is_exact_on_the_deep_shadow_patch",
    "cc7_every_scenario_verifies_at_eight_bits",
    "cc7_every_scenario_verifies_at_ten_bits",
    "cc7_canonical_node_stack_matches_the_cpu_reference_on_the_software_lane",
];

fn emit_cc7_evidence(fixture: &str, gpu: &FixtureGpu, controls: Value, metrics: Value) {
    assert!(
        CC7_EVIDENCE_FIXTURES.contains(&fixture),
        "every CC7 evidence payload must be declared in CC7_EVIDENCE_FIXTURES and in the \
         manifest; {fixture} is not"
    );
    let provenance = backend_metadata(gpu.backend());
    let field = |key: &str| provenance.get(key).cloned().unwrap_or(Value::Null);
    let payload = json!({
        "contract": CC7_CONTRACT,
        "fixture": fixture,
        "lane": gpu.lane.id(),
        "git_revision": git_revision(),
        "backend": gpu.backend(),
        "backend_name": field("backend"),
        // Q5: the adapter is RECORDED, never asserted. On Windows it is not
        // llvmpipe, and a fixture that asserted one would go red on the OS it
        // was written to protect.
        "adapter": field("adapter"),
        "software_fallback": field("software_fallback"),
        "gpu_claim": field("gpu_claim"),
        "backend_lane": field("lane"),
        "backend_metadata": provenance,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "raster": {"width": CC7_SOURCE_WIDTH, "height": CC7_SOURCE_HEIGHT},
        "controls": controls,
        "metrics": metrics,
    });
    println!("CC7_EVIDENCE {payload}");
    write_evidence_artefact(fixture, &payload);
}

// ===========================================================================
// §4(b) Wrong white balance and underexposure.
// ===========================================================================

/// CC7 §11.2.22 — the (b) exit gate. §4(b)(2): C2's `temperature_percent`
/// clamps at the descriptor bound with the raw first-order term published,
/// while `exposure_milli_stops` does not clamp. §4(b)(3): the corrected clip
/// raises **exactly one** `delivery_range_excursion` **Warning**, on the blue
/// channel, with `technical_pass` still `true`, and the per-node attribution
/// names the `primary_correction` node as the sole cause.
///
/// *Fails:* `cc7_b_c1_publishes_no_clamp` and `cc7_b_c1_raises_no_range_excursion`.
#[test]
fn cc7_wrong_balance_clamps_temperature_and_raises_one_range_warning() {
    let gpu = fallback_gpu();
    let measurement = cc7_measure_match(&gpu, Cc7Camera::C2, "cc7-b2", None);

    // --- §4(b)(2) the clamp -----------------------------------------------
    let temperature = measurement.proposal.get("temperature_percent");
    assert!(temperature.clamped, "{temperature:?}");
    assert_eq!(temperature.value, CC7_MATCH_PROPOSAL_C2.temperature_percent);
    assert_eq!(temperature.minimum, -100);
    assert_eq!(temperature.maximum, 100);
    assert_eq!(
        temperature.requested,
        CC7_MATCH_PROPOSAL_C2
            .temperature_unrounded_delta
            .expect("§2.4.3 records C2's raw temperature delta"),
        "the published `requested` is the rounded raw first-order term"
    );
    let exposure = measurement.proposal.get("exposure_milli_stops");
    assert!(!exposure.clamped, "{exposure:?}");
    assert_eq!(exposure.value, CC7_MATCH_PROPOSAL_C2.exposure_milli_stops);
    assert_eq!(
        measurement.proposal.value("tint_percent"),
        CC7_MATCH_PROPOSAL_C2.tint_percent
    );
    assert_eq!(
        measurement.proposal.parameters(),
        CC7_B2_OPERATIONS[0]
            .parameters
            .iter()
            .map(|(name, value)| ((*name).to_owned(), *value))
            .collect::<Vec<_>>(),
        "the replicated proposal must equal `CC7_B2_OPERATIONS`"
    );

    // The residual is reported, never gated (§4(b)(2)).
    assert_eq!(
        measurement.matched_spread.0,
        CC7_UNRECOVERABLE_RESIDUAL_SPREAD_REPORTED_CODE
    );

    // --- §4(b)(3) the excursion and its attribution ----------------------
    let scene = cc7_two_clip_scene("cc7-b2-qc", Cc7Camera::C2);
    let engine = cc7_engine(&gpu);
    let committed = scene.commit(
        "cc7-b2-qc",
        &cc7_canonical_operations(Cc7Scenario::WhiteBalance),
    );
    let at = TimeCode(i64::from(CC7_SOURCE_FRAMES));
    let attribution = cc7_range_attribution(&engine, &scene, &committed, at);

    assert_eq!(
        attribution.with_node.range.clamped_basis_points,
        u32::try_from(CC7_C2_OVER_RANGE_BASIS_POINTS_REPORTED).expect("a rate")
    );
    assert_eq!(
        attribution.with_node.range.blue.over_pixel_count,
        u64::try_from(CC7_C2_OVER_RANGE_PIXELS_REPORTED).expect("a count"),
        "A16: the over-range population is on the blue channel only"
    );
    assert_eq!(attribution.with_node.range.red.over_pixel_count, 0);
    assert_eq!(attribution.with_node.range.green.over_pixel_count, 0);
    assert_eq!(attribution.with_node.gamut.out_of_gamut_pixel_count, 0);

    let excursions = attribution
        .with_node
        .exceptions
        .iter()
        .filter(|exception| exception.code == "delivery_range_excursion")
        .collect::<Vec<_>>();
    assert_eq!(
        excursions.len(),
        1,
        "exactly one range excursion is expected: {:?}",
        attribution.with_node.exceptions
    );
    let excursion = excursions[0];
    assert_eq!(excursion.severity, QaSeverity::Warning);
    assert_eq!(excursion.field.as_deref(), Some("blue.over_basis_points"));
    assert_eq!(
        excursion.observed.as_deref(),
        Some(CC7_C2_OVER_RANGE_BASIS_POINTS_REPORTED.to_string().as_str())
    );
    assert!(
        attribution.with_node.technical_pass,
        "a Warning is not an Error: {:?}",
        attribution.with_node.exceptions
    );

    // Per-node attribution by the production `node_removed` method (§3.7).
    assert_eq!(
        attribution.range_delta, CC7_C2_OVER_RANGE_BASIS_POINTS_REPORTED,
        "Q7 / A20: the (b2) per-node range delta is CONFIRMED here, not pinned from the report"
    );
    assert_eq!(attribution.gamut_delta, 0);
    assert_eq!(attribution.without_node.range.clamped_basis_points, 0);

    // The corrected clip's skin band is still above the Info threshold, so the
    // compromise is visible in the number without a new code.
    let proof = cc7_working_proof(&engine, &committed, at, "cc7-b2 skin");
    let skin = cc7_skin_report(&proof, at.0)
        .skin
        .expect("the skin diagnostic");
    assert!(
        i64::from(skin.in_band_basis_points)
            >= i64::from(kinewright_core::SKIN_BAND_EXCEPTION_BASIS_POINTS),
        "corrected C2's skin band must stay above SKIN_BAND_EXCEPTION_BASIS_POINTS"
    );

    emit_cc7_evidence(
        "cc7_wrong_balance_clamps_temperature_and_raises_one_range_warning",
        &gpu,
        json!({
            "candidate": "C2",
            "proposal": {
                "exposure_milli_stops": exposure.value,
                "temperature_percent": temperature.value,
                "temperature_requested": temperature.requested,
                "temperature_unrounded_delta": temperature.unrounded_delta,
                "tint_percent": measurement.proposal.value("tint_percent"),
            },
        }),
        json!({
            "residual_spread_code": measurement.matched_spread.0,
            "clamped_basis_points": attribution.with_node.range.clamped_basis_points,
            "blue_over_pixel_count": attribution.with_node.range.blue.over_pixel_count,
            "blue_maximum_over_excursion_millionths":
                attribution.with_node.range.blue.maximum_over_excursion_millionths,
            "range_basis_points_delta": attribution.range_delta,
            "gamut_basis_points_delta": attribution.gamut_delta,
            "skin_in_band_basis_points": skin.in_band_basis_points,
            "technical_pass": attribution.with_node.technical_pass,
        }),
    );
}

/// The `node_removed` attribution of one committed node, computed the way
/// `ColorQcCheck::PerNode` computes it: the region's clipping with the node,
/// minus the region's clipping with the node removed.
struct Cc7RangeAttribution {
    with_node: ColorQcReport,
    without_node: ColorQcReport,
    range_delta: i64,
    gamut_delta: i64,
}

fn cc7_range_attribution(
    engine: &crate::engine::FfmpegMediaEngine,
    scene: &Cc7Scene,
    committed: &Arc<Document>,
    at: TimeCode,
) -> Cc7RangeAttribution {
    let request = ColorQcRequest {
        roi: None,
        checks: vec![ColorQcCheck::Range, ColorQcCheck::Gamut],
        project_frame: at.0,
        ..ColorQcRequest::default()
    };
    let with_node = measure_color_qc(
        &cc7_working_proof(engine, committed, at, "with node"),
        &request,
    )
    .expect("the CC7 range/gamut measurement");
    let without_node = measure_color_qc(
        &cc7_working_proof(engine, &scene.bare(), at, "without node"),
        &request,
    )
    .expect("the CC7 range/gamut baseline");
    Cc7RangeAttribution {
        range_delta: i64::from(with_node.range.clamped_basis_points)
            - i64::from(without_node.range.clamped_basis_points),
        gamut_delta: i64::from(with_node.gamut.out_of_gamut_basis_points)
            - i64::from(without_node.gamut.out_of_gamut_basis_points),
        with_node,
        without_node,
    }
}

/// CC7 §4(b)(1) and §4.2. C1's proposal clamps nothing, so the clamp assertion
/// in `cc7_wrong_balance_clamps_temperature_and_raises_one_range_warning` is
/// not tautological. R-M19's absent-key rule is asserted directly: the gate
/// iterates the **present** controls and additionally requires
/// `temperature_percent` to be present, so a planner that proposed nothing at
/// all could not pass by vacuous iteration.
#[test]
fn cc7_b_c1_publishes_no_clamp() {
    let gpu = fallback_gpu();
    let measurement = cc7_measure_match(
        &gpu,
        Cc7Camera::C1,
        "cc7-b1-clamp",
        Some(cc7_b1_canonical_operations()),
    );
    assert!(
        measurement
            .proposal
            .controls
            .contains_key("temperature_percent"),
        "a run in which the planner proposed nothing must not pass by vacuous iteration"
    );
    assert!(
        !measurement.proposal.controls.is_empty(),
        "the C1 proposal must carry at least one control"
    );
    for (name, control) in &measurement.proposal.controls {
        assert!(
            !control.clamped,
            "C1 is inside the planner's authority: {name} = {control:?}"
        );
        assert_eq!(control.requested, control.value);
    }
    println!(
        "CC7_B1_PROPOSAL {:?}",
        measurement
            .proposal
            .controls
            .iter()
            .map(|(name, control)| (*name, control.value, control.unrounded_delta))
            .collect::<Vec<_>>()
    );
}

/// CC7 §4(b)(1). The residual spread on the **committed** (b1) document meets
/// `CC7_B1_RESIDUAL_SPREAD_MAX_CODE`, the (b1) row's **own** budget, exactly
/// one code above the (a) match budget (C-E7's ruling, G-E1). "Recoverable"
/// means "within one code of as good as the (a) match": (b1) recovers a clip
/// that arrives wrong-balanced *and* underexposed from a planner proposal
/// alone, and the shared budget could not widen to 6 because 6 is exactly what
/// unmatched cam B measures (A15).
#[test]
fn cc7_b1_residual_spread_meets_the_match_budget() {
    let gpu = fallback_gpu();
    let measurement = cc7_measure_match(
        &gpu,
        Cc7Camera::C1,
        "cc7-b1-residual",
        Some(cc7_b1_canonical_operations()),
    );
    let (spread, patch) = measurement.matched_spread;
    // C-E7's ruling: (b1) gates against its OWN budget, one code above the (a)
    // one, because (b1) is a harder recovery and because the shared budget
    // cannot widen — 6 is exactly what unmatched cam B measures, so a shared 6
    // would admit (a)'s failing direction (A15).
    assert!(
        spread <= CC7_B1_RESIDUAL_SPREAD_MAX_CODE,
        "the corrected C1 residual is {spread} codes at chart patch {patch}, over the budget of \
         {CC7_B1_RESIDUAL_SPREAD_MAX_CODE}"
    );
    assert_eq!(
        spread, CC7_MEASURED_B1_RESIDUAL_SPREAD_CODE,
        "the (b1) residual measured {spread} at chart patch {patch} against the pinned \
         {CC7_MEASURED_B1_RESIDUAL_SPREAD_CODE}; re-pin it rather than leaving a stale number"
    );
    assert!(
        CC7_B1_RESIDUAL_SPREAD_MAX_CODE >= 2 * spread,
        "the (b1) row must carry the programme's 2x margin: budget \
         {CC7_B1_RESIDUAL_SPREAD_MAX_CODE} over measured {spread}"
    );
    // The split is one code and not a free hand: the (b1) budget is the (a)
    // budget plus one, stated here so a later widening has to change this
    // fixture too.
    assert_eq!(
        CC7_B1_RESIDUAL_SPREAD_MAX_CODE,
        CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE + 1,
        "(b1) is exactly one code above (a)"
    );

    // The replica reproduces §2.5's (b1) row exactly, including R-M19's
    // absent-key rule: the tint delta rounds to zero, so `tint_percent` is
    // OMITTED rather than stored as `0`.
    assert_eq!(
        measurement.proposal.value("exposure_milli_stops"),
        CC7_MATCH_PROPOSAL_C1.exposure_milli_stops
    );
    assert_eq!(
        measurement.proposal.value("temperature_percent"),
        CC7_MATCH_PROPOSAL_C1.temperature_percent
    );
    assert!(
        !measurement.proposal.controls.contains_key("tint_percent"),
        "C1's tint delta rounds to zero, so the control is not proposed at all"
    );
    assert_eq!(
        measurement.proposal.parameters(),
        CC7_B1_OPERATIONS[0]
            .parameters
            .iter()
            .map(|(name, value)| ((*name).to_owned(), *value))
            .collect::<Vec<_>>(),
        "the replicated proposal must equal `CC7_B1_OPERATIONS`"
    );
    // The uncorrected candidate fails the same gate, so the (b1) row is not
    // satisfied by a document that was already neutral.
    assert!(
        measurement.unmatched_spread.0 > CC7_B1_RESIDUAL_SPREAD_MAX_CODE,
        "uncorrected C1 measures {} and must fail the gate the corrected one passes",
        measurement.unmatched_spread.0
    );
    println!(
        "CC7_B1_RESIDUAL uncorrected={} corrected={spread} luma_delta={}",
        measurement.unmatched_spread.0, measurement.matched_luma_millionths
    );
}

/// CC7 §4.2, C-E7's failing direction for the **(b1) residual spread** row.
///
/// The budget `CC7_B1_RESIDUAL_SPREAD_MAX_CODE = 6` is not decorative: the
/// **uncorrected** C1 clip — the same source, without the planner's committed
/// primary correction — measures `CC7_MEASURED_UNCORRECTED_C1_SPREAD_CODE`,
/// and the unrecoverable corrected C2 measures
/// `CC7_UNRECOVERABLE_RESIDUAL_SPREAD_REPORTED_CODE`. Both are over it, so a
/// budget of 6 excludes a document that has not been recovered as well as one
/// that cannot be.
#[test]
fn cc7_b1_the_uncorrected_candidate_exceeds_the_residual_budget() {
    let gpu = fallback_gpu();
    let measurement = cc7_measure_match(
        &gpu,
        Cc7Camera::C1,
        "cc7-b1-uncorrected",
        Some(cc7_b1_canonical_operations()),
    );
    let (spread, patch) = measurement.unmatched_spread;
    assert_eq!(
        spread, CC7_MEASURED_UNCORRECTED_C1_SPREAD_CODE,
        "uncorrected C1's worst neutral spread (patch {patch})"
    );
    assert!(
        spread > CC7_B1_RESIDUAL_SPREAD_MAX_CODE,
        "the uncorrected candidate must fail the gate the corrected one passes; a budget of \
         {spread} would not have"
    );
    // The second failing direction on the same term: the candidate beyond the
    // planner's authority, which stays over the budget even once corrected.
    const {
        assert!(
            CC7_UNRECOVERABLE_RESIDUAL_SPREAD_REPORTED_CODE > CC7_B1_RESIDUAL_SPREAD_MAX_CODE,
            "corrected C2 must remain over the (b1) budget"
        );
    }
}

/// CC7 §4(b)(3)'s failing direction. The corrected **cam B** control measures
/// `clamped_basis_points == 0`, raises no exception, and attributes `0 / 0`,
/// so neither the excursion nor the attribution is vacuous.
#[test]
fn cc7_b_c1_raises_no_range_excursion() {
    let gpu = fallback_gpu();
    let engine = cc7_engine(&gpu);
    for (label, candidate, operations) in [
        (
            "corrected cam B",
            Cc7Camera::B,
            cc7_canonical_operations(Cc7Scenario::MixedCamera),
        ),
        ("corrected C1", Cc7Camera::C1, cc7_b1_canonical_operations()),
    ] {
        let scene = cc7_two_clip_scene("cc7-b-control", candidate);
        let committed = scene.commit(label, &operations);
        let at = TimeCode(i64::from(CC7_SOURCE_FRAMES));
        let attribution = cc7_range_attribution(&engine, &scene, &committed, at);
        assert_eq!(
            attribution.with_node.range.clamped_basis_points, 0,
            "{label} must clip nothing"
        );
        assert_eq!(attribution.range_delta, 0, "{label}");
        assert_eq!(attribution.gamut_delta, 0, "{label}");
        assert!(
            attribution.with_node.exceptions.is_empty(),
            "{label} must raise no exception: {:?}",
            attribution.with_node.exceptions
        );
        assert!(attribution.with_node.technical_pass, "{label}");
    }
}

// ===========================================================================
// §4(c) Log-like input.
// ===========================================================================

/// The A2 gate set: the twelve achromatic chart patches **plus** the four skin
/// patches. The five primaries are deliberately excluded (A2, A22): their
/// error is a structural floor of the exponential, not slack in the lattice.
fn cc7_log_gate_patches() -> Vec<(String, &'static Cc7Patch)> {
    CC7_CHART_PATCHES
        .iter()
        .enumerate()
        .map(|(index, patch)| (format!("chart{index:02}"), patch))
        .chain(
            CC7_ROW_PATCHES
                .iter()
                .take(4)
                .map(|patch| (patch.name.to_owned(), patch)),
        )
        .collect()
}

/// The max absolute per-pixel, per-channel code difference over one rect's
/// 2-pixel inset, between two RGBA8 rasters.
fn cc7_worst_pixel_code_error(
    actual: &RgbaImage,
    reference: &RgbaImage,
    rect: Cc7PixelRect,
) -> i64 {
    assert_eq!(
        (actual.width, actual.height),
        (reference.width, reference.height)
    );
    let rect = cc7_inset(rect);
    let mut worst = 0_i64;
    for y in rect.y..rect.y + rect.height {
        for x in rect.x..rect.x + rect.width {
            let index = ((y * actual.width + x) * 4) as usize;
            for channel in 0..3 {
                let error = i64::from(actual.pixels[index + channel])
                    - i64::from(reference.pixels[index + channel]);
                worst = worst.max(error.abs());
            }
        }
    }
    worst
}

/// One lattice size's monitoring error against camera A's own raster, per
/// patch and set-wide.
struct Cc7LogInverseMeasurement {
    per_patch: BTreeMap<String, i64>,
    gate_set_worst: i64,
    primaries_worst: i64,
    cube_bytes: u64,
}

fn cc7_measure_log_inverse(
    gpu: &FixtureGpu,
    size: u32,
    identity: bool,
) -> Cc7LogInverseMeasurement {
    let reference = {
        let scene = cc7_camera_a_scene("cc7-c-reference");
        let engine = cc7_engine(gpu);
        cc7_monitor_raster(&engine, &scene.bare(), TimeCode::ZERO, "cam A")
    };
    let scene = Cc7Scene::over("cc7-c-carrier", &[Cc7SourceKind::Log]);
    let (directory, asset, library) = cc7_import_log_inverse_cube(size, identity);
    let cube_bytes = asset.byte_len;
    let engine = cc7_engine(gpu);
    engine.set_lut_library(library);
    let committed = scene.commit(
        "cc7-c",
        &cc7_lut_backed_canonical_operations(Cc7Scenario::LogLike, asset),
    );
    let raster = cc7_monitor_raster(&engine, &committed, TimeCode::ZERO, "cc7-c");
    drop(directory);

    let mut per_patch = BTreeMap::new();
    let mut gate_set_worst = 0_i64;
    for (name, patch) in cc7_log_gate_patches() {
        let error = cc7_worst_pixel_code_error(&raster, &reference, patch.rect);
        gate_set_worst = gate_set_worst.max(error);
        per_patch.insert(name, error);
    }
    let mut primaries_worst = 0_i64;
    for (index, patch) in CC7_PRIMARY_PATCHES.iter().enumerate() {
        let error = cc7_worst_pixel_code_error(&raster, &reference, patch.rect);
        primaries_worst = primaries_worst.max(error);
        per_patch.insert(format!("primary{index}"), error);
    }
    for patch in CC7_ROW_PATCHES.iter().skip(4) {
        per_patch.insert(
            patch.name.to_owned(),
            cc7_worst_pixel_code_error(&raster, &reference, patch.rect),
        );
    }
    Cc7LogInverseMeasurement {
        per_patch,
        gate_set_worst,
        primaries_worst,
        cube_bytes,
    }
}

/// CC7 §11.2.23 — the (c) exit gate. §4(c)(2): the imported `.cube` at the
/// pinned lattice size undoes the log curve to within
/// `CC7_LOG_INVERSE_MAX_CODE` over the **set-wide** worst of the twelve
/// achromatic and four skin patches.
///
/// *Fails:* `cc7_c_an_identity_cube_does_not_undo_the_log_curve`.
#[test]
fn cc7_log_inverse_lands_every_patch_inside_the_budget() {
    let gpu = fallback_gpu();
    let measurement = cc7_measure_log_inverse(&gpu, CC7_LOG_CUBE_SIZE, false);
    assert!(
        measurement.gate_set_worst <= CC7_LOG_INVERSE_MAX_CODE,
        "the set-wide worst monitoring error is {} codes, over the budget of \
         {CC7_LOG_INVERSE_MAX_CODE}: {:?}",
        measurement.gate_set_worst,
        measurement.per_patch
    );
    assert_eq!(
        measurement.gate_set_worst, CC7_MEASURED_LOG_INVERSE_CODE,
        "§4.1 pins the measured set-wide worst at size {CC7_LOG_CUBE_SIZE}"
    );
    assert!(
        CC7_LOG_INVERSE_MAX_CODE >= 2 * measurement.gate_set_worst,
        "rule 11.0.5: the log-inverse budget must keep a 2x margin"
    );
    // §2.4.2 / A2: the black patch's error is a property of the curve, not of
    // the lattice, and it is inside the budget rather than excluded from it.
    assert_eq!(
        measurement.per_patch["chart00"], CC7_LOG_BLACK_PATCH_REPORTED_CODE,
        "the black patch's floor is `v = 0` inverting to 2^-8 linear"
    );
    // A2 excludes the primaries from the gate set; they are reported.
    assert_eq!(
        measurement.primaries_worst, CC7_LOG_PRIMARY_REPORTED_CODE,
        "the saturated primaries' floor is reported, never gated"
    );
    assert!(
        measurement.primaries_worst > measurement.gate_set_worst,
        "if the primaries were inside the gate-set worst the A2 exclusion would be decorative"
    );
    assert_eq!(
        i64::try_from(measurement.cube_bytes).expect("a byte count"),
        CC7_LOG_CUBE_BYTES_REPORTED
    );
    assert!(
        measurement.cube_bytes < LUT_MAX_FILE_BYTES,
        "the 65^3 cube must fit inside LUT_MAX_FILE_BYTES"
    );

    emit_cc7_evidence(
        "cc7_log_inverse_lands_every_patch_inside_the_budget",
        &gpu,
        json!({
            "cube_size": CC7_LOG_CUBE_SIZE,
            "cube_bytes": measurement.cube_bytes,
            "input_encoding_token": 0,
        }),
        json!({
            "gate_set_worst_code": measurement.gate_set_worst,
            "budget_code": CC7_LOG_INVERSE_MAX_CODE,
            "primaries_worst_code": measurement.primaries_worst,
            "per_patch_code": measurement.per_patch,
        }),
    );
}

/// CC7 §4(c)(2)'s failing direction. An **identity** cube leaves the log
/// carrier monitored as if it were display Rec.709, at a set-wide worst of
/// `CC7_LOG_IDENTITY_CUBE_REPORTED_CODE`. The same fixture asserts that
/// `chart06` alone reads **1** under that cube, so a single-patch gate at
/// mid-grey is proved vacuous rather than merely deprecated.
#[test]
fn cc7_c_an_identity_cube_does_not_undo_the_log_curve() {
    let gpu = fallback_gpu();
    // §2.6: no CC7 gate uses a literal lattice size; 33 is the ladder's
    // middle rung (R4-m2).
    let measurement = cc7_measure_log_inverse(&gpu, CC7_LOG_CUBE_SIZE_LADDER[1].0, true);
    assert_eq!(
        measurement.gate_set_worst, CC7_LOG_IDENTITY_CUBE_REPORTED_CODE,
        "{:?}",
        measurement.per_patch
    );
    assert!(
        measurement.gate_set_worst > CC7_LOG_INVERSE_MAX_CODE,
        "the identity cube must fail the gate the real one passes"
    );
    assert_eq!(
        measurement.per_patch["chart06"], 1,
        "a single-patch gate at mid-grey would be vacuous: the log curve is near-neutral there"
    );
    // The one inversion the contract states plainly: under the identity cube
    // black reads 0, while under the correct one it reads 4.
    assert_eq!(measurement.per_patch["chart00"], 0);
}

/// CC7 §11.2.24. §4(c)(3): the lattice sweep is **evidence**, and the size is
/// pinned rather than selected. The set-wide worst is monotone non-increasing
/// with size, size 17 genuinely fails the budget, and the black patch's error
/// is size-independent.
#[test]
fn cc7_c_the_cube_size_sweep_is_monotone_and_size_seventeen_fails() {
    let gpu = fallback_gpu();
    let mut ladder = Vec::new();
    for (size, pinned) in CC7_LOG_CUBE_SIZE_LADDER {
        let measurement = cc7_measure_log_inverse(&gpu, size, false);
        println!(
            "CC7_CUBE_LADDER size={size} pinned={pinned} measured={} bytes={} per_patch={:?}",
            measurement.gate_set_worst, measurement.cube_bytes, measurement.per_patch
        );
        assert_eq!(
            measurement.per_patch["chart00"], CC7_LOG_BLACK_PATCH_REPORTED_CODE,
            "size {size}: the black patch's error is a property of the CURVE, so it must not move \
             with the lattice"
        );
        ladder.push((size, measurement));
    }
    assert_eq!(ladder.len(), 3);

    // Monotone non-increasing with size.
    for pair in ladder.windows(2) {
        assert!(
            pair[0].1.gate_set_worst >= pair[1].1.gate_set_worst,
            "the sweep must be monotone non-increasing: size {} measured {} and size {} measured {}",
            pair[0].0,
            pair[0].1.gate_set_worst,
            pair[1].0,
            pair[1].1.gate_set_worst
        );
    }
    // `17 > CC7_LOG_INVERSE_MAX_CODE >= 33 > 65`, so the sweep is not vacuous.
    assert!(
        ladder[0].1.gate_set_worst > CC7_LOG_INVERSE_MAX_CODE,
        "size 17 must genuinely fail the budget"
    );
    assert!(ladder[1].1.gate_set_worst <= CC7_LOG_INVERSE_MAX_CODE);
    assert!(ladder[1].1.gate_set_worst > ladder[2].1.gate_set_worst);
    // The two smaller sizes are pinned exactly; size 17's measured value is
    // recorded rather than pinned (Implementer C erratum C-E4).
    assert_eq!(ladder[1].1.gate_set_worst, CC7_LOG_CUBE_SIZE_LADDER[1].1);
    assert_eq!(ladder[2].1.gate_set_worst, CC7_LOG_CUBE_SIZE_LADDER[2].1);
    assert!(
        ladder[0].1.gate_set_worst >= CC7_LOG_CUBE_SIZE_LADDER[0].1,
        "size 17 measured {} against the ladder's {}",
        ladder[0].1.gate_set_worst,
        CC7_LOG_CUBE_SIZE_LADDER[0].1
    );
    // `CC7_LOG_CUBE_SIZE` is PINNED, not selected: read as a selection rule
    // the sweep would choose 33 at a 1.7x margin, below the programme's 2x bar.
    assert_eq!(CC7_LOG_CUBE_SIZE, ladder[2].0);
    assert!(CC7_LOG_INVERSE_MAX_CODE < 2 * ladder[1].1.gate_set_worst);
    assert!(CC7_LOG_INVERSE_MAX_CODE >= 2 * ladder[2].1.gate_set_worst);
    assert_eq!(
        i64::try_from(ladder[2].1.cube_bytes).expect("a byte count"),
        CC7_LOG_CUBE_BYTES_REPORTED
    );
    assert!(ladder[2].1.cube_bytes < LUT_MAX_FILE_BYTES);
}

// ===========================================================================
// §4(d) Product and skin.
// ===========================================================================

/// The matte coverage statistics of one committed node, plus the covered set
/// as a per-pixel mask for the containment gate.
struct Cc7MatteMeasurement {
    statistics: kinewright_core::MatteCoverageStatistics,
    inside: Vec<bool>,
}

fn cc7_measure_matte(
    engine: &crate::engine::FfmpegMediaEngine,
    committed: &Arc<Document>,
    label: &str,
) -> Cc7MatteMeasurement {
    let proof = engine
        .matte_proof_for_document(
            Arc::clone(committed),
            TimeCode::ZERO,
            CC7_SINGLE_CLIP_ID,
            EffectId(1),
        )
        .unwrap_or_else(|error| panic!("{label}: the CC7 matte proof should render: {error}"));
    assert!(proof.metadata.matte_enabled, "{label}");
    // R-M6: `matte_coverage_statistics` takes ONE argument and rates every
    // count over the whole raster; it has no ROI parameter.
    let statistics = matte_coverage_statistics(&proof.coverage)
        .unwrap_or_else(|error| panic!("{label}: the coverage statistics: {error}"));
    assert_eq!(
        statistics.total_pixel_count,
        u64::from(CC7_SOURCE_WIDTH) * u64::from(CC7_SOURCE_HEIGHT)
    );
    let inside = proof
        .coverage
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .map(|pixel| pixel[0] > 0)
        .collect::<Vec<_>>();
    Cc7MatteMeasurement { statistics, inside }
}

/// CC7 §11.2.25 — the (d) exit gate. §4(d)(1): the derived `product_red`
/// qualifier covers **exactly** its own patch, with no tolerance. §4(d)(2):
/// the same node changes `CC7_MATTE_OUTSIDE_CHANGED_PIXELS_MAX` pixels outside
/// the matte on the linear working surface.
///
/// *Fails:* `cc7_d_a_qualifier_that_selects_two_patches_is_rejected`.
#[test]
fn cc7_product_qualifier_covers_exactly_its_patch_and_changes_nothing_outside() {
    let gpu = fallback_gpu();
    let engine = cc7_engine(&gpu);
    let scene = cc7_camera_a_scene("cc7-d");
    let committed = scene.commit(
        "cc7-d",
        &cc7_canonical_operations(Cc7Scenario::ProductAndSkin),
    );
    let measurement = cc7_measure_matte(&engine, &committed, "cc7-d");
    let expected = u64::from(CC7_PRODUCT_PATCH_PIXEL_COUNT);

    // §4(d)(1), exact — CC5's precedent, no tolerance.
    assert_eq!(measurement.statistics.covered_pixel_count, expected);
    assert_eq!(measurement.statistics.full_pixel_count, expected);
    assert_eq!(measurement.statistics.partial_pixel_count, 0);
    // The covered set is the `product_red` patch and nothing else, so a
    // qualifier that happened to catch the same COUNT somewhere else fails.
    let product = CC7_PRODUCT_RED_ROI
        .to_pixels(CC7_SOURCE_WIDTH, CC7_SOURCE_HEIGHT)
        .expect("the product ROI resolves");
    for (index, covered) in measurement.inside.iter().enumerate() {
        let x = (index as u32) % CC7_SOURCE_WIDTH;
        let y = (index as u32) / CC7_SOURCE_WIDTH;
        let in_patch = x >= product.x
            && x < product.x + product.width
            && y >= product.y
            && y < product.y + product.height;
        assert_eq!(
            *covered, in_patch,
            "pixel ({x}, {y}) coverage disagrees with the product_red patch"
        );
    }

    // §4(d)(2): CC5's containment helper, reused rather than restated.
    let actual = cc7_working_proof(&engine, &committed, TimeCode::ZERO, "cc7-d");
    let baseline = cc7_working_proof(&engine, &scene.bare(), TimeCode::ZERO, "cc7-d baseline");
    let counts = crate::cc5_fixtures::assert_matte_containment(
        &actual.image.pixels,
        &baseline.image.pixels,
        &measurement.inside,
        "cc7_d_product_qualifier",
    );
    assert_eq!(
        i64::try_from(counts.outside_changed_pixels).expect("a count"),
        CC7_MATTE_OUTSIDE_CHANGED_PIXELS_MAX,
        "no tolerance may excuse one changed pixel outside the matte"
    );
    assert_eq!(
        u64::try_from(counts.inside_changed_pixels).expect("a count"),
        expected,
        "every covered pixel must move, or the saturation node is a no-op"
    );

    emit_cc7_evidence(
        "cc7_product_qualifier_covers_exactly_its_patch_and_changes_nothing_outside",
        &gpu,
        json!({
            "node": "primary_correction qualifier-only",
            "saturation_percent": CC7_SECONDARY_SATURATION_PERCENT,
            "sample_roi": CC7_PRODUCT_RED_ROI,
        }),
        json!({
            "covered_pixel_count": measurement.statistics.covered_pixel_count,
            "full_pixel_count": measurement.statistics.full_pixel_count,
            "partial_pixel_count": measurement.statistics.partial_pixel_count,
            "covered_basis_points": measurement.statistics.covered_basis_points,
            "inside_changed_pixels": counts.inside_changed_pixels,
            "outside_changed_pixels": counts.outside_changed_pixels,
        }),
    );
}

/// CC7 §4(d)(1)'s failing direction. Widening `matte_hue_width_centidegrees`
/// to its neutral **disables the hue leg**, so the qualifier selects more than
/// its own patch and the exact-count gate is known to be able to fail.
#[test]
fn cc7_d_a_qualifier_that_selects_two_patches_is_rejected() {
    let gpu = fallback_gpu();
    let engine = cc7_engine(&gpu);
    let scene = cc7_camera_a_scene("cc7-d-wide");
    let wide = cc7_operation_with(
        &kinewright_core::cc7_scenarios::CC7_D_OPERATIONS[0],
        &[("matte_hue_width_centidegrees", 18_000)],
    );
    let committed = scene.commit(
        "cc7-d-wide",
        &[cc7_primary_insert(CC7_SINGLE_CLIP_ID, &wide)],
    );
    let measurement = cc7_measure_matte(&engine, &committed, "cc7-d-wide");
    assert!(
        measurement.statistics.covered_pixel_count > u64::from(CC7_PRODUCT_PATCH_PIXEL_COUNT),
        "an over-wide qualifier must select more than its own patch: {:?}",
        measurement.statistics
    );
    println!(
        "CC7_D_WIDE covered={} full={} partial={}",
        measurement.statistics.covered_pixel_count,
        measurement.statistics.full_pixel_count,
        measurement.statistics.partial_pixel_count
    );
}

/// CC7 §4(d)(3). The skin hue is unmoved by the product qualifier: the mean
/// hue is `Some` on **both** sides (R-M5: a `None` fails the gate, it is not a
/// pass by default) and the two are equal to the centidegree.
///
/// *Fails:* `cc7_d_a_qualifier_over_the_skin_row_moves_the_skin_hue`.
#[test]
fn cc7_d_the_skin_hue_is_unmoved_by_the_product_qualifier() {
    let gpu = fallback_gpu();
    let engine = cc7_engine(&gpu);
    let scene = cc7_camera_a_scene("cc7-d-hue");
    let committed = scene.commit(
        "cc7-d-hue",
        &cc7_canonical_operations(Cc7Scenario::ProductAndSkin),
    );
    let before = cc7_skin_report(
        &cc7_working_proof(&engine, &scene.bare(), TimeCode::ZERO, "before"),
        0,
    )
    .skin
    .expect("the skin diagnostic");
    let after = cc7_skin_report(
        &cc7_working_proof(&engine, &committed, TimeCode::ZERO, "after"),
        0,
    )
    .skin
    .expect("the skin diagnostic");
    let before_hue = before
        .mean_hue_centidegrees
        .expect("the skin ROI must carry a hue before the commit");
    let after_hue = after
        .mean_hue_centidegrees
        .expect("the skin ROI must carry a hue after the commit");
    assert_eq!(
        before_hue, after_hue,
        "the product qualifier must not move the skin hue"
    );
    assert_eq!(
        before.in_band_basis_points,
        u32::try_from(CC7_SKIN_IN_BAND_EXACT_BASIS_POINTS).expect("a rate")
    );
    assert_eq!(after.in_band_basis_points, before.in_band_basis_points);
    // R-M5: the rate's population is reported beside it, so 10 000 can never
    // be read as a claim about a population it did not measure.
    assert_eq!(after.excluded_achromatic_pixel_count, 0);
    assert_eq!(
        after.considered_pixel_count,
        u64::from(CC7_ROW_PATCHES[0].rect.pixels()) * 4
    );
    println!("CC7_D_HUE before={before_hue} after={after_hue}");
}

/// CC7 §4(d)(3)'s failing direction. A qualifier derived from `skin_tan`
/// instead of `product_red` moves `mean_hue_centidegrees` by a non-zero
/// amount, so the equality above is a measurement.
#[test]
fn cc7_d_a_qualifier_over_the_skin_row_moves_the_skin_hue() {
    let gpu = fallback_gpu();
    let engine = cc7_engine(&gpu);
    let scene = cc7_camera_a_scene("cc7-d-skin-hue");
    // The `skin_tan` band: hue centre 2 500 cd with the same softness, and
    // saturation/luma bands wide enough to admit the four skin patches.
    let skin_qualifier = cc7_operation_with(
        &kinewright_core::cc7_scenarios::CC7_D_OPERATIONS[0],
        &[
            ("matte_hue_center_centidegrees", 2_500),
            ("matte_hue_width_centidegrees", 3_000),
            ("matte_saturation_low_basis_points", 3_000),
            ("matte_saturation_high_basis_points", 7_000),
            ("matte_luma_low_basis_points", 2_000),
            ("matte_luma_high_basis_points", 6_000),
        ],
    );
    let committed = scene.commit(
        "cc7-d-skin-hue",
        &[cc7_primary_insert(CC7_SINGLE_CLIP_ID, &skin_qualifier)],
    );
    let before = cc7_skin_report(
        &cc7_working_proof(&engine, &scene.bare(), TimeCode::ZERO, "before"),
        0,
    )
    .skin
    .expect("the skin diagnostic")
    .mean_hue_centidegrees
    .expect("a hue");
    let after = cc7_skin_report(
        &cc7_working_proof(&engine, &committed, TimeCode::ZERO, "after"),
        0,
    )
    .skin
    .expect("the skin diagnostic")
    .mean_hue_centidegrees
    .expect("a hue");
    assert_ne!(
        before, after,
        "a qualifier over the skin row must move the skin hue, or §4(d)(3) has no failing direction"
    );
    println!("CC7_D_SKIN_HUE before={before} after={after}");
}

/// CC7 §11.2.26. §4(d)(4): the (d2) **window-only** node's feather band is the
/// discrete pixel-centre count, within `CC7_FEATHER_PARTIAL_TOLERANCE_PIXELS`.
/// The continuous-area value is asserted **not** to match, so the wrong model
/// cannot be reintroduced.
///
/// *Fails:* `cc7_d_feather_zero_has_no_partial_pixels`.
#[test]
fn cc7_feather_counts_match_the_discrete_pixel_centre_model() {
    let gpu = fallback_gpu();
    let engine = cc7_engine(&gpu);
    let scene = cc7_camera_a_scene("cc7-d2");
    let committed = scene.commit("cc7-d2", &cc7_d2_canonical_operations());

    // R-B4: the committed node stores NO qualifier parameter, so a future
    // merge of (d) and (d2) fails here rather than silently measuring
    // `192 / 140 / 52`.
    let effect = &committed.tracks[0].clips[0].effects[0];
    assert!(
        !effect
            .parameters
            .keys()
            .any(|name| name.starts_with("matte_hue")
                || name.starts_with("matte_saturation")
                || name.starts_with("matte_luma")
                || name == "matte_qualifier_enabled"),
        "(d2) is window-only: {:?}",
        effect.parameters.keys().collect::<Vec<_>>()
    );

    let measurement = cc7_measure_matte(&engine, &committed, "cc7-d2");
    let [full, covered, partial] = CC7_D2_FEATHER_COUNTS_PIXELS;
    let measured_full = i64::try_from(measurement.statistics.full_pixel_count).expect("a count");
    let measured_covered =
        i64::try_from(measurement.statistics.covered_pixel_count).expect("a count");
    let measured_partial =
        i64::try_from(measurement.statistics.partial_pixel_count).expect("a count");
    for (name, measured, model) in [
        ("full", measured_full, full),
        ("covered", measured_covered, covered),
        ("partial", measured_partial, partial),
    ] {
        assert!(
            (measured - model).abs() <= CC7_FEATHER_PARTIAL_TOLERANCE_PIXELS,
            "the {name} count measured {measured} against the discrete pixel-centre model's \
             {model}, outside the tolerance of {CC7_FEATHER_PARTIAL_TOLERANCE_PIXELS}"
        );
        // §4.1's `feather counts` row claims the model error is measured
        // *exactly* zero, so the claim is asserted rather than left to the
        // tolerance: a drift to 3 would otherwise keep both green while
        // `CC7_BUDGETS` still said "infinite (measured exactly zero)"
        // (R4-m3).
        assert_eq!(
            (measured - model).abs(),
            CC7_MEASURED_FEATHER_MODEL_ERROR_PIXELS,
            "the {name} count must measure the discrete pixel-centre model exactly, which is \
             what CC7_MEASURED_FEATHER_MODEL_ERROR_PIXELS records"
        );
    }
    assert_eq!(measured_partial, covered - full);
    // A7 / A-E12: the continuous-area formula is the WRONG model, by more than
    // the tolerance, on both the nominal and the quantized half-extents.
    let nominal_tenths = CC7_D2_CONTINUOUS_AREA_WRONG_MODEL_PIXELS_TENTHS;
    assert!(
        (measured_partial * 10 - nominal_tenths).abs() > CC7_FEATHER_PARTIAL_TOLERANCE_PIXELS * 10,
        "the continuous-area model ({nominal_tenths} tenths) must not match the measured \
         {measured_partial}, or a reader could re-derive it"
    );
    println!(
        "CC7_D2_FEATHER full={measured_full} covered={measured_covered} partial={measured_partial} \
         centre={CC7_D2_WINDOW_CENTRE_BASIS_POINTS:?} half={CC7_D2_WINDOW_HALF_EXTENTS_BASIS_POINTS:?} \
         feather={CC7_FEATHER_BASIS_POINTS}"
    );
}

/// CC7 §4(d)(4)'s failing direction. The same window at `feather = 0` reports
/// `covered == full == CC7_PRODUCT_PATCH_PIXEL_COUNT`, `partial == 0`, and
/// exact `0 / 255` coverage codes — the hard `D <= 1.0` step.
#[test]
fn cc7_d_feather_zero_has_no_partial_pixels() {
    let gpu = fallback_gpu();
    let engine = cc7_engine(&gpu);
    let scene = cc7_camera_a_scene("cc7-d2-hard");
    let hard = cc7_operation_with(
        &kinewright_core::cc7_scenarios::CC7_D2_OPERATIONS[0],
        &[("matte_window0_feather_basis_points", 0)],
    );
    let committed = scene.commit(
        "cc7-d2-hard",
        &[cc7_primary_insert(CC7_SINGLE_CLIP_ID, &hard)],
    );
    let measurement = cc7_measure_matte(&engine, &committed, "cc7-d2-hard");
    assert_eq!(measurement.statistics.partial_pixel_count, 0);
    assert_eq!(
        measurement.statistics.covered_pixel_count,
        measurement.statistics.full_pixel_count
    );
    assert_eq!(
        measurement.statistics.covered_pixel_count,
        u64::from(CC7_PRODUCT_PATCH_PIXEL_COUNT)
    );
    // Every histogram bucket but the first and last must be empty: coverage is
    // exactly 0 or 255 with no feather.
    let histogram = measurement.statistics.coverage_histogram;
    for (bucket, count) in histogram.iter().enumerate() {
        if bucket == 0 || bucket == histogram.len() - 1 {
            continue;
        }
        assert_eq!(*count, 0, "bucket {bucket} must be empty at feather 0");
    }
}

// ===========================================================================
// §4(e) Creative look.
// ===========================================================================

/// CC7 §11.2.27 — the (e) exit gate. §4(e)(2): the built-in `warm` look drives
/// **exactly** `CC7_LOOK_DEEP_SHADOW_OUT_OF_GAMUT_PIXELS` pixels out of gamut
/// on the `deep_shadow` ROI, with a `delivery_gamut_excursion` **Warning** and
/// `technical_pass == true`. The whole-raster counts are reported, never gated.
///
/// *Fails:* `cc7_e_the_base_scene_without_the_look_is_in_gamut`.
#[test]
fn cc7_warm_look_out_of_gamut_count_is_exact_on_the_deep_shadow_patch() {
    let gpu = fallback_gpu();
    let engine = cc7_engine(&gpu);
    let scene = cc7_camera_a_scene("cc7-e");
    let (warm, library) = cc7_warm_look_asset();
    engine.set_lut_library(library);
    let committed = scene.commit(
        "cc7-e",
        &cc7_lut_backed_canonical_operations(Cc7Scenario::CreativeLook, warm),
    );
    let proof = cc7_working_proof(&engine, &committed, TimeCode::ZERO, "cc7-e");

    // A19: the `ceil`ed `y` is normative. The resolved rect is asserted, not
    // the arithmetic: the naive 4222 resolves to `y 75, h 17` = 204 pixels.
    let resolved = CC7_DEEP_SHADOW_ROI
        .to_pixels(CC7_SOURCE_WIDTH, CC7_SOURCE_HEIGHT)
        .expect("the deep_shadow ROI resolves");
    assert_eq!(
        (resolved.x, resolved.y, resolved.width, resolved.height),
        (
            CC7_ROW_PATCHES[6].rect.x,
            CC7_ROW_PATCHES[6].rect.y,
            CC7_ROW_PATCHES[6].rect.width,
            CC7_ROW_PATCHES[6].rect.height
        )
    );

    let roi_report = measure_color_qc(
        &proof,
        &ColorQcRequest {
            roi: Some(CC7_DEEP_SHADOW_ROI),
            checks: vec![ColorQcCheck::Gamut, ColorQcCheck::Range],
            ..ColorQcRequest::default()
        },
    )
    .expect("the deep_shadow gamut measurement");
    assert_eq!(
        roi_report.visible_pixel_count,
        u64::from(kinewright_core::cc7_scenarios::CC7_ROW_PATCH_PIXELS),
        "the ROI must be the deep_shadow patch and nothing else; \
         CC7_PRODUCT_PATCH_PIXEL_COUNT is (d)'s alias of the same number and would read as \
         gating the wrong ROI (R4-m5)"
    );
    assert_eq!(
        i64::try_from(roi_report.gamut.out_of_gamut_pixel_count).expect("a count"),
        CC7_LOOK_DEEP_SHADOW_OUT_OF_GAMUT_PIXELS,
        "12 x 16, exactly, with zero decode sensitivity"
    );
    assert_eq!(roi_report.gamut.below_black_pixel_count, 0);
    let excursion = roi_report
        .exceptions
        .iter()
        .find(|exception| exception.code == "delivery_gamut_excursion")
        .expect("the look drives the patch out of gamut");
    assert_eq!(excursion.severity, QaSeverity::Warning);
    assert!(
        roi_report.technical_pass,
        "a Warning is not an Error: {:?}",
        roi_report.exceptions
    );

    // Reported, never gated (A19).
    let whole_report = measure_color_qc(
        &proof,
        &ColorQcRequest {
            roi: None,
            checks: vec![ColorQcCheck::Gamut, ColorQcCheck::Range],
            ..ColorQcRequest::default()
        },
    )
    .expect("the whole-raster gamut measurement");
    assert_eq!(
        i64::try_from(whole_report.gamut.out_of_gamut_pixel_count).expect("a count"),
        CC7_WARM_WHOLE_RASTER_OUT_OF_GAMUT_PIXELS_REPORTED
    );
    assert_eq!(
        i64::from(whole_report.gamut.out_of_gamut_basis_points),
        CC7_WARM_WHOLE_RASTER_OUT_OF_GAMUT_BASIS_POINTS
    );
    assert!(whole_report.technical_pass);

    emit_cc7_evidence(
        "cc7_warm_look_out_of_gamut_count_is_exact_on_the_deep_shadow_patch",
        &gpu,
        json!({
            "look": "warm",
            "input_encoding_token": 0,
            "mix_basis_points": CC7_LOOK_MIX_BASIS_POINTS,
            "roi": CC7_DEEP_SHADOW_ROI,
            "blue_zero_crossing_display709_millionths":
                CC7_LOOK_BLUE_ZERO_CROSSING_DISPLAY709_MILLIONTHS,
        }),
        json!({
            "roi_out_of_gamut_pixel_count": roi_report.gamut.out_of_gamut_pixel_count,
            "roi_out_of_gamut_basis_points": roi_report.gamut.out_of_gamut_basis_points,
            "roi_minimum_linear_millionths": roi_report.gamut.minimum_linear_millionths,
            "whole_raster_out_of_gamut_pixel_count": whole_report.gamut.out_of_gamut_pixel_count,
            "whole_raster_out_of_gamut_basis_points": whole_report.gamut.out_of_gamut_basis_points,
            "whole_raster_below_black_pixel_count": whole_report.gamut.below_black_pixel_count,
            "whole_raster_minimum_linear_millionths": whole_report.gamut.minimum_linear_millionths,
            "whole_raster_maximum_desaturation_millionths":
                whole_report.gamut.maximum_desaturation_millionths,
            "whole_raster_range_excursions": whole_report
                .exceptions
                .iter()
                .filter(|exception| exception.code == "delivery_range_excursion")
                .count(),
            "technical_pass": roi_report.technical_pass,
        }),
    );
}

/// CC7 §4(e)(2)'s failing direction. Without the look the same measurement
/// reports `out_of_gamut_pixel_count == 0` on the ROI **and** on the whole
/// raster, and raises no Warning.
#[test]
fn cc7_e_the_base_scene_without_the_look_is_in_gamut() {
    let gpu = fallback_gpu();
    let engine = cc7_engine(&gpu);
    let scene = cc7_camera_a_scene("cc7-e-bare");
    let proof = cc7_working_proof(&engine, &scene.bare(), TimeCode::ZERO, "cc7-e-bare");
    for roi in [Some(CC7_DEEP_SHADOW_ROI), None] {
        let report = measure_color_qc(
            &proof,
            &ColorQcRequest {
                roi,
                checks: vec![ColorQcCheck::Gamut, ColorQcCheck::Range],
                ..ColorQcRequest::default()
            },
        )
        .expect("the base-scene gamut measurement");
        assert_eq!(report.gamut.out_of_gamut_pixel_count, 0, "{roi:?}");
        assert!(
            !report
                .exceptions
                .iter()
                .any(|exception| exception.code == "delivery_gamut_excursion"),
            "the base scene raises no gamut Warning: {:?}",
            report.exceptions
        );
        assert!(report.technical_pass);
        if roi.is_some() {
            // On the `deep_shadow` ROI the base scene raises nothing at all;
            // the whole raster still carries the primaries band's saturated
            // channels, which sit at the top of the delivery range and are a
            // property of the RASTER rather than of the look (Implementer C
            // erratum C-E5).
            assert!(report.exceptions.is_empty(), "{:?}", report.exceptions);
        }
    }
}

// ===========================================================================
// §4(f) Tracked secondary — the source-side containment gate.
// ===========================================================================

/// One sample's containment requirement, in hundredths of a pixel.
struct Cc7ContainmentSample {
    frame: i64,
    required_half_width_hundredths: i64,
    required_half_height_hundredths: i64,
}

/// The half-extent, in hundredths of a pixel, a window centred at
/// `centre_basis_points` needs in order to contain the analytic square whose
/// centre is `square_basis_points`, on an axis of `extent` pixels.
fn cc7_required_half_extent_hundredths(
    centre_basis_points: i64,
    square_basis_points: i64,
    extent: u32,
) -> i64 {
    let scale = f64::from(extent) / 10_000.0;
    let offset = ((square_basis_points - centre_basis_points) as f64 * scale).abs();
    let half = CC7_TRACK_SQUARE_SIZE as f64 / 2.0;
    cc7_round((offset + half) * 100.0)
}

/// Every surviving sample's containment requirement against the committed
/// window's smoothed keyframe centres (A17: the gate reads the keyframes the
/// commit landed, and the analytic square, never `curves`' own claim).
fn cc7_containment_samples() -> Vec<Cc7ContainmentSample> {
    let x = cc7_track_keyframe_centres(0);
    let y = cc7_track_keyframe_centres(1);
    assert_eq!(x.len(), CC7_TRACK_SURVIVING_SAMPLE_FRAMES.len());
    assert_eq!(y.len(), CC7_TRACK_SURVIVING_SAMPLE_FRAMES.len());
    CC7_TRACK_SURVIVING_SAMPLE_FRAMES
        .into_iter()
        .enumerate()
        .map(|(index, frame)| {
            let (square_x, square_y) =
                kinewright_core::cc7_scenarios::cc7_analytic_square_centre_basis_points(frame);
            Cc7ContainmentSample {
                frame,
                required_half_width_hundredths: cc7_required_half_extent_hundredths(
                    x[index],
                    square_x,
                    CC7_SOURCE_WIDTH,
                ),
                required_half_height_hundredths: cc7_required_half_extent_hundredths(
                    y[index],
                    square_y,
                    CC7_SOURCE_HEIGHT,
                ),
            }
        })
        .collect()
}

/// One window half-extent in hundredths of a pixel.
fn cc7_window_half_extent_hundredths(basis_points: i64, extent: u32) -> i64 {
    cc7_round(basis_points as f64 * f64::from(extent) / 10_000.0 * 100.0)
}

/// CC7 §11.2.28. §4(f)(3): the **1.5x** window contains the whole 24 x 24
/// analytic square at every surviving sample except the named final keyframe,
/// which carries the tool's published `known_systematic_lag` and is excluded
/// by name rather than by tolerance.
///
/// *Fails:* `cc7_f_a_window_smaller_than_the_square_loses_containment`.
#[test]
fn cc7_tracked_window_contains_the_square_at_every_sampled_frame() {
    let window_x = cc7_window_half_extent_hundredths(
        CC7_TRACK_WINDOW_HALF_WIDTH_BASIS_POINTS,
        CC7_SOURCE_WIDTH,
    );
    let window_y = cc7_window_half_extent_hundredths(
        CC7_TRACK_WINDOW_HALF_HEIGHT_BASIS_POINTS,
        CC7_SOURCE_HEIGHT,
    );
    let samples = cc7_containment_samples();
    assert_eq!(samples.len(), CC7_TRACK_SURVIVING_SAMPLE_FRAMES.len());

    let mut worst_x = 0_i64;
    let mut worst_y = 0_i64;
    let mut lagging = None;
    for sample in &samples {
        if sample.frame == CC7_TRACK_LAGGING_FINAL_KEYFRAME {
            lagging = Some(sample);
            continue;
        }
        assert!(
            sample.required_half_width_hundredths <= window_x,
            "frame {}: the 1.5x window's {window_x} hundredths of a pixel in x do not contain the \
             square, which needs {}",
            sample.frame,
            sample.required_half_width_hundredths
        );
        assert!(
            sample.required_half_height_hundredths <= window_y,
            "frame {}: the 1.5x window's {window_y} hundredths of a pixel in y do not contain the \
             square, which needs {}",
            sample.frame,
            sample.required_half_height_hundredths
        );
        worst_x = worst_x.max(sample.required_half_width_hundredths);
        worst_y = worst_y.max(sample.required_half_height_hundredths);
    }

    // The excluded sample is named, and it is excluded because it genuinely
    // fails: a gate that silently dropped a sample that passed would be a
    // tolerance in disguise.
    let lagging = lagging.expect("the final keyframe must be one of the surviving samples");
    assert!(
        lagging.required_half_width_hundredths > window_x,
        "the final keyframe {} must be the one the smoother's known_systematic_lag moves, or the \
         exclusion is unearned",
        CC7_TRACK_LAGGING_FINAL_KEYFRAME
    );

    // §2.3.6's reported half-extents and margins, within one hundredth of a
    // pixel of the pinned figures (Implementer C erratum C-E6).
    assert!(
        (worst_x - CC7_TRACK_CONTAINMENT_REQUIRED_HALF_WIDTH_PIXELS_REPORTED).abs() <= 2,
        "the worst required half width measured {worst_x} hundredths against the reported {}",
        CC7_TRACK_CONTAINMENT_REQUIRED_HALF_WIDTH_PIXELS_REPORTED
    );
    assert!(
        (worst_y - CC7_TRACK_CONTAINMENT_REQUIRED_HALF_HEIGHT_PIXELS_REPORTED).abs() <= 2,
        "the worst required half height measured {worst_y} hundredths against the reported {}",
        CC7_TRACK_CONTAINMENT_REQUIRED_HALF_HEIGHT_PIXELS_REPORTED
    );
    assert!(
        (window_x - worst_x - CC7_TRACK_CONTAINMENT_WORST_MARGIN_X_PIXELS_HUNDREDTHS).abs() <= 2
    );
    assert!(
        (window_y - worst_y - CC7_TRACK_CONTAINMENT_WORST_MARGIN_Y_PIXELS_HUNDREDTHS).abs() <= 2
    );
    // The analytic centres this gate reads are §2.3.6's table.
    for (index, frame) in CC7_TRACK_SURVIVING_SAMPLE_FRAMES.into_iter().enumerate() {
        let (square_x, square_y) =
            kinewright_core::cc7_scenarios::cc7_analytic_square_centre_basis_points(frame);
        assert_eq!(
            [square_x, square_y],
            CC7_TRACK_ANALYTIC_CENTRES_BASIS_POINTS[index]
        );
    }
    println!(
        "CC7_F_CONTAINMENT window=({window_x},{window_y}) worst=({worst_x},{worst_y}) \
         margin=({},{}) lagging_frame={} lagging_required=({},{})",
        window_x - worst_x,
        window_y - worst_y,
        lagging.frame,
        lagging.required_half_width_hundredths,
        lagging.required_half_height_hundredths
    );
}

/// CC7 §4(f)(3)'s failing direction. The **seeded 1.0x** window — the one the
/// (f) node actually stores — is short in x and loses containment, which is
/// why the containment gate uses a 1.5x window (A17).
#[test]
fn cc7_f_a_window_smaller_than_the_square_loses_containment() {
    let seeded_x = cc7_window_half_extent_hundredths(
        CC7_TRACK_SEEDED_WINDOW_HALF_WIDTH_BASIS_POINTS,
        CC7_SOURCE_WIDTH,
    );
    let seeded_y = cc7_window_half_extent_hundredths(
        CC7_TRACK_SEEDED_WINDOW_HALF_HEIGHT_BASIS_POINTS,
        CC7_SOURCE_HEIGHT,
    );
    let samples = cc7_containment_samples();
    let short = samples
        .iter()
        .filter(|sample| sample.frame != CC7_TRACK_LAGGING_FINAL_KEYFRAME)
        .filter(|sample| sample.required_half_width_hundredths > seeded_x)
        .collect::<Vec<_>>();
    assert!(
        !short.is_empty(),
        "the seeded 1.0x window must lose containment somewhere, or the 1.5x window is decorative"
    );
    let worst = short
        .iter()
        .map(|sample| sample.required_half_width_hundredths - seeded_x)
        .max()
        .expect("a shortfall");
    // The seeded window is the square's own half extent, to within the
    // basis-point quantization of §2.3.6's `round(12/180 * 10000) = 667`,
    // which resolves back to 12.006 px rather than 12.000.
    let half = (CC7_TRACK_SQUARE_SIZE / 2) * 100;
    assert_eq!(seeded_x, half);
    assert!((seeded_y - half).abs() <= 1, "{seeded_y} against {half}");
    println!(
        "CC7_F_SEEDED_SHORTFALL frames={:?} worst_hundredths={worst}",
        short.iter().map(|sample| sample.frame).collect::<Vec<_>>()
    );
}

// ===========================================================================
// §4(g) Encoded delivery.
// ===========================================================================

/// Everything one scenario's export and verification measured.
struct Cc7DeliveryMeasurement {
    scenario: Cc7Scenario,
    depth: DeliveryEncodeDepth,
    verification: DeliveryVerification,
    export_seconds: f64,
    verify_seconds: f64,
}

/// CC6 §6.2's closed-form sample set for a document of `frames` frames.
fn cc7_expected_sample_frames(frames: i64) -> Vec<i64> {
    let count = i64::from(kinewright_core::DELIVERY_VERIFICATION_FRAME_COUNT);
    (0..count)
        .map(|index| (frames - 1) * index / (count - 1))
        .collect()
}

/// Export one scenario's canonical document through the production path and
/// verify it through the production engine.
fn cc7_export_and_verify(
    gpu: &FixtureGpu,
    plan: &Cc7CanonicalPlan,
    scenario: Cc7Scenario,
    depth: DeliveryEncodeDepth,
    bitrate: Option<u64>,
) -> (TempDirectory, PathBuf, Cc7DeliveryMeasurement) {
    let directory = TempDirectory::new("cc7-delivery");
    let mut settings = DeliveryProfile::SourceMaster.export_settings(
        plan.document.as_ref(),
        depth,
        ExportCancellation::default(),
    );
    if let Some(bitrate) = bitrate {
        settings.video_bitrate = bitrate;
    }
    assert_eq!(settings.resolution, plan.document.resolution);
    assert_eq!(settings.fps, plan.document.fps);
    let output = directory.path("cc7-delivery.mp4");
    let (progress_tx, _progress_rx) = crossbeam_channel::unbounded();
    let started = Instant::now();
    crate::export::export_document_with_luts(
        plan.document.as_ref(),
        &output,
        &settings,
        &progress_tx,
        gpu.context(),
        Arc::clone(&plan.library),
    )
    .expect("the production export path must write the CC7 delivery lane");
    let export_seconds = started.elapsed().as_secs_f64();

    let request = DeliveryVerificationRequest::new(depth, settings.delivery_color.clone());
    assert_eq!(
        request.frame_count,
        kinewright_core::DELIVERY_VERIFICATION_FRAME_COUNT
    );
    let started = Instant::now();
    // R-M7: the trait default is `NotImplemented`, so this gate holds against
    // the real `FfmpegMediaEngine` and the fixture constructs one.
    let verification = plan
        .engine
        .verify_delivery_output(Arc::clone(&plan.document), &output, &settings, request)
        .expect("the written CC7 export must verify");
    let verify_seconds = started.elapsed().as_secs_f64();
    (
        directory,
        output,
        Cc7DeliveryMeasurement {
            scenario,
            depth,
            verification,
            export_seconds,
            verify_seconds,
        },
    )
}

/// §4(g)(1)'s three conditions, plus the probed tags and the sampling rule.
fn assert_cc7_delivery_lane(measurement: &Cc7DeliveryMeasurement, output: &Path) -> Value {
    let verification = &measurement.verification;
    let comparison = &verification.comparison;
    let depth = measurement.depth;
    let label = format!("{:?}/{depth:?}", measurement.scenario);

    assert_eq!(verification.delivery_bit_depth, depth, "{label}");
    assert_eq!(verification.output_path, output, "{label}");
    assert_eq!(
        verification.decoded_pixel_format,
        depth.pixel_format(),
        "{label}"
    );
    assert_eq!(
        verification.probed.primaries,
        ColorPrimaries::Bt709,
        "{label}"
    );
    assert_eq!(
        verification.probed.transfer,
        ColorTransfer::Bt709,
        "{label}"
    );
    assert_eq!(verification.probed.matrix, ColorMatrix::Bt709, "{label}");
    assert_eq!(verification.probed.range, ColorRange::Limited, "{label}");
    assert_eq!(
        verification.probed.bit_depth,
        match depth {
            DeliveryEncodeDepth::Eight => ColorBitDepth::Eight,
            DeliveryEncodeDepth::Ten => ColorBitDepth::Ten,
        },
        "{label}: a lane that silently delivered the other depth would land here"
    );
    assert!(
        verification.tags.conforming,
        "{label}: {:?}",
        verification.tags.mismatches
    );
    assert!(verification.tags.mismatches.is_empty(), "{label}");

    // §6.2's closed-form sampling, transcribed rather than read back.
    let frames = measurement.verification.comparison.frames.len();
    assert_eq!(
        frames,
        usize::from(kinewright_core::DELIVERY_VERIFICATION_FRAME_COUNT),
        "{label}"
    );

    // --- the three conditions of §4(g)(1) --------------------------------
    assert!(
        verification.technical_pass,
        "{label}: {:?}",
        verification.exceptions
    );
    assert!(comparison.within_budgets, "{label}: {comparison:?}");
    for exception in &verification.exceptions {
        assert!(
            CC7_DELIVERY_ALLOWED_INFO_CODES.contains(&exception.code.as_str()),
            "{label}: exception {} is not in CC7_DELIVERY_ALLOWED_INFO_CODES",
            exception.code
        );
        assert_eq!(
            exception.severity,
            QaSeverity::Info,
            "{label}: the allowed set is Info-only"
        );
    }
    // A6: every verified H.264 export carries exactly one Info, on
    // `white_point`, because the format has no white-point field. "No
    // exceptions" would fail a perfectly conforming encode.
    assert_eq!(verification.tags.not_representable.len(), 1, "{label}");
    assert_eq!(
        verification.tags.not_representable[0].field, "white_point",
        "{label}"
    );
    assert_eq!(
        verification
            .exceptions
            .iter()
            .filter(|exception| exception.code == "delivery_tag_not_representable")
            .count(),
        1,
        "{label}"
    );

    // Non-vacuity: the codec is actually exercised.
    assert!(
        comparison.combined.mean_code_diff_millionths > 0,
        "{label}: the source does not exercise the codec"
    );

    let budgets = comparison.budgets;
    assert_eq!(budgets, kinewright_core::DeliveryBudgets::for_depth(depth));

    // R4-M2: every gated term of this lane equals the manifest's recorded
    // measurement for this scenario at this depth, so the per-scenario
    // `budget | measured | margin` triples cannot go stale behind the gate the
    // way `CC7_MEASURED_DELIVERY_EIGHT` did. The manifest is the measured
    // column of §4.1; `CC7_MEASURED_DELIVERY_*` is its worst-per-term
    // summary, and `cc7_manifest_declares_every_required_fixture_and_constant`
    // ties that summary to the same rows.
    let spec_id = cc7_spec(measurement.scenario).id;
    let depth_key = match depth {
        DeliveryEncodeDepth::Eight => "eight_bit",
        DeliveryEncodeDepth::Ten => "ten_bit",
    };
    let declared = &cc7_manifest()["budgets"]["delivery"][spec_id][depth_key];
    let psnr = comparison
        .psnr_db_hundredths
        .expect("a delivery lane whose MSE is non-zero reports a PSNR");
    for (term, measured) in [
        (
            "luma_max_code",
            i64::from(comparison.luma.maximum_code_diff),
        ),
        (
            "luma_p99_code_millionths",
            comparison.luma.p99_code_diff_millionths,
        ),
        (
            "luma_mean_code_millionths",
            comparison.luma.mean_code_diff_millionths,
        ),
        (
            "rgb_mean_code_millionths",
            comparison.combined.mean_code_diff_millionths,
        ),
        ("psnr_db_hundredths", i64::from(psnr)),
    ] {
        assert_manifest_i64(&declared[term], "measured", measured);
    }

    json!({
        "scenario": cc7_spec(measurement.scenario).id,
        "depth": format!("{depth:?}"),
        "sample_frames": comparison.frames,
        "luma_max_code": comparison.luma.maximum_code_diff,
        "luma_p99_code_millionths": comparison.luma.p99_code_diff_millionths,
        "luma_mean_code_millionths": comparison.luma.mean_code_diff_millionths,
        "rgb_mean_code_millionths": comparison.combined.mean_code_diff_millionths,
        "psnr_db_hundredths": comparison.psnr_db_hundredths,
        "rgb_max_code": comparison.combined.maximum_code_diff,
        "rgb_p99_code_millionths": comparison.combined.p99_code_diff_millionths,
        "red_mean_code_millionths": comparison.red.mean_code_diff_millionths,
        "green_mean_code_millionths": comparison.green.mean_code_diff_millionths,
        "blue_mean_code_millionths": comparison.blue.mean_code_diff_millionths,
        "budgets": {
            "luma_max_code": budgets.luma_max_code,
            "luma_p99_code_millionths": budgets.luma_p99_code_millionths,
            "luma_mean_code_millionths": budgets.luma_mean_code_millionths,
            "rgb_mean_code_millionths": budgets.rgb_mean_code_millionths,
            "psnr_floor_db_hundredths": budgets.psnr_floor_db_hundredths,
        },
        "export_seconds": measurement.export_seconds,
        "verify_seconds": measurement.verify_seconds,
    })
}

/// The shared body of §11.2.29's two exit gates: every scenario's canonical
/// document, exported and verified at one depth.
fn assert_cc7_every_scenario_verifies(depth: DeliveryEncodeDepth, fixture: &str) {
    let gpu = fallback_gpu();
    let mut lanes = Vec::new();
    for scenario in CC7_SCENARIOS {
        let plan = cc7_canonical_plan(&gpu, scenario, "g");
        let expected_frames = cc7_expected_sample_frames(plan.document.duration.0);
        let (directory, output, measurement) =
            cc7_export_and_verify(&gpu, &plan, scenario, depth, None);
        assert_eq!(
            measurement.verification.comparison.frames, expected_frames,
            "{scenario:?}: §6.2's closed form on this document's frame count"
        );
        lanes.push(assert_cc7_delivery_lane(&measurement, &output));
        drop(directory);
    }
    assert_eq!(lanes.len(), CC7_SCENARIOS.len());
    emit_cc7_evidence(
        fixture,
        &gpu,
        json!({
            "profile": "SourceMaster",
            "depth": format!("{depth:?}"),
            "allowed_info_codes": CC7_DELIVERY_ALLOWED_INFO_CODES,
            "budget_note": "CC6 owns these constants; CC7 measures against them and never \
                            re-baselines one",
        }),
        json!({"lanes": lanes}),
    );
}

/// CC7 §11.2.29 and §4(g)(1). Every scenario's canonical document verifies at
/// **eight** bits: `technical_pass`, `within_budgets`, and every exception
/// code in `CC7_DELIVERY_ALLOWED_INFO_CODES`.
///
/// *Fails:* `cc7_g_a_starved_encode_trips_the_decoded_difference_budget`.
#[test]
fn cc7_every_scenario_verifies_at_eight_bits() {
    assert_cc7_every_scenario_verifies(
        DeliveryEncodeDepth::Eight,
        "cc7_every_scenario_verifies_at_eight_bits",
    );
}

/// CC7 §11.2.29 and §4(g)(1), the **ten**-bit lane (A10: it is no longer a cut
/// candidate).
#[test]
fn cc7_every_scenario_verifies_at_ten_bits() {
    assert_cc7_every_scenario_verifies(
        DeliveryEncodeDepth::Ten,
        "cc7_every_scenario_verifies_at_ten_bits",
    );
}

/// CC7 §4(g)(1)'s failing direction. One scenario at `-b:v 100k` reports
/// `within_budgets == false` with a `decoded_difference_over_budget`
/// **Error**, `technical_pass == false`, and the output file still at its
/// original path, unrenamed and undeleted.
#[test]
fn cc7_g_a_starved_encode_trips_the_decoded_difference_budget() {
    let gpu = fallback_gpu();
    let plan = cc7_canonical_plan(&gpu, Cc7Scenario::MixedCamera, "g-starved");
    let (_directory, output, measurement) = cc7_export_and_verify(
        &gpu,
        &plan,
        Cc7Scenario::MixedCamera,
        DeliveryEncodeDepth::Eight,
        Some(CC7_STARVED_BITRATE_BITS_PER_SECOND),
    );
    let verification = &measurement.verification;
    let comparison = &verification.comparison;
    assert!(!comparison.within_budgets, "{comparison:?}");
    assert!(
        !verification.technical_pass,
        "{:?}",
        verification.exceptions
    );
    let error = verification
        .exceptions
        .iter()
        .find(|exception| exception.code == "decoded_difference_over_budget")
        .expect("a starved encode must trip the decoded difference budget");
    assert_eq!(error.severity, QaSeverity::Error);
    assert!(error.field.is_some(), "{error:?}");
    assert!(error.observed.is_some(), "{error:?}");
    assert!(error.allowed.is_some(), "{error:?}");
    // A measurement never moves, renames, or deletes the encode it just read.
    assert!(
        output.exists(),
        "the starved output must survive verification"
    );
    assert_eq!(verification.output_path, output);
    assert!(
        i64::from(comparison.luma.maximum_code_diff) > i64::from(comparison.budgets.luma_max_code),
        "the starved lane must exceed the luma maximum it is measured against"
    );
    println!(
        "CC7_G_STARVED bitrate={CC7_STARVED_BITRATE_BITS_PER_SECOND} luma_max={} luma_p99={} \
         luma_mean={} rgb_mean={} psnr={:?} field={:?} observed={:?} allowed={:?}",
        comparison.luma.maximum_code_diff,
        comparison.luma.p99_code_diff_millionths,
        comparison.luma.mean_code_diff_millionths,
        comparison.combined.mean_code_diff_millionths,
        comparison.psnr_db_hundredths,
        error.field,
        error.observed,
        error.allowed
    );
}

/// §4.2's starved-encode bitrate: CC6's own `-b:v 100k`, reused rather than
/// re-chosen, so the failing direction is the one CC6 already measured.
const CC7_STARVED_BITRATE_BITS_PER_SECOND: u64 = 100_000;

// ===========================================================================
// §4(a)(5) Render parity — the compositor-layer case (A5).
// ===========================================================================

/// A `WorkingFrame` carrying the CC7 base scene in scene-linear light.
///
/// The display codes are decoded through `cc7_scenarios`' own transcription,
/// which is *authoring the input surface*, not obtaining an expected value:
/// the expectation is the CPU reference the compositor is compared against.
fn cc7_base_scene_working_frame() -> WorkingFrame {
    let mut rgb = Vec::with_capacity((CC7_SOURCE_WIDTH * CC7_SOURCE_HEIGHT) as usize);
    for y in 0..CC7_SOURCE_HEIGHT {
        for x in 0..CC7_SOURCE_WIDTH {
            let codes = crate::cc7_sources::cc7_base_scene_rgb(x, y);
            rgb.push(codes.map(|code| cc7_decode_display709(f64::from(code) / 255.0) as f32));
        }
    }
    working_frame(CC7_SOURCE_WIDTH, CC7_SOURCE_HEIGHT, &rgb)
}

/// The canonical (a) node stack as `Effect`s, for the compositor layer.
fn cc7_canonical_effects(operations: &[Cc7Operation]) -> Vec<Effect> {
    operations
        .iter()
        .enumerate()
        .map(|(index, operation)| Effect {
            id: EffectId(index as u64 + 1),
            name: operation.effect_name.to_owned(),
            parameters: operation
                .parameters
                .iter()
                .map(|(name, value)| ((*name).to_owned(), ParamValue::Integer(*value)))
                .collect::<BTreeMap<_, _>>(),
            keyframes: BTreeMap::new(),
        })
        .collect()
}

/// §4(a)(5)'s shared body, on one lane.
fn assert_cc7_canonical_parity(gpu: &FixtureGpu, fixture: &str) {
    let frame = cc7_base_scene_working_frame();
    let resolution = (CC7_SOURCE_WIDTH, CC7_SOURCE_HEIGHT);
    let effects = cc7_canonical_effects(&CC7_A_OPERATIONS);
    let nodes = crate::color_pipeline::resolve_color_nodes(&effects)
        .expect("the canonical (a) node stack must resolve");
    let compositor = Compositor::new(gpu.context());

    // The independent CPU reference: the same f32 node math, evaluated here.
    let aspect = f64::from(CC7_SOURCE_WIDTH) as f32 / f64::from(CC7_SOURCE_HEIGHT) as f32;
    let width = CC7_SOURCE_WIDTH as usize;
    let expected = frame
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .flat_map(|(index, rgba)| {
            let uv = [
                ((index % width) as f32 + 0.5) / CC7_SOURCE_WIDTH as f32,
                ((index / width) as f32 + 0.5) / CC7_SOURCE_HEIGHT as f32,
            ];
            let output = crate::color_pipeline::apply_color_nodes_at(
                &nodes,
                [rgba[0].to_f32(), rgba[1].to_f32(), rgba[2].to_f32()],
                uv,
                aspect,
            );
            output
                .into_iter()
                .map(|value| f16::from_f32(value).to_f32())
                .chain(std::iter::once(f16::from_f32(rgba[3].to_f32()).to_f32()))
        })
        .collect::<Vec<f32>>();

    let actual = compositor
        .render_working(
            resolution,
            &[CompositorLayer {
                frame: &frame,
                effects: &effects,
                transition: TransitionRenderParams::default(),
            }],
        )
        .expect("the production GPU working-surface readback")
        .pixels;
    let metrics = linear_parity_metrics(&actual, &expected);
    assert!(
        metrics.in_gamut_samples > 0,
        "the in-gamut band must not be empty, or the linear gate was never applied: {metrics:?}"
    );
    assert_eq!(metrics.non_finite, 0, "{metrics:?}");
    // `LINEAR_CPU_GPU_{MAX,P99,MEAN}` are CC1's, reused unchanged (A5).
    assert_linear_parity(&metrics, "cc7_canonical_node_stack");

    // The CC6 negative control: the same comparison against a reference
    // perturbed by `2 x LINEAR_CPU_GPU_MAX` must fail.
    // Only the three colour channels move: `linear_parity_metrics` asserts
    // the alpha byte never changes, and a perturbed alpha would trip that
    // rather than the gate under test.
    let perturbed = expected
        .iter()
        .enumerate()
        .map(|(index, value)| {
            if index % 4 == 3 {
                *value
            } else {
                value + 2.0 * crate::cc1_fixtures::LINEAR_CPU_GPU_MAX
            }
        })
        .collect::<Vec<f32>>();
    let control = linear_parity_metrics(&actual, &perturbed);
    assert!(
        control.in_gamut.max > crate::cc1_fixtures::LINEAR_CPU_GPU_MAX,
        "the negative control must exceed the gate the real comparison passes: {control:?}"
    );

    // Document-level render determinism, recorded as EVIDENCE, not as a
    // budget (A5): two renders of the canonical document's working surface on
    // one lane agree exactly.
    let repeat = compositor
        .render_working(
            resolution,
            &[CompositorLayer {
                frame: &frame,
                effects: &effects,
                transition: TransitionRenderParams::default(),
            }],
        )
        .expect("the second production readback")
        .pixels;
    let determinism = linear_parity_metrics(&actual, &repeat);
    assert_eq!(determinism.non_finite, 0);
    assert_eq!(determinism.in_gamut.max, 0.0, "{determinism:?}");
    assert_eq!(determinism.over_range.max, 0.0, "{determinism:?}");
    assert_eq!(
        determinism.compared() + determinism.above_domain,
        (CC7_SOURCE_WIDTH * CC7_SOURCE_HEIGHT * 3) as usize
    );

    if fixture == CC7_EVIDENCE_FIXTURES[7] {
        emit_cc7_evidence(
            fixture,
            gpu,
            json!({
                "node_stack": "cc7 canonical (a) primary_correction",
                "surface": "Compositor::render_working",
            }),
            json!({
                "parity": metrics.as_json(),
                "determinism": determinism.as_json(),
                "determinism_samples": determinism.compared() + determinism.above_domain,
                "negative_control_max": control.in_gamut.max,
            }),
        );
    }
    println!(
        "CC7_PARITY lane={} max={} p99={} mean={} samples={} determinism_max={}",
        gpu.lane.id(),
        metrics.in_gamut.max,
        metrics.in_gamut.p99,
        metrics.in_gamut.mean,
        metrics.compared(),
        determinism.in_gamut.max
    );
}

/// CC7 §11.2.29's parity sibling and §4(a)(5). The canonical (a) node stack
/// on the CC7 raster matches the independent CPU reference inside CC1's
/// `LINEAR_CPU_GPU_*` band on the default (software-fallback) lane, and two
/// renders agree exactly.
#[test]
fn cc7_canonical_node_stack_matches_the_cpu_reference_on_the_software_lane() {
    let gpu = fallback_gpu();
    assert_cc7_canonical_parity(
        &gpu,
        "cc7_canonical_node_stack_matches_the_cpu_reference_on_the_software_lane",
    );
}

/// The `#[ignore]` hardware twin of the parity gate (rule 11.0.6).
#[test]
#[ignore = "requires a real GPU adapter; the default lane is fallback_gpu()"]
fn cc7_canonical_node_stack_matches_the_cpu_reference_on_hardware() {
    let gpu = hardware_gpu();
    assert_cc7_canonical_parity(
        &gpu,
        "cc7_canonical_node_stack_matches_the_cpu_reference_on_hardware",
    );
}

// ===========================================================================
// §11.2.12b: the transcription cross-check (R-M2).
// ===========================================================================

/// CC7 §11.2.12b. `cc7_scenarios`' own `f64` transcriptions of `encode_bt709`,
/// `decode_display709` and `grade709_decode` agree with
/// `kinewright_media::color_pipeline`'s real functions within `1e-6` at every
/// value in §2.4.1's and §2.4.2's tables, at the twelve chart codes, at the
/// five primaries, and across a dense sweep of `-2.0 ..= 2.0` in steps of
/// `1/4096` including both sides of every seam.
///
/// It lives in media because core cannot see the pipeline and the pipeline's
/// crate can see both. A transcription nobody cross-checks is a second
/// definition; a transcription with a cross-check is a boundary.
///
/// *Fails:* the same comparison against a deliberately mis-seamed
/// transcription (`linear <= 0.018`) differs at `0.018`, so the sweep is known
/// to be able to see a one-branch error.
#[test]
fn cc7_core_transcriptions_agree_with_the_pipeline() {
    let tolerance = kinewright_core::cc7_scenarios::CC7_SPEC_F64_TOLERANCE;
    let mut compared = 0_u64;
    let mut worst = 0.0_f64;
    let mut check = |label: &str, value: f64| {
        for (name, transcribed, production) in [
            (
                "encode_bt709",
                cc7_encode_bt709(value),
                f64::from(encode_bt709(value as f32)),
            ),
            (
                "decode_display709",
                cc7_decode_display709(value),
                f64::from(decode_display709(value as f32)),
            ),
            (
                "grade709_decode",
                cc7_grade709_decode(value),
                f64::from(grade709_decode(value as f32)),
            ),
        ] {
            let error = (transcribed - production).abs();
            // §2.7 and §11.2.12b both state a **flat** `1e-6`; the relative
            // widening this used to carry reached ~1.4e-6 over the sweep,
            // while the worst measured error is 8.57e-7 (C-E12, R4-m9).
            assert!(
                error <= tolerance,
                "{label}: {name}({value}) transcribed {transcribed}, production {production}, \
                 error {error}"
            );
            worst = worst.max(error);
            compared += 1;
        }
    };

    // §2.4.1's grade709 column, and the display column it produces.
    for patch in CC7_ROW_PATCHES {
        let grade = patch.grade709.expect("a row patch carries a grade709");
        for channel in grade {
            check("row patch grade709", channel as f64 / 1_000_000.0);
        }
        for channel in patch.linear_millionths_cam_a {
            check("row patch linear", channel as f64 / 1_000_000.0);
        }
    }
    // The twelve chart codes and the five primaries, as normalized display
    // values.
    for patch in CC7_CHART_PATCHES.iter().chain(CC7_PRIMARY_PATCHES.iter()) {
        for code in patch.display_code_cam_a {
            check("patch code", f64::from(code) / 255.0);
        }
    }
    // §2.4.2's linear anchors.
    for millionths in kinewright_core::cc7_scenarios::CC7_CHART_LINEAR_MILLIONTHS {
        check("chart linear", millionths as f64 / 1_000_000.0);
    }

    // The dense sweep, including both sides of every seam.
    let step = 1.0 / 4096.0;
    let mut index = -2.0 * 4096.0;
    while index <= 2.0 * 4096.0 {
        check("sweep", index * step);
        index += 1.0;
    }
    // Both sides of every seam. The offset is `1e-6` rather than an `f64`
    // hair: the production functions are `f32`, whose ULP at `0.018` is
    // `1.9e-9`, so a smaller offset would put the `f64` transcription on the
    // linear side of the branch and the `f32` original on the power side and
    // measure the seam's own representation rather than the transcription.
    for seam in [0.018_f64, 0.081, 0.081_242_86, 0.0, 1.0] {
        for offset in [-1e-6, 0.0, 1e-6] {
            check("seam", seam + offset);
        }
    }

    // The failing direction: a transcription mis-seamed at `linear <= 0.018`
    // differs from the pipeline at exactly that value, so the sweep above is
    // known to be able to see a one-branch error.
    let mis_seamed = |linear: f64| -> f64 {
        if linear <= 0.018 {
            4.5 * linear
        } else {
            1.099 * linear.powf(0.45) - 0.099
        }
    };
    let seam_error = (mis_seamed(0.018) - cc7_encode_bt709(0.018)).abs();
    assert!(
        seam_error > tolerance,
        "a mis-seamed transcription must differ by more than the tolerance at 0.018, or the \
         cross-check could not see a one-branch error; measured {seam_error}"
    );
    println!("CC7_TRANSCRIPTIONS compared={compared} worst_error={worst} tolerance={tolerance}");
}

// ===========================================================================
// R-M15: the per-scenario luma percentiles the manifest records.
// ===========================================================================

/// CC7 §11.3 and R-M15. Every scenario's canonical document is measured at its
/// sample frame and its `{first, median, ninety_ninth}` luma percentiles — the
/// **16-bit** codes `analyze_color_shot` publishes — are asserted equal to the
/// manifest's `raster.luma_percentiles`, so the manifest records a measurement
/// rather than a claim.
///
/// The unit is asserted too (A21): the percentile fields are `8-bit x 257`,
/// while `mean` is a normalized mean in millionths and is the wrong field.
#[test]
fn cc7_scenario_luma_percentiles_match_the_manifest() {
    use kinewright_core::{ScopeFrame, ScopeRequest};
    let gpu = fallback_gpu();
    let manifest = cc7_manifest();
    let declared = manifest["raster"]["luma_percentiles"]
        .as_object()
        .expect("the manifest must record per-scenario luma percentiles");
    assert_eq!(declared.len(), CC7_SCENARIOS.len());

    for scenario in CC7_SCENARIOS {
        let spec = cc7_spec(scenario);
        let plan = cc7_canonical_plan(&gpu, scenario, "luma");
        let at = cc7_target_frame(scenario);
        let raster = cc7_monitor_raster(&plan.engine, &plan.document, at, spec.id);
        let evidence = measure_scopes(&[ScopeFrame::new(at.0, &raster)], &ScopeRequest::default())
            .expect("the CC7 luma percentiles measure");
        let luma = evidence.statistics.luma;
        let entry = &declared[spec.id];
        assert_eq!(entry["frame"], at.0, "{}", spec.id);
        assert_eq!(entry["unit"], "sixteen_bit_code", "{}", spec.id);
        for (key, measured) in [
            ("first", luma.first_percentile),
            ("median", luma.median),
            ("ninety_ninth", luma.ninety_ninth_percentile),
        ] {
            assert_eq!(
                entry[key].as_i64(),
                Some(i64::from(measured)),
                "{}: manifest {key} disagrees with the measurement",
                spec.id
            );
            // The unit: every published percentile is an 8-bit code times 257.
            assert_eq!(
                i64::from(measured) % kinewright_core::cc7_scenarios::CC7_SCOPE_SIXTEEN_BIT_SCALE,
                0,
                "{}: {key} is not an 8-bit code x 257",
                spec.id
            );
        }
        println!(
            "CC7_LUMA_PERCENTILES scenario={} frame={} p1={} p50={} p99={}",
            spec.id, at.0, luma.first_percentile, luma.median, luma.ninety_ninth_percentile
        );
    }
}

// ===========================================================================
// §11.3: the manifest.
// ===========================================================================

fn cc7_manifest() -> Value {
    serde_json::from_str(include_str!("../tests/fixtures/cc7_manifest.json"))
        .expect("CC7 fixture manifest must be valid JSON")
}

fn assert_manifest_i64(parent: &Value, key: &str, expected: i64) {
    let declared = parent
        .get(key)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("manifest must declare an integer {key}"));
    assert_eq!(
        declared, expected,
        "manifest {key} does not match the code constant"
    );
}

/// Every `cc7_scenarios` constant the manifest must declare, paired with the
/// code constant it is asserted **equal to** (rule 11.0.3: never restated as a
/// literal).
fn cc7_threshold_constants() -> Vec<(&'static str, i64)> {
    use kinewright_core::cc7_scenarios as spec;
    vec![
        // --- §2.6 gated ---------------------------------------------------
        (
            "cc7_match_neutral_spread_max_code",
            CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE,
        ),
        (
            "cc7_b1_residual_spread_max_code",
            CC7_B1_RESIDUAL_SPREAD_MAX_CODE,
        ),
        (
            "cc7_match_luma_mean_max_code_millionths",
            CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS,
        ),
        (
            "cc7_log_first_percentile_min_code16",
            spec::CC7_LOG_FIRST_PERCENTILE_MIN_CODE16,
        ),
        ("cc7_log_p99_max_code16", spec::CC7_LOG_P99_MAX_CODE16),
        ("cc7_log_inverse_max_code", CC7_LOG_INVERSE_MAX_CODE),
        ("cc7_log_cube_size", i64::from(CC7_LOG_CUBE_SIZE)),
        (
            "cc7_feather_partial_tolerance_pixels",
            CC7_FEATHER_PARTIAL_TOLERANCE_PIXELS,
        ),
        (
            "cc7_look_deep_shadow_out_of_gamut_pixels",
            CC7_LOOK_DEEP_SHADOW_OUT_OF_GAMUT_PIXELS,
        ),
        (
            "cc7_track_min_confidence_basis_points",
            spec::CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS,
        ),
        (
            "cc7_track_tolerance_basis_points",
            spec::CC7_TRACK_TOLERANCE_BASIS_POINTS,
        ),
        (
            "cc7_track_range_end_local_frame",
            spec::CC7_TRACK_RANGE_END_LOCAL_FRAME,
        ),
        ("cc7_track_f2_step_frames", spec::CC7_TRACK_F2_STEP_FRAMES),
        // --- §2.6 reported, never gated -----------------------------------
        (
            "cc7_unrecoverable_residual_spread_reported_code",
            CC7_UNRECOVERABLE_RESIDUAL_SPREAD_REPORTED_CODE,
        ),
        (
            "cc7_c2_skin_in_band_reported_basis_points",
            spec::CC7_C2_SKIN_IN_BAND_REPORTED_BASIS_POINTS,
        ),
        (
            "cc7_c2_over_range_pixels_reported",
            CC7_C2_OVER_RANGE_PIXELS_REPORTED,
        ),
        (
            "cc7_c2_over_range_basis_points_reported",
            CC7_C2_OVER_RANGE_BASIS_POINTS_REPORTED,
        ),
        (
            "cc7_warm_whole_raster_out_of_gamut_pixels_reported",
            CC7_WARM_WHOLE_RASTER_OUT_OF_GAMUT_PIXELS_REPORTED,
        ),
        (
            "cc7_warm_whole_raster_out_of_gamut_basis_points",
            CC7_WARM_WHOLE_RASTER_OUT_OF_GAMUT_BASIS_POINTS,
        ),
        (
            "cc7_log_black_patch_reported_code",
            CC7_LOG_BLACK_PATCH_REPORTED_CODE,
        ),
        (
            "cc7_log_primary_reported_code",
            CC7_LOG_PRIMARY_REPORTED_CODE,
        ),
        (
            "cc7_log_identity_cube_reported_code",
            CC7_LOG_IDENTITY_CUBE_REPORTED_CODE,
        ),
        ("cc7_log_cube_bytes_reported", CC7_LOG_CUBE_BYTES_REPORTED),
        (
            "cc7_track_occluded_confidence_max_reported",
            spec::CC7_TRACK_OCCLUDED_CONFIDENCE_MAX_REPORTED,
        ),
        (
            "cc7_track_clean_confidence_min_reported",
            spec::CC7_TRACK_CLEAN_CONFIDENCE_MIN_REPORTED,
        ),
        (
            "cc7_track_occluded_confidence_on_the_recipe_reported",
            spec::CC7_TRACK_OCCLUDED_CONFIDENCE_ON_THE_RECIPE_REPORTED,
        ),
        (
            "cc7_track_f2_occluded_confidence_reported",
            spec::CC7_TRACK_F2_OCCLUDED_CONFIDENCE_REPORTED,
        ),
        (
            "cc7_track_worst_raw_observation_error_basis_points_reported",
            spec::CC7_TRACK_WORST_RAW_OBSERVATION_ERROR_BASIS_POINTS_REPORTED,
        ),
        (
            "cc7_track_final_keyframe_lag_basis_points_reported",
            spec::CC7_TRACK_FINAL_KEYFRAME_LAG_BASIS_POINTS_REPORTED,
        ),
        (
            "cc7_track_no_reacquisition_drift_basis_points",
            spec::CC7_TRACK_NO_REACQUISITION_DRIFT_BASIS_POINTS,
        ),
        (
            "cc7_d2_continuous_area_wrong_model_pixels_tenths",
            CC7_D2_CONTINUOUS_AREA_WRONG_MODEL_PIXELS_TENTHS,
        ),
        // --- §2.6 exact, derived rather than measured ----------------------
        ("cc7_source_width", i64::from(CC7_SOURCE_WIDTH)),
        ("cc7_source_height", i64::from(CC7_SOURCE_HEIGHT)),
        ("cc7_source_fps", i64::from(CC7_SOURCE_FPS)),
        ("cc7_source_frames", i64::from(CC7_SOURCE_FRAMES)),
        ("cc7_track_frames", i64::from(spec::CC7_TRACK_FRAMES)),
        ("cc7_surround_code", i64::from(spec::CC7_SURROUND_CODE)),
        (
            "cc7_chart_patch_width",
            i64::from(spec::CC7_CHART_PATCH_WIDTH),
        ),
        ("cc7_row_patch_width", i64::from(spec::CC7_ROW_PATCH_WIDTH)),
        (
            "cc7_chart_patch_pixels",
            i64::from(spec::CC7_CHART_PATCH_PIXELS),
        ),
        (
            "cc7_primary_patch_count",
            spec::CC7_PRIMARY_PATCH_COUNT as i64,
        ),
        ("cc7_chart_patch_count", spec::CC7_CHART_PATCH_COUNT as i64),
        (
            "cc7_row_patch_pixels",
            i64::from(spec::CC7_ROW_PATCH_PIXELS),
        ),
        (
            "cc7_product_patch_pixel_count",
            i64::from(CC7_PRODUCT_PATCH_PIXEL_COUNT),
        ),
        ("cc7_track_square_size", CC7_TRACK_SQUARE_SIZE),
        ("cc7_track_step_frames", spec::CC7_TRACK_STEP_FRAMES),
        (
            "cc7_track_search_radius_percent",
            spec::CC7_TRACK_SEARCH_RADIUS_PERCENT,
        ),
        ("cc7_track_max_width", spec::CC7_TRACK_MAX_WIDTH),
        (
            "cc7_track_occlusion_first_frame",
            spec::CC7_TRACK_OCCLUSION_FIRST_FRAME,
        ),
        (
            "cc7_track_occlusion_last_frame",
            spec::CC7_TRACK_OCCLUSION_LAST_FRAME,
        ),
        ("cc7_track_centre_x_pixels", spec::CC7_TRACK_CENTRE_X_PIXELS),
        (
            "cc7_track_amplitude_x_pixels",
            spec::CC7_TRACK_AMPLITUDE_X_PIXELS,
        ),
        ("cc7_track_centre_y_pixels", spec::CC7_TRACK_CENTRE_Y_PIXELS),
        (
            "cc7_track_amplitude_y_pixels",
            spec::CC7_TRACK_AMPLITUDE_Y_PIXELS,
        ),
        (
            "cc7_track_lagging_final_keyframe",
            CC7_TRACK_LAGGING_FINAL_KEYFRAME,
        ),
        (
            "cc7_track_seeded_window_half_width_basis_points",
            CC7_TRACK_SEEDED_WINDOW_HALF_WIDTH_BASIS_POINTS,
        ),
        (
            "cc7_track_seeded_window_half_height_basis_points",
            CC7_TRACK_SEEDED_WINDOW_HALF_HEIGHT_BASIS_POINTS,
        ),
        (
            "cc7_track_window_half_width_basis_points",
            CC7_TRACK_WINDOW_HALF_WIDTH_BASIS_POINTS,
        ),
        (
            "cc7_track_window_half_height_basis_points",
            CC7_TRACK_WINDOW_HALF_HEIGHT_BASIS_POINTS,
        ),
        (
            "cc7_track_containment_required_half_width_pixels_reported",
            CC7_TRACK_CONTAINMENT_REQUIRED_HALF_WIDTH_PIXELS_REPORTED,
        ),
        (
            "cc7_track_containment_required_half_height_pixels_reported",
            CC7_TRACK_CONTAINMENT_REQUIRED_HALF_HEIGHT_PIXELS_REPORTED,
        ),
        (
            "cc7_track_containment_worst_margin_x_pixels_hundredths",
            CC7_TRACK_CONTAINMENT_WORST_MARGIN_X_PIXELS_HUNDREDTHS,
        ),
        (
            "cc7_track_containment_worst_margin_y_pixels_hundredths",
            CC7_TRACK_CONTAINMENT_WORST_MARGIN_Y_PIXELS_HUNDREDTHS,
        ),
        (
            "cc7_track_confidence_separation_min_basis_points",
            spec::CC7_TRACK_CONFIDENCE_SEPARATION_MIN_BASIS_POINTS,
        ),
        ("cc7_feather_basis_points", CC7_FEATHER_BASIS_POINTS),
        (
            "cc7_secondary_saturation_percent",
            CC7_SECONDARY_SATURATION_PERCENT,
        ),
        ("cc7_look_mix_basis_points", CC7_LOOK_MIX_BASIS_POINTS),
        ("cc7_log_offset_stops", spec::CC7_LOG_OFFSET_STOPS),
        ("cc7_log_span_stops", spec::CC7_LOG_SPAN_STOPS),
        (
            "cc7_log_floor_linear_millionths",
            spec::CC7_LOG_FLOOR_LINEAR_MILLIONTHS,
        ),
        (
            "cc7_look_blue_zero_crossing_display709_millionths",
            CC7_LOOK_BLUE_ZERO_CROSSING_DISPLAY709_MILLIONTHS,
        ),
        (
            "cc7_look_blue_zero_crossing_linear_millionths",
            spec::CC7_LOOK_BLUE_ZERO_CROSSING_LINEAR_MILLIONTHS,
        ),
        (
            "cc7_skin_in_band_exact_basis_points",
            CC7_SKIN_IN_BAND_EXACT_BASIS_POINTS,
        ),
        (
            "cc7_matte_outside_changed_pixels_max",
            CC7_MATTE_OUTSIDE_CHANGED_PIXELS_MAX,
        ),
        (
            "cc7_delivery_leg_budget_seconds_linux",
            spec::CC7_DELIVERY_LEG_BUDGET_SECONDS_LINUX,
        ),
        ("cc7_scope_sixteen_bit_scale", CC7_SCOPE_SIXTEEN_BIT_SCALE),
        (
            "cc7_log_first_percentile_min_code8_prose",
            spec::CC7_LOG_FIRST_PERCENTILE_MIN_CODE8_PROSE,
        ),
        (
            "cc7_log_p99_max_code8_prose",
            spec::CC7_LOG_P99_MAX_CODE8_PROSE,
        ),
        (
            "cc7_matte_sample_hue_width_centidegrees",
            spec::CC7_MATTE_SAMPLE_HUE_WIDTH_CENTIDEGREES,
        ),
        ("cc7_matte_sample_softness", spec::CC7_MATTE_SAMPLE_SOFTNESS),
        (
            "cc7_matte_sample_band_margin_basis_points",
            spec::CC7_MATTE_SAMPLE_BAND_MARGIN_BASIS_POINTS,
        ),
        (
            "cc7_product_sample_hue_median_centidegrees",
            spec::CC7_PRODUCT_SAMPLE_HUE_MEDIAN_CENTIDEGREES,
        ),
        (
            "cc7_product_sample_saturation_basis_points",
            spec::CC7_PRODUCT_SAMPLE_SATURATION_BASIS_POINTS,
        ),
        (
            "cc7_product_sample_luma_basis_points",
            spec::CC7_PRODUCT_SAMPLE_LUMA_BASIS_POINTS,
        ),
    ]
}

/// Every parameter name a CC7 canonical document can carry, derived from
/// `cc7_scenarios` rather than from a hand-maintained list.
///
/// This is the media-side copy of `kinewright-eval.rs`'s
/// `cc7_canonical_parameter_names` (B-E9), which the media crate cannot see;
/// the manifest records the resolved set so the two are comparable.
fn cc7_canonical_parameter_names() -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();
    for scenario in CC7_SCENARIOS {
        for operation in cc7_spec(scenario).canonical_operations {
            names.extend(
                operation
                    .parameters
                    .iter()
                    .map(|(name, _)| (*name).to_owned()),
            );
        }
    }
    for operations in [
        CC7_B1_OPERATIONS.as_slice(),
        kinewright_core::cc7_scenarios::CC7_D2_OPERATIONS.as_slice(),
    ] {
        for operation in operations {
            names.extend(
                operation
                    .parameters
                    .iter()
                    .map(|(name, _)| (*name).to_owned()),
            );
        }
    }
    names.extend(kinewright_core::cc7_scenarios::CC7_F_KEYFRAMED_PARAMETERS.map(str::to_owned));
    // The two descriptor neutrals the canonical documents deliberately do NOT
    // store, and which a leaking form would still name.
    names.extend([
        "input_encoding_token".to_owned(),
        "mix_basis_points".to_owned(),
    ]);
    names.into_iter().collect()
}

/// CC7 §11.3 and §11.2.33 — the **constant half** of the inventory test.
///
/// Every declared threshold is asserted **equal to the code constant** the
/// fixtures gate with, never restated as a literal (rule 11.0.3), and the key
/// count is asserted so a constant cannot be added to `cc7_scenarios` without
/// being declared here.
///
/// The **test-name half** is here too (§12 step 9b): every `required_fixtures`
/// entry names a test some included source declares, every §4.2
/// failing-direction fixture is a declared test, and the manifest's `inventory`
/// block records `CC7_MEDIA_TESTS`, `CC7_CORE_TESTS`, `CC7_AGENT_TESTS`,
/// `CC7_APP_TESTS`, `CC7_EVAL_TESTS`, `CC7_INVENTORY_TESTS`,
/// `CC7_EVIDENCE_FIXTURES`, `CC7_EXTERNAL_OWNERS` and `CC7_TEST_SOURCES`
/// verbatim. The both-direction source scan and the `uses_outside_prose` guard
/// are `cc7_declared_test_names_exist_in_their_source_files`'s.
#[test]
fn cc7_manifest_declares_every_required_fixture_and_constant() {
    let manifest = cc7_manifest();
    assert_eq!(manifest["manifest_version"], 1);
    assert_eq!(manifest["contract"], "CC7 workflow evaluation");
    assert_eq!(manifest["contract_token"], CC7_CONTRACT);

    // --- §11.3 thresholds: one key per pinned constant --------------------
    let thresholds = &manifest["thresholds"];
    let declared = thresholds
        .as_object()
        .expect("the manifest must declare a thresholds object");
    // Rule: no unresolved probe placeholder. Every threshold key holds a
    // number, so a key count alone cannot be satisfied by a placeholder.
    for (key, value) in declared {
        assert!(
            value.is_number(),
            "manifest threshold {key} is {value}, not a number; a placeholder cannot satisfy the \
             key count"
        );
    }
    let constants = cc7_threshold_constants();
    for (key, expected) in &constants {
        assert_manifest_i64(thresholds, key, *expected);
    }
    assert_eq!(
        declared.len(),
        constants.len(),
        "every pinned CC7 constant of §2.6 and §2.3.6 must have exactly one threshold key"
    );
    assert_eq!(
        manifest["thresholds_distinctness"]["asserted_by"],
        "cc7_budgets_are_distinct_from_every_neighbouring_constant"
    );
    assert_eq!(
        manifest["thresholds_distinctness"]["track_min_confidence_is_not_the_default"],
        true
    );
    assert_eq!(
        manifest["delivery_allowed_info_codes"],
        json!(CC7_DELIVERY_ALLOWED_INFO_CODES)
    );

    // --- §11.3 scenarios ---------------------------------------------------
    let scenarios = manifest["scenarios"]
        .as_array()
        .expect("one object per scenario");
    assert_eq!(scenarios.len(), CC7_SCENARIOS.len());
    for (declared, scenario) in scenarios.iter().zip(CC7_SCENARIOS) {
        let spec = cc7_spec(scenario);
        assert_eq!(declared["id"], spec.id);
        assert_eq!(declared["title"], spec.title);
        assert_eq!(declared["frames"], spec.frames);
        assert_eq!(
            declared["clips"].as_array().expect("the clip list").len(),
            spec.clips.len()
        );
        assert_eq!(
            declared["canonical_operations"]
                .as_array()
                .expect("the canonical operations")
                .len(),
            spec.canonical_operations.len()
        );
        match spec.human_question {
            Some(question) => assert_eq!(declared["human_question"], question),
            None => assert_eq!(declared["human_question"], Value::Null),
        }
    }

    // --- §11.3 raster ------------------------------------------------------
    let raster = &manifest["raster"];
    assert_eq!(raster["size"], json!([CC7_SOURCE_WIDTH, CC7_SOURCE_HEIGHT]));
    assert_eq!(raster["fps"], CC7_SOURCE_FPS);
    let populations = raster["populations"]
        .as_object()
        .expect("the five population entries");
    assert_eq!(populations.len(), 5);
    let mut total = 0_i64;
    for (name, count) in kinewright_core::cc7_scenarios::CC7_REGION_POPULATIONS {
        assert_manifest_i64(&raster["populations"], name, i64::from(count));
        total += i64::from(count);
    }
    assert_manifest_i64(raster, "population_total", total);
    assert_eq!(
        total,
        i64::from(CC7_SOURCE_WIDTH) * i64::from(CC7_SOURCE_HEIGHT)
    );
    assert_eq!(raster["a1_guard"]["chart_is_achromatic"], true);
    assert_eq!(raster["a1_guard"]["pure_red_primary_present"], false);
    assert_eq!(
        raster["a1_guard"]["asserted_by"],
        "cc7_the_chart_band_is_achromatic_and_the_primaries_band_has_no_red"
    );

    // --- §11.3 patches -----------------------------------------------------
    assert_eq!(
        manifest["patches"]["chart"],
        json!(CC7_CHART_PATCHES.map(|patch| patch.display_code_cam_a[0]))
    );
    assert_eq!(
        manifest["patches"]["primaries"],
        json!(CC7_PRIMARY_PATCHES.map(|patch| patch.display_code_cam_a))
    );
    assert_eq!(
        manifest["patches"]["cam_a"],
        json!(CC7_ROW_PATCHES.map(|patch| patch.display_code_cam_a))
    );

    // --- §11.3 log ---------------------------------------------------------
    let log = &manifest["log"];
    assert_manifest_i64(
        &log["curve"],
        "offset_stops",
        kinewright_core::cc7_scenarios::CC7_LOG_OFFSET_STOPS,
    );
    assert_manifest_i64(
        &log["curve"],
        "span_stops",
        kinewright_core::cc7_scenarios::CC7_LOG_SPAN_STOPS,
    );
    assert_eq!(
        log["curve"]["stored_codes"],
        json!(kinewright_core::cc7_scenarios::CC7_LOG_CHART_CODES)
    );
    assert_eq!(
        log["round_trip"]["error_codes"],
        json!(kinewright_core::cc7_scenarios::CC7_LOG_CHART_INVERSE_ERROR_CODES)
    );
    assert_eq!(log["signature"]["field"], "scope_statistics.luma");
    assert_eq!(log["signature"]["unit"], "sixteen_bit_code");
    assert_manifest_i64(&log["signature"], "scale", CC7_SCOPE_SIXTEEN_BIT_SCALE);
    assert_eq!(log["signature"]["wrong_field"], "mean_code_values.luma");
    assert_eq!(
        log["signature"]["carrier"],
        json!(kinewright_core::cc7_scenarios::CC7_LOG_CARRIER_LUMA_PERCENTILES_CODE16)
    );
    assert_eq!(
        log["signature"]["cam_a"],
        json!(kinewright_core::cc7_scenarios::CC7_CAM_A_LUMA_PERCENTILES_CODE16)
    );
    assert_eq!(log["size_is_pinned_not_selected"], true);
    assert_eq!(log["clamp_kept"], true);
    let ladder = log["cube_size_ladder"]
        .as_object()
        .expect("the three lattice sizes");
    assert_eq!(ladder.len(), CC7_LOG_CUBE_SIZE_LADDER.len());
    for (size, worst) in CC7_LOG_CUBE_SIZE_LADDER {
        assert_manifest_i64(&log["cube_size_ladder"], &size.to_string(), worst);
    }
    assert_eq!(log["cube_size_ladder_monotone_non_increasing"], true);
    assert_eq!(log["cube_title"], CC7_LOG_CUBE_TITLE);

    // --- §11.3 tracking ----------------------------------------------------
    let tracking = &manifest["tracking"];
    assert_eq!(
        tracking["amplitudes"],
        json!([
            kinewright_core::cc7_scenarios::CC7_TRACK_AMPLITUDE_X_PIXELS,
            kinewright_core::cc7_scenarios::CC7_TRACK_AMPLITUDE_Y_PIXELS
        ])
    );
    assert_eq!(
        tracking["sample_frames"],
        json!(kinewright_core::cc7_scenarios::cc7_tracking_sample_frames())
    );
    assert_eq!(
        tracking["expected_low_confidence_frames"],
        json!(kinewright_core::cc7_scenarios::CC7_TRACK_EXPECTED_LOW_CONFIDENCE_FRAMES)
    );
    assert_eq!(
        tracking["analytic_centres_basis_points"],
        json!(CC7_TRACK_ANALYTIC_CENTRES_BASIS_POINTS)
    );
    assert_eq!(
        tracking["f2"]["samples"],
        json!(kinewright_core::cc7_scenarios::CC7_TRACK_F2_SAMPLE_FRAMES)
    );
    assert_eq!(tracking["radius_10_and_25_identical"], true);
    assert_eq!(tracking["owner"], "kinewright-agent");

    // --- §11.3 transcriptions ---------------------------------------------
    let transcriptions = &manifest["transcriptions"];
    assert_eq!(
        transcriptions["functions"],
        json!(["encode_bt709", "decode_display709", "grade709_decode"])
    );
    assert_eq!(
        transcriptions["owner"],
        "crates/kinewright-media/src/color_pipeline.rs"
    );
    assert_eq!(
        transcriptions["cross_checked_by"],
        "cc7_core_transcriptions_agree_with_the_pipeline"
    );
    assert_eq!(
        transcriptions["tolerance"].as_f64(),
        Some(kinewright_core::cc7_scenarios::CC7_SPEC_F64_TOLERANCE)
    );

    // --- §11.3 budgets ------------------------------------------------------
    let budgets = &manifest["budgets"];
    for row in kinewright_core::cc7_scenarios::CC7_BUDGETS {
        let declared = &budgets["terms"][row.term];
        assert_manifest_i64(declared, "budget", row.budget);
        // R4-m4: "is a number" is not "is the number". Every `CC7_BUDGETS`
        // row's `measured` is an `i64`, so the manifest declares the same
        // integer and cannot drift from `CC7_MEASURED_*`.
        assert_manifest_i64(declared, "measured", row.measured);
        assert!(
            declared.get("margin").is_some(),
            "budget row {} must record a margin",
            row.term
        );
        assert_eq!(declared["constant"], row.constant, "{}", row.term);
    }
    let delivery = budgets["delivery"]
        .as_object()
        .expect("the per-scenario delivery block");
    assert_eq!(delivery.len(), CC7_SCENARIOS.len());
    for scenario in CC7_SCENARIOS {
        let spec = cc7_spec(scenario);
        for depth in ["eight_bit", "ten_bit"] {
            let lane = &budgets["delivery"][spec.id][depth];
            for term in [
                "luma_max_code",
                "luma_p99_code_millionths",
                "luma_mean_code_millionths",
                "rgb_mean_code_millionths",
                "psnr_db_hundredths",
            ] {
                assert!(
                    lane[term]["measured"].is_number(),
                    "{}/{depth}: {term} must record a measurement",
                    spec.id
                );
                assert!(lane[term]["budget"].is_number());
                assert!(lane[term]["margin"].is_string() || lane[term]["margin"].is_number());
            }
        }
    }
    assert_eq!(
        budgets["delivery_note"],
        "CC6 owns these constants; CC7 measures against them and never re-baselines one"
    );
    for depth in [DeliveryEncodeDepth::Eight, DeliveryEncodeDepth::Ten] {
        let key = match depth {
            DeliveryEncodeDepth::Eight => "eight_bit",
            DeliveryEncodeDepth::Ten => "ten_bit",
        };
        let cc6 = kinewright_core::DeliveryBudgets::for_depth(depth);
        let declared = &budgets["delivery_constants"][key];
        assert_manifest_i64(declared, "luma_max_code", i64::from(cc6.luma_max_code));
        assert_manifest_i64(
            declared,
            "luma_p99_code_millionths",
            cc6.luma_p99_code_millionths,
        );
        assert_manifest_i64(
            declared,
            "luma_mean_code_millionths",
            cc6.luma_mean_code_millionths,
        );
        assert_manifest_i64(
            declared,
            "rgb_mean_code_millionths",
            cc6.rgb_mean_code_millionths,
        );
        assert_manifest_i64(
            declared,
            "psnr_floor_db_hundredths",
            i64::from(cc6.psnr_floor_db_hundredths),
        );
    }
    // §4.2's failing directions, mirrored.
    let failing = budgets["failing_direction"]
        .as_object()
        .expect("§4.2's table");
    assert!(failing.len() >= 12, "§4.2 lists at least twelve rows");

    // --- §11.3 measurement provenance --------------------------------------
    let measurement = &budgets["measurement"];
    for key in [
        "os",
        "arch",
        "lane",
        "adapter",
        "source_generator",
        "ffmpeg_build",
        "libavcodec",
        "libswscale",
        "x264_core",
        "rustc",
        "commit",
        "date",
    ] {
        assert!(
            measurement[key].is_string(),
            "the measurement provenance block must record {key}"
        );
    }
    assert_eq!(measurement["os"], std::env::consts::OS);
    assert_eq!(measurement["arch"], std::env::consts::ARCH);

    // --- §11.3 review, eval, scorecard, m36, external owners ---------------
    let review = &manifest["review"];
    assert_eq!(review["schema_version"], 2);
    assert_eq!(review["blind_key_location"], "run_root");
    assert_eq!(review["not_blinded"], json!(["scenario_identity"]));
    assert_eq!(
        review["leak_test_needles"]["canonical_parameter_names"],
        json!(cc7_canonical_parameter_names()),
        "the manifest's needle list must be the resolved `cc7_scenarios` parameter set"
    );
    assert_eq!(
        review["leak_test_needles"]["machine_provenance"],
        json!([
            "-sample-", "agent", "person", "model", "harness", "passed", "assert"
        ])
    );
    assert_eq!(
        review["leak_test_needles"]["task_ids"],
        json!(["c1", "c2", "c3", "c4", "c5", "c6"])
    );
    let values = review["leak_test_needles"]["value_needles"]
        .as_array()
        .expect("the value needles");
    for needle in values {
        let needle = needle.as_str().expect("a string needle");
        assert!(
            needle.trim_start_matches('-').len() >= 3,
            "a value needle shorter than three digits would match anywhere: {needle}"
        );
    }
    assert_eq!(
        manifest["m36"],
        json!({
            "registry_tools": 124,
            "registry_bytes": 1_280_060,
            "served_tools": 7,
            "served_bytes": 5_660,
            "changed_by_cc7": false,
        })
    );
    assert!(
        manifest["external_owners"]
            .as_array()
            .expect("the cited fixtures")
            .len()
            >= 6
    );
    assert_eq!(manifest["evidence_fixtures"], json!(CC7_EVIDENCE_FIXTURES));
    assert_eq!(
        manifest["manifest_self_test"],
        json!([
            "cc7_manifest_declares_every_required_fixture_and_constant",
            "cc7_declared_test_names_exist_in_their_source_files"
        ])
    );
    assert_eq!(
        manifest["fixture_quality_rules"],
        "docs/CC6-QC-AND-MANAGED-DELIVERY.md:1352-1362"
    );
    // --- the manifest names the same tests --------------------------------
    let all = cc7_declared_test_names();
    let mut verified = 0_usize;
    for entry in manifest["required_fixtures"]
        .as_array()
        .expect("the manifest must list the §11.2 items")
    {
        let item = entry["item"].as_str().expect("a numbered item");
        let name = entry["test"].as_str().expect("a test name");
        assert!(
            CC7_TEST_SOURCES
                .iter()
                .any(|(_, source)| declares_test(source, name)),
            "§11.2 item {item} claims a test named {name}, which no included source declares \
             as a #[test] function"
        );
        assert!(
            all.iter().any(|declared| declared == name),
            "§11.2 item {item}'s test {name} is not in any inventory array"
        );
        verified += 1;
    }
    assert_eq!(
        verified,
        all.len(),
        "every declared CC7 test must appear exactly once in `required_fixtures`"
    );
    for entry in manifest["budgets"]["failing_direction"]
        .as_object()
        .expect("§4.2's table")
        .values()
    {
        let fixture = entry["fixture"].as_str().expect("a fixture name");
        if !fixture.starts_with("cc7_") {
            // §4.2's render-parity row cites CC6's own negative control.
            continue;
        }
        assert!(
            all.iter().any(|declared| declared == fixture),
            "§4.2 names failing-direction fixture {fixture}, which the inventory does not declare"
        );
    }

    // --- the manifest records the arrays themselves ------------------------
    let inventory = &manifest["inventory"];
    assert_eq!(inventory["asserted_by"], CC7_INVENTORY_TESTS[1]);
    for (label, expected) in [
        ("CC7_MEDIA_TESTS", CC7_MEDIA_TESTS.as_slice()),
        ("CC7_CORE_TESTS", CC7_CORE_TESTS.as_slice()),
        ("CC7_AGENT_TESTS", CC7_AGENT_TESTS.as_slice()),
        ("CC7_APP_TESTS", CC7_APP_TESTS.as_slice()),
        ("CC7_EVAL_TESTS", CC7_EVAL_TESTS.as_slice()),
        ("CC7_INVENTORY_TESTS", CC7_INVENTORY_TESTS.as_slice()),
        ("CC7_EVIDENCE_FIXTURES", CC7_EVIDENCE_FIXTURES.as_slice()),
        ("CC7_EXTERNAL_OWNERS", CC7_EXTERNAL_OWNERS.as_slice()),
    ] {
        assert_eq!(
            inventory["arrays"][label],
            json!(expected),
            "the manifest's {label} does not match the array in cc7_fixtures.rs"
        );
    }
    assert_eq!(
        inventory["test_sources"],
        json!(CC7_TEST_SOURCES.map(|(path, _)| path)),
        "the manifest must name every include_str!'d source"
    );
    assert_eq!(inventory["forbidden_helpers"], json!(CC7_FORBIDDEN_HELPERS));
    assert_eq!(
        inventory["explicit_non_prefixed_tests"],
        json!(CC7_EXPLICIT_TEST_NAMES)
    );
    assert_eq!(inventory["gpu_skip_owner"], CC7_GPU_SKIP_OWNER);
    assert_eq!(
        inventory["skip_branch_typed_codes"],
        json!(CC7_SKIP_TYPED_CODES)
    );
    assert_eq!(
        inventory["declared_test_count"],
        i64::try_from(all.len()).expect("a test count")
    );
    assert_eq!(
        manifest["external_owners"]
            .as_array()
            .expect("the cited fixtures")
            .iter()
            .map(|owner| owner
                .get("fixture")
                .or_else(|| owner.get("constants"))
                .and_then(Value::as_str)
                .expect("every cited owner names a fixture or a constant set"))
            .collect::<Vec<_>>(),
        CC7_EXTERNAL_OWNERS.to_vec(),
        "CC7_EXTERNAL_OWNERS and the manifest's cited fixtures disagree"
    );
}

// ===========================================================================
// CC7 §12 step 9b and §11.3 — the inventory (R-M12).
// ===========================================================================
//
// This is the LAST thing CC7 writes. `CC7_TEST_SOURCES` `include_str!`s the
// agent (§12 step 4), app (step 7) and eval (step 8) files at **compile**
// time, so renaming a test in another crate rebuilds this fixture and fails
// it, instead of leaving a manifest entry that names a function nobody has
// written for three commits.

/// The two media files that own a `cc7_*` test.
///
/// §11.2 groups `cc7_fixtures.rs` and `cc7_sources.rs` under one array
/// (`CC7_MEDIA_TESTS`), and so does this: they are one crate's half of the
/// slice, and a test that moves between the two files is not a change the
/// inventory needs to notice.
const CC7_MEDIA_TEST_SOURCES: [&str; 2] = [
    "crates/kinewright-media/src/cc7_fixtures.rs",
    "crates/kinewright-media/src/cc7_sources.rs",
];

/// Every `cc7_*` test the media crate declares — §11.2 items 12–29, the two
/// §11.2.33 inventory tests, and the failing directions §4.2 names.
const CC7_MEDIA_TESTS: [&str; 41] = [
    // cc7_fixtures.rs — §11.2 items 12b and 19–29, plus the inventory pair.
    "cc7_mixed_camera_match_meets_the_neutral_spread_and_luma_budgets",
    "cc7_a_the_unmatched_candidate_exceeds_the_neutral_spread_budget",
    "cc7_a_the_unrecoverable_candidate_exceeds_the_neutral_spread_budget",
    "cc7_a_the_unmatched_candidate_exceeds_the_luma_mean_budget",
    "cc7_a_the_intentional_difference_survives_the_match",
    "cc7_a_skin_band_rejects_the_product_row",
    "cc7_wrong_balance_clamps_temperature_and_raises_one_range_warning",
    "cc7_b_c1_publishes_no_clamp",
    "cc7_b1_residual_spread_meets_the_match_budget",
    "cc7_b1_the_uncorrected_candidate_exceeds_the_residual_budget",
    "cc7_b_c1_raises_no_range_excursion",
    "cc7_log_inverse_lands_every_patch_inside_the_budget",
    "cc7_c_an_identity_cube_does_not_undo_the_log_curve",
    "cc7_c_the_cube_size_sweep_is_monotone_and_size_seventeen_fails",
    "cc7_product_qualifier_covers_exactly_its_patch_and_changes_nothing_outside",
    "cc7_d_a_qualifier_that_selects_two_patches_is_rejected",
    "cc7_d_the_skin_hue_is_unmoved_by_the_product_qualifier",
    "cc7_d_a_qualifier_over_the_skin_row_moves_the_skin_hue",
    "cc7_feather_counts_match_the_discrete_pixel_centre_model",
    "cc7_d_feather_zero_has_no_partial_pixels",
    "cc7_warm_look_out_of_gamut_count_is_exact_on_the_deep_shadow_patch",
    "cc7_e_the_base_scene_without_the_look_is_in_gamut",
    "cc7_tracked_window_contains_the_square_at_every_sampled_frame",
    "cc7_f_a_window_smaller_than_the_square_loses_containment",
    "cc7_every_scenario_verifies_at_eight_bits",
    "cc7_every_scenario_verifies_at_ten_bits",
    "cc7_g_a_starved_encode_trips_the_decoded_difference_budget",
    "cc7_canonical_node_stack_matches_the_cpu_reference_on_the_software_lane",
    "cc7_canonical_node_stack_matches_the_cpu_reference_on_hardware",
    "cc7_core_transcriptions_agree_with_the_pipeline",
    "cc7_scenario_luma_percentiles_match_the_manifest",
    "cc7_manifest_declares_every_required_fixture_and_constant",
    "cc7_declared_test_names_exist_in_their_source_files",
    // cc7_sources.rs — §11.2 items 12–18 and §3.5's non-vacuity rules.
    "cc7_base_scene_populations_are_the_contract_table",
    "cc7_the_chart_band_is_achromatic_and_the_primaries_band_has_no_red",
    "cc7_camera_sources_differ_from_the_reference_at_every_neutral_patch",
    "cc7_log_source_is_not_the_base_scene",
    "cc7_tracked_source_moves_and_occludes",
    "cc7_tracked_square_never_covers_the_static_patch_row",
    "cc7_ffv1_round_trip_is_byte_exact",
    "cc7_log_like_inverse_cube_is_canonical_text_of_the_pinned_size",
];

/// The core file that owns a `cc7_*` test.
const CC7_CORE_TEST_SOURCES: [&str; 1] = ["crates/kinewright-core/tests/cc7_core.rs"];

/// Every `cc7_*` test `crates/kinewright-core/tests/cc7_core.rs` declares —
/// §11.2 items 1–11, plus A-E21's keyframe-smoother transcription check.
const CC7_CORE_TESTS: [&str; 12] = [
    "cc7_scenario_geometry_round_trips_through_normalized_roi",
    "cc7_chart_and_primary_codes_are_the_contract_table",
    "cc7_camera_a_patch_codes_are_the_hand_derived_display_encoding",
    "cc7_log_curve_anchors_and_patch_codes_are_the_contract_table",
    "cc7_log_inverse_error_floors_are_properties_of_the_curve",
    "cc7_camera_transforms_are_applied_in_linear_light",
    "cc7_analytic_square_path_stays_in_frame_and_clears_the_patch_row",
    "cc7_tracking_sample_frames_are_the_closed_form_distribution",
    "cc7_budgets_are_distinct_from_every_neighbouring_constant",
    "cc7_every_budget_carries_the_declared_margin",
    "cc7_canonical_operations_are_accepted_by_core_in_order",
    "cc7_the_keyframe_smoother_transcription_matches_core",
];

/// The agent file that owns a `cc7_*` test.
const CC7_AGENT_TEST_SOURCES: [&str; 1] = ["crates/kinewright-agent/tests/mcp_server.rs"];

/// Every `cc7_*` test the agent's integration suite declares — §11.2 item 30:
/// §5.2's six scripts, D-E13's standalone (f2) refusal check, and §5.4's
/// surface-unchanged pin.
const CC7_AGENT_TESTS: [&str; 8] = [
    "cc7_the_agent_surface_is_unchanged_by_this_slice",
    "cc7_a_mixed_camera_match_retains_the_reference_and_lands_the_canonical_grade",
    "cc7_b_wrong_balance_publishes_the_clamp_and_the_range_warning",
    "cc7_c_log_like_input_is_normalised_by_an_imported_technical_lut",
    "cc7_d_product_qualifier_selects_its_patch_and_leaves_skin_alone",
    "cc7_e_creative_look_bypass_matches_absent_and_reports_its_gamut",
    "cc7_f_tracked_secondary_drops_only_the_occluded_samples",
    "cc7_f2_the_default_floor_does_not_refuse",
];

/// The app files that own a `cc7_*` test.
///
/// `look_browser_ui.rs` carries exactly one — the (e) built-in-look test —
/// which is why §11.2.31 requires it: an included source with no declared test
/// would make the both-direction assertion compare two empty sets (minor 7).
const CC7_APP_TEST_SOURCES: [&str; 2] = [
    "crates/kinewright-app/src/inspector_ui.rs",
    "crates/kinewright-app/src/look_browser_ui.rs",
];

/// Every `cc7_*` test the app crate declares — §11.2 item 31's seven §6
/// person-path tests.
const CC7_APP_TESTS: [&str; 7] = [
    // inspector_ui.rs
    "cc7_a_a_person_can_author_the_matched_primary_by_hand",
    "cc7_b_the_temperature_slider_stops_where_the_planner_clamps",
    "cc7_c_a_person_can_import_and_bind_the_technical_lut",
    "cc7_d_a_person_can_author_the_product_qualifier_by_hand",
    "cc7_d2_a_person_can_author_the_window_only_matte_by_hand",
    "cc7_f_the_person_path_is_not_available_and_says_so",
    // look_browser_ui.rs
    "cc7_e_a_person_can_add_the_built_in_warm_look",
];

/// The eval files that own a CC7 test.
const CC7_EVAL_TEST_SOURCES: [&str; 2] = [
    "crates/kinewright-agent/src/eval.rs",
    "crates/kinewright-agent/src/bin/kinewright-eval.rs",
];

/// Every CC7 test the eval harness declares — §11.2 item 32.
///
/// Includes the one **non-prefixed** name the contract names explicitly,
/// `published_v6_manifest_tracks_the_color_workflow_suite`: it is the
/// published-benchmark check of §7.7, written where the other published
/// manifests' checks live, and it is declared here rather than renamed
/// (§11.3's "or be named explicitly in the inventory").
const CC7_EVAL_TESTS: [&str; 28] = [
    // eval.rs — the shared-runner half.
    "cc7_a_v5_result_serialises_byte_identically_without_measurements",
    "cc7_track_keyframes_match_expected_reads_the_committed_document",
    "cc7_a_colour_assertion_without_evidence_fails_rather_than_passing",
    "cc7_only_colour_assertions_emit_measurements",
    "cc7_a_colour_request_is_derived_from_the_assertions_only",
    "cc7_human_review_v1_files_still_load_and_score",
    "cc7_human_review_v2_round_trips_blind_id_and_questions",
    "cc7_accepted_requires_every_question_answered",
    "cc7_blind_ids_are_derived_from_the_artifact_digest",
    "cc7_patch_statistics_are_taken_on_a_two_pixel_inset",
    "cc7_coverage_cropping_is_exact_and_never_inset",
    "cc7_color_evidence_is_computed_where_the_analysis_is_alive",
    "cc7_delivery_verification_without_a_deliverable_is_recorded_as_an_error",
    "cc7_the_colour_template_marks_the_editorial_dimensions_not_applicable",
    // R1-B1's addition: §11.2.32 names it, and step 9b declared the tree as it
    // stood before the R1 fixers landed it (G-E3 item 2, review R4-B1).
    "cc7_a_fixture_project_path_reaches_the_server",
    // bin/kinewright-eval.rs — the suite, the blind package and the CLI.
    "cc7_the_blind_package_discloses_no_machine_provenance",
    "cc7_score_review_resolves_a_blind_form_before_binding",
    "cc7_one_viewing_scores_every_row_with_the_same_artifact",
    "cc7_a_blind_form_without_its_key_is_refused_by_name",
    "cc7_score_review_still_takes_the_plain_human_review_path",
    "cc7_every_suite_constructor_declares_a_delivery_bit_depth",
    "cc7_color_workflow_suite_is_a_packaged_benchmark",
    "cc7_the_usage_banner_lists_every_registered_suite",
    "cc7_every_color_task_carries_a_color_eval_request",
    "cc7_every_color_assertion_threshold_is_a_cc7_scenarios_constant",
    "cc7_the_color_review_template_asks_only_the_matrix_question",
    "cc7_every_color_fixture_builds_a_valid_document",
    "published_v6_manifest_tracks_the_color_workflow_suite",
];

/// The declared names that do **not** carry the `cc7_` prefix, and are
/// therefore named explicitly rather than discovered by the prefix scan.
const CC7_EXPLICIT_TEST_NAMES: [&str; 1] =
    ["published_v6_manifest_tracks_the_color_workflow_suite"];

/// The two §11.2.33 inventory tests, which are fixture-quality rules rather
/// than numbered §11.2 items and are claimed by `manifest_self_test`.
const CC7_INVENTORY_TESTS: [&str; 2] = [
    "cc7_manifest_declares_every_required_fixture_and_constant",
    "cc7_declared_test_names_exist_in_their_source_files",
];

/// The fixtures and constants CC7 **cites** rather than duplicates (§11.3).
///
/// Each entry is the manifest's own citation key, so a fixture that is moved
/// or renamed out from under CC7 fails here rather than silently leaving a
/// claim unevidenced.
const CC7_EXTERNAL_OWNERS: [&str; 9] = [
    "cc1_fixtures.rs:3139/3146/3153",
    "cc1_fixtures.rs:3294-3296",
    "cc4_fixtures.rs:3516",
    "mcp_server.rs:1351",
    "mcp_server.rs:1439",
    "cc5_fixtures.rs:1149-1172",
    "cc6_fixtures.rs:2346",
    "inspector_ui.rs:5673",
    "CC6 6.3's DELIVERY_* budgets",
];

/// The helpers a CC7 gate must never reach for (§11.3, R-M11).
///
/// **The array-literal form is normative.** `uses_outside_prose` counts a
/// needle as used when a non-comment line contains `needle(` or `("needle")`,
/// so a single-argument helper call would put the needle directly inside a
/// call's parentheses and the guard would match itself. Inside an array
/// literal every element is preceded by `[` or `, ` and never by `(`, which is
/// the actual mechanism that lets this file name what it forbids.
///
/// The third entry extends §11.3's normative two: §10 forbids env-conditional
/// behaviour in a gate at all, so a CC7 media or core fixture must not consult
/// **any** environment variable, not merely the GPU skip opt-in.
const CC7_FORBIDDEN_HELPERS: [&str; 3] = [
    "fixture_gpu_or_skip",
    "KINEWRIGHT_GPU_TESTS_MAY_SKIP",
    "std::env::var",
];

/// The only file permitted to consult the GPU skip opt-in, and only inside
/// §5.3's template.
const CC7_GPU_SKIP_OWNER: &str = "crates/kinewright-agent/tests/mcp_server.rs";

/// The typed codes §5.3's skip branch must assert. A branch that accepts a
/// refusal without naming its code asserts nothing.
const CC7_SKIP_TYPED_CODES: [&str; 3] = [
    "matte_proof_unavailable",
    "working_proof_unavailable",
    "color_proof_render_failed",
];

/// The sources every declared CC7 test name is verified against, keyed by the
/// workspace-relative path the manifest names.
///
/// `include_str!` rather than a runtime read on purpose: the check is a
/// **compile-time** dependency (§11.3, R-M12).
const CC7_TEST_SOURCES: [(&str, &str); 8] = [
    (
        "crates/kinewright-media/src/cc7_fixtures.rs",
        include_str!("cc7_fixtures.rs"),
    ),
    (
        "crates/kinewright-media/src/cc7_sources.rs",
        include_str!("cc7_sources.rs"),
    ),
    (
        "crates/kinewright-core/tests/cc7_core.rs",
        include_str!("../../kinewright-core/tests/cc7_core.rs"),
    ),
    (
        "crates/kinewright-agent/tests/mcp_server.rs",
        include_str!("../../kinewright-agent/tests/mcp_server.rs"),
    ),
    (
        "crates/kinewright-agent/src/eval.rs",
        include_str!("../../kinewright-agent/src/eval.rs"),
    ),
    (
        "crates/kinewright-agent/src/bin/kinewright-eval.rs",
        include_str!("../../kinewright-agent/src/bin/kinewright-eval.rs"),
    ),
    (
        "crates/kinewright-app/src/inspector_ui.rs",
        include_str!("../../kinewright-app/src/inspector_ui.rs"),
    ),
    (
        "crates/kinewright-app/src/look_browser_ui.rs",
        include_str!("../../kinewright-app/src/look_browser_ui.rs"),
    ),
];

/// One source's text, or a panic naming the path the manifest invented.
fn cc7_test_source(path: &str) -> &'static str {
    CC7_TEST_SOURCES
        .iter()
        .find_map(|(candidate, source)| (*candidate == path).then_some(*source))
        .unwrap_or_else(|| {
            panic!(
                "the manifest names source {path}, which cc7_fixtures.rs does not include; add it \
                 to CC7_TEST_SOURCES"
            )
        })
}

fn is_test_attribute(line: &str) -> bool {
    line == "#[test]" || line.starts_with("#[tokio::test")
}

/// Whether `source` declares `name` as a `#[test]` (or `#[tokio::test]`)
/// function.
///
/// The attribute is required, so a name mentioned in a doc comment, a string
/// literal, or a helper function is not mistaken for a fixture — which matters
/// here, because this file names every CC7 test in prose as well as in code.
fn declares_test(source: &str, name: &str) -> bool {
    let needle = format!("fn {name}(");
    let lines = source.lines().collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        if !line.contains(&needle) {
            continue;
        }
        for previous in lines[..index].iter().rev() {
            let previous = previous.trim();
            if is_test_attribute(previous) {
                return true;
            }
            if previous.is_empty() || previous.starts_with("//") || previous.starts_with("#[") {
                continue;
            }
            break;
        }
    }
    false
}

/// Every `#[test]` function in `source` whose name starts with `prefix`, in
/// declaration order.
fn declared_test_names(source: &str, prefix: &str) -> Vec<String> {
    let lines = source.lines().collect::<Vec<_>>();
    let mut names = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !is_test_attribute(line.trim()) {
            continue;
        }
        for candidate in &lines[index + 1..] {
            let candidate = candidate.trim();
            if candidate.is_empty() || candidate.starts_with("//") || candidate.starts_with("#[") {
                continue;
            }
            let Some(rest) = candidate.split_once("fn ").map(|(_, rest)| rest) else {
                break;
            };
            let Some((name, _)) = rest.split_once('(') else {
                break;
            };
            if name.starts_with(prefix) {
                names.push(name.to_owned());
            }
            break;
        }
    }
    names
}

/// The text of one top-level test in an integration-test file, from its `fn`
/// line to the closing brace in column zero.
///
/// `tests/mcp_server.rs` is allowed to consult the GPU skip opt-in inside
/// §5.3's template, so the guard is applied per CC7 test rather than to the
/// whole file, which is shared with CC1–CC6.
fn cc7_test_body(source: &str, name: &str) -> String {
    let needle = format!("fn {name}(");
    let lines = source.lines().collect::<Vec<_>>();
    let start = lines
        .iter()
        .position(|line| line.contains(&needle))
        .unwrap_or_else(|| panic!("{name} is not declared in the source that owns it"));
    let mut body = Vec::new();
    for line in &lines[start..] {
        body.push(*line);
        if *line == "}" {
            break;
        }
    }
    assert!(
        body.len() > 1,
        "{name}'s body could not be delimited; the test is not a top-level item"
    );
    body.join("\n")
}

/// Whether `source` *uses* `needle` as code rather than merely naming it in a
/// comment or a message (`cc6_fixtures.rs:2524-2541`).
///
/// A call is the identifier followed by `(`, on a line that is not a comment,
/// with any trailing `//` comment stripped first. The quoted form is the
/// `std::env::var("NAME")` shape — the needle directly inside a call's
/// parentheses. String literals are deliberately **not** exempt, because
/// `fixture_gpu_or_skip("cc7-a")` is the natural spelling and must not evade
/// the guard; the array-literal rule above is what keeps this file's own
/// needle list from matching itself.
fn uses_outside_prose(source: &str, needle: &str) -> bool {
    let call = format!("{needle}(");
    let quoted = format!("(\"{needle}\")");
    source.lines().any(|line| {
        let code = line.split("//").next().unwrap_or_default();
        code.contains(&call) || code.contains(&quoted)
    })
}

fn sorted(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut names = names.into_iter().collect::<Vec<_>>();
    names.sort_unstable();
    names
}

/// The five inventory arrays, with the sources that own them.
fn cc7_inventory_groups() -> [(
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
); 5] {
    [
        ("MEDIA", &CC7_MEDIA_TEST_SOURCES, &CC7_MEDIA_TESTS),
        ("CORE", &CC7_CORE_TEST_SOURCES, &CC7_CORE_TESTS),
        ("AGENT", &CC7_AGENT_TEST_SOURCES, &CC7_AGENT_TESTS),
        ("APP", &CC7_APP_TEST_SOURCES, &CC7_APP_TESTS),
        ("EVAL", &CC7_EVAL_TEST_SOURCES, &CC7_EVAL_TESTS),
    ]
}

/// Every CC7 test name the inventory declares, sorted and deduplicated.
fn cc7_declared_test_names() -> Vec<String> {
    let mut names = Vec::new();
    for (_, _, expected) in cc7_inventory_groups() {
        names.extend(expected.iter().map(|name| (*name).to_owned()));
    }
    names.sort_unstable();
    names.dedup();
    names
}

/// CC7 §11.2.33 and §11.3. The declared test inventories are tied to the
/// sources they claim to describe, in **both** directions: every name this
/// file lists exists as a `#[test]` / `#[tokio::test]` function in a file that
/// owns it, **and** every `cc7_*` test those files declare is listed — so a
/// test that exists but is undeclared fails exactly as loudly as a declared
/// name that does not exist.
#[test]
fn cc7_declared_test_names_exist_in_their_source_files() {
    // --- both directions, per crate ---------------------------------------
    for (label, sources, expected) in cc7_inventory_groups() {
        for name in expected {
            assert!(
                sources
                    .iter()
                    .any(|path| declares_test(cc7_test_source(path), name)),
                "no CC7_{label}_TEST_SOURCES file declares a #[test] named {name}; the \
                 inventory names what §11.2 requires and is never trimmed to match an unfinished \
                 crate"
            );
        }
        let declared = sorted(
            sources
                .iter()
                .flat_map(|path| declared_test_names(cc7_test_source(path), "cc7_")),
        );
        let mut named = sorted(expected.iter().map(|name| (*name).to_owned()));
        named.retain(|name| !CC7_EXPLICIT_TEST_NAMES.contains(&name.as_str()));
        assert_eq!(
            declared, named,
            "CC7_{label}_TESTS and the `cc7_*` tests the {label} sources actually declare \
             disagree"
        );
    }

    // --- the `cargo test -- cc7_` filter, and no name declared twice -------
    let mut all = Vec::new();
    for (_, _, expected) in cc7_inventory_groups() {
        for name in expected {
            assert!(
                name.starts_with("cc7_") || CC7_EXPLICIT_TEST_NAMES.contains(name),
                "{name} does not match the `cargo test -- cc7_` filter and is not one of the \
                 explicitly named non-prefixed tests"
            );
            all.push((*name).to_owned());
        }
    }
    let total = all.len();
    all.sort_unstable();
    all.dedup();
    assert_eq!(
        all.len(),
        total,
        "a CC7 test name is declared in two inventory arrays"
    );
    assert_eq!(
        total, 96,
        "the CC7 slice declares 96 tests across five crates"
    );
    for name in CC7_EXPLICIT_TEST_NAMES {
        assert!(
            !name.starts_with("cc7_"),
            "{name} carries the prefix and does not need naming explicitly"
        );
        assert!(
            all.iter().any(|declared| declared == name),
            "{name} is named as an explicit exception but is in no inventory array"
        );
    }
    let fixtures = cc7_test_source(CC7_MEDIA_TEST_SOURCES[0]);
    for name in CC7_INVENTORY_TESTS {
        assert!(
            declares_test(fixtures, name),
            "the §11.2.33 inventory test {name} is not declared in cc7_fixtures.rs"
        );
        assert!(
            CC7_MEDIA_TESTS.contains(&name),
            "the inventory tests are media tests and must be declared as such"
        );
    }

    // --- §11.3's `uses_outside_prose` guard --------------------------------
    // The CC7 media and core gates take the panicking `fallback_gpu()`
    // convention: evidence that reports `ok` without running is not evidence,
    // and §10 forbids env-conditional behaviour in a gate outright.
    for path in CC7_MEDIA_TEST_SOURCES
        .into_iter()
        .chain(CC7_CORE_TEST_SOURCES)
    {
        for needle in CC7_FORBIDDEN_HELPERS {
            assert!(
                !uses_outside_prose(cc7_test_source(path), needle),
                "rule 11.0.6 and §5.3: {path} must never reach for {needle}"
            );
        }
    }
    // The app's person-path tests drive a headless `egui` context and never a
    // GPU proof, so the first two needles apply to them as well (§11.3).
    for path in CC7_APP_TEST_SOURCES {
        for needle in [CC7_FORBIDDEN_HELPERS[0], CC7_FORBIDDEN_HELPERS[1]] {
            assert!(
                !uses_outside_prose(cc7_test_source(path), needle),
                "§11.3: {path} must never reach for {needle}"
            );
        }
    }
    // `tests/mcp_server.rs` is the ONE file permitted to consult the skip
    // opt-in, and only inside §5.3's template: the branch must assert a typed
    // code and print a `SKIPPED:` line, never accept both branches silently.
    // The needles are re-assembled from the array rather than written as
    // literals, so this assertion cannot self-match (R-M11).
    assert_eq!(CC7_GPU_SKIP_OWNER, CC7_AGENT_TEST_SOURCES[0]);
    let agent = cc7_test_source(CC7_GPU_SKIP_OWNER);
    let any_call = format!("{}(", CC7_FORBIDDEN_HELPERS[2]);
    let template_call = format!(
        "{}(\"{}\")",
        CC7_FORBIDDEN_HELPERS[2], CC7_FORBIDDEN_HELPERS[1]
    );
    let mut template_branches = 0_usize;
    for name in CC7_AGENT_TESTS {
        let body = cc7_test_body(agent, name);
        assert!(
            !uses_outside_prose(&body, CC7_FORBIDDEN_HELPERS[0]),
            "{name} must take the scripted endpoint's own refusal, not {}",
            CC7_FORBIDDEN_HELPERS[0]
        );
        let consulted = body.matches(any_call.as_str()).count();
        let templated = body.matches(template_call.as_str()).count();
        assert_eq!(
            consulted, templated,
            "{name} reads an environment variable that is not §5.3's skip opt-in; §10 forbids \
             env-conditional behaviour in a gate"
        );
        if templated == 0 {
            continue;
        }
        template_branches += templated;
        assert!(
            body.contains("SKIPPED:"),
            "{name}'s skip branch must print a SKIPPED: line"
        );
        assert!(
            CC7_SKIP_TYPED_CODES.iter().any(|code| body.contains(code)),
            "{name}'s skip branch must assert a typed code; a branch that accepts both outcomes \
             asserts nothing"
        );
    }
    assert!(
        template_branches >= 5,
        "only {template_branches} §5.3 skip branches were checked; the guard has gone vacuous"
    );
}
