//! Budgeted, typed evaluation support for installed editing-agent harnesses.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use kinewright_core::{
    AgentDriver, AgentEvent, Analysis, AssetId, AssetSilences, AssetTranscript, AudioLoudness,
    CaptionMotion, Clip, ClipContent, ClipId, ColorQcCheck, ColorQcReport, ColorQcRequest, Command,
    Core, DeliveryConformanceReport, DeliveryEncodeDepth, DeliveryProfile, DeliveryVerification,
    DeliveryVerificationRequest, Document, EffectId, Event, Export, ExportCancellation,
    HarnessInfo, MatteCoverageStatistics, MediaKind, NormalizedRoi, Operation, ParamValue,
    Playback, Query, QueryResult, RgbaImage, SessionConfig, SkinDiagnostics, TimeCode,
    TimelineSceneChange, TimelineSilenceSpan, TimelineTranscriptWord, TitlePosition, Track,
    TrackId, TrackKind, TranscriptStatus, apply_batch, dedup_timeline_words, delivery_conformance,
    document_for_delivery_profile, map_source_range_to_project, matte_coverage_statistics,
    measure_color_qc, qa_document,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ConfirmationBroker, McpServer, compact_tool_names,
    pacing::dialogue_pacing_gaps,
    render::cuttable_timeline_silences,
    server::{ReframeSubjectProvenance, TrackedSubjectBounds, decode_reframe_subject_provenance},
    shrink_silence_span_for_cutting_with_transcript,
};

/// Compute the upper duration bound after cutting every qualifying reported
/// silence. Each cut receives one project-frame boundary-rounding allowance.
#[must_use]
pub fn maximum_duration_after_expected_silence_cuts(
    source_duration: TimeCode,
    silences: &AssetSilences,
    transcript: Option<&AssetTranscript>,
    minimum_source_frames: TimeCode,
) -> TimeCode {
    let (removable_frames, cut_count) = silences
        .spans
        .iter()
        .filter(|span| {
            span.source_end.0.saturating_sub(span.source_start.0) >= minimum_source_frames.0
        })
        .flat_map(|span| {
            shrink_silence_span_for_cutting_with_transcript(
                *span,
                silences.source_fps,
                transcript.map(|transcript| transcript.words.as_slice()),
            )
        })
        .fold((0_i64, 0_i64), |(frames, cuts), span| {
            (
                frames.saturating_add(span.source_end.0.saturating_sub(span.source_start.0)),
                cuts.saturating_add(1),
            )
        });
    TimeCode(
        source_duration
            .0
            .saturating_sub(removable_frames)
            .saturating_add(cut_count),
    )
}

pub type FixtureBuilder = fn() -> Result<PreparedFixture, EvalError>;

#[derive(Debug, Clone, PartialEq)]
pub struct EvalBudgets {
    pub max_turns: u32,
    pub max_tool_calls: u32,
    pub max_operations: u32,
    pub max_tokens: u64,
    /// Optional because subscription harnesses may expose token counts without
    /// exposing an attributable USD price.
    pub max_cost_usd: Option<f64>,
    pub max_wall_time: Duration,
    pub max_undos: u32,
}

pub struct EvalDefinition {
    pub name: &'static str,
    pub rationale: &'static str,
    pub fixture_builder: FixtureBuilder,
    pub prompts: &'static [&'static str],
    pub assertions: Vec<EvalAssertion>,
    pub budgets: EvalBudgets,
    pub deliverable: Option<EvalDeliverableSpec>,
    /// What the colour evidence block measures for this task, and where.
    ///
    /// `None` for every non-colour suite: the runner then records no
    /// [`ColorEvalEvidence`] at all and `EvalOutcome::color` stays `None`.
    /// A colour suite builds one with
    /// [`ColorEvalRequest::from_assertions`] so the region rectangles are
    /// written down exactly once, on the assertions that gate them.
    pub color: Option<ColorEvalRequest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvalDeliverableSpec {
    pub profile: DeliveryProfile,
    pub focus_x_percent: u8,
    pub focus_y_percent: u8,
    pub proof_frames: u8,
    pub proof_cell_width: u32,
    pub require_audio: bool,
    pub expected_transcript_word_set: Option<&'static str>,
    pub maximum_word_error_rate_basis_points: u16,
    pub maximum_caption_word_error_rate_basis_points: Option<u16>,
    pub loudness: Option<EvalLoudnessSpec>,
    pub audio_tail: Option<EvalAudioTailSpec>,
    /// The lane this task's finished encode is written and verified at.
    ///
    /// A plain field with no `serde` attribute and no `Default` impl: the
    /// struct carries no serde derives, so this is a compile-time edit at
    /// every existing literal rather than a wire migration, and a future
    /// field cannot be forgotten silently.
    pub delivery_bit_depth: DeliveryEncodeDepth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EvalLoudnessSpec {
    pub minimum_integrated_lufs_hundredths: i32,
    pub maximum_integrated_lufs_hundredths: i32,
    pub maximum_sample_peak_dbfs_hundredths: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EvalAudioTailSpec {
    pub terminal_window_frames: TimeCode,
    pub maximum_sample_peak_dbfs_hundredths: i32,
    pub activity_window_frames: TimeCode,
    pub minimum_active_integrated_lufs_hundredths: i32,
    pub maximum_trailing_inactive_frames: TimeCode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ExpectedSourceClip {
    pub asset_alias: String,
    pub source_start: TimeCode,
    pub source_end: TimeCode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ExpectedTimelineClip {
    pub asset_alias: String,
    pub timeline_start: TimeCode,
    pub timeline_end: TimeCode,
    pub source_start: TimeCode,
    pub source_end: TimeCode,
}

/// A manually reviewed source interval that an edit must not select.
///
/// Ranges use Rust's half-open convention: a clip ending exactly where an
/// exclusion begins, or beginning exactly where one ends, does not overlap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SourceRangeExclusion {
    pub asset: AssetId,
    pub source_range: std::ops::Range<TimeCode>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvalAssertion {
    TimelineNonEmpty,
    ClipCount {
        minimum: usize,
        maximum: usize,
    },
    /// Require a bounded number of real media clips on one track, with every
    /// clip's mapped project duration inside the inclusive duration bounds.
    /// Titles and freeze frames are intentionally not counted.
    /// `reject_non_media` additionally rejects title/freeze padding.
    MediaClipCount {
        track: TrackId,
        minimum: usize,
        maximum: usize,
        minimum_duration: TimeCode,
        maximum_duration: TimeCode,
        #[serde(default)]
        reject_non_media: bool,
    },
    AssetOrder {
        aliases: Vec<String>,
        collapse_adjacent: bool,
    },
    AssetAbsent {
        alias: String,
    },
    Gapless,
    MediaGapless,
    DurationBounds {
        bounds: String,
    },
    ExactSourceClips {
        clips: Vec<ExpectedSourceClip>,
    },
    ExactTrackClips {
        track: TrackId,
        clips: Vec<ExpectedTimelineClip>,
    },
    /// Require the document's declared duration and the maximum mapped clip
    /// end to equal the requested project duration exactly.
    ExactProjectDuration {
        duration: TimeCode,
    },
    /// Require real media on one track to cover exactly the requested
    /// half-open project range. Gaps, overlaps, clips crossing either
    /// boundary, and media tails outside the range all fail. Non-media clips
    /// are ignored; a gap occupied only by non-media therefore still fails.
    ExactTrackMediaCoverage {
        track: TrackId,
        range: std::ops::Range<TimeCode>,
    },
    /// Require every named asset alias to appear as media on exactly this
    /// track. Unknown aliases are failures rather than silently ignored.
    RequiredAssetsOnTrack {
        track: TrackId,
        aliases: Vec<String>,
    },
    /// Require source ranges for each asset on exactly this track to be
    /// disjoint and separated by at least the non-negative source-frame
    /// threshold.
    SourceRangesSeparated {
        track: TrackId,
        minimum_separation_frames: TimeCode,
    },
    /// Require repeated uses of each source asset to move forward in source
    /// time as the target timeline moves forward. This catches a technically
    /// disjoint edit that shuffles one narrative source into reverse or
    /// arbitrary story order.
    SourceRangesChronological {
        track: TrackId,
        minimum_forward_gap_frames: TimeCode,
    },
    /// Reject selected source envelopes that contain a detected source edit.
    /// Boundaries exactly at the source in/out marks are valid; only interior
    /// scene changes prove that one timeline clip contains multiple source
    /// shots or a baked transition. `allowed_baked_sequence_starts` names
    /// reviewed timeline slots that intentionally preserve a rapid source
    /// sequence, such as a short activation burst.
    SourceRangesSceneClean {
        track: TrackId,
        scene_set: String,
        #[serde(default)]
        allowed_baked_sequence_starts: Vec<TimeCode>,
    },
    /// Reject media source ranges that overlap a manually reviewed exclusion
    /// such as a title slate, logo card, black frame, or embedded fade.
    SourceRangesAvoid {
        track: TrackId,
        exclusion_set: String,
    },
    /// Require a deliberately varied hard-cut cadence rather than a long run
    /// of near-identical shot lengths.
    ShotCadenceVariation {
        track: TrackId,
        minimum_duration_buckets: usize,
        duration_bucket_frames: TimeCode,
        maximum_similar_run: usize,
        similar_tolerance_frames: TimeCode,
    },
    /// Reject a long period-two shot-duration pattern such as
    /// `50, 80, 50, 80, 50, 80`. A repeated pair is compared against the
    /// first pair in its run within `tolerance_frames`.
    NoAlternatingShotPattern {
        track: TrackId,
        maximum_repeated_pairs: usize,
        tolerance_frames: TimeCode,
    },
    /// Require every cut between media clips on a track to land on a project
    /// beat marker within the inclusive project-frame tolerance.
    BeatAlignedCuts {
        track: TrackId,
        beat_set: String,
        tolerance_frames: TimeCode,
    },
    /// Require a minimum count and share of internal cuts to use a stronger
    /// structural subset such as inferred bar or phrase candidates.
    CutsAlignedToBeatSetAtLeast {
        track: TrackId,
        beat_set: String,
        tolerance_frames: TimeCode,
        minimum_aligned_cuts: usize,
        minimum_aligned_basis_points: u16,
    },
    /// Require one real-time, beat-anchored music clip with no audio shaping.
    ///
    /// `source_beat_set` contains source-frame beat positions for the named
    /// asset. `timeline_start` and `timeline_end` are project-frame positions.
    MusicFit {
        track: TrackId,
        asset_alias: String,
        source_beat_set: String,
        timeline_start: TimeCode,
        timeline_end: TimeCode,
        tolerance_source_frames: TimeCode,
    },
    /// Require exactly one matching music clip and pin its source endpoint to
    /// a reviewed source-frame endpoint within the inclusive tolerance.
    MusicSourceEnd {
        track: TrackId,
        asset_alias: String,
        expected_source_end: TimeCode,
        tolerance_source_frames: TimeCode,
    },
    /// Require a named visual asset to contribute both a minimum number of
    /// real media clips and a minimum amount of mapped project duration.
    AssetUseMinimum {
        track: TrackId,
        asset_alias: String,
        minimum_clip_count: usize,
        minimum_project_frames: TimeCode,
    },
    /// Require a named visual asset to appear in two distinct timeline
    /// phases. The early clip must start no later than `latest_early_start`
    /// and the late clip must start no earlier than `earliest_late_start`.
    AssetTemporalSpread {
        track: TrackId,
        asset_alias: String,
        latest_early_start: TimeCode,
        earliest_late_start: TimeCode,
    },
    /// Require the real media clip beginning at one exact project frame to
    /// come from one named asset and remain fully inside a reviewed source
    /// window. This pins a semantic role without prescribing the exact edit.
    ClipSourceWithin {
        track: TrackId,
        timeline_start: TimeCode,
        asset_alias: String,
        source_window: std::ops::Range<TimeCode>,
    },
    /// Require the first and last individual media clips on a track to hold
    /// for at least the requested project-frame durations. This deliberately
    /// measures individual clips rather than same-asset phase envelopes.
    EdgeShotHolds {
        track: TrackId,
        minimum_opening_shot_frames: TimeCode,
        minimum_closing_shot_frames: TimeCode,
    },
    WordsRetained {
        word_set: String,
    },
    WordsAbsent {
        word_set: String,
    },
    CaptionWordsExact {
        word_set: String,
    },
    CaptionSentencesCoherent,
    CaptionPresentation {
        allowed_positions: Vec<TitlePosition>,
        color_token: u8,
        background_scrim: bool,
    },
    NoSilenceAtLeast {
        source_frames: TimeCode,
    },
    DialoguePauseBounds {
        minimum_project_frames: TimeCode,
        maximum_project_frames: TimeCode,
        capitalization_boundary_minimum_frames: TimeCode,
    },
    SceneChangesAreCuts {
        scene_set: String,
    },
    RequiredToolUsage {
        all_of: Vec<String>,
        any_of: Vec<String>,
    },
    EffectOnAsset {
        asset_alias: String,
        effect_name: String,
        integer_parameter: Option<(String, i64)>,
    },
    TransitionOnAsset {
        asset_alias: String,
        transition_name: String,
    },
    /// Require a target video track's media clips to use hard cuts with no
    /// visual effects and no playback retiming.
    NoVisualTransitionsEffectsOrRetiming {
        track: TrackId,
    },
    /// Require one intentional, non-caption title card with exact timing and
    /// declarative presentation. Freeze-frame padding on the same track is
    /// rejected so a static source frame cannot impersonate a designed end card.
    TitleCard {
        track: TrackId,
        timeline_start: TimeCode,
        duration: TimeCode,
        text: String,
        font_size_token: u8,
        color_token: u8,
        position: TitlePosition,
        background_scrim: bool,
        fade_in_frames: TimeCode,
        fade_out_frames: TimeCode,
    },
    /// Require an ordered source-phase arc on a target video track. The first
    /// media phase must use `opening_alias`, one contiguous interior pivot
    /// phase must start inside `pivot_window`, its return must start inside
    /// `return_window`, and the final phase must use `closing_alias`.
    /// Opening and closing holds include contiguous clips using their phase
    /// alias from the respective edge of the track.
    SourcePhaseArc {
        track: TrackId,
        opening_alias: String,
        pivot_alias: String,
        pivot_window: std::ops::Range<TimeCode>,
        return_window: std::ops::Range<TimeCode>,
        closing_alias: String,
        minimum_opening_hold: TimeCode,
        minimum_closing_hold: TimeCode,
    },
    StyledCaptions {
        minimum_cues: usize,
        motion: CaptionMotion,
    },
    CaptionSafeArea {
        profile: DeliveryProfile,
    },
    AudioPresent,
    ProgramAudioContinuous {
        track: TrackId,
        asset_alias: String,
    },
    /// Require exactly one audio-capable media clip in the whole document,
    /// and require that clip to be on the named track and use the named asset.
    /// Titles and freeze frames never contribute to the global audio count.
    SingleAudioMediaClip {
        track: TrackId,
        asset_alias: String,
    },
    ReframeStability {
        track: TrackId,
        minimum_keyframes_per_axis: usize,
        min_x_percent: i64,
        max_x_percent: i64,
        min_y_percent: i64,
        max_y_percent: i64,
        maximum_step_percent: i64,
    },
    QaExportReady,
    UndoIntegrity,
    // -----------------------------------------------------------------------
    // Colour workflow (CC7 §7.5). Every variant below reads only
    // `EvalOutcome::color`, `EvalOutcome::original_document` and
    // `EvalOutcome::final_document`: none needs a per-call tool log, because
    // `SessionMetrics` has none and CC7 adds none. Thresholds are variant
    // fields exactly as every existing variant carries them; a colour suite
    // passes a `cc7_scenarios` constant into each one instead of a literal.
    // -----------------------------------------------------------------------
    /// The scenario's colour QC measurement reports `technical_pass`.
    ColorQcTechnicalPass {
        clip_id: u64,
        frame: i64,
        /// Wire tokens from [`ColorQcCheck`], such as `range` or `per_node`.
        checks: Vec<String>,
    },
    /// The written delivery encode was decoded and compared inside CC6's
    /// budgets at the named depth, and raised no `Error`-severity exception.
    DeliveryVerificationWithinBudgets {
        depth: DeliveryEncodeDepth,
    },
    /// The worst per-patch channel spread `max(|R−G|, |G−B|)` over the named
    /// achromatic rectangles is at most `maximum_code` monitoring codes.
    NeutralPatchSpreadAtMost {
        patch_rois: Vec<NormalizedRoi>,
        maximum_code: i64,
    },
    /// The reference clip is byte-identical to its pre-session form and
    /// carries no effect.
    ReferenceClipUntouched {
        clip_id: u64,
    },
    /// The named region's skin diagnostic reports at least this in-band rate
    /// over its **considered (chromatic)** pixels.
    SkinHueWithinBand {
        roi: NormalizedRoi,
        minimum_in_band_basis_points: u32,
    },
    /// The matte coverage cropped to `roi` matches all three counts exactly.
    MatteContainmentExact {
        roi: NormalizedRoi,
        expected_covered_pixel_count: u64,
        expected_full_pixel_count: u64,
        expected_partial_pixel_count: u64,
    },
    /// The committed document carries a keyframe on `parameter` at every
    /// expected clip-local frame and at none of the absent ones.
    ///
    /// This replaces a tool-log assertion deliberately: the committed
    /// keyframes are the durable evidence of what the tracker did, and the
    /// harness has no per-call tool log to read instead.
    TrackKeyframesMatchExpected {
        parameter: String,
        expected_local_frames: Vec<i64>,
        absent_local_frames: Vec<i64>,
    },
    /// Bypassing the named look node renders byte-identically to removing it.
    LookBypassMatchesAbsent {
        clip_id: u64,
        effect_id: u64,
        frame: i64,
    },
}

// ---------------------------------------------------------------------------
// Colour evidence (CC7 §7.5).
//
// `evaluate_assertion` sees only an `EvalDefinition` and an `EvalOutcome`: it
// holds no `Analysis`, no `Core` and no exporter, and the fixture's handles
// are dropped before it runs. Every colour quantity is therefore measured
// once, inside `run_eval_with_artifacts`, where those handles are still alive,
// and carried on the outcome as one typed block. The assertion arms read the
// block and the two documents and nothing else.
// ---------------------------------------------------------------------------

/// The two-pixel inset every patch statistic is taken on, so that a patch's
/// own edge pixels never enter its measurement (CC7 §4).
const COLOR_PATCH_INSET_PIXELS: u32 = 2;

/// The matte master switch, whose descriptor lives in
/// `kinewright_core::effect` and which core exports no constant for.
const MATTE_ENABLED_PARAMETER: &str = "matte_enabled";

/// What one task's colour evidence measures, and where.
///
/// Every field is independently optional: the runner measures exactly what
/// was asked for and leaves the rest of [`ColorEvalEvidence`] `None`, so a
/// scenario never pays for a proof render it does not read.
#[derive(Debug, Clone, PartialEq)]
pub struct ColorEvalRequest {
    /// The project frame every proof render is taken at.
    pub project_frame: i64,
    /// Achromatic patch rectangles for `neutral_spread_max_code`.
    pub neutral_patch_rois: Vec<NormalizedRoi>,
    /// The chart-band rectangle whose luma means are differenced.
    pub chart_luma_roi: Option<NormalizedRoi>,
    /// The reference frame of that difference (the subtrahend).
    pub chart_luma_reference_frame: i64,
    /// The candidate frame of that difference (the minuend).
    pub chart_luma_candidate_frame: i64,
    /// `None` measures the whole raster.
    pub qc_roi: Option<NormalizedRoi>,
    /// An empty list measures no colour QC at all.
    pub qc_checks: Vec<ColorQcCheck>,
    pub qc_max_nodes: u8,
    pub qc_delivery_bit_depth: DeliveryEncodeDepth,
    /// The region whose [`SkinDiagnostics`] are recorded.
    pub skin_roi: Option<NormalizedRoi>,
    /// The region the coverage raster is **cropped to** before
    /// `matte_coverage_statistics`, which takes one argument, measures the
    /// whole raster it is given, and has no ROI parameter.
    pub matte_roi: Option<NormalizedRoi>,
    /// The matte-carrying node. `None` resolves the single node in the final
    /// document whose `matte_enabled` parameter is non-zero.
    pub matte_node: Option<(ClipId, EffectId)>,
    /// The region whose out-of-gamut population is recorded.
    pub gamut_roi: Option<NormalizedRoi>,
    /// The look node whose bypass render is compared against its absence,
    /// and the project frame both are rendered at.
    pub look_bypass: Option<(ClipId, EffectId, i64)>,
    /// Decode and compare the written delivery encode at this depth.
    pub delivery_verification: Option<DeliveryEncodeDepth>,
    /// Parameters whose committed keyframes are summarised into
    /// [`EffectSummary::keyframes`]. An empty list summarises every
    /// keyframed parameter.
    pub keyframe_parameters: Vec<String>,
}

impl Default for ColorEvalRequest {
    fn default() -> Self {
        Self {
            project_frame: 0,
            neutral_patch_rois: Vec::new(),
            chart_luma_roi: None,
            chart_luma_reference_frame: 0,
            chart_luma_candidate_frame: 0,
            qc_roi: None,
            qc_checks: Vec::new(),
            qc_max_nodes: ColorQcRequest::default().max_nodes,
            qc_delivery_bit_depth: DeliveryEncodeDepth::Eight,
            skin_roi: None,
            matte_roi: None,
            matte_node: None,
            gamut_roi: None,
            look_bypass: None,
            delivery_verification: None,
            keyframe_parameters: Vec::new(),
        }
    }
}

impl ColorEvalRequest {
    /// Derive the request from the assertions that gate it, so a region
    /// rectangle is written down once — on the assertion — instead of twice.
    ///
    /// Returns `None` when the list carries no colour assertion, which is
    /// what keeps every non-colour suite's `EvalOutcome::color` at `None`.
    #[must_use]
    pub fn from_assertions(assertions: &[EvalAssertion]) -> Option<Self> {
        let mut request = Self::default();
        let mut colored = false;
        for assertion in assertions {
            match assertion {
                EvalAssertion::ColorQcTechnicalPass { frame, checks, .. } => {
                    colored = true;
                    request.project_frame = *frame;
                    request.qc_checks = checks
                        .iter()
                        .filter_map(|check| color_qc_check_from_token(check))
                        .collect();
                }
                EvalAssertion::DeliveryVerificationWithinBudgets { depth } => {
                    colored = true;
                    request.delivery_verification = Some(*depth);
                    request.qc_delivery_bit_depth = *depth;
                }
                EvalAssertion::NeutralPatchSpreadAtMost { patch_rois, .. } => {
                    colored = true;
                    request.neutral_patch_rois.clone_from(patch_rois);
                }
                EvalAssertion::SkinHueWithinBand { roi, .. } => {
                    colored = true;
                    request.skin_roi = Some(*roi);
                }
                EvalAssertion::MatteContainmentExact { roi, .. } => {
                    colored = true;
                    request.matte_roi = Some(*roi);
                }
                EvalAssertion::TrackKeyframesMatchExpected { parameter, .. } => {
                    colored = true;
                    request.keyframe_parameters.push(parameter.clone());
                }
                EvalAssertion::LookBypassMatchesAbsent {
                    clip_id,
                    effect_id,
                    frame,
                } => {
                    colored = true;
                    request.look_bypass = Some((ClipId(*clip_id), EffectId(*effect_id), *frame));
                }
                EvalAssertion::ReferenceClipUntouched { .. } => colored = true,
                _ => {}
            }
        }
        colored.then_some(request)
    }
}

fn color_qc_check_from_token(token: &str) -> Option<ColorQcCheck> {
    match token {
        "range" => Some(ColorQcCheck::Range),
        "gamut" => Some(ColorQcCheck::Gamut),
        "skin" => Some(ColorQcCheck::Skin),
        "tags" => Some(ColorQcCheck::Tags),
        "per_node" => Some(ColorQcCheck::PerNode),
        _ => None,
    }
}

/// One committed colour node, flattened for assertion arms that must read a
/// document without re-walking it.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectSummary {
    pub clip: ClipId,
    pub effect: EffectId,
    pub name: String,
    pub parameters: BTreeMap<String, ParamValue>,
    /// Clip-local keyframe frames per parameter, in ascending frame order.
    pub keyframes: BTreeMap<String, Vec<i64>>,
}

/// Everything a colour scenario measured, computed where the `Analysis` and
/// the exporter were still alive.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ColorEvalEvidence {
    /// Worst `max(|R−G|, |G−B|)` over the requested achromatic patches.
    pub neutral_spread_max_code: Option<i64>,
    /// `mean(luma at candidate frame) − mean(luma at reference frame)` over
    /// the chart band, in monitoring-code millionths, half away from zero.
    pub chart_luma_mean_delta_millionths: Option<i64>,
    pub skin: Option<SkinDiagnostics>,
    /// Coverage statistics of the matte proof **cropped to** `matte_roi`.
    pub matte: Option<MatteCoverageStatistics>,
    /// `out_of_gamut_pixel_count` over `gamut_roi`.
    pub gamut_pixel_count: Option<u64>,
    pub qc: Option<ColorQcReport>,
    pub verification: Option<DeliveryVerification>,
    pub look_bypass_matches_absent: Option<bool>,
    pub final_effects: Vec<EffectSummary>,
    /// Measurement failures, recorded rather than thrown. A proof that could
    /// not render must still leave a scored, reviewable result behind, and an
    /// unmeasured quantity must read as "not measured", never as "passed".
    ///
    /// Each error names the quantity whose inputs failed, so a partially
    /// measured quantity fails **its own** assertion with the recorded reason
    /// and no other assertion is touched.
    pub errors: Vec<ColorEvidenceError>,
}

/// The measured quantity an error belongs to.
///
/// Attribution is the point: a matte proof that will not render must fail the
/// matte containment claim and leave the skin claim alone, and a neutral
/// patch that will not resolve must fail the spread claim even though the
/// other eleven patches measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ColorEvidenceQuantity {
    NeutralSpread,
    ChartLuma,
    Qc,
    Skin,
    Gamut,
    Matte,
    LookBypass,
    DeliveryVerification,
}

/// One recorded measurement failure and the quantity it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorEvidenceError {
    pub quantity: ColorEvidenceQuantity,
    pub message: String,
}

impl ColorEvalEvidence {
    fn record(&mut self, quantity: ColorEvidenceQuantity, message: String) {
        self.errors.push(ColorEvidenceError { quantity, message });
    }

    /// Every recorded reason a quantity could not be measured, joined.
    ///
    /// `Some(_)` means the quantity's inputs failed, which fails that
    /// quantity's assertion: a partially measured quantity is not a
    /// measurement.
    #[must_use]
    pub fn unmeasurable_reason(&self, quantity: ColorEvidenceQuantity) -> Option<String> {
        let reasons = self
            .errors
            .iter()
            .filter(|error| error.quantity == quantity)
            .map(|error| error.message.as_str())
            .collect::<Vec<_>>();
        (!reasons.is_empty()).then(|| reasons.join("; "))
    }
}

/// One numeric colour claim, so the number reaches `results.jsonl` as data
/// rather than as free text inside an assertion detail string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvalMeasurement {
    pub name: String,
    pub observed: i64,
    pub budget: i64,
    pub unit: String,
    pub passed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct FixtureContext {
    pub asset_aliases: BTreeMap<String, AssetId>,
    pub transcripts: BTreeMap<AssetId, Arc<AssetTranscript>>,
    /// Beat positions in each asset's source-frame time base. These are not
    /// project positions and must only be consumed by source-range checks.
    pub source_beat_sets: BTreeMap<String, Vec<TimeCode>>,
    /// Beat positions in the document's project-frame time base. These are
    /// not source positions and must only be consumed by timeline checks.
    pub timeline_beat_sets: BTreeMap<String, Vec<TimeCode>>,
    pub word_sets: BTreeMap<String, Vec<String>>,
    pub scene_sets: BTreeMap<String, Vec<(AssetId, TimeCode)>>,
    pub exclusion_sets: BTreeMap<String, Vec<SourceRangeExclusion>>,
    pub duration_bounds: BTreeMap<String, (TimeCode, TimeCode)>,
}

pub struct PreparedFixture {
    pub original_document: Document,
    pub core: Core,
    pub playback: Arc<dyn Playback>,
    pub analysis: Arc<dyn Analysis>,
    pub exporter: Arc<dyn Export>,
    pub context: FixtureContext,
    /// The saved project this fixture's session runs against.
    ///
    /// The runner starts every MCP server with a fresh `None` project path,
    /// so any tool that needs a project-relative asset store — `import_lut_asset`
    /// most visibly — refuses `project_not_saved` unless a fixture supplies
    /// one here. This is a **shared-runner** field: every suite carries it and
    /// the suites that do not save a project pass `None`.
    pub project_path: Option<PathBuf>,
    _resources: Vec<Box<dyn Send>>,
}

impl PreparedFixture {
    /// Keep generated resources alive and connect one media engine to a fresh core actor.
    ///
    /// # Errors
    ///
    /// Returns an error if the initial document violates a core invariant.
    pub fn new<T>(
        original_document: Document,
        media: Arc<T>,
        context: FixtureContext,
        project_path: Option<PathBuf>,
        resources: Vec<Box<dyn Send>>,
    ) -> Result<Self, EvalError>
    where
        T: Playback + Analysis + Export + 'static,
    {
        original_document
            .validate()
            .map_err(|error| EvalError::Fixture(error.to_string()))?;
        media.set_document(Arc::new(original_document.clone()));
        let core = Core::spawn(original_document.clone())
            .map_err(|error| EvalError::Fixture(error.to_string()))?;
        let playback: Arc<dyn Playback> = media.clone();
        let analysis: Arc<dyn Analysis> = media.clone();
        let exporter: Arc<dyn Export> = media;
        Ok(Self {
            original_document,
            core,
            playback,
            analysis,
            exporter,
            context,
            project_path,
            _resources: resources,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SessionMetrics {
    pub turns: u32,
    pub tool_calls: BTreeMap<String, u32>,
    pub input_tokens: u64,
    pub cached_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub output_tokens: u64,
    pub reasoning_output_tokens: Option<u64>,
    pub tool_surface: crate::ToolSurfaceMetrics,
    pub cost_usd: Option<f64>,
    pub wall_time_ms: u64,
    pub errors: Vec<String>,
    pub interrupted: bool,
}

impl SessionMetrics {
    #[must_use]
    pub fn tool_call_count(&self) -> u32 {
        self.tool_calls.values().copied().sum()
    }

    #[must_use]
    pub const fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    #[must_use]
    pub fn uncached_input_tokens(&self) -> Option<u64> {
        self.cached_input_tokens
            .map(|cached| self.input_tokens.saturating_sub(cached))
    }
}

#[derive(Debug, Clone)]
pub struct EvalOutcome {
    pub final_document: Document,
    /// The fixture's document as it stood before the session, so an assertion
    /// can prove a clip was left alone instead of trusting a planner's own
    /// hardcoded claim that it was.
    pub original_document: Document,
    /// Colour measurements taken inside the runner, where the fixture's
    /// `Analysis` and exporter were still alive. `None` for every suite whose
    /// definition carries no [`ColorEvalRequest`].
    pub color: Option<ColorEvalEvidence>,
    pub final_words: Vec<String>,
    pub final_timeline_words: Vec<TimelineTranscriptWord>,
    pub remaining_silences: Vec<TimelineSilenceSpan>,
    pub remaining_scenes: Vec<TimelineSceneChange>,
    pub context: FixtureContext,
    pub session: SessionMetrics,
    pub operations: Vec<Operation>,
    pub undo_steps_to_original: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AssertionResult {
    pub assertion: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvalResult {
    pub name: String,
    pub rationale: String,
    pub passed: bool,
    pub assertions: Vec<AssertionResult>,
    /// Numeric evidence beside the boolean assertions.
    ///
    /// `EvalResult` derives `Serialize` only and `results.jsonl` is
    /// write-only, so there is no parse-compatibility claim to make here and
    /// `#[serde(default)]` would be inert. What is guaranteed instead is that
    /// a result with no measurements serialises byte-identically to a
    /// pre-CC7 one, which `skip_serializing_if` is exactly what delivers.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub measurements: Vec<EvalMeasurement>,
    pub turns: u32,
    pub tool_calls: BTreeMap<String, u32>,
    pub input_tokens: u64,
    pub cached_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub output_tokens: u64,
    pub reasoning_output_tokens: Option<u64>,
    pub tool_surface: crate::ToolSurfaceMetrics,
    pub cost_usd: Option<f64>,
    pub wall_time_ms: u64,
    pub operations_applied: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deliverable: Option<EvalDeliverableResult>,
    pub execution_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvalDeliverableResult {
    pub profile: DeliveryProfile,
    pub output_path: PathBuf,
    pub document_path: PathBuf,
    pub proof_path: PathBuf,
    pub resolution: (u32, u32),
    pub duration_frames: TimeCode,
    pub conformance: Option<DeliveryConformanceReport>,
    pub output_bytes: Option<u64>,
    pub output_sha256: Option<String>,
    pub exported_frames: Option<u64>,
    pub probed_resolution: Option<(u32, u32)>,
    pub probed_duration_frames: Option<TimeCode>,
    pub probed_media_kind: Option<MediaKind>,
    pub rendered_transcript_required: bool,
    pub rendered_transcript: Option<RenderedTranscriptVerification>,
    pub rendered_caption_alignment_required: bool,
    pub rendered_caption_alignment: Option<RenderedTranscriptVerification>,
    pub rendered_loudness_contract: Option<EvalLoudnessSpec>,
    pub rendered_loudness: Option<RenderedLoudnessVerification>,
    pub rendered_audio_tail_contract: Option<EvalAudioTailSpec>,
    pub rendered_audio_tail: Option<RenderedAudioTailVerification>,
    pub rendered_reframe: Option<RenderedReframeVerification>,
    pub proof_sample_frames: Vec<TimeCode>,
    pub machine_passed: bool,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RenderedLoudnessVerification {
    pub measurement: AudioLoudness,
    pub minimum_integrated_lufs_hundredths: i32,
    pub maximum_integrated_lufs_hundredths: i32,
    pub maximum_sample_peak_dbfs_hundredths: i32,
    pub passed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RenderedAudioTailVerification {
    pub tail_start_frame: TimeCode,
    pub tail_end_frame: TimeCode,
    pub measurement: AudioLoudness,
    pub terminal_window_frames: TimeCode,
    pub maximum_sample_peak_dbfs_hundredths: i32,
    pub activity_window_frames: TimeCode,
    pub minimum_active_integrated_lufs_hundredths: i32,
    pub maximum_trailing_inactive_frames: TimeCode,
    pub observed_trailing_inactive_frames: TimeCode,
    pub latest_active_window_start_frame: Option<TimeCode>,
    pub latest_active_window_end_frame: Option<TimeCode>,
    pub passed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RenderedReframeVerification {
    pub expected_animated_clips: usize,
    pub preserved_animated_clips: usize,
    pub expected_subject_provenance_clips: usize,
    pub preserved_subject_provenance_clips: usize,
    pub passed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RenderedTranscriptVerification {
    pub expected_words: Vec<String>,
    pub observed_words: Vec<String>,
    pub missing_words: Vec<String>,
    pub unexpected_words: Vec<String>,
    pub edit_distance: usize,
    pub word_error_rate_basis_points: u16,
    pub maximum_word_error_rate_basis_points: u16,
    pub passed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HumanReviewFile {
    /// `1` or `2`. Version 2 adds `blind_id` and `questions`; a version 1
    /// file loads and scores exactly as it always has, and a version 2 file
    /// carrying neither new field round-trips byte-identically to one.
    pub schema_version: u32,
    pub benchmark_id: String,
    pub run_id: String,
    pub reviewer: Option<String>,
    pub tasks: Vec<HumanTaskReview>,
}

/// The highest `human-review.json` schema this build writes.
pub const HUMAN_REVIEW_SCHEMA_VERSION: u32 = 2;

/// Characters of the artefact digest that make a blind identifier.
pub const BLIND_ID_HEX_LENGTH: usize = 12;

/// Derive a task's blind identifier from its artefact digest.
///
/// Derived, never random: two identical artefacts share a `blind_id`, which
/// is what makes the existing "one viewing may be applied to several rows
/// when the artifact hashes are identical" convention mechanical rather than
/// a note in a document.
#[must_use]
pub fn blind_id_for_artifact(artifact_sha256: Option<&str>) -> Option<String> {
    let hash = artifact_sha256?;
    if hash.len() < BLIND_ID_HEX_LENGTH || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(hash[..BLIND_ID_HEX_LENGTH].to_ascii_lowercase())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HumanTaskReview {
    pub task_id: String,
    /// Twelve lowercase hex characters derived from `artifact_sha256`, or
    /// `None` for a task with no artefact. Absent in schema version 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blind_id: Option<String>,
    pub artifact_sha256: Option<String>,
    pub accepted: Option<bool>,
    pub ratings: HumanRatings,
    /// Rating dimensions intentionally not applicable to this task, such as
    /// captions for an instrumental montage. Missing in legacy JSON means an
    /// empty list.
    #[serde(default)]
    pub not_applicable: Vec<HumanRatingDimension>,
    /// The scenario questions a machine has no business answering. Absent in
    /// schema version 1, and empty for every suite that poses none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<HumanQuestion>,
    pub notes: Option<String>,
}

/// One creative question put to a blind reviewer, verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HumanQuestion {
    /// The scenario letter this question belongs to, such as `a` or `g`.
    pub id: String,
    pub prompt: String,
    pub answer: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// The schema of `blind/review-form.json` and `blind-key.json`.
pub const BLIND_SCHEMA_VERSION: u32 = 1;

/// The directory the reviewer is handed, and the only one they open.
pub const BLIND_DIRECTORY_NAME: &str = "blind";

/// The reviewer's file, inside [`BLIND_DIRECTORY_NAME`].
pub const BLIND_FORM_FILE_NAME: &str = "review-form.json";

/// The key, deliberately in the **run root** and never inside `blind/`.
pub const BLIND_KEY_FILE_NAME: &str = "blind-key.json";

/// The only file a blind reviewer opens.
///
/// It is keyed on `blind_id` and carries nothing else that identifies
/// anything: no task id, no run id, no benchmark id, no harness or model
/// name, no machine result, no assertion name, and no parameter name or
/// value. What is deliberately **not** blinded is the scenario identity,
/// which is inherent in the question being asked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlindReviewForm {
    pub schema_version: u32,
    pub entries: Vec<BlindReviewEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlindReviewEntry {
    pub blind_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<HumanQuestion>,
    pub ratings: HumanRatings,
    #[serde(default)]
    pub not_applicable: Vec<HumanRatingDimension>,
    pub accepted: Option<bool>,
    pub notes: Option<String>,
}

/// The mapping from a blind identifier back to the run it came from.
///
/// It lives in the run root rather than in `blind/`, so the directory handed
/// to a reviewer can be handed over whole.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlindKeyFile {
    pub schema_version: u32,
    pub benchmark_id: String,
    pub run_id: String,
    pub entries: Vec<BlindKeyEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlindKeyEntry {
    pub blind_id: String,
    pub task_id: String,
    pub sample: u32,
    pub artifact_sha256: String,
    pub artifact_path: String,
}

impl BlindKeyFile {
    /// Resolve one blind identifier to every task it stands for, or refuse
    /// by name.
    ///
    /// A blind identifier is a digest prefix, so two tasks whose artefacts
    /// are byte-identical share one. Returning every match is what makes the
    /// existing "one viewing may be applied to several rows when the artifact
    /// hashes are identical" convention mechanical instead of manual.
    ///
    /// # Errors
    ///
    /// Returns an error naming the identifier when the key does not carry it.
    pub fn resolve_all(&self, blind_id: &str) -> Result<Vec<&BlindKeyEntry>, EvalError> {
        let matches = self
            .entries
            .iter()
            .filter(|entry| entry.blind_id == blind_id)
            .collect::<Vec<_>>();
        if matches.is_empty() {
            return Err(EvalError::Output(format!(
                "blind id {blind_id:?} is not in {BLIND_KEY_FILE_NAME}"
            )));
        }
        Ok(matches)
    }

    /// Rebuild the unblinded review a blind form stands for.
    ///
    /// This is the resolution that must happen **before** artefact bindings
    /// are verified: the binding check looks tasks up by `task_id`, and a
    /// form keyed on `blind_id` fails there for every task.
    ///
    /// # Errors
    ///
    /// Returns an error naming any `blind_id` the key does not carry.
    pub fn unblind(&self, form: &BlindReviewForm) -> Result<HumanReviewFile, EvalError> {
        let mut tasks = Vec::with_capacity(form.entries.len());
        for entry in &form.entries {
            for keyed in self.resolve_all(&entry.blind_id)? {
                tasks.push(HumanTaskReview {
                    task_id: keyed.task_id.clone(),
                    blind_id: Some(entry.blind_id.clone()),
                    artifact_sha256: Some(keyed.artifact_sha256.clone()),
                    accepted: entry.accepted,
                    ratings: entry.ratings.clone(),
                    not_applicable: entry.not_applicable.clone(),
                    questions: entry.questions.clone(),
                    notes: entry.notes.clone(),
                });
            }
        }
        Ok(HumanReviewFile {
            schema_version: HUMAN_REVIEW_SCHEMA_VERSION,
            benchmark_id: self.benchmark_id.clone(),
            run_id: self.run_id.clone(),
            reviewer: None,
            tasks,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanRatingDimension {
    Story,
    Pacing,
    VisualFinish,
    AudioFinish,
    Captions,
    DeliveryReadiness,
}

impl HumanRatingDimension {
    const ALL: [Self; 6] = [
        Self::Story,
        Self::Pacing,
        Self::VisualFinish,
        Self::AudioFinish,
        Self::Captions,
        Self::DeliveryReadiness,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Story => "story",
            Self::Pacing => "pacing",
            Self::VisualFinish => "visual_finish",
            Self::AudioFinish => "audio_finish",
            Self::Captions => "captions",
            Self::DeliveryReadiness => "delivery_readiness",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HumanRatings {
    pub story: Option<f64>,
    pub pacing: Option<f64>,
    pub visual_finish: Option<f64>,
    pub audio_finish: Option<f64>,
    pub captions: Option<f64>,
    pub delivery_readiness: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HumanReviewSummary {
    pub schema_version: u32,
    pub benchmark_id: String,
    pub run_id: String,
    pub reviewer: Option<String>,
    pub tasks_total: usize,
    pub tasks_reviewed: usize,
    pub tasks_pending: usize,
    pub tasks_accepted: usize,
    pub acceptance_rate: Option<f64>,
    pub overall_mean_rating: Option<f64>,
    pub mean_ratings: HumanMeanRatings,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HumanMeanRatings {
    pub story: Option<f64>,
    pub pacing: Option<f64>,
    pub visual_finish: Option<f64>,
    pub audio_finish: Option<f64>,
    pub captions: Option<f64>,
    pub delivery_readiness: Option<f64>,
}

impl EvalResult {
    #[must_use]
    pub fn execution_failure(definition: &EvalDefinition, error: &EvalError) -> Self {
        Self {
            name: definition.name.to_owned(),
            rationale: definition.rationale.to_owned(),
            passed: false,
            assertions: Vec::new(),
            measurements: Vec::new(),
            turns: 0,
            tool_calls: BTreeMap::new(),
            input_tokens: 0,
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            output_tokens: 0,
            reasoning_output_tokens: None,
            tool_surface: crate::ToolSurfaceMetrics::default(),
            cost_usd: None,
            wall_time_ms: 0,
            operations_applied: 0,
            deliverable: None,
            execution_error: Some(error.to_string()),
        }
    }

    #[must_use]
    pub fn passed_assertion_count(&self) -> usize {
        self.assertions
            .iter()
            .filter(|assertion| assertion.passed)
            .count()
    }

    #[must_use]
    pub fn tool_call_count(&self) -> u32 {
        self.tool_calls.values().copied().sum()
    }

    #[must_use]
    pub const fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvironmentStamp {
    pub timestamp_utc: String,
    pub timestamp_unix_ms: u128,
    pub harness: String,
    pub harness_version: Option<String>,
    pub model: String,
    pub os: String,
    pub architecture: String,
    pub kinewright_version: String,
}

impl EnvironmentStamp {
    #[must_use]
    pub fn capture(info: Option<&HarnessInfo>, harness: &str, model: Option<&str>) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Self {
            timestamp_utc: format_utc_timestamp(now.as_secs()),
            timestamp_unix_ms: now.as_millis(),
            harness: harness.to_owned(),
            harness_version: info.and_then(|value| value.version.clone()),
            model: model.unwrap_or("harness-default").to_owned(),
            os: std::env::consts::OS.to_owned(),
            architecture: std::env::consts::ARCH.to_owned(),
            kinewright_version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EvalError {
    #[error("fixture setup failed: {0}")]
    Fixture(String),
    #[error("MCP server setup failed: {0}")]
    Server(String),
    #[error("agent session failed: {0}")]
    Agent(String),
    #[error("core query failed: {0}")]
    Core(String),
    #[error("media observation failed: {0}")]
    Media(String),
    #[error("result output failed: {0}")]
    Output(String),
}

/// Execute one real driver session, observe the edited project, and evaluate its contracts.
///
/// # Errors
///
/// Returns setup or observation failures. Agent-reported errors are recorded as failed assertions.
pub fn run_eval(
    definition: &EvalDefinition,
    driver: &dyn AgentDriver,
    model: Option<&str>,
    working_directory: Option<&Path>,
) -> Result<EvalResult, EvalError> {
    run_eval_with_artifacts(definition, driver, model, working_directory, None)
}

/// Execute one eval and, when requested by its definition, render a real
/// delivery package before restoring the original timeline.
///
/// # Errors
///
/// Returns setup or observation failures. Delivery failures are retained as
/// scored task failures so the run still produces reviewable evidence.
#[allow(clippy::too_many_lines)]
pub fn run_eval_with_artifacts(
    definition: &EvalDefinition,
    driver: &dyn AgentDriver,
    model: Option<&str>,
    working_directory: Option<&Path>,
    artifact_directory: Option<&Path>,
) -> Result<EvalResult, EvalError> {
    let eval_started = Instant::now();
    let fixture = std::panic::catch_unwind(definition.fixture_builder)
        .map_err(|payload| EvalError::Fixture(panic_message(&payload)))??;
    let server = McpServer::start(
        fixture.core.clone(),
        Arc::clone(&fixture.playback),
        Arc::clone(&fixture.analysis),
    )
    .map_err(|error| EvalError::Server(error.to_string()))?;
    apply_fixture_project_path(&server, &fixture);
    let confirmations = server.confirmations();
    let config = SessionConfig {
        working_directory: working_directory.map(Path::to_path_buf),
        model: model.map(str::to_owned),
        effort: None,
        service_tier: None,
        max_turns: Some(definition.budgets.max_tool_calls.saturating_add(2)),
        mcp_url: Some(server.endpoint().to_owned()),
        tool_names: Some(compact_tool_names()),
    };
    let mut session = collect_session(
        driver,
        config,
        definition.prompts,
        &definition.budgets,
        Some(&confirmations),
        || query_operations(&fixture.core).map(|operations| operations.len()),
    )?;
    session.tool_surface = server.tool_surface_metrics();
    let final_document = query_document(&fixture.core)?;
    let operations = query_operations(&fixture.core)?;
    let final_timeline_words = dedup_timeline_words(
        fixture
            .analysis
            .timeline_transcript(&final_document, None)
            .map_err(|error| EvalError::Media(error.to_string()))?,
    );
    let final_words = final_timeline_words
        .iter()
        .map(|word| word.text.clone())
        .collect::<Vec<_>>();
    let remaining_silences = fixture
        .analysis
        .timeline_silences(&final_document, None, TimeCode(1))
        .map_err(|error| EvalError::Media(error.to_string()))?;
    let remaining_scenes = fixture
        .analysis
        .timeline_scene_changes(&final_document, None, 0)
        .map_err(|error| EvalError::Media(error.to_string()))?;
    let deliverable = definition.deliverable.map(|spec| {
        artifact_directory.map_or_else(
            || {
                failed_deliverable(
                    spec,
                    &final_document,
                    Path::new("unavailable"),
                    "the benchmark runner did not provide an artifact directory".to_owned(),
                )
            },
            |directory| {
                produce_deliverable(
                    spec,
                    &final_document,
                    fixture.analysis.as_ref(),
                    fixture.exporter.as_ref(),
                    &fixture.context,
                    directory,
                )
            },
        )
    });
    // Measure colour HERE, before the fixture's handles go out of scope and
    // before the timeline is restored: this is the last point at which the
    // `Analysis`, the written deliverable and the edited document are all
    // alive at once, and no later stage of the pipeline sees any of them.
    let color = measure_color_block(
        definition,
        fixture.analysis.as_ref(),
        &final_document,
        definition.deliverable.zip(deliverable.as_ref()),
    );
    let undo_steps_to_original = restore_original(
        &fixture.core,
        &fixture.original_document,
        definition.budgets.max_undos,
    )?;
    session.wall_time_ms = duration_millis(eval_started.elapsed());
    let outcome = EvalOutcome {
        final_document: (*final_document).clone(),
        original_document: fixture.original_document.clone(),
        color,
        final_words,
        final_timeline_words,
        remaining_silences,
        remaining_scenes,
        context: fixture.context.clone(),
        session,
        operations,
        undo_steps_to_original,
    };
    let mut result = evaluate(definition, &outcome);
    if let Some(deliverable) = deliverable {
        result
            .assertions
            .extend(deliverable_assertions(&deliverable));
        result.passed = result.assertions.iter().all(|assertion| assertion.passed);
        result.deliverable = Some(deliverable);
    }
    server.shutdown();
    Ok(result)
}

/// Hand a fixture's saved project path to the server it will be driven
/// through.
///
/// [`McpServer::start`] always begins with a fresh `None` project path, so a
/// fixture that saved a project must publish it here or every
/// project-relative tool — `import_lut_asset` most visibly — refuses
/// `project_not_saved` for the whole session. This is **shared-runner**
/// behaviour: a fixture that saved nothing carries `None` and the call is a
/// no-op, which is why v1-v5 are byte-unchanged.
fn apply_fixture_project_path(server: &McpServer, fixture: &PreparedFixture) {
    if let Some(project_path) = fixture.project_path.clone() {
        server.set_project_path(Some(project_path));
    }
}

/// The colour block, built exactly where `run_eval_with_artifacts` builds it.
///
/// The plumbing **is** the design (R-B1): a definition that carries a colour
/// request measures one, and a definition that does not leaves
/// `EvalOutcome::color` at `None`, which is what keeps v1-v5 untouched. It is
/// a named function so a test can exercise the same expression the runner
/// runs rather than a copy of it.
#[must_use]
fn measure_color_block(
    definition: &EvalDefinition,
    analysis: &dyn Analysis,
    final_document: &Arc<Document>,
    deliverable: Option<(EvalDeliverableSpec, &EvalDeliverableResult)>,
) -> Option<ColorEvalEvidence> {
    definition
        .color
        .as_ref()
        .map(|request| measure_color_evidence(request, analysis, final_document, deliverable))
}

fn produce_deliverable(
    spec: EvalDeliverableSpec,
    document: &Document,
    analysis: &dyn Analysis,
    exporter: &dyn Export,
    context: &FixtureContext,
    directory: &Path,
) -> EvalDeliverableResult {
    let mut result = deliverable_shell(spec, document, directory);
    if let Err(error) = fs::create_dir_all(directory) {
        result.errors.push(format!(
            "could not create artifact directory {}: {error}",
            directory.display()
        ));
        return finish_deliverable(result);
    }
    match serde_json::to_vec_pretty(document)
        .map_err(|error| error.to_string())
        .and_then(|json| fs::write(&result.document_path, json).map_err(|error| error.to_string()))
    {
        Ok(()) => {}
        Err(error) => result.errors.push(format!(
            "could not write final document {}: {error}",
            result.document_path.display()
        )),
    }

    let report = match delivery_conformance(
        document,
        spec.profile,
        spec.delivery_bit_depth,
        spec.focus_x_percent,
        spec.focus_y_percent,
    ) {
        Ok(report) => report,
        Err(error) => {
            result.errors.push(format!(
                "delivery profile could not be materialized: {error}"
            ));
            return finish_deliverable(result);
        }
    };
    result.resolution = report.resolution;
    if !report.export_ready() {
        result.errors.push(format!(
            "delivery conformance reported {} blocking issue(s)",
            report
                .issues
                .iter()
                .filter(|issue| issue.severity == kinewright_core::QaSeverity::Error)
                .count()
        ));
    }
    result.conformance = Some(report);

    let delivery_document = match document_for_delivery_profile(
        document,
        spec.profile,
        spec.focus_x_percent,
        spec.focus_y_percent,
    ) {
        Ok(document) => Arc::new(document),
        Err(error) => {
            result
                .errors
                .push(format!("delivery document could not be built: {error}"));
            return finish_deliverable(result);
        }
    };
    result.rendered_reframe = rendered_reframe_verification(document, &delivery_document);
    match render_proof_sheet(
        analysis,
        &delivery_document,
        spec.proof_frames,
        spec.proof_cell_width,
        &result.proof_path,
    ) {
        Ok(frames) => result.proof_sample_frames = frames,
        Err(error) => result.errors.push(error),
    }

    if result
        .conformance
        .as_ref()
        .is_some_and(DeliveryConformanceReport::export_ready)
    {
        let expected_words = rendered_transcript_expectation(spec, context, &mut result);
        export_and_probe(
            &mut result,
            spec,
            analysis,
            exporter,
            &delivery_document,
            expected_words,
        );
    }
    finish_deliverable(result)
}

/// Render and verify a saved edit decision without starting another agent turn.
///
/// This is useful when a renderer or delivery-contract fix needs to be checked
/// against the exact document an agent already produced.
#[must_use]
pub fn render_saved_deliverable(
    spec: EvalDeliverableSpec,
    document: &Document,
    analysis: &dyn Analysis,
    exporter: &dyn Export,
    directory: &Path,
) -> EvalDeliverableResult {
    produce_deliverable(
        spec,
        document,
        analysis,
        exporter,
        &FixtureContext::default(),
        directory,
    )
}

/// Measure everything a colour scenario claims, in the one place where the
/// fixture's `Analysis`, the written deliverable and the edited document are
/// all still alive.
///
/// Nothing here throws: a proof that will not render records its failure in
/// [`ColorEvalEvidence::errors`] and leaves its quantity `None`, so an
/// unmeasured claim reads as "not measured" rather than as "passed", and the
/// run still produces a scored, reviewable result.
///
/// The exporter is deliberately **not** a parameter: by the time this runs
/// the deliverable step has already written the encode through it, and
/// `Analysis::verify_delivery_output` reads that written file.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn measure_color_evidence(
    request: &ColorEvalRequest,
    analysis: &dyn Analysis,
    final_document: &Arc<Document>,
    deliverable: Option<(EvalDeliverableSpec, &EvalDeliverableResult)>,
) -> ColorEvalEvidence {
    let mut evidence = ColorEvalEvidence {
        final_effects: summarize_effects(final_document, &request.keyframe_parameters),
        ..ColorEvalEvidence::default()
    };
    let at = TimeCode(request.project_frame);

    if !request.neutral_patch_rois.is_empty() {
        match analysis.monitor_proof_for_document(Arc::clone(final_document), at) {
            Ok(proof) => {
                // CC6's rule for a QC consumer: a raster that does not claim
                // to be full resolution is refused rather than measured, and
                // a refusal here is an error on the record, not a pass.
                if !proof.metadata.full_resolution {
                    evidence.record(
                        ColorEvidenceQuantity::NeutralSpread,
                        "the monitor proof is not full resolution, so its patch statistics cannot be vouched for"
                            .to_owned(),
                    );
                }
                let mut worst: Option<i64> = None;
                for roi in &request.neutral_patch_rois {
                    match patch_spread_max_code(&proof.image, *roi) {
                        Ok(Some(spread)) => {
                            worst = Some(worst.map_or(spread, |current| current.max(spread)));
                        }
                        // A patch that resolved to no pixel was requested and
                        // not measured. Recording it keeps the detail's
                        // "over N patch(es)" honest: N is the requested
                        // count, and a shortfall now fails rather than
                        // silently reporting the worst of the rest.
                        Ok(None) => evidence.record(
                            ColorEvidenceQuantity::NeutralSpread,
                            format!("the neutral patch {roi:?} resolved to no pixel"),
                        ),
                        Err(message) => {
                            evidence.record(ColorEvidenceQuantity::NeutralSpread, message);
                        }
                    }
                }
                evidence.neutral_spread_max_code = worst;
            }
            Err(error) => evidence.record(
                ColorEvidenceQuantity::NeutralSpread,
                format!("monitor proof for the neutral patches failed: {error}"),
            ),
        }
    }

    if let Some(roi) = request.chart_luma_roi {
        let reference = mean_luma_millionths_at(
            analysis,
            final_document,
            TimeCode(request.chart_luma_reference_frame),
            roi,
        );
        let candidate = mean_luma_millionths_at(
            analysis,
            final_document,
            TimeCode(request.chart_luma_candidate_frame),
            roi,
        );
        match (reference, candidate) {
            (Ok(Some(reference)), Ok(Some(candidate))) => {
                evidence.chart_luma_mean_delta_millionths =
                    Some(candidate.saturating_sub(reference));
            }
            (Err(message), _) | (_, Err(message)) => {
                evidence.record(ColorEvidenceQuantity::ChartLuma, message);
            }
            _ => evidence.record(
                ColorEvidenceQuantity::ChartLuma,
                "the chart luma region resolved to no pixel".to_owned(),
            ),
        }
    }

    if !request.qc_checks.is_empty() || request.skin_roi.is_some() || request.gamut_roi.is_some() {
        match analysis.working_proof_for_document(Arc::clone(final_document), at) {
            Ok(proof) => {
                if !request.qc_checks.is_empty() {
                    match measure_color_qc(
                        &proof,
                        &ColorQcRequest {
                            roi: request.qc_roi,
                            checks: request.qc_checks.clone(),
                            delivery_bit_depth: request.qc_delivery_bit_depth,
                            max_nodes: request.qc_max_nodes,
                            project_frame: request.project_frame,
                            ..ColorQcRequest::default()
                        },
                    ) {
                        Ok(report) => evidence.qc = Some(report),
                        Err(error) => evidence.record(
                            ColorEvidenceQuantity::Qc,
                            format!("colour qc failed: {error}"),
                        ),
                    }
                }
                if let Some(roi) = request.skin_roi {
                    match measure_color_qc(
                        &proof,
                        &ColorQcRequest {
                            roi: Some(roi),
                            checks: vec![ColorQcCheck::Skin],
                            delivery_bit_depth: request.qc_delivery_bit_depth,
                            max_nodes: request.qc_max_nodes,
                            project_frame: request.project_frame,
                            ..ColorQcRequest::default()
                        },
                    ) {
                        Ok(report) => evidence.skin = report.skin,
                        Err(error) => evidence.record(
                            ColorEvidenceQuantity::Skin,
                            format!("skin diagnostic failed: {error}"),
                        ),
                    }
                }
                if let Some(roi) = request.gamut_roi {
                    match measure_color_qc(
                        &proof,
                        &ColorQcRequest {
                            roi: Some(roi),
                            checks: vec![ColorQcCheck::Gamut],
                            delivery_bit_depth: request.qc_delivery_bit_depth,
                            max_nodes: request.qc_max_nodes,
                            project_frame: request.project_frame,
                            ..ColorQcRequest::default()
                        },
                    ) {
                        Ok(report) => {
                            evidence.gamut_pixel_count =
                                Some(report.gamut.out_of_gamut_pixel_count);
                        }
                        Err(error) => evidence.record(
                            ColorEvidenceQuantity::Gamut,
                            format!("gamut measurement failed: {error}"),
                        ),
                    }
                }
            }
            Err(error) => {
                // One proof feeds three quantities, so one failure to render
                // it is recorded against each of the three that asked for it.
                let message = format!("working proof failed: {error}");
                if !request.qc_checks.is_empty() {
                    evidence.record(ColorEvidenceQuantity::Qc, message.clone());
                }
                if request.skin_roi.is_some() {
                    evidence.record(ColorEvidenceQuantity::Skin, message.clone());
                }
                if request.gamut_roi.is_some() {
                    evidence.record(ColorEvidenceQuantity::Gamut, message);
                }
            }
        }
    }

    if let Some(roi) = request.matte_roi {
        match request
            .matte_node
            .or_else(|| resolve_matte_node(final_document))
        {
            Some((clip, effect)) => {
                match analysis.matte_proof_for_document(
                    Arc::clone(final_document),
                    at,
                    clip,
                    effect,
                ) {
                    // `matte_coverage_statistics` takes one argument, measures
                    // the whole raster it is given, and has no ROI parameter,
                    // so a region measurement crops the coverage raster first.
                    Ok(proof) => {
                        if !proof.metadata.render.full_resolution {
                            evidence.record(
                                ColorEvidenceQuantity::Matte,
                                "the matte proof is not full resolution, so its coverage counts cannot be vouched for"
                                    .to_owned(),
                            );
                        }
                        match crop_rgba(&proof.coverage, roi) {
                            Ok(cropped) => match matte_coverage_statistics(&cropped) {
                                Ok(statistics) => evidence.matte = Some(statistics),
                                Err(error) => evidence.record(
                                    ColorEvidenceQuantity::Matte,
                                    format!("matte coverage statistics failed: {error}"),
                                ),
                            },
                            Err(message) => {
                                evidence.record(ColorEvidenceQuantity::Matte, message);
                            }
                        }
                    }
                    Err(error) => evidence.record(
                        ColorEvidenceQuantity::Matte,
                        format!("matte proof failed: {error}"),
                    ),
                }
            }
            None => evidence.record(
                ColorEvidenceQuantity::Matte,
                "matte containment was requested but the final document carries no matte-enabled colour node"
                    .to_owned(),
            ),
        }
    }

    if let Some((clip, effect, frame)) = request.look_bypass {
        match look_bypass_matches_absent(analysis, final_document, clip, effect, TimeCode(frame)) {
            Ok(matches) => evidence.look_bypass_matches_absent = Some(matches),
            Err(message) => evidence.record(ColorEvidenceQuantity::LookBypass, message),
        }
    }

    if let Some(depth) = request.delivery_verification {
        match deliverable {
            Some((spec, result)) => {
                match verify_delivery_encode(analysis, final_document, spec, result, depth) {
                    Ok(verification) => evidence.verification = Some(verification),
                    Err(message) => {
                        evidence.record(ColorEvidenceQuantity::DeliveryVerification, message);
                    }
                }
            }
            None => evidence.record(
                ColorEvidenceQuantity::DeliveryVerification,
                "delivery verification was requested but this task produced no deliverable"
                    .to_owned(),
            ),
        }
    }

    evidence
}

fn summarize_effects(document: &Document, keyframe_parameters: &[String]) -> Vec<EffectSummary> {
    let mut summaries = Vec::new();
    for track in &document.tracks {
        for clip in &track.clips {
            for effect in &clip.effects {
                let keyframes = effect
                    .keyframes
                    .iter()
                    .filter(|(name, _)| {
                        keyframe_parameters.is_empty()
                            || keyframe_parameters.iter().any(|wanted| wanted == *name)
                    })
                    .map(|(name, curve)| {
                        (
                            name.clone(),
                            curve
                                .keyframes
                                .iter()
                                .map(|keyframe| keyframe.at.0)
                                .collect::<Vec<_>>(),
                        )
                    })
                    .collect::<BTreeMap<_, _>>();
                summaries.push(EffectSummary {
                    clip: clip.id,
                    effect: effect.id,
                    name: effect.name.clone(),
                    parameters: effect.parameters.clone(),
                    keyframes,
                });
            }
        }
    }
    summaries
}

/// The single matte-carrying colour node, when the request did not name one.
fn resolve_matte_node(document: &Document) -> Option<(ClipId, EffectId)> {
    document
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .find_map(|clip| {
            clip.effects
                .iter()
                .find(|effect| {
                    matches!(
                        effect.parameters.get(MATTE_ENABLED_PARAMETER),
                        Some(ParamValue::Integer(value)) if *value != 0
                    )
                })
                .map(|effect| (clip.id, effect.id))
        })
}

/// Resolve a normalized rectangle against a raster, insetting each edge so a
/// patch's own boundary pixels never enter its statistic. An inset that would
/// empty the rectangle is not applied.
fn resolve_patch_rect(
    width: u32,
    height: u32,
    roi: NormalizedRoi,
    inset: u32,
) -> Result<(u32, u32, u32, u32), String> {
    let pixels = roi
        .to_pixels(width, height)
        .map_err(|error| format!("region {roi:?} does not resolve on {width}x{height}: {error}"))?;
    let shrink = inset.saturating_mul(2);
    if pixels.width > shrink && pixels.height > shrink {
        Ok((
            pixels.x.saturating_add(inset),
            pixels.y.saturating_add(inset),
            pixels.width - shrink,
            pixels.height - shrink,
        ))
    } else {
        Ok((pixels.x, pixels.y, pixels.width, pixels.height))
    }
}

/// The worst `max(|R−G|, |G−B|)` over one patch's inset pixels.
fn patch_spread_max_code(image: &RgbaImage, roi: NormalizedRoi) -> Result<Option<i64>, String> {
    let (x0, y0, width, height) =
        resolve_patch_rect(image.width, image.height, roi, COLOR_PATCH_INSET_PIXELS)?;
    let mut worst: Option<i64> = None;
    for y in y0..y0.saturating_add(height) {
        for x in x0..x0.saturating_add(width) {
            let index = ((y as usize * image.width as usize) + x as usize) * 4;
            let Some(pixel) = image.pixels.get(index..index + 3) else {
                return Err(format!("monitor proof raster is shorter than {roi:?}"));
            };
            let red = i64::from(pixel[0]);
            let green = i64::from(pixel[1]);
            let blue = i64::from(pixel[2]);
            let spread = (red - green).abs().max((green - blue).abs());
            worst = Some(worst.map_or(spread, |current: i64| current.max(spread)));
        }
    }
    Ok(worst)
}

/// The BT.709 luma mean of one region's inset pixels, in monitoring-code
/// millionths, rounded half away from zero.
fn mean_luma_millionths(image: &RgbaImage, roi: NormalizedRoi) -> Result<Option<i64>, String> {
    let (x0, y0, width, height) =
        resolve_patch_rect(image.width, image.height, roi, COLOR_PATCH_INSET_PIXELS)?;
    let mut total = 0.0_f64;
    let mut count = 0_u32;
    for y in y0..y0.saturating_add(height) {
        for x in x0..x0.saturating_add(width) {
            let index = ((y as usize * image.width as usize) + x as usize) * 4;
            let Some(pixel) = image.pixels.get(index..index + 3) else {
                return Err(format!("monitor proof raster is shorter than {roi:?}"));
            };
            total += 0.2126 * f64::from(pixel[0])
                + 0.7152 * f64::from(pixel[1])
                + 0.0722 * f64::from(pixel[2]);
            count = count.saturating_add(1);
        }
    }
    if count == 0 {
        return Ok(None);
    }
    let mean = total / f64::from(count);
    Ok(Some(round_half_away_from_zero(mean * 1_000_000.0)))
}

fn mean_luma_millionths_at(
    analysis: &dyn Analysis,
    document: &Arc<Document>,
    at: TimeCode,
    roi: NormalizedRoi,
) -> Result<Option<i64>, String> {
    let proof = analysis
        .monitor_proof_for_document(Arc::clone(document), at)
        .map_err(|error| format!("monitor proof at frame {} failed: {error}", at.0))?;
    mean_luma_millionths(&proof.image, roi)
}

/// Round half away from zero and saturate at the `i64` bounds, so the
/// millionths convention is the same one every other CC7 number uses.
#[allow(clippy::cast_possible_truncation)]
fn round_half_away_from_zero(value: f64) -> i64 {
    if !value.is_finite() {
        return 0;
    }
    let rounded = if value >= 0.0 {
        (value + 0.5).floor()
    } else {
        (value - 0.5).ceil()
    };
    #[allow(clippy::cast_precision_loss)]
    let clamped = rounded.clamp(i64::MIN as f64, i64::MAX as f64);
    clamped as i64
}

fn crop_rgba(image: &RgbaImage, roi: NormalizedRoi) -> Result<RgbaImage, String> {
    let (x0, y0, width, height) = resolve_patch_rect(image.width, image.height, roi, 0)?;
    if width == 0 || height == 0 {
        return Err(format!("region {roi:?} resolves to no pixel"));
    }
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for y in y0..y0.saturating_add(height) {
        for x in x0..x0.saturating_add(width) {
            let index = ((y as usize * image.width as usize) + x as usize) * 4;
            let Some(pixel) = image.pixels.get(index..index + 4) else {
                return Err(format!("coverage raster is shorter than {roi:?}"));
            };
            pixels.extend_from_slice(pixel);
        }
    }
    Ok(RgbaImage {
        width,
        height,
        pixels,
    })
}

/// Render the node bypassed and the node removed, and compare the two
/// rasters byte for byte. A bypassed node that still contributes something is
/// exactly what this is here to catch.
fn look_bypass_matches_absent(
    analysis: &dyn Analysis,
    document: &Arc<Document>,
    clip: ClipId,
    effect: EffectId,
    at: TimeCode,
) -> Result<bool, String> {
    let absent = scratch_document(
        document,
        &[Operation::RemoveEffect { clip, effect }],
        "node-removed",
    )?;
    let bypassed = scratch_document(
        document,
        &[Operation::SetEffectParam {
            clip,
            effect,
            name: kinewright_core::COLOR_NODE_BYPASS_PARAMETER.to_owned(),
            value: ParamValue::Integer(1),
        }],
        "node-bypassed",
    )?;
    let absent = analysis
        .monitor_proof_for_document(absent, at)
        .map_err(|error| format!("node-removed proof failed: {error}"))?;
    let bypassed = analysis
        .monitor_proof_for_document(bypassed, at)
        .map_err(|error| format!("node-bypassed proof failed: {error}"))?;
    Ok(absent.image.width == bypassed.image.width
        && absent.image.height == bypassed.image.height
        && absent.image.pixels == bypassed.image.pixels)
}

fn scratch_document(
    document: &Arc<Document>,
    operations: &[Operation],
    label: &str,
) -> Result<Arc<Document>, String> {
    let mut scratch = (**document).clone();
    apply_batch(&mut scratch, operations)
        .map_err(|error| format!("{label} scratch document could not be built: {error}"))?;
    Ok(Arc::new(scratch))
}

fn verify_delivery_encode(
    analysis: &dyn Analysis,
    document: &Arc<Document>,
    spec: EvalDeliverableSpec,
    result: &EvalDeliverableResult,
    depth: DeliveryEncodeDepth,
) -> Result<DeliveryVerification, String> {
    if !result.output_path.exists() {
        return Err(format!(
            "delivery verification found no encode at {}",
            result.output_path.display()
        ));
    }
    let delivery_document = document_for_delivery_profile(
        document,
        spec.profile,
        spec.focus_x_percent,
        spec.focus_y_percent,
    )
    .map_err(|error| format!("delivery document could not be built: {error}"))?;
    let delivery_document = Arc::new(delivery_document);
    let settings =
        spec.profile
            .export_settings(&delivery_document, depth, ExportCancellation::default());
    let verification_request =
        DeliveryVerificationRequest::new(depth, settings.delivery_color.clone());
    analysis
        .verify_delivery_output(
            Arc::clone(&delivery_document),
            &result.output_path,
            &settings,
            verification_request,
        )
        .map_err(|error| format!("delivery verification failed: {error}"))
}

fn rendered_transcript_expectation<'a>(
    spec: EvalDeliverableSpec,
    context: &'a FixtureContext,
    result: &mut EvalDeliverableResult,
) -> Option<&'a [String]> {
    let expected = spec
        .expected_transcript_word_set
        .and_then(|word_set| context.word_sets.get(word_set).map(Vec::as_slice));
    if spec.expected_transcript_word_set.is_some() && expected.is_none() {
        result.errors.push(format!(
            "unknown rendered transcript word set {:?}",
            spec.expected_transcript_word_set
        ));
    }
    if spec.maximum_word_error_rate_basis_points > 10_000 {
        result.errors.push(format!(
            "maximum rendered transcript word error rate must be at most 10000 basis points, got {}",
            spec.maximum_word_error_rate_basis_points
        ));
    }
    if spec
        .maximum_caption_word_error_rate_basis_points
        .is_some_and(|maximum| maximum > 10_000)
    {
        result.errors.push(format!(
            "maximum rendered caption word error rate must be at most 10000 basis points, got {:?}",
            spec.maximum_caption_word_error_rate_basis_points
        ));
    }
    expected
}

fn export_and_probe(
    result: &mut EvalDeliverableResult,
    spec: EvalDeliverableSpec,
    analysis: &dyn Analysis,
    exporter: &dyn Export,
    document: &Arc<Document>,
    expected_transcript_words: Option<&[String]>,
) {
    if result.output_path.exists() {
        result.errors.push(format!(
            "refusing to overwrite existing benchmark artifact {}",
            result.output_path.display()
        ));
        return;
    }
    let settings = spec.profile.export_settings(
        document,
        spec.delivery_bit_depth,
        ExportCancellation::default(),
    );
    let (progress_tx, progress_rx) = crossbeam_channel::unbounded();
    if let Err(error) = exporter.export_document(
        Arc::clone(document),
        &result.output_path,
        settings,
        progress_tx,
    ) {
        result.errors.push(format!("export failed: {error}"));
        return;
    }
    result.exported_frames = progress_rx
        .try_iter()
        .last()
        .map(|progress| progress.completed_frames);
    let metadata = match fs::metadata(&result.output_path) {
        Ok(metadata) if metadata.len() > 0 => metadata,
        Ok(_) => {
            result
                .errors
                .push("export backend produced an empty media file".to_owned());
            return;
        }
        Err(error) => {
            result.errors.push(format!(
                "export backend returned success but {} is unavailable: {error}",
                result.output_path.display()
            ));
            return;
        }
    };
    result.output_bytes = Some(metadata.len());
    match kinewright_media::sha256_file(&result.output_path) {
        Ok(hash) => result.output_sha256 = Some(hash),
        Err(error) => result.errors.push(error.to_string()),
    }
    let asset = match analysis.probe(&result.output_path) {
        Ok(asset) => asset,
        Err(error) => {
            result
                .errors
                .push(format!("export could not be probed: {error}"));
            return;
        }
    };
    result.probed_resolution = asset.resolution;
    result.probed_duration_frames = Some(asset.duration);
    result.probed_media_kind = Some(asset.kind);
    if asset.resolution != Some(result.resolution) {
        result.errors.push(format!(
            "export probe raster {:?} does not match {:?}",
            asset.resolution, result.resolution
        ));
    }
    if asset.duration.0.abs_diff(result.duration_frames.0) > 1 {
        result.errors.push(format!(
            "export probe duration {} differs from timeline {} by more than one frame",
            asset.duration.0, result.duration_frames.0
        ));
    }
    if spec.require_audio && asset.kind != MediaKind::AudioVideo {
        result.errors.push(format!(
            "export probe found {:?}; finished cut requires video with an audio stream",
            asset.kind
        ));
    }
    if let Some(contract) = spec.loudness {
        verify_rendered_loudness(result, analysis, &asset, contract);
    }
    if let Some(contract) = spec.audio_tail {
        verify_rendered_audio_tail(result, analysis, &asset, contract);
    }
    if let Some(expected_words) = expected_transcript_words {
        verify_rendered_delivery_transcript(
            result,
            spec,
            analysis,
            &asset,
            document,
            expected_words,
        );
    }
}

fn verify_rendered_loudness(
    result: &mut EvalDeliverableResult,
    analysis: &dyn Analysis,
    asset: &kinewright_core::MediaAsset,
    contract: EvalLoudnessSpec,
) {
    if contract.minimum_integrated_lufs_hundredths > contract.maximum_integrated_lufs_hundredths {
        result
            .errors
            .push("rendered loudness bounds are reversed".to_owned());
        return;
    }
    let measurement = match analysis.asset_loudness(asset) {
        Ok(measurement) => measurement,
        Err(error) => {
            result
                .errors
                .push(format!("rendered loudness measurement failed: {error}"));
            return;
        }
    };
    let passed = measurement
        .integrated_lufs_hundredths
        .is_some_and(|loudness| {
            (contract.minimum_integrated_lufs_hundredths
                ..=contract.maximum_integrated_lufs_hundredths)
                .contains(&loudness)
        })
        && measurement
            .sample_peak_dbfs_hundredths
            .is_some_and(|peak| peak <= contract.maximum_sample_peak_dbfs_hundredths);
    if !passed {
        result.errors.push(format!(
            "rendered audio violates loudness delivery: integrated_lufs_hundredths={:?}, required={}..={}; sample_peak_dbfs_hundredths={:?}, maximum={}",
            measurement.integrated_lufs_hundredths,
            contract.minimum_integrated_lufs_hundredths,
            contract.maximum_integrated_lufs_hundredths,
            measurement.sample_peak_dbfs_hundredths,
            contract.maximum_sample_peak_dbfs_hundredths,
        ));
    }
    result.rendered_loudness = Some(RenderedLoudnessVerification {
        measurement,
        minimum_integrated_lufs_hundredths: contract.minimum_integrated_lufs_hundredths,
        maximum_integrated_lufs_hundredths: contract.maximum_integrated_lufs_hundredths,
        maximum_sample_peak_dbfs_hundredths: contract.maximum_sample_peak_dbfs_hundredths,
        passed,
    });
}

fn verify_rendered_audio_tail(
    result: &mut EvalDeliverableResult,
    analysis: &dyn Analysis,
    asset: &kinewright_core::MediaAsset,
    contract: EvalAudioTailSpec,
) {
    let tail_range = match audio_tail_range(asset.duration, contract) {
        Ok(range) => range,
        Err(error) => {
            result
                .errors
                .push(format!("rendered audio-tail contract is invalid: {error}"));
            return;
        }
    };
    let tail_document = audio_tail_document(asset, tail_range.clone());
    let measurement = match analysis.timeline_loudness(&tail_document) {
        Ok(measurement) => measurement,
        Err(error) => {
            result
                .errors
                .push(format!("rendered audio-tail measurement failed: {error}"));
            return;
        }
    };
    let passed = audio_tail_peak_passes(&measurement, contract.maximum_sample_peak_dbfs_hundredths);
    let (observed_trailing_inactive_frames, latest_active_window) =
        match measure_trailing_audio_activity(analysis, asset, contract) {
            Ok(activity) => activity,
            Err(error) => {
                result.errors.push(format!(
                    "rendered trailing-audio activity measurement failed: {error}"
                ));
                return;
            }
        };
    let activity_passed = latest_active_window.is_some()
        && observed_trailing_inactive_frames <= contract.maximum_trailing_inactive_frames;
    if !passed {
        result.errors.push(format!(
            "rendered audio tail violates terminal peak delivery: frames={}..{}, sample_peak_dbfs_hundredths={:?}, maximum={}",
            tail_range.start,
            tail_range.end,
            measurement.sample_peak_dbfs_hundredths,
            contract.maximum_sample_peak_dbfs_hundredths,
        ));
    }
    if !activity_passed {
        result.errors.push(format!(
            "rendered audio becomes perceptually inactive too early: observed_trailing_inactive_frames_at_least={}, maximum={}, activity_window_frames={}, minimum_active_integrated_lufs_hundredths={}",
            observed_trailing_inactive_frames.0,
            contract.maximum_trailing_inactive_frames.0,
            contract.activity_window_frames.0,
            contract.minimum_active_integrated_lufs_hundredths,
        ));
    }
    result.rendered_audio_tail = Some(RenderedAudioTailVerification {
        tail_start_frame: tail_range.start,
        tail_end_frame: tail_range.end,
        measurement,
        terminal_window_frames: contract.terminal_window_frames,
        maximum_sample_peak_dbfs_hundredths: contract.maximum_sample_peak_dbfs_hundredths,
        activity_window_frames: contract.activity_window_frames,
        minimum_active_integrated_lufs_hundredths: contract
            .minimum_active_integrated_lufs_hundredths,
        maximum_trailing_inactive_frames: contract.maximum_trailing_inactive_frames,
        observed_trailing_inactive_frames,
        latest_active_window_start_frame: latest_active_window.as_ref().map(|range| range.start),
        latest_active_window_end_frame: latest_active_window.as_ref().map(|range| range.end),
        passed: passed && activity_passed,
    });
}

fn measure_trailing_audio_activity(
    analysis: &dyn Analysis,
    asset: &kinewright_core::MediaAsset,
    contract: EvalAudioTailSpec,
) -> Result<(TimeCode, Option<std::ops::Range<TimeCode>>), String> {
    let mut window_end = asset.duration;
    let mut observed_inactive = TimeCode::ZERO;
    loop {
        let window_start = TimeCode(
            window_end
                .0
                .saturating_sub(contract.activity_window_frames.0),
        );
        let range = window_start..window_end;
        let document = audio_tail_document(asset, range.clone());
        let measurement = analysis
            .timeline_loudness(&document)
            .map_err(|error| format!("frames={}..{}: {error}", range.start, range.end))?;
        if audio_activity_loudness_passes(
            &measurement,
            contract.minimum_active_integrated_lufs_hundredths,
        ) {
            return Ok((observed_inactive, Some(range)));
        }
        observed_inactive = TimeCode(observed_inactive.0 + range.end.0 - range.start.0);
        if observed_inactive > contract.maximum_trailing_inactive_frames
            || window_start == TimeCode::ZERO
        {
            return Ok((observed_inactive, None));
        }
        window_end = window_start;
    }
}

fn audio_tail_range(
    encoded_duration: TimeCode,
    contract: EvalAudioTailSpec,
) -> Result<std::ops::Range<TimeCode>, String> {
    if encoded_duration <= TimeCode::ZERO {
        return Err(format!(
            "encoded duration must be positive, got {} frames",
            encoded_duration.0
        ));
    }
    if contract.terminal_window_frames <= TimeCode::ZERO {
        return Err(format!(
            "terminal window must be positive, got {} frames",
            contract.terminal_window_frames.0
        ));
    }
    if contract.terminal_window_frames > encoded_duration {
        return Err(format!(
            "terminal window {} frames exceeds encoded duration {} frames",
            contract.terminal_window_frames.0, encoded_duration.0
        ));
    }
    if contract.activity_window_frames <= TimeCode::ZERO {
        return Err(format!(
            "activity window must be positive, got {} frames",
            contract.activity_window_frames.0
        ));
    }
    if contract.activity_window_frames > encoded_duration {
        return Err(format!(
            "activity window {} frames exceeds encoded duration {} frames",
            contract.activity_window_frames.0, encoded_duration.0
        ));
    }
    if contract.maximum_trailing_inactive_frames < TimeCode::ZERO {
        return Err(format!(
            "maximum trailing inactive duration cannot be negative, got {} frames",
            contract.maximum_trailing_inactive_frames.0
        ));
    }
    if contract.minimum_active_integrated_lufs_hundredths > 0 {
        return Err(format!(
            "active integrated-loudness threshold cannot be positive, got {}",
            contract.minimum_active_integrated_lufs_hundredths
        ));
    }
    let start = TimeCode(encoded_duration.0 - contract.terminal_window_frames.0);
    Ok(start..encoded_duration)
}

fn audio_tail_document(
    asset: &kinewright_core::MediaAsset,
    tail_range: std::ops::Range<TimeCode>,
) -> Document {
    let duration = TimeCode(tail_range.end.0 - tail_range.start.0);
    Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: vec![Clip {
                id: ClipId(1),
                asset: asset.id,
                source_range: tail_range,
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
        media_pool: vec![asset.clone()],
        fps: asset.fps,
        resolution: asset.resolution.unwrap_or((1, 1)),
        duration,
        ..Document::default()
    }
}

fn audio_tail_peak_passes(measurement: &AudioLoudness, maximum_peak: i32) -> bool {
    measurement
        .sample_peak_dbfs_hundredths
        .is_none_or(|peak| peak <= maximum_peak)
}

fn audio_activity_loudness_passes(measurement: &AudioLoudness, minimum_loudness: i32) -> bool {
    measurement
        .integrated_lufs_hundredths
        .is_some_and(|loudness| loudness >= minimum_loudness)
}

fn verify_rendered_delivery_transcript(
    result: &mut EvalDeliverableResult,
    spec: EvalDeliverableSpec,
    analysis: &dyn Analysis,
    asset: &kinewright_core::MediaAsset,
    document: &Document,
    expected_words: &[String],
) {
    match verify_rendered_transcript(
        analysis,
        asset,
        expected_words,
        spec.maximum_word_error_rate_basis_points,
    ) {
        Ok(verification) => {
            if !verification.passed {
                result.errors.push(format!(
                    "rendered transcript exceeds its authored-ground-truth error ceiling: wer_bp={}, maximum_bp={}, missing={:?}, unexpected={:?}",
                    verification.word_error_rate_basis_points,
                    verification.maximum_word_error_rate_basis_points,
                    verification.missing_words,
                    verification.unexpected_words
                ));
            }
            if let Some(maximum) = spec.maximum_caption_word_error_rate_basis_points {
                let caption_words = ordered_caption_words(document);
                let alignment =
                    verify_word_sequences(&verification.observed_words, &caption_words, maximum);
                if !alignment.passed {
                    result.errors.push(format!(
                        "rendered captions disagree with rendered audio: wer_bp={}, maximum_bp={}, missing={:?}, unexpected={:?}",
                        alignment.word_error_rate_basis_points,
                        alignment.maximum_word_error_rate_basis_points,
                        alignment.missing_words,
                        alignment.unexpected_words
                    ));
                }
                result.rendered_caption_alignment = Some(alignment);
            }
            result.rendered_transcript = Some(verification);
        }
        Err(error) => result.errors.push(error),
    }
}

fn verify_rendered_transcript(
    analysis: &dyn Analysis,
    asset: &kinewright_core::MediaAsset,
    expected_words: &[String],
    maximum_word_error_rate_basis_points: u16,
) -> Result<RenderedTranscriptVerification, String> {
    analysis.request_transcription_with_language(asset.clone(), Some("en"));
    let deadline = Instant::now() + Duration::from_mins(20);
    let transcript = loop {
        match analysis.transcript_status(asset) {
            TranscriptStatus::Ready(transcript) => break transcript,
            TranscriptStatus::NoSpeech => {
                return Err("post-render transcription found no speech".to_owned());
            }
            TranscriptStatus::Cancelled => {
                return Err("post-render transcription was cancelled".to_owned());
            }
            TranscriptStatus::Failed(error) => {
                return Err(format!("post-render transcription failed: {error}"));
            }
            TranscriptStatus::NotRequested
            | TranscriptStatus::Queued
            | TranscriptStatus::Hashing
            | TranscriptStatus::DownloadingModel { .. }
            | TranscriptStatus::Transcribing { .. } => {}
        }
        if Instant::now() >= deadline {
            return Err("post-render transcription timed out after twenty minutes".to_owned());
        }
        thread::sleep(Duration::from_millis(100));
    };
    if normalize_word_sequence(expected_words.iter().map(String::as_str)).is_empty() {
        return Err("rendered transcript ground truth contains no words".to_owned());
    }
    let observed_words =
        normalize_word_sequence(transcript.words.iter().map(|word| word.text.as_str()));
    Ok(verify_word_sequences(
        expected_words,
        &observed_words,
        maximum_word_error_rate_basis_points,
    ))
}

fn verify_word_sequences(
    expected_words: &[String],
    observed_words: &[String],
    maximum_word_error_rate_basis_points: u16,
) -> RenderedTranscriptVerification {
    let expected_words = normalize_word_sequence(expected_words.iter().map(String::as_str));
    let observed_words = normalize_word_sequence(observed_words.iter().map(String::as_str));
    let (missing_words, unexpected_words) = word_sequence_delta(&expected_words, &observed_words);
    let edit_distance = word_sequence_edit_distance(&expected_words, &observed_words);
    let word_error_rate_basis_points =
        word_error_rate_basis_points(edit_distance, expected_words.len());
    RenderedTranscriptVerification {
        passed: word_error_rate_basis_points <= maximum_word_error_rate_basis_points,
        expected_words,
        observed_words,
        missing_words,
        unexpected_words,
        edit_distance,
        word_error_rate_basis_points,
        maximum_word_error_rate_basis_points,
    }
}

fn ordered_caption_words(document: &Document) -> Vec<String> {
    let mut captions = timeline_clips(document)
        .filter_map(|clip| match &clip.content {
            ClipContent::Title(title) if title.caption_preset.is_some() => {
                Some((clip.timeline_start, clip.id, title.text.as_str()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    captions.sort_by_key(|(start, clip, _)| (*start, *clip));
    normalize_word_sequence(captions.into_iter().map(|(_, _, text)| text))
}

fn failed_deliverable(
    spec: EvalDeliverableSpec,
    document: &Document,
    directory: &Path,
    error: String,
) -> EvalDeliverableResult {
    let mut result = deliverable_shell(spec, document, directory);
    result.errors.push(error);
    finish_deliverable(result)
}

fn deliverable_shell(
    spec: EvalDeliverableSpec,
    document: &Document,
    directory: &Path,
) -> EvalDeliverableResult {
    EvalDeliverableResult {
        profile: spec.profile,
        output_path: directory.join(format!("finished.{}", spec.profile.container_extension())),
        document_path: directory.join("final-document.json"),
        proof_path: directory.join("proof.png"),
        resolution: spec.profile.resolution(document.resolution),
        duration_frames: document.duration,
        conformance: None,
        output_bytes: None,
        output_sha256: None,
        exported_frames: None,
        probed_resolution: None,
        probed_duration_frames: None,
        probed_media_kind: None,
        rendered_transcript_required: spec.expected_transcript_word_set.is_some(),
        rendered_transcript: None,
        rendered_caption_alignment_required: spec
            .maximum_caption_word_error_rate_basis_points
            .is_some(),
        rendered_caption_alignment: None,
        rendered_loudness_contract: spec.loudness,
        rendered_loudness: None,
        rendered_audio_tail_contract: spec.audio_tail,
        rendered_audio_tail: None,
        rendered_reframe: None,
        proof_sample_frames: Vec::new(),
        machine_passed: false,
        errors: Vec::new(),
    }
}

/// The colour-workflow benchmark, whose review template renders no dialogue,
/// no music and no captions and therefore rates none of them.
pub const COLOR_WORKFLOW_BENCHMARK_ID: &str = "kinewright-color-workflow-v6";

/// The dimensions the colour suite pre-marks not applicable: rating a story,
/// a pacing, an audio finish or a caption on a chart raster would be
/// fabrication, so the template says so instead of asking.
pub const COLOR_WORKFLOW_NOT_APPLICABLE: [HumanRatingDimension; 4] = [
    HumanRatingDimension::Story,
    HumanRatingDimension::Pacing,
    HumanRatingDimension::AudioFinish,
    HumanRatingDimension::Captions,
];

#[must_use]
pub fn human_review_template(
    benchmark_id: &str,
    run_id: &str,
    results: &[EvalResult],
) -> HumanReviewFile {
    human_review_template_with_questions(benchmark_id, run_id, results, &BTreeMap::new())
}

/// Build the review template, attaching each task's scenario questions.
///
/// `questions` is keyed by **base** task id — `c1`, not `c1-sample-2` — so a
/// multi-sample run poses the same question to every sample of one scenario.
#[must_use]
pub fn human_review_template_with_questions(
    benchmark_id: &str,
    run_id: &str,
    results: &[EvalResult],
    questions: &BTreeMap<String, Vec<HumanQuestion>>,
) -> HumanReviewFile {
    let mut occurrences = BTreeMap::<&str, usize>::new();
    let mut tasks = Vec::new();
    for result in results {
        let Some(deliverable) = result.deliverable.as_ref() else {
            continue;
        };
        let base_task_id = result
            .name
            .split_whitespace()
            .next()
            .unwrap_or(&result.name);
        let occurrence = occurrences.entry(base_task_id).or_default();
        *occurrence += 1;
        let task_id = if *occurrence == 1 {
            base_task_id.to_owned()
        } else {
            format!("{base_task_id}-sample-{occurrence}")
        };
        let not_applicable = if benchmark_id == COLOR_WORKFLOW_BENCHMARK_ID {
            COLOR_WORKFLOW_NOT_APPLICABLE.to_vec()
        } else if deliverable.rendered_caption_alignment_required {
            Vec::new()
        } else {
            vec![HumanRatingDimension::Captions]
        };
        tasks.push(HumanTaskReview {
            task_id,
            blind_id: blind_id_for_artifact(deliverable.output_sha256.as_deref()),
            artifact_sha256: deliverable.output_sha256.clone(),
            accepted: None,
            ratings: HumanRatings::default(),
            not_applicable,
            questions: questions.get(base_task_id).cloned().unwrap_or_default(),
            notes: None,
        });
    }

    HumanReviewFile {
        schema_version: HUMAN_REVIEW_SCHEMA_VERSION,
        benchmark_id: benchmark_id.to_owned(),
        run_id: run_id.to_owned(),
        reviewer: None,
        tasks,
    }
}

/// Reduce a review template to the reviewer's copy.
///
/// Only the blind identifier, the questions and the empty rating slots
/// survive: the task id, the run, the benchmark, the machine verdict and
/// every parameter the agent chose are all left behind in the run root.
/// Entries are ordered by `blind_id` ascending — deterministic, and carrying
/// no provenance, because the identifier is a hash prefix rather than a
/// position in the run.
#[must_use]
pub fn blind_review_form(review: &HumanReviewFile) -> BlindReviewForm {
    let mut entries = review
        .tasks
        .iter()
        .filter_map(|task| {
            let blind_id = task.blind_id.clone()?;
            Some(BlindReviewEntry {
                blind_id,
                questions: task.questions.clone(),
                ratings: task.ratings.clone(),
                not_applicable: task.not_applicable.clone(),
                accepted: task.accepted,
                notes: task.notes.clone(),
            })
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.blind_id.cmp(&right.blind_id));
    entries.dedup_by(|left, right| left.blind_id == right.blind_id);
    BlindReviewForm {
        schema_version: BLIND_SCHEMA_VERSION,
        entries,
    }
}

/// Validate a review and compute acceptance separately from machine scores.
/// Pending tasks remain pending and never count as rejected or accepted.
///
/// # Errors
///
/// Returns an error for duplicate tasks, partial reviews, invalid digests, or
/// ratings outside the inclusive 1..=5 scale or its half-point increments.
#[allow(clippy::too_many_lines)]
pub fn summarize_human_review(review: &HumanReviewFile) -> Result<HumanReviewSummary, EvalError> {
    if !matches!(review.schema_version, 1 | 2) {
        return Err(EvalError::Output(format!(
            "unsupported human-review schema version {}",
            review.schema_version
        )));
    }
    let mut task_ids = BTreeSet::new();
    let mut reviewed = Vec::new();
    for task in &review.tasks {
        if task.task_id.trim().is_empty() || !task_ids.insert(task.task_id.as_str()) {
            return Err(EvalError::Output(format!(
                "human review contains an empty or duplicate task id {:?}",
                task.task_id
            )));
        }
        if let Some(hash) = &task.artifact_sha256
            && (hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(EvalError::Output(format!(
                "task {} has an invalid artifact sha256",
                task.task_id
            )));
        }
        let mut not_applicable = BTreeSet::new();
        for dimension in &task.not_applicable {
            if !not_applicable.insert(*dimension) {
                return Err(EvalError::Output(format!(
                    "task {} lists rating dimension {} as not applicable more than once",
                    task.task_id,
                    dimension.label()
                )));
            }
            if task.ratings.get(*dimension).is_some() {
                return Err(EvalError::Output(format!(
                    "task {} marks rating dimension {} both rated and not applicable",
                    task.task_id,
                    dimension.label()
                )));
            }
        }
        let rating_values = task.ratings.values();
        let any_rating = rating_values.iter().any(Option::is_some);
        match task.accepted {
            None if any_rating => {
                return Err(EvalError::Output(format!(
                    "task {} has ratings but no acceptance decision",
                    task.task_id
                )));
            }
            None => {}
            Some(_) => {
                let mut missing = Vec::new();
                for dimension in HumanRatingDimension::ALL {
                    if task.ratings.get(dimension).is_none() && !not_applicable.contains(&dimension)
                    {
                        missing.push(dimension.label());
                    }
                    if let Some(rating) = task.ratings.get(dimension)
                        && !valid_human_rating(rating)
                    {
                        return Err(EvalError::Output(format!(
                            "task {} rating {} for {} must be between 1 and 5 in 0.5 increments",
                            task.task_id,
                            rating,
                            dimension.label()
                        )));
                    }
                }
                if !missing.is_empty() {
                    return Err(EvalError::Output(format!(
                        "task {} must rate or mark not applicable every dimension; missing {:?}",
                        task.task_id, missing
                    )));
                }
                // Schema version 2: an acceptance decision that leaves the
                // scenario question unanswered is not a review, because the
                // question is the only thing the machine could not decide.
                if let Some(unanswered) = task
                    .questions
                    .iter()
                    .find(|question| question.answer.is_none())
                {
                    return Err(EvalError::Output(format!(
                        "task {} must answer question {:?} before it can be accepted or rejected",
                        task.task_id, unanswered.id
                    )));
                }
                reviewed.push(task);
            }
        }
    }
    let accepted = reviewed
        .iter()
        .filter(|task| task.accepted == Some(true))
        .count();
    let acceptance_rate = if reviewed.is_empty() {
        None
    } else {
        let accepted = u32::try_from(accepted)
            .map_err(|_| EvalError::Output("too many accepted human-review tasks".to_owned()))?;
        let reviewed = u32::try_from(reviewed.len())
            .map_err(|_| EvalError::Output("too many human-review tasks".to_owned()))?;
        Some(f64::from(accepted) / f64::from(reviewed))
    };
    Ok(HumanReviewSummary {
        schema_version: 1,
        benchmark_id: review.benchmark_id.clone(),
        run_id: review.run_id.clone(),
        reviewer: review.reviewer.clone(),
        tasks_total: review.tasks.len(),
        tasks_reviewed: reviewed.len(),
        tasks_pending: review.tasks.len().saturating_sub(reviewed.len()),
        tasks_accepted: accepted,
        acceptance_rate,
        overall_mean_rating: overall_mean_rating(&reviewed),
        mean_ratings: HumanMeanRatings {
            story: mean_rating(&reviewed, HumanRatingDimension::Story),
            pacing: mean_rating(&reviewed, HumanRatingDimension::Pacing),
            visual_finish: mean_rating(&reviewed, HumanRatingDimension::VisualFinish),
            audio_finish: mean_rating(&reviewed, HumanRatingDimension::AudioFinish),
            captions: mean_rating(&reviewed, HumanRatingDimension::Captions),
            delivery_readiness: mean_rating(&reviewed, HumanRatingDimension::DeliveryReadiness),
        },
    })
}

impl HumanRatings {
    fn values(&self) -> [Option<f64>; 6] {
        [
            self.story,
            self.pacing,
            self.visual_finish,
            self.audio_finish,
            self.captions,
            self.delivery_readiness,
        ]
    }

    fn get(&self, dimension: HumanRatingDimension) -> Option<f64> {
        match dimension {
            HumanRatingDimension::Story => self.story,
            HumanRatingDimension::Pacing => self.pacing,
            HumanRatingDimension::VisualFinish => self.visual_finish,
            HumanRatingDimension::AudioFinish => self.audio_finish,
            HumanRatingDimension::Captions => self.captions,
            HumanRatingDimension::DeliveryReadiness => self.delivery_readiness,
        }
    }
}

fn valid_human_rating(rating: f64) -> bool {
    rating.is_finite()
        && (1.0..=5.0).contains(&rating)
        && (rating * 2.0).fract().abs() <= f64::EPSILON
}

fn mean_rating(reviewed: &[&HumanTaskReview], dimension: HumanRatingDimension) -> Option<f64> {
    let sum = reviewed
        .iter()
        .filter(|task| !task.not_applicable.contains(&dimension))
        .filter_map(|task| task.ratings.get(dimension))
        .sum::<f64>();
    let count = reviewed
        .iter()
        .filter(|task| {
            !task.not_applicable.contains(&dimension) && task.ratings.get(dimension).is_some()
        })
        .count();
    let count = u32::try_from(count).ok()?;
    if count == 0 {
        return None;
    }
    Some(sum / f64::from(count))
}

fn overall_mean_rating(reviewed: &[&HumanTaskReview]) -> Option<f64> {
    if reviewed.is_empty() {
        return None;
    }
    let mut sum = 0.0;
    let mut rating_count = 0_usize;
    for task in reviewed {
        for dimension in HumanRatingDimension::ALL {
            if !task.not_applicable.contains(&dimension)
                && let Some(rating) = task.ratings.get(dimension)
            {
                sum += rating;
                rating_count = rating_count.saturating_add(1);
            }
        }
    }
    let rating_count = u32::try_from(rating_count).ok()?;
    if rating_count == 0 {
        return None;
    }
    Some(sum / f64::from(rating_count))
}

fn finish_deliverable(mut result: EvalDeliverableResult) -> EvalDeliverableResult {
    result.machine_passed = result.errors.is_empty()
        && result
            .conformance
            .as_ref()
            .is_some_and(DeliveryConformanceReport::export_ready)
        && result.output_bytes.is_some_and(|bytes| bytes > 0)
        && result
            .output_sha256
            .as_ref()
            .is_some_and(|hash| hash.len() == 64)
        && result.probed_resolution == Some(result.resolution)
        && result
            .probed_duration_frames
            .is_some_and(|duration| duration.0.abs_diff(result.duration_frames.0) <= 1)
        && result.probed_media_kind.is_some()
        && (!result.rendered_transcript_required
            || result
                .rendered_transcript
                .as_ref()
                .is_some_and(|verification| verification.passed))
        && (!result.rendered_caption_alignment_required
            || result
                .rendered_caption_alignment
                .as_ref()
                .is_some_and(|verification| verification.passed))
        && (result.rendered_loudness_contract.is_none()
            || result
                .rendered_loudness
                .as_ref()
                .is_some_and(|verification| verification.passed))
        && (result.rendered_audio_tail_contract.is_none()
            || result
                .rendered_audio_tail
                .as_ref()
                .is_some_and(|verification| verification.passed))
        && result
            .rendered_reframe
            .as_ref()
            .is_none_or(|verification| verification.passed)
        && !result.proof_sample_frames.is_empty()
        && result.proof_path.is_file()
        && result.document_path.is_file();
    result
}

fn deliverable_assertions(result: &EvalDeliverableResult) -> Vec<AssertionResult> {
    let conformance_ready = result
        .conformance
        .as_ref()
        .is_some_and(DeliveryConformanceReport::export_ready);
    let mut assertions = vec![
        assertion_result(
            "delivery conformance",
            conformance_ready,
            format!(
                "profile={}, resolution={}x{}, ready={conformance_ready}",
                result.profile.as_str(),
                result.resolution.0,
                result.resolution.1
            ),
        ),
        assertion_result(
            "rendered proof",
            result.proof_path.is_file() && !result.proof_sample_frames.is_empty(),
            format!(
                "path={}, sampled_frames={:?}",
                result.proof_path.display(),
                result.proof_sample_frames
            ),
        ),
        assertion_result(
            "finished media artifact",
            result.machine_passed,
            format!(
                "path={}, bytes={:?}, sha256={:?}, exported_frames={:?}, probed_resolution={:?}, probed_duration={:?}, probed_kind={:?}, errors={:?}",
                result.output_path.display(),
                result.output_bytes,
                result.output_sha256,
                result.exported_frames,
                result.probed_resolution,
                result.probed_duration_frames,
                result.probed_media_kind,
                result.errors
            ),
        ),
    ];
    if result.rendered_transcript_required {
        assertions.push(rendered_dialogue_assertion(result));
    }
    if result.rendered_caption_alignment_required {
        assertions.push(rendered_caption_alignment_assertion(result));
    }
    if result.rendered_loudness_contract.is_some() {
        assertions.push(rendered_loudness_assertion(result));
    }
    if result.rendered_audio_tail_contract.is_some() {
        assertions.push(rendered_audio_tail_assertion(result));
    }
    if result.rendered_reframe.is_some() {
        assertions.push(rendered_reframe_assertion(result));
    }
    assertions
}

fn rendered_dialogue_assertion(result: &EvalDeliverableResult) -> AssertionResult {
    let Some(verification) = &result.rendered_transcript else {
        return assertion_result(
            "rendered dialogue accuracy",
            false,
            format!(
                "required post-render transcription is unavailable; errors={:?}",
                result.errors
            ),
        );
    };
    assertion_result(
        "rendered dialogue accuracy",
        verification.passed,
        format!(
            "expected={:?}, observed={:?}, edit_distance={}, wer_bp={}, maximum_bp={}, missing={:?}, unexpected={:?}",
            verification.expected_words,
            verification.observed_words,
            verification.edit_distance,
            verification.word_error_rate_basis_points,
            verification.maximum_word_error_rate_basis_points,
            verification.missing_words,
            verification.unexpected_words
        ),
    )
}

fn rendered_caption_alignment_assertion(result: &EvalDeliverableResult) -> AssertionResult {
    let Some(verification) = &result.rendered_caption_alignment else {
        return assertion_result(
            "rendered caption/audio agreement",
            false,
            format!(
                "required rendered caption/audio comparison is unavailable; errors={:?}",
                result.errors
            ),
        );
    };
    assertion_result(
        "rendered caption/audio agreement",
        verification.passed,
        format!(
            "audio={:?}, captions={:?}, edit_distance={}, wer_bp={}, maximum_bp={}, missing_from_captions={:?}, unexpected_in_captions={:?}",
            verification.expected_words,
            verification.observed_words,
            verification.edit_distance,
            verification.word_error_rate_basis_points,
            verification.maximum_word_error_rate_basis_points,
            verification.missing_words,
            verification.unexpected_words
        ),
    )
}

fn rendered_reframe_assertion(result: &EvalDeliverableResult) -> AssertionResult {
    let Some(verification) = &result.rendered_reframe else {
        return assertion_result(
            "rendered reframe automation",
            false,
            "animated reframe verification is unavailable".to_owned(),
        );
    };
    assertion_result(
        "rendered reframe automation",
        verification.passed,
        format!(
            "preserved {} of {} same-aspect animated reframe clips and {} of {} tracked-subject provenance sidecars",
            verification.preserved_animated_clips,
            verification.expected_animated_clips,
            verification.preserved_subject_provenance_clips,
            verification.expected_subject_provenance_clips,
        ),
    )
}

fn rendered_reframe_verification(
    source: &Document,
    delivered: &Document,
) -> Option<RenderedReframeVerification> {
    let (width, height) = delivered.resolution;
    let aspect_basis_points = i64::from(width)
        .saturating_mul(10_000)
        .saturating_add(i64::from(height) / 2)
        / i64::from(height.max(1));
    let expected = source
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .flat_map(|clip| {
            clip.effects.iter().filter_map(move |effect| {
                (effect.name == "reframe"
                    && !effect.keyframes.is_empty()
                    && effect.parameters.get("target_aspect_basis_points")
                        == Some(&ParamValue::Integer(aspect_basis_points)))
                .then_some((clip.id, effect))
            })
        })
        .collect::<Vec<_>>();
    if expected.is_empty() {
        return None;
    }
    let source_provenance = valid_reframe_subject_provenances(source);
    let delivered_provenance = valid_reframe_subject_provenances(delivered);
    let preserved = expected
        .iter()
        .filter(|(clip_id, effect)| {
            delivered
                .tracks
                .iter()
                .flat_map(|track| &track.clips)
                .find(|clip| clip.id == *clip_id)
                .is_some_and(|clip| clip.effects.contains(effect))
        })
        .count();
    let expected_subject_provenance = expected
        .iter()
        .filter_map(|(clip_id, effect)| {
            source_provenance
                .iter()
                .find(|provenance| provenance.clip == *clip_id && provenance.effect == effect.id)
        })
        .collect::<Vec<_>>();
    let preserved_subject_provenance = expected_subject_provenance
        .iter()
        .filter(|provenance| delivered_provenance.contains(provenance))
        .count();
    Some(RenderedReframeVerification {
        expected_animated_clips: expected.len(),
        preserved_animated_clips: preserved,
        expected_subject_provenance_clips: expected_subject_provenance.len(),
        preserved_subject_provenance_clips: preserved_subject_provenance,
        passed: preserved == expected.len()
            && preserved_subject_provenance == expected_subject_provenance.len(),
    })
}

fn rendered_loudness_assertion(result: &EvalDeliverableResult) -> AssertionResult {
    match &result.rendered_loudness {
        Some(verification) => assertion_result(
            "rendered audio loudness",
            verification.passed,
            format!(
                "integrated_lufs_hundredths={:?}, required={}..={}; sample_peak_dbfs_hundredths={:?}, maximum={}",
                verification.measurement.integrated_lufs_hundredths,
                verification.minimum_integrated_lufs_hundredths,
                verification.maximum_integrated_lufs_hundredths,
                verification.measurement.sample_peak_dbfs_hundredths,
                verification.maximum_sample_peak_dbfs_hundredths,
            ),
        ),
        None => assertion_result(
            "rendered audio loudness",
            false,
            format!(
                "required rendered loudness measurement is unavailable; errors={:?}",
                result.errors
            ),
        ),
    }
}

fn rendered_audio_tail_assertion(result: &EvalDeliverableResult) -> AssertionResult {
    match &result.rendered_audio_tail {
        Some(verification) => assertion_result(
            "encoded audio tail",
            verification.passed,
            format!(
                "terminal_frames={}..{}, terminal_sample_peak_dbfs_hundredths={:?}, terminal_maximum={}, trailing_inactive_frames={}, maximum_trailing_inactive_frames={}, activity_window_frames={}, minimum_active_integrated_lufs_hundredths={}, latest_active_window={:?}..{:?}, sample_frames={}",
                verification.tail_start_frame,
                verification.tail_end_frame,
                verification.measurement.sample_peak_dbfs_hundredths,
                verification.maximum_sample_peak_dbfs_hundredths,
                verification.observed_trailing_inactive_frames,
                verification.maximum_trailing_inactive_frames,
                verification.activity_window_frames,
                verification.minimum_active_integrated_lufs_hundredths,
                verification.latest_active_window_start_frame,
                verification.latest_active_window_end_frame,
                verification.measurement.sample_frames,
            ),
        ),
        None => assertion_result(
            "encoded audio tail",
            false,
            format!(
                "required encoded audio-tail measurement is unavailable; errors={:?}",
                result.errors
            ),
        ),
    }
}

fn render_proof_sheet(
    analysis: &dyn Analysis,
    document: &Arc<Document>,
    requested_frames: u8,
    requested_width: u32,
    output: &Path,
) -> Result<Vec<TimeCode>, String> {
    if document.duration <= TimeCode::ZERO {
        return Err("cannot render proof frames for an empty timeline".to_owned());
    }
    let count = usize::from(requested_frames.clamp(1, 16));
    let cell_width = requested_width.clamp(64, 512);
    let frames = uniform_sample_frames(document.duration, count);
    let mut cells = Vec::with_capacity(frames.len());
    for frame in &frames {
        let image = analysis
            .thumbnail_for_document(Arc::clone(document), *frame, cell_width)
            .map_err(|error| format!("proof frame {} failed: {error}", frame.0))?;
        let image = image::RgbaImage::from_raw(image.width, image.height, image.pixels)
            .ok_or_else(|| format!("proof frame {} returned truncated RGBA data", frame.0))?;
        cells.push(image);
    }
    let mut columns = 1_usize;
    while columns.saturating_mul(columns) < cells.len() {
        columns = columns.saturating_add(1);
    }
    let rows = cells.len().div_ceil(columns);
    let gutter = 4_u32;
    let width = cells
        .iter()
        .map(image::GenericImageView::width)
        .max()
        .unwrap_or(cell_width);
    let height = cells
        .iter()
        .map(image::GenericImageView::height)
        .max()
        .unwrap_or(1);
    let sheet_width = u32::try_from(columns)
        .unwrap_or(u32::MAX)
        .saturating_mul(width.saturating_add(gutter))
        .saturating_sub(gutter);
    let sheet_height = u32::try_from(rows)
        .unwrap_or(u32::MAX)
        .saturating_mul(height.saturating_add(gutter))
        .saturating_sub(gutter);
    let mut sheet =
        image::RgbaImage::from_pixel(sheet_width, sheet_height, image::Rgba([18, 20, 24, 255]));
    for (index, cell) in cells.iter().enumerate() {
        let column = u32::try_from(index % columns).unwrap_or(u32::MAX);
        let row = u32::try_from(index / columns).unwrap_or(u32::MAX);
        image::imageops::replace(
            &mut sheet,
            cell,
            i64::from(column.saturating_mul(width.saturating_add(gutter))),
            i64::from(row.saturating_mul(height.saturating_add(gutter))),
        );
    }
    sheet
        .save_with_format(output, image::ImageFormat::Png)
        .map_err(|error| format!("could not write proof sheet {}: {error}", output.display()))?;
    Ok(frames)
}

fn uniform_sample_frames(duration: TimeCode, count: usize) -> Vec<TimeCode> {
    let last = duration.0.saturating_sub(1).max(0);
    if count <= 1 {
        return vec![TimeCode(last / 2)];
    }
    let denominator = i64::try_from(count.saturating_sub(1)).unwrap_or(i64::MAX);
    (0..count)
        .map(|index| {
            let index = i64::try_from(index).unwrap_or(i64::MAX);
            TimeCode(last.saturating_mul(index).saturating_div(denominator))
        })
        .collect()
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    payload.downcast_ref::<String>().map_or_else(
        || {
            payload.downcast_ref::<&str>().map_or_else(
                || "fixture builder panicked".to_owned(),
                |message| (*message).to_owned(),
            )
        },
        Clone::clone,
    )
}

/// Run prompt turns through an `AgentDriver`, including a fake driver in unit tests.
///
/// # Errors
///
/// Returns driver setup/protocol failures or an operation-budget probe failure.
#[allow(clippy::too_many_lines)]
pub fn collect_session<F>(
    driver: &dyn AgentDriver,
    config: SessionConfig,
    prompts: &[&str],
    budgets: &EvalBudgets,
    confirmations: Option<&ConfirmationBroker>,
    mut operation_count: F,
) -> Result<SessionMetrics, EvalError>
where
    F: FnMut() -> Result<usize, EvalError>,
{
    let started = Instant::now();
    let mut session = driver
        .start_session(config)
        .map_err(|error| EvalError::Agent(error.to_string()))?;
    let events = session.events();
    let mut metrics = SessionMetrics {
        cost_usd: Some(0.0),
        cached_input_tokens: Some(0),
        cache_creation_input_tokens: Some(0),
        reasoning_output_tokens: Some(0),
        ..SessionMetrics::default()
    };
    let mut cost_is_complete = true;
    let mut saw_usage = false;
    let trace_events = std::env::var_os("KINEWRIGHT_EVAL_TRACE").is_some();
    for prompt in prompts {
        if metrics.turns >= budgets.max_turns {
            metrics.errors.push(format!(
                "turn budget exceeded before prompt {}",
                metrics.turns.saturating_add(1)
            ));
            metrics.interrupted = true;
            session.interrupt();
            break;
        }
        session
            .send_user_message((*prompt).to_owned())
            .map_err(|error| EvalError::Agent(error.to_string()))?;
        metrics.turns = metrics.turns.saturating_add(1);
        let mut turn_done = false;
        while !turn_done {
            if let Some(broker) = confirmations {
                for request in broker.pending_requests() {
                    if !broker.approve(request.id) {
                        metrics.errors.push(format!(
                            "confirmation {} disappeared before approval",
                            request.id
                        ));
                    }
                }
            }
            if started.elapsed() > budgets.max_wall_time {
                metrics.errors.push(format!(
                    "wall-time budget exceeded ({:.1}s)",
                    budgets.max_wall_time.as_secs_f64()
                ));
                metrics.interrupted = true;
                session.interrupt();
                break;
            }
            let observed_operations = operation_count()?;
            if observed_operations > usize::try_from(budgets.max_operations).unwrap_or(usize::MAX) {
                metrics.errors.push(format!(
                    "operation budget exceeded ({observed_operations} > {})",
                    budgets.max_operations
                ));
                metrics.interrupted = true;
                session.interrupt();
                break;
            }
            match events.recv_timeout(Duration::from_millis(100)) {
                Ok(AgentEvent::Error(error)) => metrics.errors.push(error),
                Ok(AgentEvent::ToolCall { name, arguments }) => {
                    if trace_events {
                        eprintln!("EVAL TRACE tool_call {name}: {}", bounded_trace(&arguments));
                    }
                    let count = metrics.tool_calls.entry(name).or_default();
                    *count = count.saturating_add(1);
                    if metrics.tool_call_count() > budgets.max_tool_calls {
                        metrics.errors.push(format!(
                            "tool-call budget exceeded ({} > {})",
                            metrics.tool_call_count(),
                            budgets.max_tool_calls
                        ));
                        metrics.interrupted = true;
                        session.interrupt();
                        break;
                    }
                }
                Ok(AgentEvent::Cost {
                    input_tokens,
                    cached_input_tokens,
                    cache_creation_input_tokens,
                    output_tokens,
                    reasoning_output_tokens,
                    cost_usd,
                }) => {
                    saw_usage = true;
                    metrics.input_tokens = metrics.input_tokens.saturating_add(input_tokens);
                    accumulate_optional_tokens(
                        &mut metrics.cached_input_tokens,
                        cached_input_tokens,
                    );
                    accumulate_optional_tokens(
                        &mut metrics.cache_creation_input_tokens,
                        cache_creation_input_tokens,
                    );
                    metrics.output_tokens = metrics.output_tokens.saturating_add(output_tokens);
                    accumulate_optional_tokens(
                        &mut metrics.reasoning_output_tokens,
                        reasoning_output_tokens,
                    );
                    match cost_usd {
                        Some(cost) if cost_is_complete => {
                            let total = metrics.cost_usd.get_or_insert(0.0);
                            *total += cost;
                            if budgets.max_cost_usd.is_some_and(|maximum| *total > maximum) {
                                metrics.errors.push(format!(
                                    "cost ceiling exceeded (${total:.4} > ${:.2})",
                                    budgets.max_cost_usd.unwrap_or_default()
                                ));
                                metrics.interrupted = true;
                                session.interrupt();
                                break;
                            }
                        }
                        Some(_) => {}
                        None => {
                            cost_is_complete = false;
                            metrics.cost_usd = None;
                        }
                    }
                }
                Ok(AgentEvent::Done) => turn_done = true,
                Ok(AgentEvent::Text(text)) => {
                    if trace_events {
                        eprintln!("EVAL TRACE agent_text: {}", bounded_trace(&text));
                    }
                }
                Ok(AgentEvent::ToolResult { name, result }) => {
                    if trace_events {
                        eprintln!("EVAL TRACE tool_result {name}: {}", bounded_trace(&result));
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    metrics
                        .errors
                        .push("agent event stream disconnected".to_owned());
                    metrics.interrupted = true;
                    break;
                }
            }
        }
        if metrics.interrupted {
            break;
        }
    }
    if !saw_usage {
        metrics.cached_input_tokens = None;
        metrics.cache_creation_input_tokens = None;
        metrics.reasoning_output_tokens = None;
    }
    metrics.wall_time_ms = duration_millis(started.elapsed());
    session.interrupt();
    Ok(metrics)
}

fn bounded_trace(value: &str) -> String {
    const MAX_CHARS: usize = 2_000;
    let mut characters = value.chars();
    let bounded = characters.by_ref().take(MAX_CHARS).collect::<String>();
    if characters.next().is_some() {
        format!("{bounded}...[truncated]")
    } else {
        bounded
    }
}

fn accumulate_optional_tokens(total: &mut Option<u64>, reported: Option<u64>) {
    match (total.as_mut(), reported) {
        (Some(total), Some(reported)) => *total = total.saturating_add(reported),
        (_, None) => *total = None,
        (None, Some(_)) => {}
    }
}

#[must_use]
pub fn evaluate(definition: &EvalDefinition, outcome: &EvalOutcome) -> EvalResult {
    let mut assertions = definition
        .assertions
        .iter()
        .map(|assertion| evaluate_assertion(assertion, definition, outcome))
        .collect::<Vec<_>>();
    assertions.extend(evaluate_budgets(&definition.budgets, outcome));
    let mut measurements = definition
        .assertions
        .iter()
        .filter_map(|assertion| color_measurement(assertion, outcome))
        .collect::<Vec<_>>();
    measurements.extend(ungated_color_measurements(outcome));
    let passed = assertions.iter().all(|assertion| assertion.passed);
    EvalResult {
        name: definition.name.to_owned(),
        rationale: definition.rationale.to_owned(),
        passed,
        assertions,
        measurements,
        turns: outcome.session.turns,
        tool_calls: outcome.session.tool_calls.clone(),
        input_tokens: outcome.session.input_tokens,
        cached_input_tokens: outcome.session.cached_input_tokens,
        cache_creation_input_tokens: outcome.session.cache_creation_input_tokens,
        output_tokens: outcome.session.output_tokens,
        reasoning_output_tokens: outcome.session.reasoning_output_tokens,
        tool_surface: outcome.session.tool_surface,
        cost_usd: outcome.session.cost_usd,
        wall_time_ms: outcome.session.wall_time_ms,
        operations_applied: u32::try_from(outcome.operations.len()).unwrap_or(u32::MAX),
        deliverable: None,
        execution_error: None,
    }
}

#[allow(clippy::too_many_lines)]
fn evaluate_assertion(
    assertion: &EvalAssertion,
    definition: &EvalDefinition,
    outcome: &EvalOutcome,
) -> AssertionResult {
    match assertion {
        EvalAssertion::TimelineNonEmpty => {
            let count = timeline_clips(&outcome.final_document).count();
            assertion_result(
                "timeline non-empty",
                count > 0,
                format!("observed {count} clips"),
            )
        }
        EvalAssertion::ClipCount { minimum, maximum } => {
            let count = timeline_clips(&outcome.final_document).count();
            assertion_result(
                "clip count",
                (*minimum..=*maximum).contains(&count),
                format!("expected {minimum}..={maximum}, observed {count}"),
            )
        }
        EvalAssertion::MediaClipCount {
            track,
            minimum,
            maximum,
            minimum_duration,
            maximum_duration,
            reject_non_media,
        } => evaluate_media_clip_count(
            *track,
            *minimum,
            *maximum,
            *minimum_duration,
            *maximum_duration,
            *reject_non_media,
            outcome,
        ),
        EvalAssertion::AssetOrder {
            aliases,
            collapse_adjacent,
        } => evaluate_asset_order(aliases, *collapse_adjacent, outcome),
        EvalAssertion::AssetAbsent { alias } => {
            let Some(asset) = outcome.context.asset_aliases.get(alias) else {
                return assertion_result(
                    "asset absent",
                    false,
                    format!("unknown asset alias {alias:?}"),
                );
            };
            let present =
                timeline_media_clips(&outcome.final_document).any(|clip| clip.asset == *asset);
            assertion_result(
                "asset absent",
                !present,
                format!("asset {alias} ({asset}) present={present}"),
            )
        }
        EvalAssertion::Gapless => evaluate_gapless(&outcome.final_document),
        EvalAssertion::MediaGapless => evaluate_media_gapless(&outcome.final_document),
        EvalAssertion::DurationBounds { bounds } => {
            let Some((minimum, maximum)) = outcome.context.duration_bounds.get(bounds) else {
                return assertion_result(
                    "duration bounds",
                    false,
                    format!("unknown duration bounds {bounds:?}"),
                );
            };
            assertion_result(
                "duration bounds",
                (*minimum..=*maximum).contains(&outcome.final_document.duration),
                format!(
                    "expected {}..={} frames, observed {}",
                    minimum.0, maximum.0, outcome.final_document.duration.0
                ),
            )
        }
        EvalAssertion::ExactSourceClips { clips } => evaluate_source_clips(clips, outcome),
        EvalAssertion::ExactTrackClips { track, clips } => {
            evaluate_track_clips(*track, clips, outcome)
        }
        EvalAssertion::ExactProjectDuration { duration } => {
            evaluate_exact_project_duration(*duration, outcome)
        }
        EvalAssertion::ExactTrackMediaCoverage { track, range } => {
            evaluate_exact_track_media_coverage(*track, range, outcome)
        }
        EvalAssertion::RequiredAssetsOnTrack { track, aliases } => {
            evaluate_required_assets_on_track(*track, aliases, outcome)
        }
        EvalAssertion::SourceRangesSeparated {
            track,
            minimum_separation_frames,
        } => evaluate_source_ranges_separated(*track, *minimum_separation_frames, outcome),
        EvalAssertion::SourceRangesChronological {
            track,
            minimum_forward_gap_frames,
        } => evaluate_source_ranges_chronological(*track, *minimum_forward_gap_frames, outcome),
        EvalAssertion::SourceRangesSceneClean {
            track,
            scene_set,
            allowed_baked_sequence_starts,
        } => evaluate_source_ranges_scene_clean(
            *track,
            scene_set,
            allowed_baked_sequence_starts,
            outcome,
        ),
        EvalAssertion::SourceRangesAvoid {
            track,
            exclusion_set,
        } => evaluate_source_ranges_avoid(*track, exclusion_set, outcome),
        EvalAssertion::ShotCadenceVariation {
            track,
            minimum_duration_buckets,
            duration_bucket_frames,
            maximum_similar_run,
            similar_tolerance_frames,
        } => evaluate_shot_cadence_variation(
            *track,
            *minimum_duration_buckets,
            *duration_bucket_frames,
            *maximum_similar_run,
            *similar_tolerance_frames,
            outcome,
        ),
        EvalAssertion::NoAlternatingShotPattern {
            track,
            maximum_repeated_pairs,
            tolerance_frames,
        } => evaluate_no_alternating_shot_pattern(
            *track,
            *maximum_repeated_pairs,
            *tolerance_frames,
            outcome,
        ),
        EvalAssertion::BeatAlignedCuts {
            track,
            beat_set,
            tolerance_frames,
        } => evaluate_beat_aligned_cuts(*track, beat_set, *tolerance_frames, outcome),
        EvalAssertion::CutsAlignedToBeatSetAtLeast {
            track,
            beat_set,
            tolerance_frames,
            minimum_aligned_cuts,
            minimum_aligned_basis_points,
        } => evaluate_cuts_aligned_to_beat_set_at_least(
            *track,
            beat_set,
            *tolerance_frames,
            *minimum_aligned_cuts,
            *minimum_aligned_basis_points,
            outcome,
        ),
        EvalAssertion::MusicFit {
            track,
            asset_alias,
            source_beat_set,
            timeline_start,
            timeline_end,
            tolerance_source_frames,
        } => evaluate_music_fit(
            *track,
            asset_alias,
            source_beat_set,
            *timeline_start,
            *timeline_end,
            *tolerance_source_frames,
            outcome,
        ),
        EvalAssertion::MusicSourceEnd {
            track,
            asset_alias,
            expected_source_end,
            tolerance_source_frames,
        } => evaluate_music_source_end(
            *track,
            asset_alias,
            *expected_source_end,
            *tolerance_source_frames,
            outcome,
        ),
        EvalAssertion::AssetUseMinimum {
            track,
            asset_alias,
            minimum_clip_count,
            minimum_project_frames,
        } => evaluate_asset_use_minimum(
            *track,
            asset_alias,
            *minimum_clip_count,
            *minimum_project_frames,
            outcome,
        ),
        EvalAssertion::AssetTemporalSpread {
            track,
            asset_alias,
            latest_early_start,
            earliest_late_start,
        } => evaluate_asset_temporal_spread(
            *track,
            asset_alias,
            *latest_early_start,
            *earliest_late_start,
            outcome,
        ),
        EvalAssertion::ClipSourceWithin {
            track,
            timeline_start,
            asset_alias,
            source_window,
        } => evaluate_clip_source_within(
            *track,
            *timeline_start,
            asset_alias,
            source_window,
            outcome,
        ),
        EvalAssertion::EdgeShotHolds {
            track,
            minimum_opening_shot_frames,
            minimum_closing_shot_frames,
        } => evaluate_edge_shot_holds(
            *track,
            *minimum_opening_shot_frames,
            *minimum_closing_shot_frames,
            outcome,
        ),
        EvalAssertion::WordsRetained { word_set } => evaluate_word_set(word_set, outcome, true),
        EvalAssertion::WordsAbsent { word_set } => evaluate_word_set(word_set, outcome, false),
        EvalAssertion::CaptionWordsExact { word_set } => evaluate_caption_words(word_set, outcome),
        EvalAssertion::CaptionSentencesCoherent => evaluate_caption_sentences(outcome),
        EvalAssertion::CaptionPresentation {
            allowed_positions,
            color_token,
            background_scrim,
        } => evaluate_caption_presentation(
            allowed_positions,
            *color_token,
            *background_scrim,
            outcome,
        ),
        EvalAssertion::NoSilenceAtLeast { source_frames } => {
            let remaining = cuttable_timeline_silences(
                &outcome.final_document,
                &outcome.remaining_silences,
                &outcome.context.transcripts,
                *source_frames,
            )
            .len();
            assertion_result(
                "long silence absent",
                remaining == 0,
                format!(
                    "observed {remaining} transcript-safe cuttable silence spans at least {} source frames",
                    source_frames.0
                ),
            )
        }
        EvalAssertion::DialoguePauseBounds {
            minimum_project_frames,
            maximum_project_frames,
            capitalization_boundary_minimum_frames,
        } => evaluate_dialogue_pause_bounds(
            &outcome.final_timeline_words,
            &outcome.remaining_silences,
            *minimum_project_frames,
            *maximum_project_frames,
            *capitalization_boundary_minimum_frames,
        ),
        EvalAssertion::SceneChangesAreCuts { scene_set } => evaluate_scene_cuts(scene_set, outcome),
        EvalAssertion::RequiredToolUsage { all_of, any_of } => {
            let called = outcome
                .session
                .tool_calls
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            let missing = all_of
                .iter()
                .filter(|tool| !called.contains(tool.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            let any_match =
                any_of.is_empty() || any_of.iter().any(|tool| called.contains(tool.as_str()));
            assertion_result(
                "required tool usage",
                missing.is_empty() && any_match,
                format!(
                    "called={called:?}, missing_all={missing:?}, any_of={any_of:?} matched={any_match}"
                ),
            )
        }
        EvalAssertion::EffectOnAsset {
            asset_alias,
            effect_name,
            integer_parameter,
        } => evaluate_effect(
            asset_alias,
            effect_name,
            integer_parameter.as_ref(),
            outcome,
        ),
        EvalAssertion::TransitionOnAsset {
            asset_alias,
            transition_name,
        } => evaluate_transition(asset_alias, transition_name, outcome),
        EvalAssertion::NoVisualTransitionsEffectsOrRetiming { track } => {
            evaluate_no_visual_transitions_effects_or_retiming(*track, outcome)
        }
        EvalAssertion::TitleCard {
            track,
            timeline_start,
            duration,
            text,
            font_size_token,
            color_token,
            position,
            background_scrim,
            fade_in_frames,
            fade_out_frames,
        } => evaluate_title_card(
            *track,
            *timeline_start,
            *duration,
            text,
            *font_size_token,
            *color_token,
            *position,
            *background_scrim,
            *fade_in_frames,
            *fade_out_frames,
            outcome,
        ),
        EvalAssertion::SourcePhaseArc {
            track,
            opening_alias,
            pivot_alias,
            pivot_window,
            return_window,
            closing_alias,
            minimum_opening_hold,
            minimum_closing_hold,
        } => evaluate_source_phase_arc(
            *track,
            opening_alias,
            pivot_alias,
            pivot_window,
            return_window,
            closing_alias,
            *minimum_opening_hold,
            *minimum_closing_hold,
            outcome,
        ),
        EvalAssertion::StyledCaptions {
            minimum_cues,
            motion,
        } => evaluate_styled_captions(*minimum_cues, *motion, outcome),
        EvalAssertion::CaptionSafeArea { profile } => evaluate_caption_safe_area(*profile, outcome),
        EvalAssertion::AudioPresent => evaluate_audio_present(&outcome.final_document),
        EvalAssertion::ProgramAudioContinuous { track, asset_alias } => {
            evaluate_program_audio_continuous(*track, asset_alias, outcome)
        }
        EvalAssertion::SingleAudioMediaClip { track, asset_alias } => {
            evaluate_single_audio_media_clip(*track, asset_alias, outcome)
        }
        EvalAssertion::ReframeStability {
            track,
            minimum_keyframes_per_axis,
            min_x_percent,
            max_x_percent,
            min_y_percent,
            max_y_percent,
            maximum_step_percent,
        } => evaluate_reframe_stability(
            *track,
            *minimum_keyframes_per_axis,
            *min_x_percent..=*max_x_percent,
            *min_y_percent..=*max_y_percent,
            *maximum_step_percent,
            outcome,
        ),
        EvalAssertion::QaExportReady => {
            let report = qa_document(&outcome.final_document);
            assertion_result(
                "technical QA",
                report.export_ready(),
                format!(
                    "errors={}, warnings={}, info={}",
                    report.count(kinewright_core::QaSeverity::Error),
                    report.count(kinewright_core::QaSeverity::Warning),
                    report.count(kinewright_core::QaSeverity::Info)
                ),
            )
        }
        EvalAssertion::UndoIntegrity => assertion_result(
            "undo integrity",
            outcome.undo_steps_to_original.is_some(),
            outcome.undo_steps_to_original.map_or_else(
                || {
                    format!(
                        "original was not restored within {} undos",
                        definition.budgets.max_undos
                    )
                },
                |steps| format!("original restored after {steps} undos"),
            ),
        ),
        EvalAssertion::ColorQcTechnicalPass { .. }
        | EvalAssertion::DeliveryVerificationWithinBudgets { .. }
        | EvalAssertion::NeutralPatchSpreadAtMost { .. }
        | EvalAssertion::ReferenceClipUntouched { .. }
        | EvalAssertion::SkinHueWithinBand { .. }
        | EvalAssertion::MatteContainmentExact { .. }
        | EvalAssertion::TrackKeyframesMatchExpected { .. }
        | EvalAssertion::LookBypassMatchesAbsent { .. } => {
            color_assertion_outcome(assertion, outcome).result
        }
    }
}

/// The boolean verdict and the number behind it, produced together so the
/// two can never disagree.
struct ColorAssertionOutcome {
    result: AssertionResult,
    measurement: EvalMeasurement,
}

/// The eight colour variants, written as an **exhaustive** match rather than
/// a `matches!`.
///
/// A ninth colour variant added later must be classified here or the build
/// fails; under a `matches!` it would compile, emit no `EvalMeasurement`, and
/// fall through `color_assertion_outcome`'s generic arm at run time.
const fn is_color_assertion(assertion: &EvalAssertion) -> bool {
    match assertion {
        EvalAssertion::ColorQcTechnicalPass { .. }
        | EvalAssertion::DeliveryVerificationWithinBudgets { .. }
        | EvalAssertion::NeutralPatchSpreadAtMost { .. }
        | EvalAssertion::ReferenceClipUntouched { .. }
        | EvalAssertion::SkinHueWithinBand { .. }
        | EvalAssertion::MatteContainmentExact { .. }
        | EvalAssertion::TrackKeyframesMatchExpected { .. }
        | EvalAssertion::LookBypassMatchesAbsent { .. } => true,
        EvalAssertion::TimelineNonEmpty
        | EvalAssertion::ClipCount { .. }
        | EvalAssertion::MediaClipCount { .. }
        | EvalAssertion::AssetOrder { .. }
        | EvalAssertion::AssetAbsent { .. }
        | EvalAssertion::Gapless
        | EvalAssertion::MediaGapless
        | EvalAssertion::DurationBounds { .. }
        | EvalAssertion::ExactSourceClips { .. }
        | EvalAssertion::ExactTrackClips { .. }
        | EvalAssertion::ExactProjectDuration { .. }
        | EvalAssertion::ExactTrackMediaCoverage { .. }
        | EvalAssertion::RequiredAssetsOnTrack { .. }
        | EvalAssertion::SourceRangesSeparated { .. }
        | EvalAssertion::SourceRangesChronological { .. }
        | EvalAssertion::SourceRangesSceneClean { .. }
        | EvalAssertion::SourceRangesAvoid { .. }
        | EvalAssertion::ShotCadenceVariation { .. }
        | EvalAssertion::NoAlternatingShotPattern { .. }
        | EvalAssertion::BeatAlignedCuts { .. }
        | EvalAssertion::CutsAlignedToBeatSetAtLeast { .. }
        | EvalAssertion::MusicFit { .. }
        | EvalAssertion::MusicSourceEnd { .. }
        | EvalAssertion::AssetUseMinimum { .. }
        | EvalAssertion::AssetTemporalSpread { .. }
        | EvalAssertion::ClipSourceWithin { .. }
        | EvalAssertion::EdgeShotHolds { .. }
        | EvalAssertion::WordsRetained { .. }
        | EvalAssertion::WordsAbsent { .. }
        | EvalAssertion::CaptionWordsExact { .. }
        | EvalAssertion::CaptionSentencesCoherent
        | EvalAssertion::CaptionPresentation { .. }
        | EvalAssertion::NoSilenceAtLeast { .. }
        | EvalAssertion::DialoguePauseBounds { .. }
        | EvalAssertion::SceneChangesAreCuts { .. }
        | EvalAssertion::RequiredToolUsage { .. }
        | EvalAssertion::EffectOnAsset { .. }
        | EvalAssertion::TransitionOnAsset { .. }
        | EvalAssertion::NoVisualTransitionsEffectsOrRetiming { .. }
        | EvalAssertion::TitleCard { .. }
        | EvalAssertion::SourcePhaseArc { .. }
        | EvalAssertion::StyledCaptions { .. }
        | EvalAssertion::CaptionSafeArea { .. }
        | EvalAssertion::AudioPresent
        | EvalAssertion::ProgramAudioContinuous { .. }
        | EvalAssertion::SingleAudioMediaClip { .. }
        | EvalAssertion::ReframeStability { .. }
        | EvalAssertion::QaExportReady
        | EvalAssertion::UndoIntegrity => false,
    }
}

/// Every colour assertion emits an `EvalMeasurement` beside its
/// `AssertionResult`, so the observed number reaches `results.jsonl` as data
/// rather than as prose inside a detail string.
fn color_measurement(assertion: &EvalAssertion, outcome: &EvalOutcome) -> Option<EvalMeasurement> {
    is_color_assertion(assertion).then(|| color_assertion_outcome(assertion, outcome).measurement)
}

/// The two colour quantities §4.1 records as **measured, not budgeted**.
///
/// No `EvalAssertion` reads either of them, so without this they would be
/// rendered at full resolution and then dropped when `run_eval_with_artifacts`
/// returns. Emitting them as `budget: 0, passed: true` rows puts them in
/// `results.jsonl` beside the gated numbers, which is where §4.1's
/// measured-not-budgeted column has to come from.
fn ungated_color_measurements(outcome: &EvalOutcome) -> Vec<EvalMeasurement> {
    let Some(color) = outcome.color.as_ref() else {
        return Vec::new();
    };
    let mut measurements = Vec::new();
    if let Some(delta) = color.chart_luma_mean_delta_millionths {
        measurements.push(EvalMeasurement {
            name: CHART_LUMA_MEASUREMENT_NAME.to_owned(),
            observed: delta,
            budget: 0,
            unit: "millionths".to_owned(),
            passed: true,
        });
    }
    if let Some(count) = color.gamut_pixel_count {
        measurements.push(EvalMeasurement {
            name: GAMUT_MEASUREMENT_NAME.to_owned(),
            observed: i64::try_from(count).unwrap_or(i64::MAX),
            budget: 0,
            unit: "pixels".to_owned(),
            passed: true,
        });
    }
    measurements
}

/// The name the chart luma delta reaches `results.jsonl` under.
pub const CHART_LUMA_MEASUREMENT_NAME: &str = "chart luma mean delta";

/// The name the deep-shadow gamut population reaches `results.jsonl` under.
pub const GAMUT_MEASUREMENT_NAME: &str = "deep shadow out-of-gamut pixels";

fn color_outcome(
    name: &str,
    passed: bool,
    detail: String,
    observed: i64,
    budget: i64,
    unit: &str,
) -> ColorAssertionOutcome {
    ColorAssertionOutcome {
        result: assertion_result(name, passed, detail),
        measurement: EvalMeasurement {
            name: name.to_owned(),
            observed,
            budget,
            unit: unit.to_owned(),
            passed,
        },
    }
}

fn color_not_measured(
    name: &str,
    unit: &str,
    evidence: Option<&ColorEvalEvidence>,
    quantity: ColorEvidenceQuantity,
) -> String {
    evidence.map_or_else(
        || format!("{name} has no colour evidence block ({unit} not measured)"),
        |evidence| {
            evidence.unmeasurable_reason(quantity).map_or_else(
                || format!("{name} was not measured for this task ({unit})"),
                |reason| format!("{name} could not be measured: {reason}"),
            )
        },
    )
}

/// A quantity whose inputs failed is **not** a measurement.
///
/// A partially measured quantity — three of twelve patch rectangles that
/// would not resolve, a proof that does not claim full resolution — fails the
/// assertion that gates it, with the recorded reason, rather than reporting
/// the worst of whatever did measure. Returns `None` when nothing was
/// recorded against the quantity, which is the ordinary path.
fn color_unmeasurable(
    name: &str,
    evidence: Option<&ColorEvalEvidence>,
    quantity: ColorEvidenceQuantity,
    not_measured: i64,
    budget: i64,
    unit: &str,
) -> Option<ColorAssertionOutcome> {
    let reason = evidence?.unmeasurable_reason(quantity)?;
    Some(color_outcome(
        name,
        false,
        format!("{name} could not be measured: {reason}"),
        not_measured,
        budget,
        unit,
    ))
}

#[allow(clippy::too_many_lines)]
fn color_assertion_outcome(
    assertion: &EvalAssertion,
    outcome: &EvalOutcome,
) -> ColorAssertionOutcome {
    let evidence = outcome.color.as_ref();
    match assertion {
        EvalAssertion::ColorQcTechnicalPass {
            clip_id,
            frame,
            checks,
        } => {
            let name = "colour qc technical pass";
            if let Some(refused) =
                color_unmeasurable(name, evidence, ColorEvidenceQuantity::Qc, 0, 1, "boolean")
            {
                return refused;
            }
            evidence.and_then(|evidence| evidence.qc.as_ref()).map_or_else(
                || color_outcome(name, false, color_not_measured(name, "boolean", evidence, ColorEvidenceQuantity::Qc), 0, 1, "boolean"),
                |report| {
                    let codes = report
                        .exceptions
                        .iter()
                        .map(|exception| exception.code.clone())
                        .collect::<Vec<_>>();
                    color_outcome(
                        name,
                        report.technical_pass,
                        format!(
                            "clip {clip_id} frame {frame} checks {checks:?}: technical_pass={}, exceptions={codes:?}",
                            report.technical_pass
                        ),
                        i64::from(report.technical_pass),
                        1,
                        "boolean",
                    )
                },
            )
        }
        EvalAssertion::DeliveryVerificationWithinBudgets { depth } => {
            let name = "delivery verification within budgets";
            if let Some(refused) = color_unmeasurable(
                name,
                evidence,
                ColorEvidenceQuantity::DeliveryVerification,
                0,
                1,
                "boolean",
            ) {
                return refused;
            }
            evidence
                .and_then(|evidence| evidence.verification.as_ref())
                .map_or_else(
                    || {
                        color_outcome(
                            name,
                            false,
                            color_not_measured(
                                name,
                                "boolean",
                                evidence,
                                ColorEvidenceQuantity::DeliveryVerification,
                            ),
                            0,
                            1,
                            "boolean",
                        )
                    },
                    |verification| {
                        let depth_matches = verification.delivery_bit_depth == *depth;
                        let passed = depth_matches
                            && verification.comparison.within_budgets
                            && verification.technical_pass;
                        color_outcome(
                            name,
                            passed,
                            format!(
                                "depth={} (requested {}), within_budgets={}, technical_pass={}, conforming={}, decoded_pixel_format={}",
                                verification.delivery_bit_depth.as_str(),
                                depth.as_str(),
                                verification.comparison.within_budgets,
                                verification.technical_pass,
                                verification.tags.conforming,
                                verification.decoded_pixel_format,
                            ),
                            i64::from(passed),
                            1,
                            "boolean",
                        )
                    },
                )
        }
        EvalAssertion::NeutralPatchSpreadAtMost {
            patch_rois,
            maximum_code,
        } => {
            let name = "neutral patch spread";
            if let Some(refused) = color_unmeasurable(
                name,
                evidence,
                ColorEvidenceQuantity::NeutralSpread,
                -1,
                *maximum_code,
                "monitoring_code",
            ) {
                return refused;
            }
            evidence
                .and_then(|evidence| evidence.neutral_spread_max_code)
                .map_or_else(
                    || {
                        color_outcome(
                            name,
                            false,
                            color_not_measured(
                                name,
                                "monitoring_code",
                                evidence,
                                ColorEvidenceQuantity::NeutralSpread,
                            ),
                            -1,
                            *maximum_code,
                            "monitoring_code",
                        )
                    },
                    |observed| {
                        color_outcome(
                            name,
                            observed <= *maximum_code,
                            format!(
                                "worst spread {observed} over {} patch(es), budget {maximum_code}",
                                patch_rois.len()
                            ),
                            observed,
                            *maximum_code,
                            "monitoring_code",
                        )
                    },
                )
        }
        EvalAssertion::ReferenceClipUntouched { clip_id } => {
            evaluate_reference_clip_untouched(*clip_id, outcome)
        }
        EvalAssertion::SkinHueWithinBand {
            roi,
            minimum_in_band_basis_points,
        } => {
            let name = "skin hue within band";
            let budget = i64::from(*minimum_in_band_basis_points);
            if let Some(refused) = color_unmeasurable(
                name,
                evidence,
                ColorEvidenceQuantity::Skin,
                -1,
                budget,
                "basis_points",
            ) {
                return refused;
            }
            evidence.and_then(|evidence| evidence.skin.as_ref()).map_or_else(
                || color_outcome(name, false, color_not_measured(name, "basis_points", evidence, ColorEvidenceQuantity::Skin), -1, budget, "basis_points"),
                |skin| {
                    let observed = i64::from(skin.in_band_basis_points);
                    // A `None` mean hue is a failure, not a pass by default:
                    // a region with no chromatic pixel has no hue to be in or
                    // out of the band.
                    let passed = skin.mean_hue_centidegrees.is_some() && observed >= budget;
                    color_outcome(
                        name,
                        passed,
                        format!(
                            "roi {roi:?}: in_band={observed} bp of {} considered ({} achromatic excluded), mean_hue={:?}, minimum {budget} bp",
                            skin.considered_pixel_count,
                            skin.excluded_achromatic_pixel_count,
                            skin.mean_hue_centidegrees,
                        ),
                        observed,
                        budget,
                        "basis_points",
                    )
                },
            )
        }
        EvalAssertion::MatteContainmentExact {
            roi,
            expected_covered_pixel_count,
            expected_full_pixel_count,
            expected_partial_pixel_count,
        } => {
            let name = "matte containment";
            let budget = i64::try_from(*expected_covered_pixel_count).unwrap_or(i64::MAX);
            if let Some(refused) = color_unmeasurable(
                name,
                evidence,
                ColorEvidenceQuantity::Matte,
                -1,
                budget,
                "pixels",
            ) {
                return refused;
            }
            evidence.and_then(|evidence| evidence.matte.as_ref()).map_or_else(
                || color_outcome(name, false, color_not_measured(name, "pixels", evidence, ColorEvidenceQuantity::Matte), -1, budget, "pixels"),
                |matte| {
                    let passed = matte.covered_pixel_count == *expected_covered_pixel_count
                        && matte.full_pixel_count == *expected_full_pixel_count
                        && matte.partial_pixel_count == *expected_partial_pixel_count;
                    color_outcome(
                        name,
                        passed,
                        format!(
                            "inside roi {roi:?} only: covered {}/{expected_covered_pixel_count}, full {}/{expected_full_pixel_count}, partial {}/{expected_partial_pixel_count} of {} pixel(s)",
                            matte.covered_pixel_count,
                            matte.full_pixel_count,
                            matte.partial_pixel_count,
                            matte.total_pixel_count,
                        ),
                        i64::try_from(matte.covered_pixel_count).unwrap_or(i64::MAX),
                        budget,
                        "pixels",
                    )
                },
            )
        }
        EvalAssertion::TrackKeyframesMatchExpected {
            parameter,
            expected_local_frames,
            absent_local_frames,
        } => evaluate_track_keyframes(
            parameter,
            expected_local_frames,
            absent_local_frames,
            outcome,
        ),
        EvalAssertion::LookBypassMatchesAbsent {
            clip_id,
            effect_id,
            frame,
        } => {
            let name = "look bypass matches absent";
            if let Some(refused) = color_unmeasurable(
                name,
                evidence,
                ColorEvidenceQuantity::LookBypass,
                0,
                1,
                "boolean",
            ) {
                return refused;
            }
            evidence
                .and_then(|evidence| evidence.look_bypass_matches_absent)
                .map_or_else(
                    || {
                        color_outcome(
                            name,
                            false,
                            color_not_measured(
                                name,
                                "boolean",
                                evidence,
                                ColorEvidenceQuantity::LookBypass,
                            ),
                            0,
                            1,
                            "boolean",
                        )
                    },
                    |matches| {
                        color_outcome(
                            name,
                            matches,
                            format!(
                                "clip {clip_id} effect {effect_id} at frame {frame}: bypass_matches_absent={matches}"
                            ),
                            i64::from(matches),
                            1,
                            "boolean",
                        )
                    },
                )
        }
        _ => color_outcome(
            "colour assertion",
            false,
            "assertion is not a colour assertion".to_owned(),
            0,
            1,
            "boolean",
        ),
    }
}

/// The reference clip must be byte-identical to its pre-session form.
///
/// The **effect count and the serialized clip are the evidence**: a planner's
/// own `reference_retained` field is a hardcoded literal and asserts nothing
/// about what happened, so nothing here reads one.
fn evaluate_reference_clip_untouched(clip_id: u64, outcome: &EvalOutcome) -> ColorAssertionOutcome {
    let name = "reference clip untouched";
    let clip = ClipId(clip_id);
    let before = outcome.original_document.clip(clip);
    let after = outcome.final_document.clip(clip);
    match (before, after) {
        (Some(before), Some(after)) => {
            let observed = i64::try_from(after.effects.len()).unwrap_or(i64::MAX);
            let identical = serde_json::to_string(before).ok() == serde_json::to_string(after).ok();
            let passed = identical && after.effects.is_empty();
            color_outcome(
                name,
                passed,
                format!(
                    "clip {clip_id}: {observed} effect(s) after the session, serialized clip {}",
                    if identical { "unchanged" } else { "CHANGED" }
                ),
                observed,
                0,
                "effects",
            )
        }
        _ => color_outcome(
            name,
            false,
            format!("clip {clip_id} is missing from the original or the final document"),
            -1,
            0,
            "effects",
        ),
    }
}

/// Read the committed keyframes, which are the durable evidence of what the
/// tracker did. There is no per-call tool log in `SessionMetrics` and CC7
/// adds none, so an assertion about "which samples survived" is an assertion
/// about which keyframes were written.
fn evaluate_track_keyframes(
    parameter: &str,
    expected_local_frames: &[i64],
    absent_local_frames: &[i64],
    outcome: &EvalOutcome,
) -> ColorAssertionOutcome {
    let name = "track keyframes match expected";
    let mut observed_frames = BTreeSet::new();
    for track in &outcome.final_document.tracks {
        for clip in &track.clips {
            for effect in &clip.effects {
                if let Some(curve) = effect.keyframes.get(parameter) {
                    observed_frames.extend(curve.keyframes.iter().map(|keyframe| keyframe.at.0));
                }
            }
        }
    }
    let missing = expected_local_frames
        .iter()
        .filter(|frame| !observed_frames.contains(frame))
        .copied()
        .collect::<Vec<_>>();
    let unexpected = absent_local_frames
        .iter()
        .filter(|frame| observed_frames.contains(frame))
        .copied()
        .collect::<Vec<_>>();
    let present = i64::try_from(expected_local_frames.len().saturating_sub(missing.len()))
        .unwrap_or(i64::MAX);
    let budget = i64::try_from(expected_local_frames.len()).unwrap_or(i64::MAX);
    color_outcome(
        name,
        missing.is_empty() && unexpected.is_empty(),
        format!(
            "{parameter}: observed {:?}, missing {missing:?}, unexpectedly present {unexpected:?}",
            observed_frames.iter().copied().collect::<Vec<_>>()
        ),
        present,
        budget,
        "keyframes",
    )
}

fn evaluate_budgets(budgets: &EvalBudgets, outcome: &EvalOutcome) -> Vec<AssertionResult> {
    let turns = outcome.session.turns;
    let tool_calls = outcome.session.tool_call_count();
    let operations = u32::try_from(outcome.operations.len()).unwrap_or(u32::MAX);
    let tokens = outcome.session.total_tokens();
    let mut results = vec![
        assertion_result(
            "agent completed without errors",
            outcome.session.errors.is_empty(),
            if outcome.session.errors.is_empty() {
                "no driver errors".to_owned()
            } else {
                outcome.session.errors.join("; ")
            },
        ),
        assertion_result(
            "turn budget",
            turns <= budgets.max_turns,
            format!("{turns} <= {}", budgets.max_turns),
        ),
        assertion_result(
            "tool-call budget",
            tool_calls <= budgets.max_tool_calls,
            format!("{tool_calls} <= {}", budgets.max_tool_calls),
        ),
        assertion_result(
            "operation budget",
            operations <= budgets.max_operations,
            format!("{operations} <= {}", budgets.max_operations),
        ),
        assertion_result(
            "token budget",
            tokens <= budgets.max_tokens,
            format!("{tokens} <= {}", budgets.max_tokens),
        ),
        assertion_result(
            "wall-time budget",
            outcome.session.wall_time_ms <= duration_millis(budgets.max_wall_time),
            format!(
                "{}ms <= {}ms",
                outcome.session.wall_time_ms,
                duration_millis(budgets.max_wall_time)
            ),
        ),
    ];
    let cost = outcome.session.cost_usd;
    let (cost_passed, cost_detail) = match (cost, budgets.max_cost_usd) {
        (Some(value), Some(maximum)) => {
            (value <= maximum, format!("${value:.4} <= ${maximum:.2}"))
        }
        (None, Some(_)) => (false, "harness did not report USD cost".to_owned()),
        (Some(value), None) => (
            true,
            format!("${value:.4} reported; no portable USD ceiling is enforced"),
        ),
        (None, None) => (
            true,
            "subscription harness does not expose attributable USD cost; token and wall-time ceilings remain enforced".to_owned(),
        ),
    };
    results.push(assertion_result("cost ceiling", cost_passed, cost_detail));
    results
}

fn evaluate_asset_order(
    aliases: &[String],
    collapse_adjacent: bool,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let reverse = outcome
        .context
        .asset_aliases
        .iter()
        .map(|(alias, asset)| (*asset, alias.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut observed = timeline_media_clips(&outcome.final_document)
        .map(|clip| {
            reverse.get(&clip.asset).map_or_else(
                || format!("asset-{}", clip.asset.0),
                |alias| (*alias).to_owned(),
            )
        })
        .collect::<Vec<_>>();
    if collapse_adjacent {
        observed.dedup();
    }
    assertion_result(
        "asset order",
        observed == aliases,
        format!("expected {aliases:?}, observed {observed:?}"),
    )
}

fn evaluate_gapless(document: &Document) -> AssertionResult {
    let mut errors = Vec::new();
    for track in &document.tracks {
        if let Some(first) = track.clips.first()
            && first.timeline_start != TimeCode::ZERO
        {
            errors.push(format!(
                "track {} starts at frame {}",
                track.id, first.timeline_start.0
            ));
        }
        for adjacent in track.clips.windows(2) {
            let Some(asset) = document.asset(adjacent[0].asset) else {
                errors.push(format!("missing asset {}", adjacent[0].asset));
                continue;
            };
            match map_source_range_to_project(
                adjacent[0].source_range.clone(),
                asset.fps,
                document.fps,
            )
            .and_then(|duration| {
                adjacent[0]
                    .timeline_start
                    .checked_add(duration)
                    .ok_or(kinewright_core::TimeMappingError::Overflow)
            }) {
                Ok(left_end) if left_end == adjacent[1].timeline_start => {}
                Ok(left_end) => errors.push(format!(
                    "track {} gap/overlap between clips {} and {}: {} then {}",
                    track.id,
                    adjacent[0].id,
                    adjacent[1].id,
                    left_end.0,
                    adjacent[1].timeline_start.0
                )),
                Err(error) => errors.push(error.to_string()),
            }
        }
    }
    assertion_result(
        "timeline gapless",
        errors.is_empty(),
        if errors.is_empty() {
            "all populated tracks start at zero and are contiguous".to_owned()
        } else {
            errors.join("; ")
        },
    )
}

fn evaluate_media_gapless(document: &Document) -> AssertionResult {
    let mut errors = Vec::new();
    for track in &document.tracks {
        let clips = track
            .clips
            .iter()
            .filter(|clip| {
                matches!(
                    &clip.content,
                    ClipContent::Media | ClipContent::Freeze { .. }
                )
            })
            .collect::<Vec<_>>();
        if let Some(first) = clips.first()
            && first.timeline_start != TimeCode::ZERO
        {
            errors.push(format!(
                "track {} media starts at frame {}",
                track.id, first.timeline_start.0
            ));
        }
        for adjacent in clips.windows(2) {
            let Some(asset) = document.asset(adjacent[0].asset) else {
                errors.push(format!("missing asset {}", adjacent[0].asset));
                continue;
            };
            match map_source_range_to_project(
                adjacent[0].source_range.clone(),
                asset.fps,
                document.fps,
            )
            .and_then(|duration| {
                adjacent[0]
                    .timeline_start
                    .checked_add(duration)
                    .ok_or(kinewright_core::TimeMappingError::Overflow)
            }) {
                Ok(left_end) if left_end == adjacent[1].timeline_start => {}
                Ok(left_end) => errors.push(format!(
                    "track {} media gap/overlap between clips {} and {}: {} then {}",
                    track.id,
                    adjacent[0].id,
                    adjacent[1].id,
                    left_end.0,
                    adjacent[1].timeline_start.0
                )),
                Err(error) => errors.push(error.to_string()),
            }
        }
    }
    assertion_result(
        "primary media gapless",
        errors.is_empty(),
        if errors.is_empty() {
            "all populated media tracks start at zero and are contiguous; caption gaps are allowed"
                .to_owned()
        } else {
            errors.join("; ")
        },
    )
}

fn evaluate_source_clips(clips: &[ExpectedSourceClip], outcome: &EvalOutcome) -> AssertionResult {
    let reverse = outcome
        .context
        .asset_aliases
        .iter()
        .map(|(alias, asset)| (*asset, alias.as_str()))
        .collect::<BTreeMap<_, _>>();
    let observed = timeline_media_clips(&outcome.final_document)
        .map(|clip| ExpectedSourceClip {
            asset_alias: reverse.get(&clip.asset).map_or_else(
                || format!("asset-{}", clip.asset.0),
                |alias| (*alias).to_owned(),
            ),
            source_start: clip.source_range.start,
            source_end: clip.source_range.end,
        })
        .collect::<Vec<_>>();
    assertion_result(
        "exact source clips",
        observed == clips,
        format!("expected {clips:?}, observed {observed:?}"),
    )
}

fn evaluate_track_clips(
    track_id: TrackId,
    clips: &[ExpectedTimelineClip],
    outcome: &EvalOutcome,
) -> AssertionResult {
    let reverse = outcome
        .context
        .asset_aliases
        .iter()
        .map(|(alias, asset)| (*asset, alias.as_str()))
        .collect::<BTreeMap<_, _>>();
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return assertion_result(
            "exact track clips",
            false,
            format!("track {track_id} does not exist"),
        );
    };
    let observed = track
        .clips
        .iter()
        .filter(|clip| clip.content.is_media())
        .map(|clip| {
            let timeline_end = outcome
                .final_document
                .asset(clip.asset)
                .and_then(|asset| {
                    map_source_range_to_project(
                        clip.source_range.clone(),
                        asset.fps,
                        outcome.final_document.fps,
                    )
                    .ok()
                })
                .and_then(|duration| clip.timeline_start.checked_add(duration))
                .unwrap_or(TimeCode(i64::MIN));
            ExpectedTimelineClip {
                asset_alias: reverse.get(&clip.asset).map_or_else(
                    || format!("asset-{}", clip.asset.0),
                    |alias| (*alias).to_owned(),
                ),
                timeline_start: clip.timeline_start,
                timeline_end,
                source_start: clip.source_range.start,
                source_end: clip.source_range.end,
            }
        })
        .collect::<Vec<_>>();
    assertion_result(
        "exact track clips",
        observed == clips,
        format!("track={track_id}, expected={clips:?}, observed={observed:?}"),
    )
}

fn project_range_for_media_clip(
    document: &Document,
    clip: &kinewright_core::Clip,
) -> Result<std::ops::Range<TimeCode>, String> {
    let duration = document
        .clip_duration(clip)
        .map_err(|error| format!("clip {} duration mapping failed: {error}", clip.id))?;
    let end = clip
        .timeline_start
        .checked_add(duration)
        .ok_or_else(|| format!("clip {} project end overflowed", clip.id))?;
    if end <= clip.timeline_start {
        return Err(format!(
            "clip {} has non-positive project range {}..{}",
            clip.id, clip.timeline_start.0, end.0
        ));
    }
    Ok(clip.timeline_start..end)
}

fn ordered_media_project_ranges<'a>(
    document: &Document,
    track: &'a kinewright_core::Track,
) -> (
    Vec<(&'a kinewright_core::Clip, std::ops::Range<TimeCode>)>,
    Vec<String>,
) {
    let mut ranges = Vec::new();
    let mut errors = Vec::new();
    for clip in track
        .clips
        .iter()
        .filter(|clip| matches!(clip.content, ClipContent::Media))
    {
        match project_range_for_media_clip(document, clip) {
            Ok(range) => ranges.push((clip, range)),
            Err(error) => errors.push(error),
        }
    }
    ranges.sort_by_key(|(clip, range)| (range.start, clip.id));
    (ranges, errors)
}

fn evaluate_exact_project_duration(expected: TimeCode, outcome: &EvalOutcome) -> AssertionResult {
    if expected.0 < 0 {
        return assertion_result(
            "exact project duration",
            false,
            format!(
                "requested project duration must be non-negative, observed {}",
                expected.0
            ),
        );
    }

    let mut errors = Vec::new();
    let mut maximum_clip_end = TimeCode::ZERO;
    for clip in outcome
        .final_document
        .tracks
        .iter()
        .flat_map(|track| track.clips.iter())
    {
        if clip.timeline_start.0 < 0 {
            errors.push(format!(
                "clip {} starts before project frame zero at {}",
                clip.id, clip.timeline_start.0
            ));
        }
        match outcome.final_document.clip_duration(clip) {
            Ok(duration) => match clip.timeline_start.checked_add(duration) {
                Some(end) => maximum_clip_end = maximum_clip_end.max(end),
                None => errors.push(format!("clip {} project end overflowed", clip.id)),
            },
            Err(error) => errors.push(format!("clip {} duration mapping failed: {error}", clip.id)),
        }
    }

    let declared = outcome.final_document.duration;
    let passed = errors.is_empty() && declared == expected && maximum_clip_end == expected;
    assertion_result(
        "exact project duration",
        passed,
        if errors.is_empty() {
            format!(
                "expected {expected} frames, declared document duration={}, mapped clip end={}, observed clip end must equal the contract",
                declared.0, maximum_clip_end.0
            )
        } else {
            format!(
                "expected {expected} frames, declared document duration={}, mapped clip end={}; {}",
                declared.0,
                maximum_clip_end.0,
                errors.join("; ")
            )
        },
    )
}

fn evaluate_exact_track_media_coverage(
    track_id: TrackId,
    requested: &std::ops::Range<TimeCode>,
    outcome: &EvalOutcome,
) -> AssertionResult {
    if requested.start.0 < 0 || requested.start >= requested.end {
        return assertion_result(
            "exact track media coverage",
            false,
            format!(
                "requested project range must be non-empty and non-negative, observed {}..{}",
                requested.start.0, requested.end.0
            ),
        );
    }
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return assertion_result(
            "exact track media coverage",
            false,
            format!("track {track_id} does not exist"),
        );
    };

    let (ranges, mapping_errors) = ordered_media_project_ranges(&outcome.final_document, track);
    let mut errors = mapping_errors;
    if ranges.is_empty() {
        errors.push("track has no real media clips".to_owned());
    } else {
        if ranges[0].1.start != requested.start {
            errors.push(format!(
                "coverage starts at {} but requested range starts at {}",
                ranges[0].1.start.0, requested.start.0
            ));
        }
        for (clip, range) in &ranges {
            if range.start < requested.start || range.end > requested.end {
                errors.push(format!(
                    "clip {} range {}..{} falls outside requested {}..{}",
                    clip.id, range.start.0, range.end.0, requested.start.0, requested.end.0
                ));
            }
        }
        for ((left_clip, left), (right_clip, right)) in ranges.iter().zip(ranges.iter().skip(1)) {
            if left.end != right.start {
                let relation = if left.end < right.start {
                    "gap"
                } else {
                    "overlap"
                };
                errors.push(format!(
                    "{relation} between clips {} ({}..{}) and {} ({}..{})",
                    left_clip.id,
                    left.start.0,
                    left.end.0,
                    right_clip.id,
                    right.start.0,
                    right.end.0
                ));
            }
        }
        if let Some((clip, range)) = ranges.last()
            && range.end != requested.end
        {
            errors.push(format!(
                "coverage ends at {} on clip {} but requested range ends at {}",
                range.end.0, clip.id, requested.end.0
            ));
        }
    }
    assertion_result(
        "exact track media coverage",
        errors.is_empty(),
        if errors.is_empty() {
            format!(
                "track {track_id} has contiguous real-media coverage exactly over {}..{} (half-open)",
                requested.start.0, requested.end.0
            )
        } else {
            errors.join("; ")
        },
    )
}

fn evaluate_required_assets_on_track(
    track_id: TrackId,
    aliases: &[String],
    outcome: &EvalOutcome,
) -> AssertionResult {
    let unknown = aliases
        .iter()
        .filter(|alias| !outcome.context.asset_aliases.contains_key(*alias))
        .cloned()
        .collect::<Vec<_>>();
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return assertion_result(
            "required assets on track",
            false,
            format!("track {track_id} does not exist; unknown aliases={unknown:?}"),
        );
    };
    let present = track
        .clips
        .iter()
        .filter(|clip| clip.content.is_media())
        .map(|clip| clip.asset)
        .collect::<BTreeSet<_>>();
    let missing = aliases
        .iter()
        .filter(|alias| {
            outcome
                .context
                .asset_aliases
                .get(*alias)
                .is_some_and(|asset| !present.contains(asset))
        })
        .cloned()
        .collect::<Vec<_>>();
    assertion_result(
        "required assets on track",
        unknown.is_empty() && missing.is_empty(),
        if unknown.is_empty() && missing.is_empty() {
            format!("track {track_id} contains every required media asset alias {aliases:?}")
        } else {
            format!("track {track_id} missing aliases={missing:?}; unknown aliases={unknown:?}")
        },
    )
}

fn evaluate_source_ranges_separated(
    track_id: TrackId,
    minimum_separation_frames: TimeCode,
    outcome: &EvalOutcome,
) -> AssertionResult {
    if minimum_separation_frames.0 < 0 {
        return assertion_result(
            "source ranges separated",
            false,
            format!(
                "minimum source-frame separation must be non-negative, observed {}",
                minimum_separation_frames.0
            ),
        );
    }
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return assertion_result(
            "source ranges separated",
            false,
            format!("track {track_id} does not exist"),
        );
    };
    let mut ranges_by_asset =
        BTreeMap::<AssetId, Vec<(kinewright_core::ClipId, TimeCode, TimeCode)>>::new();
    for clip in track.clips.iter().filter(|clip| clip.content.is_media()) {
        ranges_by_asset.entry(clip.asset).or_default().push((
            clip.id,
            clip.source_range.start,
            clip.source_range.end,
        ));
    }
    let aliases_by_asset = outcome
        .context
        .asset_aliases
        .iter()
        .map(|(alias, asset)| (*asset, alias.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut conflicts = Vec::new();
    for (asset, mut ranges) in ranges_by_asset {
        ranges.sort_by_key(|(_, start, end)| (*start, *end));
        for pair in ranges.windows(2) {
            let (_, left_start, left_end) = pair[0];
            let (right_clip, right_start, right_end) = pair[1];
            let separation = right_start.0.saturating_sub(left_end.0);
            let overlaps = right_start < left_end;
            if overlaps || separation < minimum_separation_frames.0 {
                let label = aliases_by_asset
                    .get(&asset)
                    .map_or_else(|| format!("asset-{asset}"), |alias| (*alias).to_owned());
                conflicts.push(format!(
                    "{label} ({asset}) source range {}..{} and clip {} range {}..{} overlap={} and are separated by {} source frames; minimum is {}",
                    left_start.0,
                    left_end.0,
                    right_clip,
                    right_start.0,
                    right_end.0,
                    overlaps,
                    separation,
                    minimum_separation_frames.0,
                ));
            }
        }
    }
    assertion_result(
        "source ranges separated",
        conflicts.is_empty(),
        if conflicts.is_empty() {
            format!(
                "all media source ranges on track {track_id} are disjoint and separated by at least {} source frames",
                minimum_separation_frames.0
            )
        } else {
            conflicts.join("; ")
        },
    )
}

fn evaluate_source_ranges_chronological(
    track_id: TrackId,
    minimum_forward_gap_frames: TimeCode,
    outcome: &EvalOutcome,
) -> AssertionResult {
    if minimum_forward_gap_frames.0 < 0 {
        return assertion_result(
            "source ranges chronological",
            false,
            format!(
                "minimum forward source-frame gap must be non-negative, observed {}",
                minimum_forward_gap_frames.0
            ),
        );
    }
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return assertion_result(
            "source ranges chronological",
            false,
            format!("track {track_id} does not exist"),
        );
    };
    let aliases_by_asset = outcome
        .context
        .asset_aliases
        .iter()
        .map(|(alias, asset)| (*asset, alias.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut clips = track
        .clips
        .iter()
        .filter(|clip| clip.content.is_media())
        .collect::<Vec<_>>();
    clips.sort_by_key(|clip| (clip.timeline_start, clip.id));
    let mut previous_by_asset =
        BTreeMap::<AssetId, (kinewright_core::ClipId, TimeCode, TimeCode)>::new();
    let mut conflicts = Vec::new();
    for clip in clips {
        if let Some((previous_clip, previous_start, previous_end)) =
            previous_by_asset.get(&clip.asset).copied()
        {
            let required_start = previous_end.checked_add(minimum_forward_gap_frames);
            if required_start.is_none_or(|minimum| clip.source_range.start < minimum) {
                let label = aliases_by_asset.get(&clip.asset).map_or_else(
                    || format!("asset-{}", clip.asset),
                    |alias| (*alias).to_owned(),
                );
                conflicts.push(format!(
                    "{label} ({}) moves backward or reuses earlier source time: timeline clip {previous_clip} uses {}..{} before clip {} uses {}..{}; minimum forward gap is {} source frames",
                    clip.asset,
                    previous_start.0,
                    previous_end.0,
                    clip.id,
                    clip.source_range.start.0,
                    clip.source_range.end.0,
                    minimum_forward_gap_frames.0,
                ));
            }
        }
        previous_by_asset.insert(
            clip.asset,
            (clip.id, clip.source_range.start, clip.source_range.end),
        );
    }
    assertion_result(
        "source ranges chronological",
        conflicts.is_empty(),
        if conflicts.is_empty() {
            format!(
                "every repeated source asset on track {track_id} moves forward with at least {} source frames between ranges",
                minimum_forward_gap_frames.0
            )
        } else {
            conflicts.join("; ")
        },
    )
}

fn evaluate_source_ranges_scene_clean(
    track_id: TrackId,
    scene_set: &str,
    allowed_baked_sequence_starts: &[TimeCode],
    outcome: &EvalOutcome,
) -> AssertionResult {
    let Some(scene_boundaries) = outcome.context.scene_sets.get(scene_set) else {
        return assertion_result(
            "source ranges scene clean",
            false,
            format!("unknown source scene set {scene_set:?}"),
        );
    };
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return assertion_result(
            "source ranges scene clean",
            false,
            format!("track {track_id} does not exist"),
        );
    };
    let aliases_by_asset = outcome
        .context
        .asset_aliases
        .iter()
        .map(|(alias, asset)| (*asset, alias.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut crossings = Vec::new();
    for clip in track.clips.iter().filter(|clip| clip.content.is_media()) {
        if allowed_baked_sequence_starts.contains(&clip.timeline_start) {
            continue;
        }
        for (_, boundary) in scene_boundaries
            .iter()
            .filter(|(asset, _)| *asset == clip.asset)
        {
            if *boundary > clip.source_range.start && *boundary < clip.source_range.end {
                let label = aliases_by_asset.get(&clip.asset).map_or_else(
                    || format!("asset-{}", clip.asset),
                    |alias| (*alias).to_owned(),
                );
                crossings.push(format!(
                    "clip {} uses {label} ({}) source {}..{} across detected boundary {}",
                    clip.id,
                    clip.asset,
                    clip.source_range.start.0,
                    clip.source_range.end.0,
                    boundary.0,
                ));
            }
        }
    }
    assertion_result(
        "source ranges scene clean",
        crossings.is_empty(),
        if crossings.is_empty() {
            format!(
                "every non-exempt media source range on track {track_id} contains at most one detected source shot from {scene_set:?}; reviewed baked-sequence starts={allowed_baked_sequence_starts:?}"
            )
        } else {
            crossings.join("; ")
        },
    )
}

fn evaluate_source_ranges_avoid(
    track_id: TrackId,
    exclusion_set: &str,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let Some(exclusions) = outcome.context.exclusion_sets.get(exclusion_set) else {
        return assertion_result(
            "source ranges avoid exclusions",
            false,
            format!("unknown source-range exclusion set {exclusion_set:?}"),
        );
    };
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return assertion_result(
            "source ranges avoid exclusions",
            false,
            format!("track {track_id} does not exist"),
        );
    };

    let aliases_by_asset = outcome
        .context
        .asset_aliases
        .iter()
        .map(|(alias, asset)| (*asset, alias.as_str()))
        .collect::<BTreeMap<_, _>>();
    let known_assets = outcome
        .final_document
        .media_pool
        .iter()
        .map(|asset| asset.id)
        .collect::<BTreeSet<_>>();
    let mut problems = Vec::new();
    for (index, exclusion) in exclusions.iter().enumerate() {
        if exclusion.source_range.start >= exclusion.source_range.end {
            problems.push(format!(
                "exclusion {index} has an empty or reversed source range {}..{}",
                exclusion.source_range.start.0, exclusion.source_range.end.0
            ));
        }
        if !known_assets.contains(&exclusion.asset) {
            problems.push(format!(
                "exclusion {index} references unknown asset {}",
                exclusion.asset
            ));
        }
        if exclusion.reason.trim().is_empty() {
            problems.push(format!("exclusion {index} has an empty reason"));
        }
    }

    for clip in track.clips.iter().filter(|clip| clip.content.is_media()) {
        for exclusion in exclusions
            .iter()
            .filter(|exclusion| exclusion.asset == clip.asset)
        {
            // Source ranges are half-open. Touching at an endpoint is valid;
            // only a positive-width intersection is prohibited.
            if clip.source_range.start < exclusion.source_range.end
                && exclusion.source_range.start < clip.source_range.end
            {
                let label = aliases_by_asset.get(&clip.asset).map_or_else(
                    || format!("asset-{}", clip.asset),
                    |alias| (*alias).to_owned(),
                );
                problems.push(format!(
                    "clip {} uses {label} ({}) source {}..{} overlapping prohibited range {}..{} ({})",
                    clip.id,
                    clip.asset,
                    clip.source_range.start.0,
                    clip.source_range.end.0,
                    exclusion.source_range.start.0,
                    exclusion.source_range.end.0,
                    exclusion.reason,
                ));
            }
        }
    }

    assertion_result(
        "source ranges avoid exclusions",
        problems.is_empty(),
        if problems.is_empty() {
            format!(
                "all media source ranges on track {track_id} avoid {} manually reviewed exclusions from {exclusion_set:?}",
                exclusions.len()
            )
        } else {
            problems.join("; ")
        },
    )
}

fn evaluate_shot_cadence_variation(
    track_id: TrackId,
    minimum_duration_buckets: usize,
    duration_bucket_frames: TimeCode,
    maximum_similar_run: usize,
    similar_tolerance_frames: TimeCode,
    outcome: &EvalOutcome,
) -> AssertionResult {
    if minimum_duration_buckets == 0
        || duration_bucket_frames.0 <= 0
        || maximum_similar_run == 0
        || similar_tolerance_frames.0 < 0
    {
        return assertion_result(
            "shot cadence variation",
            false,
            format!(
                "invalid cadence contract: minimum_duration_buckets={minimum_duration_buckets}, duration_bucket_frames={}, maximum_similar_run={maximum_similar_run}, similar_tolerance_frames={}",
                duration_bucket_frames.0, similar_tolerance_frames.0
            ),
        );
    }
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return assertion_result(
            "shot cadence variation",
            false,
            format!("track {track_id} does not exist"),
        );
    };
    let mut clips = track
        .clips
        .iter()
        .filter(|clip| clip.content.is_media())
        .collect::<Vec<_>>();
    clips.sort_by_key(|clip| (clip.timeline_start, clip.id));
    let durations = clips
        .iter()
        .map(|clip| outcome.final_document.clip_duration(clip))
        .collect::<Result<Vec<_>, _>>();
    let Ok(durations) = durations else {
        return assertion_result(
            "shot cadence variation",
            false,
            "one or more media clip durations could not be mapped into project frames".to_owned(),
        );
    };
    let buckets = durations
        .iter()
        .map(|duration| {
            duration
                .0
                .saturating_add(duration_bucket_frames.0 / 2)
                .div_euclid(duration_bucket_frames.0)
        })
        .collect::<BTreeSet<_>>();
    let mut current_run = usize::from(!durations.is_empty());
    let mut longest_run = current_run;
    for pair in durations.windows(2) {
        if pair[0].0.abs_diff(pair[1].0) <= similar_tolerance_frames.0.unsigned_abs() {
            current_run += 1;
            longest_run = longest_run.max(current_run);
        } else {
            current_run = 1;
        }
    }
    let passed = buckets.len() >= minimum_duration_buckets && longest_run <= maximum_similar_run;
    assertion_result(
        "shot cadence variation",
        passed,
        format!(
            "track {track_id} mapped durations={:?}, rounded buckets={buckets:?} using {} frames, distinct={} required>={minimum_duration_buckets}, longest similar run={longest_run} allowed<={maximum_similar_run} at tolerance {}",
            durations
                .iter()
                .map(|duration| duration.0)
                .collect::<Vec<_>>(),
            duration_bucket_frames.0,
            buckets.len(),
            similar_tolerance_frames.0,
        ),
    )
}

fn evaluate_no_alternating_shot_pattern(
    track_id: TrackId,
    maximum_repeated_pairs: usize,
    tolerance_frames: TimeCode,
    outcome: &EvalOutcome,
) -> AssertionResult {
    if maximum_repeated_pairs == 0 || tolerance_frames.0 < 0 {
        return assertion_result(
            "non-alternating shot pattern",
            false,
            format!(
                "invalid alternating-pattern contract: maximum_repeated_pairs={maximum_repeated_pairs}, tolerance_frames={}",
                tolerance_frames.0
            ),
        );
    }
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return assertion_result(
            "non-alternating shot pattern",
            false,
            format!("track {track_id} does not exist"),
        );
    };
    if track.kind != kinewright_core::TrackKind::Video {
        return assertion_result(
            "non-alternating shot pattern",
            false,
            format!(
                "track {track_id} has kind {:?}; an alternating visual-shot contract requires a video track",
                track.kind
            ),
        );
    }

    let (clips, mapping_errors) = ordered_media_project_ranges(&outcome.final_document, track);
    if !mapping_errors.is_empty() {
        return assertion_result(
            "non-alternating shot pattern",
            false,
            mapping_errors.join("; "),
        );
    }
    let durations = clips
        .iter()
        .map(|(_, range)| range.end.0.saturating_sub(range.start.0))
        .collect::<Vec<_>>();
    let tolerance = tolerance_frames.0.unsigned_abs();
    let mut longest_run = 0_usize;
    let mut violation = None;
    for start in 0..durations.len().saturating_sub(3) {
        // A constant run is period one, not the alternating pattern this
        // predicate is intended to reject. ShotCadenceVariation owns that
        // separate case.
        if durations[start].abs_diff(durations[start + 1]) <= tolerance {
            continue;
        }
        let mut repeated_pairs = 1_usize;
        while start
            .saturating_add(repeated_pairs.saturating_mul(2))
            .saturating_add(1)
            < durations.len()
            && durations[start].abs_diff(durations[start + repeated_pairs * 2]) <= tolerance
            && durations[start + 1].abs_diff(durations[start + repeated_pairs * 2 + 1]) <= tolerance
        {
            repeated_pairs += 1;
        }
        longest_run = longest_run.max(repeated_pairs);
        if repeated_pairs > maximum_repeated_pairs {
            violation = Some((start, repeated_pairs));
            break;
        }
    }

    let passed = violation.is_none();
    let detail = if let Some((start, repeated_pairs)) = violation {
        format!(
            "track {track_id} mapped durations={durations:?}; period-two pair starting at clip index {start} repeats {repeated_pairs} times, allowed maximum is {maximum_repeated_pairs}, tolerance={}",
            tolerance_frames.0
        )
    } else {
        format!(
            "track {track_id} mapped durations={durations:?}; longest repeated AB-pair run={longest_run}, allowed maximum={maximum_repeated_pairs}, tolerance={}",
            tolerance_frames.0
        )
    };
    assertion_result("non-alternating shot pattern", passed, detail)
}

fn evaluate_beat_aligned_cuts(
    track_id: TrackId,
    beat_set: &str,
    tolerance_frames: TimeCode,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let Some(beats) = outcome.context.timeline_beat_sets.get(beat_set) else {
        return assertion_result(
            "beat-aligned cuts",
            false,
            format!("unknown project-frame beat set {beat_set:?}"),
        );
    };
    if tolerance_frames.0 < 0 {
        return assertion_result(
            "beat-aligned cuts",
            false,
            format!(
                "project-frame tolerance must be non-negative, observed {}",
                tolerance_frames.0
            ),
        );
    }
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return assertion_result(
            "beat-aligned cuts",
            false,
            format!("track {track_id} does not exist"),
        );
    };
    let mut media_clips = track
        .clips
        .iter()
        .filter(|clip| clip.content.is_media())
        .collect::<Vec<_>>();
    media_clips.sort_by_key(|clip| clip.timeline_start);
    if media_clips.len() < 2 {
        return assertion_result(
            "beat-aligned cuts",
            false,
            format!(
                "track {track_id} has {} media clips; at least two are required",
                media_clips.len()
            ),
        );
    }
    if beats.is_empty() {
        return assertion_result(
            "beat-aligned cuts",
            false,
            format!("project-frame beat set {beat_set:?} is empty"),
        );
    }

    let tolerance = tolerance_frames.0.cast_unsigned();
    let misses = media_clips
        .windows(2)
        .filter_map(|pair| {
            let boundary = pair[1].timeline_start;
            let nearest = nearest_frame_distance(boundary, beats);
            nearest.filter(|distance| *distance > tolerance).map(|distance| {
                format!(
                    "cut boundary between clips {} and {} at project frame {} misses every beat in {beat_set:?} (nearest distance {} > tolerance {})",
                    pair[0].id,
                    pair[1].id,
                    boundary.0,
                    distance,
                    tolerance_frames.0,
                )
            })
        })
        .collect::<Vec<_>>();
    assertion_result(
        "beat-aligned cuts",
        misses.is_empty(),
        if misses.is_empty() {
            format!(
                "{} media clips on track {track_id}; every internal project-frame boundary is within inclusive tolerance {} of beat set {beat_set:?}",
                media_clips.len(),
                tolerance_frames.0,
            )
        } else {
            misses.join("; ")
        },
    )
}

fn evaluate_cuts_aligned_to_beat_set_at_least(
    track_id: TrackId,
    beat_set: &str,
    tolerance_frames: TimeCode,
    minimum_aligned_cuts: usize,
    minimum_aligned_basis_points: u16,
    outcome: &EvalOutcome,
) -> AssertionResult {
    if tolerance_frames.0 < 0 || minimum_aligned_basis_points > 10_000 {
        return assertion_result(
            "selected beat-set-aligned cuts",
            false,
            format!(
                "invalid selected beat-set alignment contract: tolerance={} minimum_aligned_basis_points={minimum_aligned_basis_points}",
                tolerance_frames.0
            ),
        );
    }
    let Some(beats) = outcome.context.timeline_beat_sets.get(beat_set) else {
        return assertion_result(
            "selected beat-set-aligned cuts",
            false,
            format!("unknown project-frame beat set {beat_set:?}"),
        );
    };
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return assertion_result(
            "selected beat-set-aligned cuts",
            false,
            format!("track {track_id} does not exist"),
        );
    };
    let mut media_clips = track
        .clips
        .iter()
        .filter(|clip| clip.content.is_media())
        .collect::<Vec<_>>();
    media_clips.sort_by_key(|clip| (clip.timeline_start, clip.id));
    let total_cuts = media_clips.len().saturating_sub(1);
    if total_cuts == 0 || beats.is_empty() {
        return assertion_result(
            "selected beat-set-aligned cuts",
            false,
            format!(
                "track {track_id} internal cuts={total_cuts}; selected beat set {beat_set:?} contains {} frames",
                beats.len()
            ),
        );
    }
    let tolerance = tolerance_frames.0.cast_unsigned();
    let aligned = media_clips
        .iter()
        .skip(1)
        .filter(|clip| {
            nearest_frame_distance(clip.timeline_start, beats)
                .is_some_and(|distance| distance <= tolerance)
        })
        .count();
    let aligned_basis_points = u16::try_from(
        (u128::try_from(aligned).unwrap_or(u128::MAX) * 10_000)
            / u128::try_from(total_cuts).unwrap_or(u128::MAX),
    )
    .unwrap_or(10_000);
    let passed =
        aligned >= minimum_aligned_cuts && aligned_basis_points >= minimum_aligned_basis_points;
    assertion_result(
        "selected beat-set-aligned cuts",
        passed,
        format!(
            "track {track_id} aligned {aligned}/{total_cuts} internal cuts ({aligned_basis_points} basis points) to {beat_set:?} within {} frames; required count>={minimum_aligned_cuts} and share>={minimum_aligned_basis_points}",
            tolerance_frames.0
        ),
    )
}

const MUSIC_FIT_MODEL_GUARANTEE: &str = "repeat/looping is impossible with one finite source_range; speed_percent=100 rejects time-stretch";

#[allow(clippy::too_many_lines)]
fn evaluate_music_fit(
    track_id: TrackId,
    asset_alias: &str,
    source_beat_set: &str,
    timeline_start: TimeCode,
    timeline_end: TimeCode,
    tolerance_source_frames: TimeCode,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let music_result = |passed: bool, detail: String| {
        assertion_result(
            "music fit",
            passed,
            format!("{detail}; {MUSIC_FIT_MODEL_GUARANTEE}"),
        )
    };
    if tolerance_source_frames.0 < 0 {
        return music_result(
            false,
            format!(
                "source-frame tolerance must be non-negative, observed {}",
                tolerance_source_frames.0
            ),
        );
    }
    let Some(expected_asset) = outcome.context.asset_aliases.get(asset_alias) else {
        return music_result(false, format!("unknown asset alias {asset_alias:?}"));
    };
    let Some(source_beats) = outcome.context.source_beat_sets.get(source_beat_set) else {
        return music_result(
            false,
            format!("unknown source-frame beat set {source_beat_set:?}"),
        );
    };
    if source_beats.is_empty() {
        return music_result(
            false,
            format!("source-frame beat set {source_beat_set:?} is empty"),
        );
    }
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return music_result(false, format!("track {track_id} does not exist"));
    };
    let media_clips = track
        .clips
        .iter()
        .filter(|clip| clip.content.is_media())
        .collect::<Vec<_>>();
    if media_clips.len() != 1 {
        return music_result(
            false,
            format!(
                "track {track_id} has {} media clips; exactly one is required",
                media_clips.len()
            ),
        );
    }
    let clip = media_clips[0];
    let mut errors = Vec::new();
    if track.kind != kinewright_core::TrackKind::Audio {
        errors.push(format!(
            "track {track_id} has kind {:?}; music must be on an audio track",
            track.kind
        ));
    }
    let Some(asset) = outcome.final_document.asset(clip.asset) else {
        errors.push(format!(
            "clip {} references missing asset {}",
            clip.id, clip.asset
        ));
        return music_result(false, errors.join("; "));
    };
    if clip.asset != *expected_asset {
        errors.push(format!(
            "expected asset alias {asset_alias:?} ({expected_asset}), observed {}",
            clip.asset
        ));
    }
    if !matches!(asset.kind, MediaKind::Audio | MediaKind::AudioVideo) {
        errors.push(format!(
            "asset {} has kind {:?}; expected audio-capable media",
            clip.asset, asset.kind
        ));
    }
    if clip.timeline_start != timeline_start {
        errors.push(format!(
            "expected project start {}, observed {}",
            timeline_start.0, clip.timeline_start.0
        ));
    }
    match outcome.final_document.clip_duration(clip) {
        Ok(duration) => match clip.timeline_start.checked_add(duration) {
            Some(observed_end) if observed_end == timeline_end => {}
            Some(observed_end) => errors.push(format!(
                "expected project coverage {}..{}, observed {}..{}",
                timeline_start.0, timeline_end.0, clip.timeline_start.0, observed_end.0
            )),
            None => errors.push(format!(
                "project end overflowed from start {} and duration {}",
                clip.timeline_start.0, duration.0
            )),
        },
        Err(error) => errors.push(format!("could not map music clip duration: {error}")),
    }
    if clip.speed_percent != 100 {
        errors.push(format!(
            "music clip speed is {}%; expected real-time 100%",
            clip.speed_percent
        ));
    }
    let source_start = clip.source_range.start;
    let nearest = nearest_frame_distance(source_start, source_beats);
    let tolerance = tolerance_source_frames.0.cast_unsigned();
    if nearest.is_none_or(|distance| distance > tolerance) {
        match nearest {
            Some(distance) => errors.push(format!(
                "source start {} is {} frames from the nearest source beat in {source_beat_set:?}; inclusive tolerance is {}",
                source_start.0,
                distance,
                tolerance_source_frames.0
            )),
            None => errors.push(format!(
                "source start {} has no eligible source beat in {source_beat_set:?}",
                source_start.0
            )),
        }
    }
    if clip.audio_gain_tenth_db != 0 {
        errors.push(format!(
            "clip gain is {} tenths of a dB; expected zero",
            clip.audio_gain_tenth_db
        ));
    }
    if clip.audio_fade_in_frames != TimeCode::ZERO {
        errors.push(format!(
            "audio fade-in is {} project frames; expected zero",
            clip.audio_fade_in_frames.0
        ));
    }
    if clip.audio_fade_out_frames != TimeCode::ZERO {
        errors.push(format!(
            "audio fade-out is {} project frames; expected zero",
            clip.audio_fade_out_frames.0
        ));
    }
    if !clip.effects.is_empty() {
        errors.push(format!(
            "music clip has {} effect(s); expected none",
            clip.effects.len()
        ));
    }
    if clip.transition_in.is_some() {
        errors.push("music clip has an incoming transition; expected none".to_owned());
    }
    if errors.is_empty() {
        music_result(
            true,
            format!(
                "track {track_id} has one audio clip from project {}..{} using asset alias {asset_alias:?}; source start {} is within {} source frames of an eligible beat",
                timeline_start.0, timeline_end.0, source_start.0, tolerance_source_frames.0,
            ),
        )
    } else {
        music_result(false, errors.join("; "))
    }
}

fn evaluate_music_source_end(
    track_id: TrackId,
    asset_alias: &str,
    expected_source_end: TimeCode,
    tolerance_source_frames: TimeCode,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let source_end_result =
        |passed: bool, detail: String| assertion_result("music source end", passed, detail);
    if expected_source_end.0 < 0 {
        return source_end_result(
            false,
            format!(
                "expected source endpoint must be non-negative, observed {}",
                expected_source_end.0
            ),
        );
    }
    if tolerance_source_frames.0 < 0 {
        return source_end_result(
            false,
            format!(
                "source-frame tolerance must be non-negative, observed {}",
                tolerance_source_frames.0
            ),
        );
    }
    let Some(expected_asset) = outcome.context.asset_aliases.get(asset_alias) else {
        return source_end_result(false, format!("unknown asset alias {asset_alias:?}"));
    };
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return source_end_result(false, format!("track {track_id} does not exist"));
    };
    let matching = track
        .clips
        .iter()
        .filter(|clip| clip.content.is_media() && clip.asset == *expected_asset)
        .collect::<Vec<_>>();
    if matching.len() != 1 {
        return source_end_result(
            false,
            format!(
                "track {track_id} has {} matching media clips for asset alias {asset_alias:?}; exactly one is required",
                matching.len()
            ),
        );
    }
    let clip = matching[0];
    let observed_source_end = clip.source_range.end;
    let distance = observed_source_end.0.abs_diff(expected_source_end.0);
    let passed = distance <= tolerance_source_frames.0.cast_unsigned();
    source_end_result(
        passed,
        format!(
            "track {track_id} has one matching music clip for asset alias {asset_alias:?}; expected source end {}, observed {}, distance {} (inclusive tolerance {})",
            expected_source_end.0, observed_source_end.0, distance, tolerance_source_frames.0
        ),
    )
}

fn evaluate_asset_use_minimum(
    track_id: TrackId,
    asset_alias: &str,
    minimum_clip_count: usize,
    minimum_project_frames: TimeCode,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let asset_use_result =
        |passed: bool, detail: String| assertion_result("asset use minimum", passed, detail);
    if minimum_project_frames.0 < 0 {
        return asset_use_result(
            false,
            format!(
                "minimum project duration must be non-negative, observed {}",
                minimum_project_frames.0
            ),
        );
    }
    let Some(expected_asset) = outcome.context.asset_aliases.get(asset_alias) else {
        return asset_use_result(false, format!("unknown asset alias {asset_alias:?}"));
    };
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return asset_use_result(false, format!("track {track_id} does not exist"));
    };

    let matching = track
        .clips
        .iter()
        .filter(|clip| clip.content.is_media() && clip.asset == *expected_asset)
        .collect::<Vec<_>>();
    let mut mapped_project_frames = TimeCode::ZERO;
    let mut errors = Vec::new();
    for clip in &matching {
        match outcome.final_document.clip_duration(clip) {
            Ok(duration) if duration.0 >= 0 => {
                if let Some(total) = mapped_project_frames.checked_add(duration) {
                    mapped_project_frames = total;
                } else {
                    errors.push(format!(
                        "project duration overflowed while adding clip {}",
                        clip.id
                    ));
                }
            }
            Ok(duration) => errors.push(format!(
                "clip {} maps to a negative project duration {}",
                clip.id, duration.0
            )),
            Err(error) => errors.push(format!("clip {} duration mapping failed: {error}", clip.id)),
        }
    }
    let count_passed = matching.len() >= minimum_clip_count;
    let duration_passed = mapped_project_frames >= minimum_project_frames;
    let passed = errors.is_empty() && count_passed && duration_passed;
    if !errors.is_empty() {
        return asset_use_result(
            false,
            format!(
                "track {track_id} asset alias {asset_alias:?}: {}",
                errors.join("; ")
            ),
        );
    }
    asset_use_result(
        passed,
        format!(
            "track {track_id} asset alias {asset_alias:?}: observed {} media clips and {} project frames; required at least {minimum_clip_count} clips and {minimum_project_frames} frames",
            matching.len(),
            mapped_project_frames.0
        ),
    )
}

fn evaluate_asset_temporal_spread(
    track_id: TrackId,
    asset_alias: &str,
    latest_early_start: TimeCode,
    earliest_late_start: TimeCode,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let spread_result =
        |passed: bool, detail: String| assertion_result("asset temporal spread", passed, detail);
    if latest_early_start.0 < 0 || earliest_late_start.0 < 0 {
        return spread_result(
            false,
            format!(
                "phase thresholds must be non-negative, observed latest early={} earliest late={}",
                latest_early_start.0, earliest_late_start.0
            ),
        );
    }
    if latest_early_start > earliest_late_start {
        return spread_result(
            false,
            format!(
                "phase thresholds are reversed: latest early={} is after earliest late={}",
                latest_early_start.0, earliest_late_start.0
            ),
        );
    }
    let Some(expected_asset) = outcome.context.asset_aliases.get(asset_alias) else {
        return spread_result(false, format!("unknown asset alias {asset_alias:?}"));
    };
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return spread_result(false, format!("track {track_id} does not exist"));
    };

    let matching = track
        .clips
        .iter()
        .filter(|clip| clip.content.is_media() && clip.asset == *expected_asset)
        .collect::<Vec<_>>();
    let matching_starts = matching
        .iter()
        .map(|clip| clip.timeline_start.0)
        .collect::<Vec<_>>();
    let pair = matching
        .iter()
        .enumerate()
        .find_map(|(early_index, early)| {
            matching.iter().enumerate().find_map(|(late_index, late)| {
                (early_index != late_index
                    && early.timeline_start <= latest_early_start
                    && late.timeline_start >= earliest_late_start)
                    .then_some((early_index, late_index))
            })
        });
    let passed = pair.is_some();
    spread_result(
        passed,
        match pair {
            Some((early_index, late_index)) => format!(
                "track {track_id} asset alias {asset_alias:?} has distinct early clip {} at {} and late clip {} at {}; thresholds are <= {} and >= {}",
                matching[early_index].id,
                matching[early_index].timeline_start.0,
                matching[late_index].id,
                matching[late_index].timeline_start.0,
                latest_early_start.0,
                earliest_late_start.0
            ),
            None => format!(
                "track {track_id} asset alias {asset_alias:?} has no distinct early/late clip pair; observed matching starts {matching_starts:?}, thresholds are <= {} and >= {}",
                latest_early_start.0, earliest_late_start.0
            ),
        },
    )
}

fn evaluate_clip_source_within(
    track_id: TrackId,
    timeline_start: TimeCode,
    asset_alias: &str,
    source_window: &std::ops::Range<TimeCode>,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let source_result = |passed: bool, detail: String| {
        assertion_result("clip source within reviewed window", passed, detail)
    };
    if timeline_start.0 < 0 {
        return source_result(
            false,
            format!("timeline start must be non-negative, observed {timeline_start}"),
        );
    }
    if source_window.start.0 < 0 || source_window.end <= source_window.start {
        return source_result(
            false,
            format!(
                "reviewed source window {}..{} is invalid",
                source_window.start, source_window.end
            ),
        );
    }
    let Some(expected_asset) = outcome.context.asset_aliases.get(asset_alias) else {
        return source_result(false, format!("unknown asset alias {asset_alias:?}"));
    };
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return source_result(false, format!("track {track_id} does not exist"));
    };
    let matching = track
        .clips
        .iter()
        .filter(|clip| clip.content.is_media() && clip.timeline_start == timeline_start)
        .collect::<Vec<_>>();
    if matching.len() != 1 {
        return source_result(
            false,
            format!(
                "track {track_id} has {} media clips starting at project frame {timeline_start}; exactly one is required",
                matching.len()
            ),
        );
    }
    let clip = matching[0];
    let asset_matches = clip.asset == *expected_asset;
    let source_matches = clip.source_range.start >= source_window.start
        && clip.source_range.end <= source_window.end;
    source_result(
        asset_matches && source_matches,
        format!(
            "track {track_id} clip {} at project frame {timeline_start} uses asset {} and source {}..{}; required alias {asset_alias:?} ({expected_asset}) fully inside {}..{}",
            clip.id,
            clip.asset,
            clip.source_range.start,
            clip.source_range.end,
            source_window.start,
            source_window.end
        ),
    )
}

fn evaluate_edge_shot_holds(
    track_id: TrackId,
    minimum_opening_shot_frames: TimeCode,
    minimum_closing_shot_frames: TimeCode,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let edge_hold_result =
        |passed: bool, detail: String| assertion_result("edge shot holds", passed, detail);
    if minimum_opening_shot_frames.0 < 0 || minimum_closing_shot_frames.0 < 0 {
        return edge_hold_result(
            false,
            format!(
                "minimum edge-shot durations must be non-negative, observed opening={} closing={}",
                minimum_opening_shot_frames.0, minimum_closing_shot_frames.0
            ),
        );
    }
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return edge_hold_result(false, format!("track {track_id} does not exist"));
    };
    let (media_ranges, mapping_errors) =
        ordered_media_project_ranges(&outcome.final_document, track);
    if !mapping_errors.is_empty() {
        return edge_hold_result(false, mapping_errors.join("; "));
    }
    let Some((opening_clip, opening_range)) = media_ranges.first() else {
        return edge_hold_result(false, format!("track {track_id} has no real media clips"));
    };
    let Some((closing_clip, closing_range)) = media_ranges.last() else {
        return edge_hold_result(false, format!("track {track_id} has no real media clips"));
    };
    let opening_duration = opening_range.end.0 - opening_range.start.0;
    let closing_duration = closing_range.end.0 - closing_range.start.0;
    let opening_passed = opening_duration >= minimum_opening_shot_frames.0;
    let closing_passed = closing_duration >= minimum_closing_shot_frames.0;
    edge_hold_result(
        opening_passed && closing_passed,
        format!(
            "track {track_id} first media clip {} holds {} project frames and last media clip {} holds {}; required opening >= {} and closing >= {}",
            opening_clip.id,
            opening_duration,
            closing_clip.id,
            closing_duration,
            minimum_opening_shot_frames.0,
            minimum_closing_shot_frames.0
        ),
    )
}

fn nearest_frame_distance(frame: TimeCode, candidates: &[TimeCode]) -> Option<u64> {
    candidates
        .iter()
        .map(|candidate| frame.0.abs_diff(candidate.0))
        .min()
}

#[allow(clippy::too_many_lines)]
fn evaluate_reframe_stability(
    track_id: TrackId,
    minimum_keyframes_per_axis: usize,
    x_bounds: std::ops::RangeInclusive<i64>,
    y_bounds: std::ops::RangeInclusive<i64>,
    maximum_step_percent: i64,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return assertion_result(
            "reframe stability",
            false,
            format!("track {track_id} does not exist"),
        );
    };
    let (provenances, mut errors) = reframe_subject_provenances(&outcome.final_document);
    let media_clips = track
        .clips
        .iter()
        .filter(|clip| clip.content.is_media())
        .collect::<Vec<_>>();
    for clip in &media_clips {
        let reframes = clip
            .effects
            .iter()
            .filter(|effect| effect.name == "reframe")
            .collect::<Vec<_>>();
        if reframes.len() != 1 {
            errors.push(format!(
                "clip {} has {} reframe effects",
                clip.id,
                reframes.len()
            ));
            continue;
        }
        let effect = reframes[0];
        for (axis, percent_name, basis_points_name, bounds) in [
            (
                "focus_x",
                "focus_x_percent",
                "focus_x_basis_points",
                &x_bounds,
            ),
            (
                "focus_y",
                "focus_y_percent",
                "focus_y_basis_points",
                &y_bounds,
            ),
        ] {
            let Some((curve, units)) = reframe_focus_curve(effect, percent_name, basis_points_name)
            else {
                errors.push(format!("clip {} has no {axis} curve", clip.id));
                continue;
            };
            if curve.keyframes.len() < minimum_keyframes_per_axis {
                errors.push(format!(
                    "clip {} {axis} has {} keyframes, expected at least {minimum_keyframes_per_axis}",
                    clip.id,
                    curve.keyframes.len()
                ));
            }
            if curve.keyframes.iter().any(|keyframe| {
                keyframe.interpolation != kinewright_core::KeyframeInterpolation::Linear
            }) {
                errors.push(format!(
                    "clip {} {axis} is not linearly interpolated",
                    clip.id
                ));
            }
            for keyframe in &curve.keyframes {
                if !units.contains(bounds, keyframe.value) {
                    errors.push(format!(
                        "clip {} {axis} value {} is outside {}",
                        clip.id,
                        keyframe.value,
                        units.render_bounds(bounds),
                    ));
                }
            }
            for pair in curve.keyframes.windows(2) {
                let step = pair[0].value.abs_diff(pair[1].value);
                if step > units.step_limit(maximum_step_percent) {
                    errors.push(format!(
                        "clip {} {axis} jumps {} between frames {} and {}",
                        clip.id,
                        units.render_value(step),
                        pair[0].at.0,
                        pair[1].at.0
                    ));
                }
            }
        }
        let matching_provenance = provenances
            .iter()
            .filter(|provenance| provenance.clip == clip.id && provenance.effect == effect.id)
            .collect::<Vec<_>>();
        if matching_provenance.len() != 1 {
            errors.push(format!(
                "clip {} has {} tracked-subject provenance sidecars for reframe effect {}",
                clip.id,
                matching_provenance.len(),
                effect.id,
            ));
            continue;
        }
        errors.extend(evaluate_tracked_subject_containment(
            &outcome.final_document,
            clip,
            effect,
            matching_provenance[0],
        ));
    }
    assertion_result(
        "reframe stability",
        !media_clips.is_empty() && errors.is_empty(),
        if errors.is_empty() {
            format!(
                "track {track_id} has {} bounded, speed-limited, linearly interpolated reframes that contain their tracked subjects",
                media_clips.len()
            )
        } else {
            errors.join("; ")
        },
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReframeFocusUnits {
    Percent,
    BasisPoints,
}

impl ReframeFocusUnits {
    fn contains(self, bounds: &std::ops::RangeInclusive<i64>, value: i64) -> bool {
        match self {
            Self::Percent => bounds.contains(&value),
            Self::BasisPoints => {
                let minimum = bounds.start().saturating_mul(100);
                let maximum = bounds.end().saturating_mul(100);
                (minimum..=maximum).contains(&value)
            }
        }
    }

    fn render_bounds(self, bounds: &std::ops::RangeInclusive<i64>) -> String {
        match self {
            Self::Percent => format!("{}..={} percent", bounds.start(), bounds.end()),
            Self::BasisPoints => format!(
                "{}..={} basis points",
                bounds.start().saturating_mul(100),
                bounds.end().saturating_mul(100),
            ),
        }
    }

    fn step_limit(self, percent: i64) -> u64 {
        let limit = match self {
            Self::Percent => percent,
            Self::BasisPoints => percent.saturating_mul(100),
        };
        u64::try_from(limit).unwrap_or_default()
    }

    fn render_value(self, value: u64) -> String {
        match self {
            Self::Percent => format!("{value} percent"),
            Self::BasisPoints => format!("{value} basis points"),
        }
    }
}

fn reframe_focus_curve<'a>(
    effect: &'a kinewright_core::Effect,
    percent_name: &str,
    basis_points_name: &str,
) -> Option<(&'a kinewright_core::AutomationCurve, ReframeFocusUnits)> {
    effect
        .keyframes
        .get(basis_points_name)
        .map(|curve| (curve, ReframeFocusUnits::BasisPoints))
        .or_else(|| {
            effect
                .keyframes
                .get(percent_name)
                .map(|curve| (curve, ReframeFocusUnits::Percent))
        })
}

fn reframe_focus_at_basis_points(
    effect: &kinewright_core::Effect,
    percent_name: &str,
    basis_points_name: &str,
    at: TimeCode,
) -> Option<i64> {
    effect
        .integer_parameter_at(basis_points_name, at)
        .or_else(|| {
            effect
                .integer_parameter_at(percent_name, at)
                .map(|percent| percent.saturating_mul(100))
        })
}

fn reframe_subject_provenances(
    document: &Document,
) -> (Vec<ReframeSubjectProvenance>, Vec<String>) {
    let mut provenances = Vec::new();
    let mut errors = Vec::new();
    for marker in &document.markers {
        match decode_reframe_subject_provenance(&marker.label) {
            Ok(Some(provenance)) => provenances.push(provenance),
            Ok(None) => {}
            Err(error) => errors.push(format!(
                "tracked-subject provenance marker {} is malformed: {error}",
                marker.id.0
            )),
        }
    }
    (provenances, errors)
}

fn valid_reframe_subject_provenances(document: &Document) -> Vec<ReframeSubjectProvenance> {
    reframe_subject_provenances(document).0
}

// Template matching follows a supplied search box, not a segmented face edge.
//
// `track_reframe_subject` builds each provenance box in *layer* uv: the tracked
// composite centre pulled back through the layer transform resolved at that
// observation's own frame, bracketed by the declared `subject_width_percent` /
// `subject_height_percent` (half extent = percent * 50 basis points), with the
// left/top edge floored, the right/bottom edge ceiled, and both clamped to
// 0..=10000. It is never routed through the composite template's own bounds,
// whose size is pinned to the seed frame's scale. Provenance bounds therefore
// round outward, and crop bounds round outward, so strict containment is both
// deterministic and conservative.
const SUBJECT_CONTAINMENT_TOLERANCE_BASIS_POINTS: i64 = 0;
const SUBJECT_CONTAINMENT_ENDPOINT_WINDOW_FRAMES: i64 = 25;

fn evaluate_tracked_subject_containment(
    document: &Document,
    clip: &kinewright_core::Clip,
    effect: &kinewright_core::Effect,
    provenance: &ReframeSubjectProvenance,
) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(asset) = document.asset(clip.asset) else {
        return vec![format!("clip {} has no source asset", clip.id)];
    };
    let Some((source_width, source_height)) = asset.resolution else {
        return vec![format!(
            "clip {} source asset {} has no resolution for reframe containment",
            clip.id, asset.id
        )];
    };
    if source_width == 0 || source_height == 0 {
        return vec![format!(
            "clip {} source asset {} has an invalid {}x{} resolution",
            clip.id, asset.id, source_width, source_height
        )];
    }
    let duration = match document.clip_duration(clip) {
        Ok(duration) => duration,
        Err(error) => {
            return vec![format!(
                "clip {} duration is unavailable for reframe containment: {error}",
                clip.id
            )];
        }
    };
    let trailing_start = TimeCode(
        duration
            .0
            .saturating_sub(SUBJECT_CONTAINMENT_ENDPOINT_WINDOW_FRAMES),
    );
    let mut trailing_samples = 0_usize;
    for sample in &provenance.samples {
        if sample.at >= duration {
            errors.push(format!(
                "clip {} tracked-subject sample at frame {} is outside duration {}",
                clip.id, sample.at.0, duration.0
            ));
            continue;
        }
        if sample.at >= trailing_start {
            trailing_samples = trailing_samples.saturating_add(1);
        }
        let crop = match reframe_crop_bounds_basis_points(
            effect,
            source_width,
            source_height,
            sample.at,
        ) {
            Ok(crop) => crop,
            Err(error) => {
                errors.push(format!(
                    "clip {} frame {} cannot resolve reframe crop: {error}",
                    clip.id, sample.at.0
                ));
                continue;
            }
        };
        if !crop.contains(sample, SUBJECT_CONTAINMENT_TOLERANCE_BASIS_POINTS) {
            errors.push(format!(
                "clip {} frame {} crop {}..={} x {}..={} basis points does not contain tracked subject {}..={} x {}..={} basis points",
                clip.id,
                sample.at.0,
                crop.left,
                crop.right,
                crop.top,
                crop.bottom,
                sample.left_basis_points,
                sample.right_basis_points,
                sample.top_basis_points,
                sample.bottom_basis_points,
            ));
        }
    }
    if trailing_samples == 0 {
        errors.push(format!(
            "clip {} has no tracked-subject sample in its final {} frames",
            clip.id, SUBJECT_CONTAINMENT_ENDPOINT_WINDOW_FRAMES
        ));
    }
    errors
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReframeCropBounds {
    left: i64,
    right: i64,
    top: i64,
    bottom: i64,
}

impl ReframeCropBounds {
    fn contains(self, subject: &TrackedSubjectBounds, tolerance: i64) -> bool {
        self.left <= i64::from(subject.left_basis_points).saturating_add(tolerance)
            && self.right >= i64::from(subject.right_basis_points).saturating_sub(tolerance)
            && self.top <= i64::from(subject.top_basis_points).saturating_add(tolerance)
            && self.bottom >= i64::from(subject.bottom_basis_points).saturating_sub(tolerance)
    }
}

fn reframe_crop_bounds_basis_points(
    effect: &kinewright_core::Effect,
    source_width: u32,
    source_height: u32,
    at: TimeCode,
) -> Result<ReframeCropBounds, String> {
    let target_aspect_basis_points = effect
        .integer_parameter_at("target_aspect_basis_points", at)
        .ok_or_else(|| "missing target_aspect_basis_points".to_owned())?;
    if target_aspect_basis_points <= 0 {
        return Err(format!(
            "target_aspect_basis_points must be positive, found {target_aspect_basis_points}"
        ));
    }
    let focus_x =
        reframe_focus_at_basis_points(effect, "focus_x_percent", "focus_x_basis_points", at)
            .ok_or_else(|| "missing horizontal focus".to_owned())?
            .clamp(0, 10_000);
    let focus_y =
        reframe_focus_at_basis_points(effect, "focus_y_percent", "focus_y_basis_points", at)
            .ok_or_else(|| "missing vertical focus".to_owned())?
            .clamp(0, 10_000);
    let source_width = i128::from(source_width);
    let source_height = i128::from(source_height);
    let target_aspect = i128::from(target_aspect_basis_points);
    let source_is_wider =
        source_width.saturating_mul(10_000) > source_height.saturating_mul(target_aspect);
    let source_is_taller =
        source_width.saturating_mul(10_000) < source_height.saturating_mul(target_aspect);
    let (visible_width, visible_height) = if source_is_wider {
        (
            i64::try_from(ceil_div_positive(
                target_aspect.saturating_mul(source_height),
                source_width,
            ))
            .unwrap_or(10_000)
            .clamp(1, 10_000),
            10_000,
        )
    } else if source_is_taller {
        (
            10_000,
            i64::try_from(ceil_div_positive(
                source_width.saturating_mul(100_000_000),
                source_height.saturating_mul(target_aspect),
            ))
            .unwrap_or(10_000)
            .clamp(1, 10_000),
        )
    } else {
        (10_000, 10_000)
    };
    let (left, right) = crop_axis(focus_x, visible_width);
    let (top, bottom) = crop_axis(focus_y, visible_height);
    Ok(ReframeCropBounds {
        left,
        right,
        top,
        bottom,
    })
}

fn ceil_div_positive(numerator: i128, denominator: i128) -> i128 {
    numerator
        .saturating_add(denominator.saturating_sub(1))
        .checked_div(denominator.max(1))
        .unwrap_or_default()
}

fn crop_axis(focus_basis_points: i64, visible_basis_points: i64) -> (i64, i64) {
    let visible_basis_points = visible_basis_points.clamp(1, 10_000);
    let maximum_left = 10_000_i64.saturating_sub(visible_basis_points);
    let left = focus_basis_points
        .saturating_sub(visible_basis_points / 2)
        .clamp(0, maximum_left);
    (left, left.saturating_add(visible_basis_points))
}

fn evaluate_program_audio_continuous(
    track_id: TrackId,
    asset_alias: &str,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let Some(asset) = outcome.context.asset_aliases.get(asset_alias) else {
        return assertion_result(
            "program audio continuous",
            false,
            format!("unknown asset alias {asset_alias:?}"),
        );
    };
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return assertion_result(
            "program audio continuous",
            false,
            format!("track {track_id} does not exist"),
        );
    };
    let media_clips = track
        .clips
        .iter()
        .filter(|clip| clip.content.is_media())
        .collect::<Vec<_>>();
    let passed = track.kind == kinewright_core::TrackKind::Audio
        && media_clips.len() == 1
        && media_clips[0].asset == *asset
        && media_clips[0].audio_gain_tenth_db == 0
        && media_clips[0].audio_fade_in_frames == TimeCode::ZERO
        && media_clips[0].audio_fade_out_frames == TimeCode::ZERO
        && media_clips[0].speed_percent == 100
        && media_clips[0].effects.is_empty()
        && media_clips[0].transition_in.is_none();
    assertion_result(
        "program audio continuous",
        passed,
        format!(
            "track={track_id}, kind={:?}, clips={}, asset={}, expected_asset={}, gain_tenth_db={:?}, fades={:?}, speed={:?}, effects={:?}, transition={:?}",
            track.kind,
            media_clips.len(),
            media_clips.first().map_or(AssetId(0), |clip| clip.asset),
            asset,
            media_clips.first().map(|clip| clip.audio_gain_tenth_db),
            media_clips
                .first()
                .map(|clip| (clip.audio_fade_in_frames, clip.audio_fade_out_frames)),
            media_clips.first().map(|clip| clip.speed_percent),
            media_clips.first().map(|clip| clip.effects.len()),
            media_clips
                .first()
                .and_then(|clip| clip.transition_in.as_ref())
                .map(|transition| transition.name.as_str()),
        ),
    )
}

fn evaluate_media_clip_count(
    track_id: TrackId,
    minimum: usize,
    maximum: usize,
    minimum_duration: TimeCode,
    maximum_duration: TimeCode,
    reject_non_media: bool,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return assertion_result(
            "media clip count and duration bounds",
            false,
            format!("track {track_id} does not exist"),
        );
    };

    if minimum > maximum {
        return assertion_result(
            "media clip count and duration bounds",
            false,
            format!("invalid media clip count bounds {minimum}..={maximum}"),
        );
    }
    if minimum_duration > maximum_duration {
        return assertion_result(
            "media clip count and duration bounds",
            false,
            format!(
                "invalid media clip duration bounds {}..={}",
                minimum_duration.0, maximum_duration.0
            ),
        );
    }

    let media_clips = track
        .clips
        .iter()
        .filter(|clip| matches!(clip.content, ClipContent::Media))
        .collect::<Vec<_>>();
    let non_media_clips = track
        .clips
        .iter()
        .filter(|clip| !matches!(clip.content, ClipContent::Media))
        .map(|clip| format!("clip {} ({:?})", clip.id, clip.content))
        .collect::<Vec<_>>();
    let count = media_clips.len();
    let count_passed = (minimum..=maximum).contains(&count);
    let duration_violations = media_clips
        .iter()
        .filter_map(|clip| match outcome.final_document.clip_duration(clip) {
            Ok(duration) if (minimum_duration..=maximum_duration).contains(&duration) => None,
            Ok(duration) => Some(format!(
                "clip {} duration {} outside {}..={}",
                clip.id, duration.0, minimum_duration.0, maximum_duration.0
            )),
            Err(error) => Some(format!("clip {} duration mapping failed: {error}", clip.id)),
        })
        .collect::<Vec<_>>();
    let non_media_passed = !reject_non_media || non_media_clips.is_empty();
    let passed = count_passed && duration_violations.is_empty() && non_media_passed;
    let detail = if !non_media_passed {
        format!(
            "track {track_id}: non-media clips are forbidden but observed [{}]",
            non_media_clips.join(", ")
        )
    } else if duration_violations.is_empty() {
        format!(
            "track {track_id}: expected {minimum}..={maximum} media clips, observed {count}; every mapped project duration is within {}..={}; non-media clips allowed={}",
            minimum_duration.0, maximum_duration.0, !reject_non_media
        )
    } else {
        format!(
            "track {track_id}: expected {minimum}..={maximum} media clips, observed {count}; {}",
            duration_violations.join("; ")
        )
    };
    assertion_result("media clip count and duration bounds", passed, detail)
}

fn evaluate_single_audio_media_clip(
    track_id: TrackId,
    asset_alias: &str,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let Some(expected_asset) = outcome.context.asset_aliases.get(asset_alias) else {
        return assertion_result(
            "single audio media clip",
            false,
            format!("unknown asset alias {asset_alias:?}"),
        );
    };

    let audio_clips = outcome
        .final_document
        .tracks
        .iter()
        .flat_map(|track| {
            track.clips.iter().filter_map(move |clip| {
                let asset = outcome.final_document.asset(clip.asset)?;
                (clip.content.is_media()
                    && matches!(asset.kind, MediaKind::Audio | MediaKind::AudioVideo))
                .then_some((track.id, clip))
            })
        })
        .collect::<Vec<_>>();
    let passed = audio_clips.len() == 1
        && audio_clips[0].0 == track_id
        && audio_clips[0].1.asset == *expected_asset;
    let observed = audio_clips
        .iter()
        .map(|(track, clip)| format!("track {track}/clip {} asset {}", clip.id, clip.asset))
        .collect::<Vec<_>>();
    assertion_result(
        "single audio media clip",
        passed,
        format!(
            "expected exactly one audio-capable media clip on track {track_id} using asset {asset_alias:?} ({expected_asset}); observed [{}]",
            observed.join(", ")
        ),
    )
}

fn evaluate_dialogue_pause_bounds(
    words: &[TimelineTranscriptWord],
    silences: &[TimelineSilenceSpan],
    minimum: TimeCode,
    maximum: TimeCode,
    capitalization_minimum: TimeCode,
) -> AssertionResult {
    let boundaries =
        dialogue_pacing_gaps(words, silences, minimum, maximum, capitalization_minimum);
    let violations = boundaries
        .iter()
        .filter(|gap| gap.status != "target")
        .map(|gap| {
            format!(
                "{:?}->{:?}={} ({}, transcript={})",
                gap.previous_word,
                gap.next_word,
                gap.pause_frames.0,
                gap.measurement,
                gap.transcript_pause_frames.0,
            )
        })
        .collect::<Vec<_>>();
    let observed = boundaries
        .iter()
        .map(|gap| {
            format!(
                "{:?}->{:?}={} ({})",
                gap.previous_word, gap.next_word, gap.pause_frames.0, gap.measurement,
            )
        })
        .collect::<Vec<_>>();
    assertion_result(
        "dialogue sentence pacing",
        !boundaries.is_empty() && violations.is_empty(),
        format!(
            "expected every detected boundary in {}..={} project frames; observed={observed:?}, violations={violations:?}",
            minimum.0, maximum.0,
        ),
    )
}

fn evaluate_word_set(word_set: &str, outcome: &EvalOutcome, retained: bool) -> AssertionResult {
    let Some(expected) = outcome.context.word_sets.get(word_set) else {
        return assertion_result(
            if retained {
                "words retained"
            } else {
                "words absent"
            },
            false,
            format!("unknown word set {word_set:?}"),
        );
    };
    if expected.is_empty() {
        return assertion_result(
            if retained {
                "words retained"
            } else {
                "words absent"
            },
            false,
            format!("pre-edit word set {word_set:?} is empty"),
        );
    }
    let final_words = normalize_words(outcome.final_words.iter().map(String::as_str));
    let expected = normalize_words(expected.iter().map(String::as_str));
    let matches = expected
        .iter()
        .filter(|word| final_words.contains(*word))
        .cloned()
        .collect::<BTreeSet<_>>();
    let passed = if retained {
        matches == expected
    } else {
        matches.is_empty()
    };
    assertion_result(
        if retained {
            "words retained"
        } else {
            "words absent"
        },
        passed,
        format!("pre-edit set={word_set:?} expected={expected:?}, present after edit={matches:?}"),
    )
}

fn evaluate_caption_words(word_set: &str, outcome: &EvalOutcome) -> AssertionResult {
    let Some(expected) = outcome.context.word_sets.get(word_set) else {
        return assertion_result(
            "caption words exact",
            false,
            format!("unknown authored word set {word_set:?}"),
        );
    };
    let mut captions = timeline_clips(&outcome.final_document)
        .filter_map(|clip| match &clip.content {
            ClipContent::Title(title) if title.caption_preset.is_some() => {
                Some((clip.timeline_start, clip.id, title.text.as_str()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    captions.sort_by_key(|(start, clip, _)| (*start, *clip));
    let observed = normalize_word_sequence(captions.iter().map(|(_, _, text)| *text));
    let expected = normalize_word_sequence(expected.iter().map(String::as_str));
    let (missing, unexpected) = word_sequence_delta(&expected, &observed);
    assertion_result(
        "caption words exact",
        observed == expected,
        format!(
            "authored_set={word_set:?}, expected={expected:?}, observed={observed:?}, missing={missing:?}, unexpected={unexpected:?}"
        ),
    )
}

fn evaluate_caption_sentences(outcome: &EvalOutcome) -> AssertionResult {
    let mut captions = timeline_clips(&outcome.final_document)
        .filter_map(|clip| match &clip.content {
            ClipContent::Title(title) if title.caption_preset.is_some() => {
                Some((clip.timeline_start, clip.id, title.text.as_str()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    captions.sort_by_key(|(start, clip, _)| (*start, *clip));
    let crossovers = captions
        .iter()
        .filter(|(_, _, text)| {
            caption_contains_sentence_crossover(text)
                || caption_contains_capitalized_sentence_crossover(text)
        })
        .map(|(_, clip, text)| format!("clip {}: {text:?}", clip.0))
        .collect::<Vec<_>>();
    let dangling = captions
        .iter()
        .filter(|(_, _, text)| caption_ends_with_dangling_word(text))
        .map(|(_, clip, text)| format!("clip {}: {text:?}", clip.0))
        .collect::<Vec<_>>();
    let semantic_splits = captions
        .windows(2)
        .filter(|pair| caption_boundary_breaks_phrase(pair[0].2, pair[1].2))
        .map(|pair| {
            format!(
                "clips {} -> {}: {:?} / {:?}",
                pair[0].1.0, pair[1].1.0, pair[0].2, pair[1].2
            )
        })
        .collect::<Vec<_>>();
    let missing_punctuation = captions
        .windows(2)
        .filter(|pair| {
            caption_starts_likely_sentence(pair[1].2) && !caption_ends_sentence(pair[0].2)
        })
        .map(|pair| {
            format!(
                "clips {} -> {}: {:?} / {:?}",
                pair[0].1.0, pair[1].1.0, pair[0].2, pair[1].2
            )
        })
        .collect::<Vec<_>>();
    let final_punctuated = captions
        .last()
        .is_some_and(|(_, _, text)| caption_ends_sentence(text));
    let passed = crossovers.is_empty()
        && dangling.is_empty()
        && semantic_splits.is_empty()
        && missing_punctuation.is_empty()
        && final_punctuated;
    assertion_result(
        "caption sentence grouping",
        passed,
        if passed {
            format!(
                "{} caption cues preserve punctuation and semantic phrase boundaries",
                captions.len()
            )
        } else {
            format!(
                "sentence_crossovers={crossovers:?}, dangling_endings={dangling:?}, semantic_splits={semantic_splits:?}, missing_punctuation={missing_punctuation:?}, final_punctuated={final_punctuated}"
            )
        },
    )
}

fn evaluate_caption_presentation(
    allowed_positions: &[TitlePosition],
    color_token: u8,
    background_scrim: bool,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let violations = timeline_clips(&outcome.final_document)
        .filter_map(|clip| match &clip.content {
            ClipContent::Title(title) if title.caption_preset.is_some() => (!allowed_positions
                .contains(&title.position)
                || title.color_token != color_token
                || title.background_scrim != background_scrim)
                .then(|| {
                    format!(
                        "clip {} position={} color_token={} scrim={}",
                        clip.id.0,
                        title.position.as_str(),
                        title.color_token,
                        title.background_scrim
                    )
                }),
            _ => None,
        })
        .collect::<Vec<_>>();
    assertion_result(
        "caption presentation",
        violations.is_empty(),
        if violations.is_empty() {
            format!(
                "all captions use positions={:?}, color_token={color_token}, scrim={background_scrim}",
                allowed_positions
                    .iter()
                    .map(|position| position.as_str())
                    .collect::<Vec<_>>()
            )
        } else {
            format!("violations={violations:?}")
        },
    )
}

fn caption_contains_sentence_crossover(text: &str) -> bool {
    let words = text.split_whitespace().collect::<Vec<_>>();
    words
        .iter()
        .take(words.len().saturating_sub(1))
        .any(|word| {
            let without_closers = word.trim_end_matches(|character| {
                matches!(
                    character,
                    '\'' | '"' | ')' | ']' | '}' | '\u{2019}' | '\u{201d}'
                )
            });
            matches!(without_closers.chars().next_back(), Some('.' | '!' | '?'))
        })
}

fn caption_contains_capitalized_sentence_crossover(text: &str) -> bool {
    text.split_whitespace()
        .skip(1)
        .any(caption_starts_likely_sentence)
}

fn caption_starts_likely_sentence(text: &str) -> bool {
    let word = text
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_matches(|character: char| !character.is_ascii_alphanumeric());
    let starts_uppercase = word
        .chars()
        .find(|character| character.is_alphabetic())
        .is_some_and(char::is_uppercase);
    starts_uppercase
        && matches!(
            word.to_ascii_lowercase().as_str(),
            "and" | "but" | "so" | "then" | "they" | "meanwhile" | "however"
        )
}

fn caption_ends_sentence(text: &str) -> bool {
    let without_closers = text.trim_end_matches(|character| {
        matches!(
            character,
            '\'' | '"' | ')' | ']' | '}' | '\u{2019}' | '\u{201d}'
        )
    });
    matches!(without_closers.chars().next_back(), Some('.' | '!' | '?'))
}

fn caption_ends_with_dangling_word(text: &str) -> bool {
    let word = text
        .split_whitespace()
        .next_back()
        .unwrap_or_default()
        .trim_matches(|character: char| !character.is_ascii_alphanumeric())
        .to_ascii_lowercase();
    matches!(
        word.as_str(),
        "a" | "an"
            | "the"
            | "and"
            | "or"
            | "but"
            | "of"
            | "to"
            | "in"
            | "on"
            | "at"
            | "for"
            | "from"
            | "with"
            | "my"
            | "your"
            | "their"
            | "our"
            | "its"
    )
}

fn caption_boundary_breaks_phrase(previous: &str, next: &str) -> bool {
    let previous_word = previous.split_whitespace().next_back().unwrap_or_default();
    let next_word = next.split_whitespace().next().unwrap_or_default();
    if caption_ends_sentence(previous) || caption_ends_clause(previous_word) {
        return false;
    }
    let previous_normalized = normalize_caption_word(previous_word);
    let next_normalized = normalize_caption_word(next_word);
    let proper_name = starts_with_uppercase(previous_word) && starts_with_uppercase(next_word);
    proper_name
        || caption_ends_with_dangling_word(previous)
        || matches!(
            next_normalized.as_str(),
            "of" | "to" | "in" | "on" | "at" | "for" | "from" | "with"
        )
        || matches!(
            previous_normalized.as_str(),
            "i" | "ive"
                | "im"
                | "you"
                | "youre"
                | "he"
                | "hes"
                | "she"
                | "shes"
                | "it"
                | "its"
                | "we"
                | "were"
                | "they"
                | "theyre"
                | "very"
                | "recently"
                | "especially"
                | "maybe"
                | "just"
                | "even"
                | "that"
                | "these"
                | "those"
                | "this"
                | "where"
        )
        || matches!(
            (previous_normalized.as_str(), next_normalized.as_str()),
            ("super", "8") | ("home", "movies")
        )
}

fn caption_ends_clause(text: &str) -> bool {
    matches!(
        text.trim_end_matches(['\'', '"', ')', ']', '}'])
            .chars()
            .next_back(),
        Some(',' | ';' | ':')
    )
}

fn normalize_caption_word(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}

fn starts_with_uppercase(text: &str) -> bool {
    text.chars()
        .find(|character| character.is_alphabetic())
        .is_some_and(char::is_uppercase)
}

fn evaluate_scene_cuts(scene_set: &str, outcome: &EvalOutcome) -> AssertionResult {
    let Some(scenes) = outcome.context.scene_sets.get(scene_set) else {
        return assertion_result(
            "scene changes are cuts",
            false,
            format!("unknown scene set {scene_set:?}"),
        );
    };
    if scenes.is_empty() {
        return assertion_result(
            "scene changes are cuts",
            false,
            format!("pre-edit scene set {scene_set:?} is empty"),
        );
    }
    let missing = scenes
        .iter()
        .filter(|(asset, source_frame)| {
            !timeline_media_clips(&outcome.final_document)
                .any(|clip| clip.asset == *asset && clip.source_range.start == *source_frame)
        })
        .copied()
        .collect::<Vec<_>>();
    let interior = scenes
        .iter()
        .filter(|(asset, source_frame)| {
            timeline_media_clips(&outcome.final_document).any(|clip| {
                clip.asset == *asset
                    && clip.source_range.start < *source_frame
                    && *source_frame < clip.source_range.end
            })
        })
        .copied()
        .collect::<Vec<_>>();
    assertion_result(
        "scene changes are cuts",
        missing.is_empty() && interior.is_empty(),
        format!("missing boundaries={missing:?}, interior changes={interior:?}"),
    )
}

fn evaluate_effect(
    asset_alias: &str,
    effect_name: &str,
    integer_parameter: Option<&(String, i64)>,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let Some(asset) = outcome.context.asset_aliases.get(asset_alias) else {
        return assertion_result(
            "effect on asset",
            false,
            format!("unknown asset alias {asset_alias:?}"),
        );
    };
    let matched = timeline_media_clips(&outcome.final_document)
        .filter(|clip| clip.asset == *asset)
        .flat_map(|clip| &clip.effects)
        .any(|effect| {
            effect.name == effect_name
                && integer_parameter.as_ref().is_none_or(|(name, expected)| {
                    effect.parameters.get(name) == Some(&ParamValue::Integer(*expected))
                })
        });
    assertion_result(
        "effect on asset",
        matched,
        format!(
            "asset={asset_alias:?}, effect={effect_name:?}, parameter={integer_parameter:?}, matched={matched}"
        ),
    )
}

fn evaluate_transition(
    asset_alias: &str,
    transition_name: &str,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let Some(asset) = outcome.context.asset_aliases.get(asset_alias) else {
        return assertion_result(
            "transition on asset",
            false,
            format!("unknown asset alias {asset_alias:?}"),
        );
    };
    let matched = timeline_media_clips(&outcome.final_document)
        .filter(|clip| clip.asset == *asset)
        .any(|clip| {
            clip.transition_in
                .as_ref()
                .is_some_and(|transition| transition.name == transition_name)
        });
    assertion_result(
        "transition on asset",
        matched,
        format!("asset={asset_alias:?}, transition={transition_name:?}, matched={matched}"),
    )
}

fn evaluate_no_visual_transitions_effects_or_retiming(
    track_id: TrackId,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return assertion_result(
            "hard-cut video track",
            false,
            format!("track {track_id} does not exist"),
        );
    };
    if track.kind != kinewright_core::TrackKind::Video {
        return assertion_result(
            "hard-cut video track",
            false,
            format!(
                "track {track_id} has kind {:?}; a hard-cut visual contract requires a video track",
                track.kind
            ),
        );
    }

    let mut violations = Vec::new();
    for clip in track
        .clips
        .iter()
        .filter(|clip| matches!(clip.content, ClipContent::Media))
    {
        if let Some(transition) = &clip.transition_in {
            violations.push(format!(
                "clip {} has transition {} ({} frames)",
                clip.id, transition.name, transition.duration.0
            ));
        }
        if !clip.effects.is_empty() {
            let effects = clip
                .effects
                .iter()
                .map(|effect| effect.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            violations.push(format!("clip {} has effects [{effects}]", clip.id));
        }
        if clip.speed_percent != 100 {
            violations.push(format!(
                "clip {} is retimed to {}%",
                clip.id, clip.speed_percent
            ));
        }
    }
    assertion_result(
        "hard-cut video track",
        violations.is_empty(),
        if violations.is_empty() {
            format!(
                "track {track_id} media clips have no incoming transitions, effects, or non-real-time playback"
            )
        } else {
            violations.join("; ")
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn evaluate_title_card(
    track_id: TrackId,
    timeline_start: TimeCode,
    duration: TimeCode,
    text: &str,
    font_size_token: u8,
    color_token: u8,
    position: TitlePosition,
    background_scrim: bool,
    fade_in_frames: TimeCode,
    fade_out_frames: TimeCode,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return assertion_result(
            "title card",
            false,
            format!("track {track_id} does not exist"),
        );
    };
    if track.kind != kinewright_core::TrackKind::Video {
        return assertion_result(
            "title card",
            false,
            format!(
                "track {track_id} has kind {:?}; a title card requires video",
                track.kind
            ),
        );
    }

    let title_clips = track
        .clips
        .iter()
        .filter_map(|clip| match &clip.content {
            ClipContent::Title(title) => Some((clip, title)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let freeze_count = track
        .clips
        .iter()
        .filter(|clip| matches!(clip.content, ClipContent::Freeze(_)))
        .count();
    if title_clips.len() != 1 || freeze_count != 0 {
        return assertion_result(
            "title card",
            false,
            format!(
                "track {track_id} has {} title clips and {freeze_count} freeze clips; expected exactly one title and no freeze padding",
                title_clips.len()
            ),
        );
    }

    let (clip, title) = title_clips[0];
    let observed_duration = outcome.final_document.clip_duration(clip);
    let presentation_matches = title.text == text
        && title.font_size_token == font_size_token
        && title.color_token == color_token
        && title.position == position
        && title.background_scrim == background_scrim
        && title.fade_in_frames == fade_in_frames
        && title.fade_out_frames == fade_out_frames
        && title.caption_preset.is_none();
    let clip_is_plain = clip.effects.is_empty()
        && clip.transition_in.is_none()
        && clip.speed_percent == 100
        && clip.audio_gain_tenth_db == 0
        && clip.audio_fade_in_frames == TimeCode::ZERO
        && clip.audio_fade_out_frames == TimeCode::ZERO;
    let passed = clip.timeline_start == timeline_start
        && observed_duration
            .as_ref()
            .is_ok_and(|observed| *observed == duration)
        && presentation_matches
        && clip_is_plain;
    assertion_result(
        "title card",
        passed,
        format!(
            "track {track_id} clip {} starts at {}, duration={observed_duration:?}, text={:?}, font_size_token={}, color_token={}, position={}, scrim={}, fades={}/{} frames, caption_preset={:?}, effects={}, transition={}, speed={}%; expected start {}, duration {}, text={text:?}, font_size_token={font_size_token}, color_token={color_token}, position={}, scrim={background_scrim}, fades={}/{} frames",
            clip.id,
            clip.timeline_start.0,
            title.text,
            title.font_size_token,
            title.color_token,
            title.position.as_str(),
            title.background_scrim,
            title.fade_in_frames.0,
            title.fade_out_frames.0,
            title.caption_preset,
            clip.effects.len(),
            clip.transition_in.is_some(),
            clip.speed_percent,
            timeline_start.0,
            duration.0,
            position.as_str(),
            fade_in_frames.0,
            fade_out_frames.0,
        ),
    )
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn evaluate_source_phase_arc(
    track_id: TrackId,
    opening_alias: &str,
    pivot_alias: &str,
    pivot_window: &std::ops::Range<TimeCode>,
    return_window: &std::ops::Range<TimeCode>,
    closing_alias: &str,
    minimum_opening_hold: TimeCode,
    minimum_closing_hold: TimeCode,
    outcome: &EvalOutcome,
) -> AssertionResult {
    if minimum_opening_hold.0 < 0
        || minimum_closing_hold.0 < 0
        || pivot_window.start.0 < 0
        || pivot_window.start >= pivot_window.end
        || return_window.start.0 < 0
        || return_window.start >= return_window.end
    {
        return assertion_result(
            "source-phase arc",
            false,
            format!(
                "invalid source-phase contract: pivot_window={}..{}, return_window={}..{}, minimum_opening_hold={}, minimum_closing_hold={}",
                pivot_window.start.0,
                pivot_window.end.0,
                return_window.start.0,
                return_window.end.0,
                minimum_opening_hold.0,
                minimum_closing_hold.0
            ),
        );
    }

    let unknown_aliases = [opening_alias, pivot_alias, closing_alias]
        .into_iter()
        .filter(|alias| !outcome.context.asset_aliases.contains_key(*alias))
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    if !unknown_aliases.is_empty() {
        return assertion_result(
            "source-phase arc",
            false,
            format!("unknown asset aliases {unknown_aliases:?}"),
        );
    }
    let opening_asset = outcome
        .context
        .asset_aliases
        .get(opening_alias)
        .copied()
        .expect("unknown aliases returned above");
    let pivot_asset = outcome
        .context
        .asset_aliases
        .get(pivot_alias)
        .copied()
        .expect("unknown aliases returned above");
    let closing_asset = outcome
        .context
        .asset_aliases
        .get(closing_alias)
        .copied()
        .expect("unknown aliases returned above");

    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return assertion_result(
            "source-phase arc",
            false,
            format!("track {track_id} does not exist"),
        );
    };
    if track.kind != kinewright_core::TrackKind::Video {
        return assertion_result(
            "source-phase arc",
            false,
            format!(
                "track {track_id} has kind {:?}; a source-phase arc requires a video track",
                track.kind
            ),
        );
    }
    let (ranges, mapping_errors) = ordered_media_project_ranges(&outcome.final_document, track);
    if !mapping_errors.is_empty() {
        return assertion_result("source-phase arc", false, mapping_errors.join("; "));
    }
    if ranges.is_empty() {
        return assertion_result(
            "source-phase arc",
            false,
            format!("track {track_id} has no real media clips"),
        );
    }
    let non_media = track
        .clips
        .iter()
        .filter(|clip| !matches!(clip.content, ClipContent::Media))
        .map(|clip| format!("clip {} ({:?})", clip.id, clip.content))
        .collect::<Vec<_>>();
    if !non_media.is_empty() {
        return assertion_result(
            "source-phase arc",
            false,
            format!(
                "track {track_id} contains non-media clips [{}]",
                non_media.join(", ")
            ),
        );
    }

    let first = &ranges[0];
    let last = ranges.last().expect("ranges checked non-empty");
    let first_alias_matches = first.0.asset == opening_asset;
    let last_alias_matches = last.0.asset == closing_asset;

    let mut opening_hold_end = first.1.end;
    for (clip, range) in ranges.iter().skip(1) {
        if clip.asset == opening_asset && range.start == opening_hold_end {
            opening_hold_end = range.end;
        } else {
            break;
        }
    }
    let opening_hold = opening_hold_end
        .checked_sub(first.1.start)
        .unwrap_or(TimeCode::ZERO);

    let mut closing_hold_start = last.1.start;
    for (clip, range) in ranges.iter().rev().skip(1) {
        if clip.asset == closing_asset && range.end == closing_hold_start {
            closing_hold_start = range.start;
        } else {
            break;
        }
    }
    let closing_hold = last
        .1
        .end
        .checked_sub(closing_hold_start)
        .unwrap_or(TimeCode::ZERO);

    let pivot_indices = ranges
        .iter()
        .enumerate()
        .filter(|(_, (clip, _))| clip.asset == pivot_asset)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let pivot_start_index = pivot_indices.first().copied();
    let pivot_end_index = pivot_indices.last().copied();
    let pivot_is_interior = pivot_start_index.is_some_and(|index| index > 0)
        && pivot_end_index.is_some_and(|index| index + 1 < ranges.len());
    let pivot_is_contiguous = pivot_start_index
        .zip(pivot_end_index)
        .is_some_and(|(start, end)| {
            ranges[start..=end]
                .iter()
                .all(|(clip, _)| clip.asset == pivot_asset)
        });
    let pivot_start = pivot_start_index.map(|index| ranges[index].1.start);
    let return_start = pivot_end_index
        .and_then(|index| ranges.get(index + 1))
        .map(|(_, range)| range.start);
    let pivot_starts_in_window =
        pivot_start.is_some_and(|start| pivot_window.start <= start && start < pivot_window.end);
    let return_starts_in_window =
        return_start.is_some_and(|start| return_window.start <= start && start < return_window.end);
    let passed = first.1.start == TimeCode::ZERO
        && first_alias_matches
        && last_alias_matches
        && opening_hold >= minimum_opening_hold
        && closing_hold >= minimum_closing_hold
        && pivot_is_interior
        && pivot_is_contiguous
        && pivot_starts_in_window
        && return_starts_in_window;
    assertion_result(
        "source-phase arc",
        passed,
        format!(
            "track {track_id}: first={} at {}..{}, opening={opening_alias:?} first_match={first_alias_matches} hold={} minimum={}, pivot={pivot_alias:?} indices={pivot_indices:?} contiguous={pivot_is_contiguous} interior={pivot_is_interior} start={:?} window={}..{}, return={:?} window={}..{}, closing={closing_alias:?} last_match={last_alias_matches} hold={} minimum={}, last_end={}",
            first.0.asset,
            first.1.start.0,
            first.1.end.0,
            opening_hold.0,
            minimum_opening_hold.0,
            pivot_start.map(|frame| frame.0),
            pivot_window.start.0,
            pivot_window.end.0,
            return_start.map(|frame| frame.0),
            return_window.start.0,
            return_window.end.0,
            closing_hold.0,
            minimum_closing_hold.0,
            last.1.end.0,
        ),
    )
}

fn evaluate_styled_captions(
    minimum_cues: usize,
    motion: CaptionMotion,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let titles = timeline_clips(&outcome.final_document)
        .filter(|clip| matches!(&clip.content, ClipContent::Title(_)))
        .collect::<Vec<_>>();
    let matching = titles
        .iter()
        .filter(|clip| match motion {
            CaptionMotion::None => clip.effects.is_empty(),
            CaptionMotion::Fade => has_animated_parameter(clip, "opacity", "percent"),
            CaptionMotion::Pop => {
                has_animated_parameter(clip, "opacity", "percent")
                    && has_animated_parameter(clip, "transform", "scale_percent")
            }
            CaptionMotion::SlideUp => {
                has_animated_parameter(clip, "opacity", "percent")
                    && has_animated_parameter(clip, "transform", "y_percent")
            }
        })
        .count();
    assertion_result(
        "styled captions",
        titles.len() >= minimum_cues && matching >= minimum_cues,
        format!(
            "expected at least {minimum_cues} {} cues, observed {} title cues and {matching} matching motion curves",
            motion.as_str(),
            titles.len()
        ),
    )
}

fn evaluate_caption_safe_area(profile: DeliveryProfile, outcome: &EvalOutcome) -> AssertionResult {
    // The third hard-coded depth in this file, and deliberately the one that
    // stays: this assertion has no `EvalDeliverableSpec` in scope, and a
    // caption safe area is not a colour deliverable. Plumbing a depth through
    // the caption path is explicitly out of scope.
    match delivery_conformance(
        &outcome.final_document,
        profile,
        DeliveryEncodeDepth::Eight,
        50,
        50,
    ) {
        Ok(report) => {
            let violations = report
                .issues
                .iter()
                .filter(|issue| {
                    matches!(
                        issue.code.as_str(),
                        "caption_outside_safe_area" | "title_layout_unavailable"
                    )
                })
                .count();
            assertion_result(
                "delivery caption safe area",
                violations == 0,
                format!(
                    "profile={}, raster={}x{}, violations={violations}",
                    profile.as_str(),
                    report.resolution.0,
                    report.resolution.1
                ),
            )
        }
        Err(error) => assertion_result(
            "delivery caption safe area",
            false,
            format!(
                "profile={} could not be materialized: {error}",
                profile.as_str()
            ),
        ),
    }
}

fn evaluate_audio_present(document: &Document) -> AssertionResult {
    let audible_clips = document
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .filter(|clip| {
            clip.content.is_media()
                && clip.speed_percent == 100
                && document.asset(clip.asset).is_some_and(|asset| {
                    matches!(asset.kind, MediaKind::Audio | MediaKind::AudioVideo)
                })
        })
        .count();
    assertion_result(
        "timeline audio present",
        audible_clips > 0,
        format!("real-time audio-bearing media clips={audible_clips}"),
    )
}

fn has_animated_parameter(
    clip: &kinewright_core::Clip,
    effect_name: &str,
    parameter: &str,
) -> bool {
    clip.effects.iter().any(|effect| {
        effect.name == effect_name
            && effect
                .keyframes
                .get(parameter)
                .is_some_and(|curve| !curve.keyframes.is_empty())
    })
}

fn assertion_result(assertion: impl Into<String>, passed: bool, detail: String) -> AssertionResult {
    AssertionResult {
        assertion: assertion.into(),
        passed,
        detail,
    }
}

fn timeline_clips(document: &Document) -> impl Iterator<Item = &kinewright_core::Clip> {
    document.tracks.iter().flat_map(|track| &track.clips)
}

fn timeline_media_clips(document: &Document) -> impl Iterator<Item = &kinewright_core::Clip> {
    timeline_clips(document).filter(|clip| {
        matches!(
            &clip.content,
            ClipContent::Media | ClipContent::Freeze { .. }
        )
    })
}

fn normalize_words<'a>(words: impl Iterator<Item = &'a str>) -> BTreeSet<String> {
    words
        .map(|word| {
            word.chars()
                .filter(|character| character.is_alphabetic())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

fn normalize_word_sequence<'a>(words: impl Iterator<Item = &'a str>) -> Vec<String> {
    words
        .flat_map(str::split_whitespace)
        .map(|word| {
            word.chars()
                .filter(|character| character.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

fn word_sequence_delta(expected: &[String], observed: &[String]) -> (Vec<String>, Vec<String>) {
    let mut expected_counts = BTreeMap::<&str, usize>::new();
    let mut observed_counts = BTreeMap::<&str, usize>::new();
    for word in expected {
        *expected_counts.entry(word).or_default() += 1;
    }
    for word in observed {
        *observed_counts.entry(word).or_default() += 1;
    }
    let missing = expected_counts
        .iter()
        .flat_map(|(word, count)| {
            let observed = observed_counts.get(word).copied().unwrap_or(0);
            std::iter::repeat_n((*word).to_owned(), count.saturating_sub(observed))
        })
        .collect();
    let unexpected = observed_counts
        .iter()
        .flat_map(|(word, count)| {
            let expected = expected_counts.get(word).copied().unwrap_or(0);
            std::iter::repeat_n((*word).to_owned(), count.saturating_sub(expected))
        })
        .collect();
    (missing, unexpected)
}

fn word_sequence_edit_distance(expected: &[String], observed: &[String]) -> usize {
    let mut previous = (0..=observed.len()).collect::<Vec<_>>();
    let mut current = vec![0; observed.len() + 1];
    for (expected_index, expected_word) in expected.iter().enumerate() {
        current[0] = expected_index + 1;
        for (observed_index, observed_word) in observed.iter().enumerate() {
            let substitution = previous[observed_index]
                .saturating_add(usize::from(expected_word != observed_word));
            let deletion = previous[observed_index + 1].saturating_add(1);
            let insertion = current[observed_index].saturating_add(1);
            current[observed_index + 1] = substitution.min(deletion).min(insertion);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[observed.len()]
}

fn word_error_rate_basis_points(edit_distance: usize, expected_words: usize) -> u16 {
    if expected_words == 0 {
        return u16::MAX;
    }
    let numerator = u64::try_from(edit_distance)
        .unwrap_or(u64::MAX)
        .saturating_mul(10_000);
    let denominator = u64::try_from(expected_words).unwrap_or(u64::MAX);
    u16::try_from(
        numerator
            .saturating_add(denominator.saturating_sub(1))
            .checked_div(denominator)
            .unwrap_or(u64::MAX),
    )
    .unwrap_or(u16::MAX)
}

fn query_document(core: &Core) -> Result<Arc<Document>, EvalError> {
    let Event::QueryResult(QueryResult::Document(document)) = core
        .request(Command::Query(Query::Document))
        .map_err(|error| EvalError::Core(error.to_string()))?
    else {
        return Err(EvalError::Core(
            "document query returned the wrong event".to_owned(),
        ));
    };
    Ok(document)
}

fn query_operations(core: &Core) -> Result<Vec<Operation>, EvalError> {
    let Event::QueryResult(QueryResult::OpLog(operations)) = core
        .request(Command::Query(Query::OpLog))
        .map_err(|error| EvalError::Core(error.to_string()))?
    else {
        return Err(EvalError::Core(
            "operation-log query returned the wrong event".to_owned(),
        ));
    };
    Ok((*operations).clone())
}

fn restore_original(
    core: &Core,
    original: &Document,
    maximum_undos: u32,
) -> Result<Option<u32>, EvalError> {
    if &*query_document(core)? == original {
        return Ok(Some(0));
    }
    for step in 1..=maximum_undos {
        let Event::DocumentChanged { doc, .. } = core
            .request(Command::Undo)
            .map_err(|error| EvalError::Core(error.to_string()))?
        else {
            return Err(EvalError::Core("undo returned the wrong event".to_owned()));
        };
        if &*doc == original {
            return Ok(Some(step));
        }
    }
    Ok(None)
}

#[derive(Serialize)]
#[serde(tag = "record_type", rename_all = "snake_case")]
enum JsonlRecord<'a> {
    Environment { environment: &'a EnvironmentStamp },
    EvalResult { result: &'a EvalResult },
    Totals { totals: SuiteTotals },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SuiteTotals {
    pub evals: usize,
    pub passed: usize,
    pub failed: usize,
    pub turns: u32,
    pub tool_calls: u32,
    pub input_tokens: u64,
    pub cached_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub output_tokens: u64,
    pub reasoning_output_tokens: Option<u64>,
    pub tool_schema_bytes: u64,
    pub cost_usd: Option<f64>,
    pub wall_time_ms: u64,
    pub operations_applied: u32,
}

impl SuiteTotals {
    #[must_use]
    pub fn from_results(results: &[EvalResult]) -> Self {
        let all_costs_reported = results.iter().all(|result| result.cost_usd.is_some());
        Self {
            evals: results.len(),
            passed: results.iter().filter(|result| result.passed).count(),
            failed: results.iter().filter(|result| !result.passed).count(),
            turns: results.iter().map(|result| result.turns).sum(),
            tool_calls: results.iter().map(EvalResult::tool_call_count).sum(),
            input_tokens: results.iter().map(|result| result.input_tokens).sum(),
            cached_input_tokens: sum_reported_tokens(
                results.iter().map(|result| result.cached_input_tokens),
            ),
            cache_creation_input_tokens: sum_reported_tokens(
                results
                    .iter()
                    .map(|result| result.cache_creation_input_tokens),
            ),
            output_tokens: results.iter().map(|result| result.output_tokens).sum(),
            reasoning_output_tokens: sum_reported_tokens(
                results.iter().map(|result| result.reasoning_output_tokens),
            ),
            tool_schema_bytes: results
                .iter()
                .map(|result| result.tool_surface.serialized_bytes)
                .sum(),
            cost_usd: all_costs_reported
                .then(|| results.iter().filter_map(|result| result.cost_usd).sum()),
            wall_time_ms: results.iter().map(|result| result.wall_time_ms).sum(),
            operations_applied: results.iter().map(|result| result.operations_applied).sum(),
        }
    }
}

fn sum_reported_tokens(mut values: impl Iterator<Item = Option<u64>>) -> Option<u64> {
    values.try_fold(0_u64, |total, value| {
        value.map(|value| total.saturating_add(value))
    })
}

/// Render the same compact Markdown scoreboard used on stdout and in `docs/EVALS.md`.
#[must_use]
pub fn render_scoreboard(results: &[EvalResult]) -> String {
    let mut output = String::from(
        "| Eval | Pass | Assertions | Turns | Tools | Tokens | Cached in | Reasoning | Schema | USD | Wall | Ops |\n\
         |---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n",
    );
    for result in results {
        let status = if result.passed { "PASS" } else { "FAIL" };
        let usd = result
            .cost_usd
            .map_or_else(|| "n/a".to_owned(), |cost| format!("${cost:.4}"));
        let cached = result
            .cached_input_tokens
            .map_or_else(|| "n/a".to_owned(), |tokens| tokens.to_string());
        let reasoning = result
            .reasoning_output_tokens
            .map_or_else(|| "n/a".to_owned(), |tokens| tokens.to_string());
        let _ = writeln!(
            output,
            "| {} | {status} | {}/{} | {} | {} | {} | {cached} | {reasoning} | {} B | {usd} | {} | {} |",
            result.name,
            result.passed_assertion_count(),
            result.assertions.len(),
            result.turns,
            result.tool_call_count(),
            result.total_tokens(),
            result.tool_surface.serialized_bytes,
            format_millis(result.wall_time_ms),
            result.operations_applied,
        );
    }
    let totals = SuiteTotals::from_results(results);
    let status = if totals.failed == 0 { "PASS" } else { "FAIL" };
    let usd = totals
        .cost_usd
        .map_or_else(|| "n/a".to_owned(), |cost| format!("${cost:.4}"));
    let cached = totals
        .cached_input_tokens
        .map_or_else(|| "n/a".to_owned(), |tokens| tokens.to_string());
    let reasoning = totals
        .reasoning_output_tokens
        .map_or_else(|| "n/a".to_owned(), |tokens| tokens.to_string());
    let assertion_passes = results
        .iter()
        .map(EvalResult::passed_assertion_count)
        .sum::<usize>();
    let assertion_total = results
        .iter()
        .map(|result| result.assertions.len())
        .sum::<usize>();
    let _ = writeln!(
        output,
        "| **TOTAL** | **{status}** | **{assertion_passes}/{assertion_total}** | **{}** | **{}** | **{}** | **{cached}** | **{reasoning}** | **{} B** | **{usd}** | **{}** | **{}** |",
        totals.turns,
        totals.tool_calls,
        totals.input_tokens.saturating_add(totals.output_tokens),
        totals.tool_schema_bytes,
        format_millis(totals.wall_time_ms),
        totals.operations_applied,
    );
    output
}

/// Serialize one environment header, one line per eval, and one totals footer.
///
/// # Errors
///
/// Returns a serialization error if a result cannot be represented as JSON.
pub fn render_jsonl(
    environment: &EnvironmentStamp,
    results: &[EvalResult],
) -> Result<String, EvalError> {
    let mut output = String::new();
    append_json_line(&mut output, &JsonlRecord::Environment { environment })?;
    for result in results {
        append_json_line(&mut output, &JsonlRecord::EvalResult { result })?;
    }
    append_json_line(
        &mut output,
        &JsonlRecord::Totals {
            totals: SuiteTotals::from_results(results),
        },
    )?;
    Ok(output)
}

fn append_json_line<T: Serialize>(output: &mut String, value: &T) -> Result<(), EvalError> {
    let line =
        serde_json::to_string(value).map_err(|error| EvalError::Output(error.to_string()))?;
    output.push_str(&line);
    output.push('\n');
    Ok(())
}

#[must_use]
pub fn result_path(root: &Path, environment: &EnvironmentStamp) -> PathBuf {
    let timestamp = environment
        .timestamp_utc
        .replace([':', '-'], "")
        .replace('T', "-")
        .replace('Z', "");
    root.join(format!(
        "kinewright-eval-{timestamp}-{}.jsonl",
        environment.harness
    ))
}

fn format_millis(milliseconds: u64) -> String {
    format!(
        "{}.{:01}s",
        milliseconds / 1_000,
        milliseconds % 1_000 / 100
    )
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn format_utc_timestamp(unix_seconds: u64) -> String {
    let days = i64::try_from(unix_seconds / 86_400).unwrap_or(i64::MAX);
    let seconds = unix_seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds / 3_600;
    let minute = seconds % 3_600 / 60;
    let second = seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

const fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Mutex};

    use crossbeam_channel::{Receiver, Sender, unbounded};
    use kinewright_core::{
        AgentError, AgentSession, AssetSilences, AuthenticationStatus, CaptionPreset, Clip, ClipId,
        HarnessId, Marker, MarkerId, MediaAsset, MediaKind, Rational, SilenceSpan, Track, TrackId,
        TrackKind,
    };

    use super::*;

    #[test]
    fn diagnostic_trace_is_bounded_on_character_boundaries() {
        let long = "é".repeat(2_001);
        let traced = bounded_trace(&long);

        assert_eq!(traced.chars().take(2_000).count(), 2_000);
        assert!(traced.ends_with("...[truncated]"));
        assert_eq!(bounded_trace("short"), "short");
    }

    struct FakeDriver {
        events: Vec<AgentEvent>,
    }

    impl AgentDriver for FakeDriver {
        fn id(&self) -> HarnessId {
            HarnessId::new("fake")
        }

        fn detect(&self) -> Option<HarnessInfo> {
            Some(HarnessInfo {
                id: self.id(),
                executable: PathBuf::from("fake"),
                version: Some("1.0".to_owned()),
                authentication: AuthenticationStatus::Authenticated,
                subscription_tier: None,
            })
        }

        fn start_session(&self, _cfg: SessionConfig) -> Result<Box<dyn AgentSession>, AgentError> {
            let (sender, receiver) = unbounded();
            Ok(Box::new(FakeSession {
                scripted: Mutex::new(Some(self.events.clone())),
                sender,
                receiver,
            }))
        }
    }

    struct FakeSession {
        scripted: Mutex<Option<Vec<AgentEvent>>>,
        sender: Sender<AgentEvent>,
        receiver: Receiver<AgentEvent>,
    }

    impl AgentSession for FakeSession {
        fn send_user_message(&mut self, _text: String) -> Result<(), AgentError> {
            for event in self
                .scripted
                .lock()
                .map_err(|_| AgentError::Harness("fake lock poisoned".to_owned()))?
                .take()
                .unwrap_or_default()
            {
                self.sender
                    .send(event)
                    .map_err(|error| AgentError::Harness(error.to_string()))?;
            }
            Ok(())
        }

        fn events(&self) -> Receiver<AgentEvent> {
            self.receiver.clone()
        }

        fn interrupt(&mut self) {}
    }

    fn budgets() -> EvalBudgets {
        EvalBudgets {
            max_turns: 1,
            max_tool_calls: 4,
            max_operations: 3,
            max_tokens: 1_000,
            max_cost_usd: Some(0.75),
            max_wall_time: Duration::from_secs(1),
            max_undos: 2,
        }
    }

    fn document() -> Document {
        let asset = MediaAsset {
            id: AssetId(1),
            path: PathBuf::from("fixture.mp4"),
            name: "fixture".to_owned(),
            duration: TimeCode(60),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Video,
            resolution: Some((320, 180)),
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
        };
        Document {
            catalog: kinewright_core::MediaCatalog::default(),
            audio_mix: kinewright_core::AudioMix::default(),
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: asset.id,
                    source_range: TimeCode(0)..TimeCode(60),
                    content: kinewright_core::ClipContent::Media,
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
            markers: Vec::new(),
            fps: Rational::new(30, 1).unwrap(),
            resolution: (320, 180),
            duration: TimeCode(60),
            color_context: kinewright_core::ColorContext::default(),
            lut_assets: Vec::new(),
        }
    }

    fn outcome_for(final_document: Document, context: FixtureContext) -> EvalOutcome {
        EvalOutcome {
            final_document,
            original_document: Document::default(),
            color: None,
            final_words: Vec::new(),
            final_timeline_words: Vec::new(),
            remaining_silences: Vec::new(),
            remaining_scenes: Vec::new(),
            context,
            session: SessionMetrics::default(),
            operations: Vec::new(),
            undo_steps_to_original: None,
        }
    }

    fn three_cut_document() -> Document {
        let mut final_document = document();
        let mut second = final_document.tracks[0].clips[0].clone();
        second.id = ClipId(2);
        second.timeline_start = TimeCode(60);
        let mut third = second.clone();
        third.id = ClipId(3);
        third.timeline_start = TimeCode(120);
        final_document.tracks[0].clips =
            vec![final_document.tracks[0].clips[0].clone(), second, third];
        final_document.duration = TimeCode(180);
        final_document
    }

    fn music_document() -> Document {
        let mut final_document = document();
        final_document.media_pool[0].kind = MediaKind::Audio;
        final_document.media_pool[0].duration = TimeCode(120);
        final_document.tracks[0].kind = TrackKind::Audio;
        final_document.tracks[0].clips[0].source_range = TimeCode(10)..TimeCode(70);
        final_document.duration = TimeCode(60);
        final_document
    }

    fn clip_fixture(
        id: u64,
        asset: AssetId,
        timeline_start: i64,
        source_start: i64,
        source_end: i64,
        content: ClipContent,
    ) -> Clip {
        Clip {
            id: ClipId(id),
            asset,
            source_range: TimeCode(source_start)..TimeCode(source_end),
            content,
            timeline_start: TimeCode(timeline_start),
            effects: Vec::new(),
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        }
    }

    fn audio_fixture_document() -> (Document, FixtureContext) {
        let mut final_document = document();
        let audio_asset = MediaAsset {
            id: AssetId(2),
            path: PathBuf::from("music.wav"),
            name: "music".to_owned(),
            duration: TimeCode(120),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Audio,
            resolution: None,
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
        };
        final_document.media_pool.push(audio_asset);
        final_document.tracks.push(Track {
            id: TrackId(2),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: vec![clip_fixture(2, AssetId(2), 0, 0, 60, ClipContent::Media)],
        });
        final_document.duration = TimeCode(60);
        let mut context = FixtureContext::default();
        context.asset_aliases.insert("music".to_owned(), AssetId(2));
        (final_document, context)
    }

    #[test]
    fn media_clip_count_ignores_title_and_freeze_padding() {
        let mut final_document = document();
        final_document.tracks[0].clips.extend([
            clip_fixture(
                2,
                AssetId::default(),
                60,
                60,
                120,
                ClipContent::Title(CaptionPreset::Social.title("padding")),
            ),
            clip_fixture(
                3,
                AssetId(1),
                120,
                120,
                180,
                ClipContent::Freeze(kinewright_core::FreezeFrame {
                    source_frame: TimeCode(0),
                }),
            ),
        ]);
        final_document.duration = TimeCode(180);
        let outcome = outcome_for(final_document, FixtureContext::default());

        let result = evaluate_media_clip_count(
            TrackId(1),
            1,
            1,
            TimeCode(60),
            TimeCode(60),
            false,
            &outcome,
        );
        assert!(result.passed, "{result:?}");
    }

    #[test]
    fn media_clip_count_can_reject_title_and_freeze_padding() {
        let mut final_document = document();
        final_document.tracks[0].clips.extend([
            clip_fixture(
                2,
                AssetId::default(),
                60,
                60,
                120,
                ClipContent::Title(CaptionPreset::Social.title("padding")),
            ),
            clip_fixture(
                3,
                AssetId(1),
                120,
                120,
                180,
                ClipContent::Freeze(kinewright_core::FreezeFrame {
                    source_frame: TimeCode(0),
                }),
            ),
        ]);
        final_document.duration = TimeCode(180);
        let outcome = outcome_for(final_document, FixtureContext::default());

        let result =
            evaluate_media_clip_count(TrackId(1), 1, 1, TimeCode(60), TimeCode(60), true, &outcome);
        assert!(
            !result.passed,
            "non-media padding unexpectedly passed: {result:?}"
        );
        assert!(result.detail.contains("non-media clips are forbidden"));
    }

    #[test]
    fn title_card_requires_exact_presentation_and_rejects_freeze_padding() {
        let mut final_document = document();
        final_document.tracks[0].clips.push(clip_fixture(
            2,
            AssetId::default(),
            60,
            0,
            62,
            ClipContent::Title(kinewright_core::Title {
                text: "TEARS OF STEEL".to_owned(),
                font_size_token: 2,
                color_token: 0,
                position: TitlePosition::Center,
                background_scrim: false,
                fade_in_frames: TimeCode(5),
                fade_out_frames: TimeCode(15),
                caption_preset: None,
            }),
        ));
        final_document.duration = TimeCode(122);
        let outcome = outcome_for(final_document.clone(), FixtureContext::default());
        let result = evaluate_title_card(
            TrackId(1),
            TimeCode(60),
            TimeCode(62),
            "TEARS OF STEEL",
            2,
            0,
            TitlePosition::Center,
            false,
            TimeCode(5),
            TimeCode(15),
            &outcome,
        );
        assert!(result.passed, "{result:?}");

        final_document.tracks[0].clips.push(clip_fixture(
            3,
            AssetId(1),
            122,
            0,
            1,
            ClipContent::Freeze(kinewright_core::FreezeFrame {
                source_frame: TimeCode::ZERO,
            }),
        ));
        let outcome = outcome_for(final_document, FixtureContext::default());
        let result = evaluate_title_card(
            TrackId(1),
            TimeCode(60),
            TimeCode(62),
            "TEARS OF STEEL",
            2,
            0,
            TitlePosition::Center,
            false,
            TimeCode(5),
            TimeCode(15),
            &outcome,
        );
        assert!(!result.passed);
        assert!(result.detail.contains("freeze clips"));
    }

    #[test]
    fn media_clip_count_rejects_too_short_and_too_long_media() {
        for (label, duration) in [("too short", 39_i64), ("too long", 81_i64)] {
            let mut final_document = document();
            final_document.tracks[0].clips[0].source_range = TimeCode::ZERO..TimeCode(duration);
            final_document.duration = TimeCode(duration);
            let outcome = outcome_for(final_document, FixtureContext::default());
            let result = evaluate_media_clip_count(
                TrackId(1),
                1,
                1,
                TimeCode(40),
                TimeCode(80),
                true,
                &outcome,
            );
            assert!(
                !result.passed,
                "{label} clip unexpectedly passed: {result:?}"
            );
            assert!(result.detail.contains(&duration.to_string()), "{result:?}");
        }
    }

    #[test]
    fn media_clip_count_uses_mapped_project_duration() {
        let mut final_document = document();
        final_document.tracks[0].clips[0].source_range = TimeCode::ZERO..TimeCode(30);
        final_document.tracks[0].clips[0].speed_percent = 50;
        final_document.duration = TimeCode(60);
        let outcome = outcome_for(final_document, FixtureContext::default());

        let result =
            evaluate_media_clip_count(TrackId(1), 1, 1, TimeCode(60), TimeCode(60), true, &outcome);
        assert!(result.passed, "{result:?}");
    }

    #[test]
    fn single_audio_media_clip_accepts_the_unique_named_asset() {
        let (final_document, context) = audio_fixture_document();
        let outcome = outcome_for(final_document, context);
        let result = evaluate_single_audio_media_clip(TrackId(2), "music", &outcome);
        assert!(result.passed, "{result:?}");
    }

    #[test]
    fn single_audio_media_clip_rejects_extra_audio_track_or_clip() {
        let (mut final_document, context) = audio_fixture_document();
        final_document.tracks.push(Track {
            id: TrackId(3),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: vec![clip_fixture(3, AssetId(2), 0, 60, 120, ClipContent::Media)],
        });
        let outcome = outcome_for(final_document, context.clone());
        let result = evaluate_single_audio_media_clip(TrackId(2), "music", &outcome);
        assert!(
            !result.passed,
            "extra audio track unexpectedly passed: {result:?}"
        );

        let (mut final_document, context) = audio_fixture_document();
        final_document.tracks[1].clips.push(clip_fixture(
            3,
            AssetId(2),
            60,
            60,
            120,
            ClipContent::Media,
        ));
        let outcome = outcome_for(final_document, context);
        let result = evaluate_single_audio_media_clip(TrackId(2), "music", &outcome);
        assert!(
            !result.passed,
            "extra audio clip unexpectedly passed: {result:?}"
        );
    }

    #[test]
    fn source_ranges_scene_clean_rejects_interior_source_edits_but_allows_boundaries() {
        let mut context = FixtureContext::default();
        context.asset_aliases.insert("video".to_owned(), AssetId(1));
        context.scene_sets.insert(
            "source-scenes".to_owned(),
            vec![(AssetId(1), TimeCode(20)), (AssetId(1), TimeCode(40))],
        );

        let crossing = outcome_for(document(), context.clone());
        let result =
            evaluate_source_ranges_scene_clean(TrackId(1), "source-scenes", &[], &crossing);
        assert!(!result.passed, "interior source edits passed: {result:?}");
        assert!(result.detail.contains("boundary 20"));

        let result = evaluate_source_ranges_scene_clean(
            TrackId(1),
            "source-scenes",
            &[TimeCode::ZERO],
            &crossing,
        );
        assert!(
            result.passed,
            "reviewed baked sequence did not pass: {result:?}"
        );

        let mut clean_document = document();
        clean_document.tracks[0].clips = vec![
            clip_fixture(1, AssetId(1), 0, 0, 20, ClipContent::Media),
            clip_fixture(2, AssetId(1), 20, 20, 40, ClipContent::Media),
            clip_fixture(3, AssetId(1), 40, 40, 60, ClipContent::Media),
        ];
        let clean = outcome_for(clean_document, context);
        let result = evaluate_source_ranges_scene_clean(TrackId(1), "source-scenes", &[], &clean);
        assert!(result.passed, "scene-aligned clips failed: {result:?}");
    }

    #[test]
    fn source_ranges_avoid_uses_half_open_overlap_and_reports_reason() {
        let mut context = FixtureContext::default();
        context.asset_aliases.insert("video".to_owned(), AssetId(1));
        context.exclusion_sets.insert(
            "reviewed-slates".to_owned(),
            vec![SourceRangeExclusion {
                asset: AssetId(1),
                source_range: TimeCode(20)..TimeCode(40),
                reason: "embedded title slate".to_owned(),
            }],
        );

        let mut boundary_document = document();
        boundary_document.tracks[0].clips[0].source_range = TimeCode::ZERO..TimeCode(20);
        boundary_document.duration = TimeCode(20);
        let boundary = outcome_for(boundary_document, context.clone());
        let result = evaluate_source_ranges_avoid(TrackId(1), "reviewed-slates", &boundary);
        assert!(
            result.passed,
            "touching exclusion boundary failed: {result:?}"
        );

        let mut crossing_document = document();
        crossing_document.tracks[0].clips[0].source_range = TimeCode::ZERO..TimeCode(21);
        crossing_document.duration = TimeCode(21);
        let crossing = outcome_for(crossing_document, context);
        let result = evaluate_source_ranges_avoid(TrackId(1), "reviewed-slates", &crossing);
        assert!(!result.passed, "overlapping exclusion passed: {result:?}");
        assert!(result.detail.contains("embedded title slate"));
        assert!(result.detail.contains("20..40"));
    }

    #[test]
    fn shot_cadence_variation_rejects_a_metronome_and_accepts_a_varied_shape() {
        let cadence_outcome = |durations: &[i64]| {
            let mut final_document = document();
            final_document.media_pool[0].duration = TimeCode(200);
            let mut timeline_start = 0_i64;
            final_document.tracks[0].clips = durations
                .iter()
                .enumerate()
                .map(|(index, duration)| {
                    let clip = clip_fixture(
                        u64::try_from(index + 1).unwrap(),
                        AssetId(1),
                        timeline_start,
                        0,
                        *duration,
                        ClipContent::Media,
                    );
                    timeline_start += duration;
                    clip
                })
                .collect();
            final_document.duration = TimeCode(timeline_start);
            outcome_for(final_document, FixtureContext::default())
        };

        let metronome = cadence_outcome(&[76, 80, 83, 80, 77, 80, 80, 84]);
        let result = evaluate_shot_cadence_variation(
            TrackId(1),
            3,
            TimeCode(20),
            3,
            TimeCode(8),
            &metronome,
        );
        assert!(!result.passed, "metronomic cadence passed: {result:?}");
        assert!(result.detail.contains("longest similar run=8"));

        let varied = cadence_outcome(&[120, 80, 40, 60, 40, 80, 120]);
        let result =
            evaluate_shot_cadence_variation(TrackId(1), 3, TimeCode(20), 3, TimeCode(8), &varied);
        assert!(result.passed, "varied cadence failed: {result:?}");
    }

    #[test]
    fn structural_cut_alignment_enforces_both_count_and_share() {
        let mut context = FixtureContext::default();
        context
            .timeline_beat_sets
            .insert("structural".to_owned(), vec![TimeCode(60)]);
        let outcome = outcome_for(three_cut_document(), context);
        let passing = evaluate_cuts_aligned_to_beat_set_at_least(
            TrackId(1),
            "structural",
            TimeCode::ZERO,
            1,
            5_000,
            &outcome,
        );
        assert!(passing.passed, "half-aligned cadence failed: {passing:?}");

        let count_failure = evaluate_cuts_aligned_to_beat_set_at_least(
            TrackId(1),
            "structural",
            TimeCode::ZERO,
            2,
            5_000,
            &outcome,
        );
        assert!(!count_failure.passed);
        let share_failure = evaluate_cuts_aligned_to_beat_set_at_least(
            TrackId(1),
            "structural",
            TimeCode::ZERO,
            1,
            5_001,
            &outcome,
        );
        assert!(!share_failure.passed);
    }

    #[test]
    fn eval_assertion_media_contracts_round_trip_through_json() {
        let assertions = vec![
            EvalAssertion::ExactProjectDuration {
                duration: TimeCode(600),
            },
            EvalAssertion::ExactTrackMediaCoverage {
                track: TrackId(1),
                range: TimeCode::ZERO..TimeCode(600),
            },
            EvalAssertion::MediaClipCount {
                track: TrackId(1),
                minimum: 8,
                maximum: 12,
                minimum_duration: TimeCode(40),
                maximum_duration: TimeCode(120),
                reject_non_media: true,
            },
            EvalAssertion::SingleAudioMediaClip {
                track: TrackId(2),
                asset_alias: "music".to_owned(),
            },
            EvalAssertion::SourceRangesSceneClean {
                track: TrackId(1),
                scene_set: "source-scenes".to_owned(),
                allowed_baked_sequence_starts: Vec::new(),
            },
            EvalAssertion::SourceRangesAvoid {
                track: TrackId(1),
                exclusion_set: "reviewed-slates".to_owned(),
            },
            EvalAssertion::SourceRangesChronological {
                track: TrackId(1),
                minimum_forward_gap_frames: TimeCode::ZERO,
            },
            EvalAssertion::ShotCadenceVariation {
                track: TrackId(1),
                minimum_duration_buckets: 3,
                duration_bucket_frames: TimeCode(20),
                maximum_similar_run: 3,
                similar_tolerance_frames: TimeCode(8),
            },
            EvalAssertion::NoAlternatingShotPattern {
                track: TrackId(1),
                maximum_repeated_pairs: 2,
                tolerance_frames: TimeCode(8),
            },
            EvalAssertion::CutsAlignedToBeatSetAtLeast {
                track: TrackId(1),
                beat_set: "structural".to_owned(),
                tolerance_frames: TimeCode(1),
                minimum_aligned_cuts: 3,
                minimum_aligned_basis_points: 5_000,
            },
            EvalAssertion::NoVisualTransitionsEffectsOrRetiming { track: TrackId(1) },
            EvalAssertion::TitleCard {
                track: TrackId(1),
                timeline_start: TimeCode(538),
                duration: TimeCode(62),
                text: "TEARS OF STEEL".to_owned(),
                font_size_token: 2,
                color_token: 0,
                position: TitlePosition::Center,
                background_scrim: false,
                fade_in_frames: TimeCode(5),
                fade_out_frames: TimeCode(15),
            },
            EvalAssertion::SourcePhaseArc {
                track: TrackId(1),
                opening_alias: "opening".to_owned(),
                pivot_alias: "pivot".to_owned(),
                pivot_window: TimeCode(150)..TimeCode(275),
                return_window: TimeCode(275)..TimeCode(400),
                closing_alias: "closing".to_owned(),
                minimum_opening_hold: TimeCode(60),
                minimum_closing_hold: TimeCode(60),
            },
        ];
        let encoded = serde_json::to_string(&assertions).unwrap();
        let decoded = serde_json::from_str::<Vec<EvalAssertion>>(&encoded).unwrap();
        assert_eq!(decoded, assertions);

        let exclusion = SourceRangeExclusion {
            asset: AssetId(1),
            source_range: TimeCode(12)..TimeCode(24),
            reason: "black transition".to_owned(),
        };
        let encoded = serde_json::to_string(&exclusion).unwrap();
        let decoded = serde_json::from_str::<SourceRangeExclusion>(&encoded).unwrap();
        assert_eq!(decoded, exclusion);
    }

    #[test]
    fn exact_duration_and_media_coverage_reject_tails_gaps_and_cross_track_mismatch() {
        let mut exact_document = document();
        exact_document.media_pool[0].duration = TimeCode(800);
        exact_document.tracks[0].clips[0] =
            clip_fixture(1, AssetId(1), 0, 0, 600, ClipContent::Media);
        let audio_asset = MediaAsset {
            id: AssetId(2),
            path: PathBuf::from("music.wav"),
            name: "music".to_owned(),
            duration: TimeCode(600),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Audio,
            resolution: None,
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
        };
        exact_document.media_pool.push(audio_asset);
        exact_document.tracks.push(Track {
            id: TrackId(2),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: vec![clip_fixture(2, AssetId(2), 0, 0, 600, ClipContent::Media)],
        });
        exact_document.duration = TimeCode(600);
        let mut context = FixtureContext::default();
        context
            .asset_aliases
            .insert("opening".to_owned(), AssetId(1));
        context.asset_aliases.insert("music".to_owned(), AssetId(2));

        let exact = outcome_for(exact_document.clone(), context.clone());
        assert!(evaluate_exact_project_duration(TimeCode(600), &exact).passed);
        assert!(
            evaluate_exact_track_media_coverage(
                TrackId(1),
                &(TimeCode::ZERO..TimeCode(600)),
                &exact
            )
            .passed
        );
        assert!(
            evaluate_exact_track_media_coverage(
                TrackId(2),
                &(TimeCode::ZERO..TimeCode(600)),
                &exact
            )
            .passed
        );

        let mut wide_document = exact_document.clone();
        wide_document.tracks[0].clips[0].source_range = TimeCode::ZERO..TimeCode(800);
        // Keep the declared duration at the requested range to prove the
        // hard gate inspects mapped final clip ranges, not only the field.
        wide_document.duration = TimeCode(600);
        let wide = outcome_for(wide_document, context.clone());
        let duration_failure = evaluate_exact_project_duration(TimeCode(600), &wide);
        assert!(!duration_failure.passed, "{duration_failure:?}");
        assert!(duration_failure.detail.contains("mapped clip end=800"));
        let coverage_failure = evaluate_exact_track_media_coverage(
            TrackId(1),
            &(TimeCode::ZERO..TimeCode(600)),
            &wide,
        );
        assert!(!coverage_failure.passed, "{coverage_failure:?}");
        assert!(coverage_failure.detail.contains("outside requested"));
        assert!(
            evaluate_exact_track_media_coverage(
                TrackId(2),
                &(TimeCode::ZERO..TimeCode(600)),
                &wide
            )
            .passed
        );

        let mut gap_document = exact_document;
        gap_document.tracks[0].clips = vec![
            clip_fixture(1, AssetId(1), 0, 0, 300, ClipContent::Media),
            clip_fixture(3, AssetId(1), 350, 300, 550, ClipContent::Media),
        ];
        let gap = outcome_for(gap_document, context);
        let gap_failure =
            evaluate_exact_track_media_coverage(TrackId(1), &(TimeCode::ZERO..TimeCode(600)), &gap);
        assert!(!gap_failure.passed, "{gap_failure:?}");
        assert!(gap_failure.detail.contains("gap"));
    }

    #[test]
    fn hard_cut_gate_rejects_every_visual_treatment_on_media_clips() {
        let clean = outcome_for(document(), FixtureContext::default());
        assert!(evaluate_no_visual_transitions_effects_or_retiming(TrackId(1), &clean).passed);

        let mut treated_document = document();
        let clip = &mut treated_document.tracks[0].clips[0];
        clip.transition_in = Some(kinewright_core::Transition {
            name: "crossfade".to_owned(),
            duration: TimeCode(6),
        });
        clip.effects.push(kinewright_core::Effect {
            id: kinewright_core::EffectId(4),
            name: "color_grade".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::new(),
        });
        clip.speed_percent = 200;
        let treated = outcome_for(treated_document, FixtureContext::default());
        let rejected = evaluate_no_visual_transitions_effects_or_retiming(TrackId(1), &treated);
        assert!(!rejected.passed, "{rejected:?}");
        assert!(rejected.detail.contains("transition"));
        assert!(rejected.detail.contains("effects"));
        assert!(rejected.detail.contains("retimed to 200%"));
    }

    #[test]
    fn source_phase_arc_uses_actual_alias_order_window_and_edge_holds() {
        let mut final_document = document();
        final_document.media_pool[0].duration = TimeCode(600);
        let pivot_asset = MediaAsset {
            id: AssetId(2),
            path: PathBuf::from("pivot.mp4"),
            name: "pivot".to_owned(),
            duration: TimeCode(600),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Video,
            resolution: Some((320, 180)),
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
        };
        let closing_asset = MediaAsset {
            id: AssetId(3),
            path: PathBuf::from("closing.mp4"),
            name: "closing".to_owned(),
            duration: TimeCode(600),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Video,
            resolution: Some((320, 180)),
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
        };
        final_document
            .media_pool
            .extend([pivot_asset, closing_asset]);
        final_document.tracks[0].clips = vec![
            clip_fixture(1, AssetId(1), 0, 0, 100, ClipContent::Media),
            clip_fixture(2, AssetId(2), 100, 0, 200, ClipContent::Media),
            clip_fixture(3, AssetId(3), 300, 0, 300, ClipContent::Media),
        ];
        final_document.duration = TimeCode(600);
        let mut context = FixtureContext::default();
        context
            .asset_aliases
            .insert("opening".to_owned(), AssetId(1));
        context.asset_aliases.insert("pivot".to_owned(), AssetId(2));
        context
            .asset_aliases
            .insert("closing".to_owned(), AssetId(3));
        let outcome = outcome_for(final_document.clone(), context.clone());
        let contract = |outcome: &EvalOutcome| {
            evaluate_source_phase_arc(
                TrackId(1),
                "opening",
                "pivot",
                &(TimeCode(100)..TimeCode(200)),
                &(TimeCode(250)..TimeCode(350)),
                "closing",
                TimeCode(80),
                TimeCode(200),
                outcome,
            )
        };
        let accepted = contract(&outcome);
        assert!(accepted.passed, "{accepted:?}");

        let mut pivot_before_window = final_document.clone();
        pivot_before_window.tracks[0].clips[1].timeline_start = TimeCode(50);
        let rejected_pivot = contract(&outcome_for(pivot_before_window, context.clone()));
        assert!(!rejected_pivot.passed, "{rejected_pivot:?}");
        assert!(rejected_pivot.detail.contains("start=Some(50)"));

        let mut short_opening = final_document.clone();
        short_opening.tracks[0].clips[0].source_range = TimeCode::ZERO..TimeCode(60);
        short_opening.tracks[0].clips[1].timeline_start = TimeCode(60);
        short_opening.tracks[0].clips[2].timeline_start = TimeCode(260);
        let rejected_hold = contract(&outcome_for(short_opening, context.clone()));
        assert!(!rejected_hold.passed, "{rejected_hold:?}");
        assert!(rejected_hold.detail.contains("hold=60 minimum=80"));

        let mut pivot_reentry = final_document;
        pivot_reentry.tracks[0].clips = vec![
            clip_fixture(1, AssetId(1), 0, 0, 100, ClipContent::Media),
            clip_fixture(2, AssetId(2), 100, 0, 100, ClipContent::Media),
            clip_fixture(3, AssetId(3), 200, 0, 100, ClipContent::Media),
            clip_fixture(4, AssetId(2), 300, 100, 200, ClipContent::Media),
            clip_fixture(5, AssetId(3), 400, 100, 300, ClipContent::Media),
        ];
        let rejected_reentry = contract(&outcome_for(pivot_reentry, context.clone()));
        assert!(!rejected_reentry.passed, "{rejected_reentry:?}");
        assert!(rejected_reentry.detail.contains("contiguous=false"));
    }

    #[test]
    fn alternating_shot_pattern_gate_rejects_period_two_but_allows_varied_durations() {
        let cadence_document = |durations: &[i64]| {
            let mut final_document = document();
            final_document.media_pool[0].duration = TimeCode(2_000);
            let mut timeline_start = 0_i64;
            final_document.tracks[0].clips = durations
                .iter()
                .enumerate()
                .map(|(index, duration)| {
                    let clip = clip_fixture(
                        u64::try_from(index + 1).unwrap(),
                        AssetId(1),
                        timeline_start,
                        timeline_start,
                        timeline_start + duration,
                        ClipContent::Media,
                    );
                    timeline_start += duration;
                    clip
                })
                .collect();
            final_document.duration = TimeCode(timeline_start);
            outcome_for(final_document, FixtureContext::default())
        };

        let metronome = cadence_document(&[50, 80, 50, 80, 50, 80]);
        let rejected = evaluate_no_alternating_shot_pattern(TrackId(1), 2, TimeCode(5), &metronome);
        assert!(!rejected.passed, "{rejected:?}");
        assert!(rejected.detail.contains("repeats 3 times"));

        let varied = cadence_document(&[50, 80, 60, 90, 50, 80]);
        let accepted = evaluate_no_alternating_shot_pattern(TrackId(1), 2, TimeCode(5), &varied);
        assert!(accepted.passed, "{accepted:?}");

        let near_metronome = cadence_document(&[50, 80, 54, 84, 51, 79]);
        let rejected_near =
            evaluate_no_alternating_shot_pattern(TrackId(1), 2, TimeCode(5), &near_metronome);
        assert!(!rejected_near.passed, "{rejected_near:?}");
    }

    fn provenance_marker(effect: u64, samples: &[(i64, u16, u16, u16, u16)]) -> Marker {
        Marker {
            id: MarkerId(99),
            position: TimeCode::ZERO,
            label: crate::server::encode_reframe_subject_provenance(&ReframeSubjectProvenance {
                clip: ClipId(1),
                effect: kinewright_core::EffectId(effect),
                samples: samples
                    .iter()
                    .map(
                        |(
                            at,
                            left_basis_points,
                            right_basis_points,
                            top_basis_points,
                            bottom_basis_points,
                        )| {
                            TrackedSubjectBounds {
                                at: TimeCode(*at),
                                left_basis_points: *left_basis_points,
                                right_basis_points: *right_basis_points,
                                top_basis_points: *top_basis_points,
                                bottom_basis_points: *bottom_basis_points,
                            }
                        },
                    )
                    .collect(),
            }),
            color_token: 3,
        }
    }

    #[test]
    fn rendered_reframe_verification_detects_lost_delivery_curves() {
        let mut source = document();
        source.tracks[0].clips[0]
            .effects
            .push(kinewright_core::Effect {
                id: kinewright_core::EffectId(9),
                name: "reframe".to_owned(),
                parameters: BTreeMap::from([(
                    "target_aspect_basis_points".to_owned(),
                    ParamValue::Integer(5_625),
                )]),
                keyframes: BTreeMap::from([(
                    "focus_x_percent".to_owned(),
                    kinewright_core::AutomationCurve {
                        keyframes: vec![kinewright_core::Keyframe {
                            at: TimeCode::ZERO,
                            value: 42,
                            interpolation: kinewright_core::KeyframeInterpolation::Linear,
                        }],
                    },
                )]),
            });
        source.markers.push(provenance_marker(
            9,
            &[
                (0, 4_000, 5_000, 3_500, 6_500),
                (59, 4_000, 5_000, 3_500, 6_500),
            ],
        ));
        let delivered =
            document_for_delivery_profile(&source, DeliveryProfile::VerticalShort, 50, 50).unwrap();
        let preserved = rendered_reframe_verification(&source, &delivered).unwrap();
        assert_eq!(preserved.expected_animated_clips, 1);
        assert_eq!(preserved.preserved_animated_clips, 1);
        assert_eq!(preserved.expected_subject_provenance_clips, 1);
        assert_eq!(preserved.preserved_subject_provenance_clips, 1);
        assert!(preserved.passed);

        let mut lost = delivered;
        lost.tracks[0].clips[0].effects[0].keyframes.clear();
        let rejected = rendered_reframe_verification(&source, &lost).unwrap();
        assert_eq!(rejected.preserved_animated_clips, 0);
        assert!(!rejected.passed);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn tracked_subject_containment_rejects_static_or_wrong_direction_reframes() {
        let effect = |focus_at_end: i64| kinewright_core::Effect {
            id: kinewright_core::EffectId(7),
            name: "reframe".to_owned(),
            parameters: BTreeMap::from([(
                "target_aspect_basis_points".to_owned(),
                ParamValue::Integer(5_625),
            )]),
            keyframes: BTreeMap::from([
                (
                    "focus_x_basis_points".to_owned(),
                    kinewright_core::AutomationCurve {
                        keyframes: vec![
                            kinewright_core::Keyframe {
                                at: TimeCode::ZERO,
                                value: 5_000,
                                interpolation: kinewright_core::KeyframeInterpolation::Linear,
                            },
                            kinewright_core::Keyframe {
                                at: TimeCode(36),
                                value: focus_at_end,
                                interpolation: kinewright_core::KeyframeInterpolation::Linear,
                            },
                        ],
                    },
                ),
                (
                    "focus_y_basis_points".to_owned(),
                    kinewright_core::AutomationCurve {
                        keyframes: vec![kinewright_core::Keyframe {
                            at: TimeCode::ZERO,
                            value: 5_000,
                            interpolation: kinewright_core::KeyframeInterpolation::Linear,
                        }],
                    },
                ),
            ]),
        };
        let provenance = ReframeSubjectProvenance {
            clip: ClipId(1),
            effect: kinewright_core::EffectId(7),
            samples: vec![
                TrackedSubjectBounds {
                    at: TimeCode::ZERO,
                    left_basis_points: 4_400,
                    right_basis_points: 5_600,
                    top_basis_points: 3_500,
                    bottom_basis_points: 6_500,
                },
                TrackedSubjectBounds {
                    at: TimeCode(36),
                    left_basis_points: 7_000,
                    right_basis_points: 8_000,
                    top_basis_points: 3_500,
                    bottom_basis_points: 6_500,
                },
            ],
        };
        let mut final_document = document();
        final_document.tracks[0].clips[0]
            .effects
            .push(effect(5_000));
        let clip = &final_document.tracks[0].clips[0];
        let rejected_static = evaluate_tracked_subject_containment(
            &final_document,
            clip,
            &clip.effects[0],
            &provenance,
        );
        assert!(
            rejected_static
                .iter()
                .any(|detail| detail.contains("does not contain tracked subject")),
            "{rejected_static:?}"
        );

        final_document.tracks[0].clips[0].effects[0] = effect(3_000);
        let clip = &final_document.tracks[0].clips[0];
        let rejected_wrong_direction = evaluate_tracked_subject_containment(
            &final_document,
            clip,
            &clip.effects[0],
            &provenance,
        );
        assert!(
            rejected_wrong_direction
                .iter()
                .any(|detail| detail.contains("does not contain tracked subject")),
            "{rejected_wrong_direction:?}"
        );

        final_document.media_pool[0].resolution = Some((352, 288));
        final_document.tracks[0].clips[0].effects[0] = effect(4_500);
        let laura_edge = ReframeSubjectProvenance {
            clip: ClipId(1),
            effect: kinewright_core::EffectId(7),
            samples: vec![TrackedSubjectBounds {
                at: TimeCode(36),
                left_basis_points: 2_150,
                right_basis_points: 4_650,
                top_basis_points: 3_500,
                bottom_basis_points: 6_500,
            }],
        };
        let clip = &final_document.tracks[0].clips[0];
        let rejected_edge_clip = evaluate_tracked_subject_containment(
            &final_document,
            clip,
            &clip.effects[0],
            &laura_edge,
        );
        assert!(
            rejected_edge_clip
                .iter()
                .any(|detail| detail.contains("does not contain tracked subject")),
            "{rejected_edge_clip:?}"
        );

        final_document.media_pool[0].resolution = Some((320, 180));
        final_document.tracks[0].clips[0].effects[0] = effect(7_400);
        let clip = &final_document.tracks[0].clips[0];
        let accepted = evaluate_tracked_subject_containment(
            &final_document,
            clip,
            &clip.effects[0],
            &provenance,
        );
        assert!(accepted.is_empty(), "{accepted:?}");
    }

    fn unused_fixture() -> Result<PreparedFixture, EvalError> {
        Err(EvalError::Fixture("unused by unit test".to_owned()))
    }

    #[test]
    fn fake_driver_metrics_are_collected_without_a_subscription_call() {
        let driver = FakeDriver {
            events: vec![
                AgentEvent::ToolCall {
                    name: "get_timeline_state".to_owned(),
                    arguments: "{}".to_owned(),
                },
                AgentEvent::ToolCall {
                    name: "apply_edit_plan".to_owned(),
                    arguments: "{}".to_owned(),
                },
                AgentEvent::Cost {
                    input_tokens: 120,
                    cached_input_tokens: Some(100),
                    cache_creation_input_tokens: Some(4),
                    output_tokens: 30,
                    reasoning_output_tokens: Some(12),
                    cost_usd: Some(0.04),
                },
                AgentEvent::Done,
            ],
        };
        let metrics = collect_session(
            &driver,
            SessionConfig::default(),
            &["edit it"],
            &budgets(),
            None,
            || Ok(2),
        )
        .unwrap();
        assert_eq!(metrics.turns, 1);
        assert_eq!(metrics.tool_call_count(), 2);
        assert_eq!(metrics.total_tokens(), 150);
        assert_eq!(metrics.uncached_input_tokens(), Some(20));
        assert_eq!(metrics.reasoning_output_tokens, Some(12));
        assert_eq!(metrics.cost_usd, Some(0.04));
        assert!(metrics.errors.is_empty());
    }

    #[test]
    // Two more fields on `EvalOutcome` pushed this existing case one line over
    // the pedantic limit; the case itself is unchanged.
    #[allow(clippy::too_many_lines)]
    fn fake_driver_eval_accepts_the_transcript_clamped_bound_and_rounding_allowance() {
        let silences = AssetSilences {
            asset: AssetId(1),
            content_sha256: "fixture".to_owned(),
            source_fps: Rational::new(30, 1).unwrap(),
            source_frames: TimeCode(100),
            threshold_dbfs_hundredths: -4_000,
            window_milliseconds: 20,
            spans: vec![
                SilenceSpan {
                    source_start: TimeCode(10),
                    source_end: TimeCode(40),
                },
                SilenceSpan {
                    source_start: TimeCode(50),
                    source_end: TimeCode(80),
                },
            ],
        };
        let transcript = AssetTranscript {
            asset: AssetId(1),
            content_sha256: "fixture".to_owned(),
            source_fps: Rational::new(30, 1).unwrap(),
            words: vec![
                kinewright_core::TranscriptWord {
                    text: "left".to_owned(),
                    source_start: TimeCode(0),
                    source_end: TimeCode(15),
                    speaker: None,
                },
                kinewright_core::TranscriptWord {
                    text: "middle".to_owned(),
                    source_start: TimeCode(38),
                    source_end: TimeCode(52),
                    speaker: None,
                },
                kinewright_core::TranscriptWord {
                    text: "right".to_owned(),
                    source_start: TimeCode(78),
                    source_end: TimeCode(90),
                    speaker: None,
                },
            ],
        };
        let maximum = maximum_duration_after_expected_silence_cuts(
            TimeCode(100),
            &silences,
            Some(&transcript),
            TimeCode(20),
        );
        assert_eq!(maximum, TimeCode(65));

        let driver = FakeDriver {
            events: vec![AgentEvent::Done],
        };
        let session = collect_session(
            &driver,
            SessionConfig::default(),
            &["remove the silence"],
            &budgets(),
            None,
            || Ok(0),
        )
        .unwrap();
        assert_eq!(session.cached_input_tokens, None);
        assert_eq!(session.cache_creation_input_tokens, None);
        assert_eq!(session.reasoning_output_tokens, None);
        let mut final_document = document();
        final_document.media_pool[0].duration = TimeCode(100);
        final_document.tracks[0].clips[0].source_range.end = maximum;
        final_document.duration = maximum;
        let mut context = FixtureContext::default();
        context.transcripts.insert(AssetId(1), Arc::new(transcript));
        context
            .duration_bounds
            .insert("padded-cut".to_owned(), (TimeCode(20), maximum));
        let definition = EvalDefinition {
            name: "fake-padded-bound",
            rationale: "exercise padded silence bounds",
            fixture_builder: unused_fixture,
            prompts: &["remove the silence"],
            assertions: vec![EvalAssertion::DurationBounds {
                bounds: "padded-cut".to_owned(),
            }],
            budgets: budgets(),
            deliverable: None,
            color: None,
        };
        let outcome = EvalOutcome {
            final_document,
            original_document: Document::default(),
            color: None,
            final_words: Vec::new(),
            final_timeline_words: Vec::new(),
            remaining_silences: Vec::new(),
            remaining_scenes: Vec::new(),
            context,
            session,
            operations: Vec::new(),
            undo_steps_to_original: None,
        };

        let result = evaluate(&definition, &outcome);
        assert!(result.passed, "{:#?}", result.assertions);
    }

    #[test]
    fn typed_predicates_cover_shape_words_tools_budgets_and_undo() {
        let mut context = FixtureContext::default();
        context
            .asset_aliases
            .insert("take-a".to_owned(), AssetId(1));
        context.word_sets.insert(
            "content".to_owned(),
            vec!["Alpha".to_owned(), "Bravo".to_owned()],
        );
        context
            .word_sets
            .insert("removed".to_owned(), vec!["um".to_owned()]);
        context
            .duration_bounds
            .insert("rough-cut".to_owned(), (TimeCode(50), TimeCode(70)));
        let definition = EvalDefinition {
            name: "fake-eval",
            rationale: "exercise predicates",
            fixture_builder: unused_fixture,
            prompts: &["edit it"],
            assertions: vec![
                EvalAssertion::TimelineNonEmpty,
                EvalAssertion::ClipCount {
                    minimum: 1,
                    maximum: 1,
                },
                EvalAssertion::Gapless,
                EvalAssertion::DurationBounds {
                    bounds: "rough-cut".to_owned(),
                },
                EvalAssertion::WordsRetained {
                    word_set: "content".to_owned(),
                },
                EvalAssertion::WordsAbsent {
                    word_set: "removed".to_owned(),
                },
                EvalAssertion::RequiredToolUsage {
                    all_of: vec!["apply_edit_plan".to_owned()],
                    any_of: vec!["get_timeline_state".to_owned()],
                },
                EvalAssertion::UndoIntegrity,
            ],
            budgets: budgets(),
            deliverable: None,
            color: None,
        };
        let outcome = EvalOutcome {
            final_document: document(),
            original_document: Document::default(),
            color: None,
            final_words: vec!["alpha".to_owned(), "bravo".to_owned()],
            final_timeline_words: Vec::new(),
            remaining_silences: Vec::new(),
            remaining_scenes: Vec::new(),
            context,
            session: SessionMetrics {
                turns: 1,
                tool_calls: BTreeMap::from([
                    ("apply_edit_plan".to_owned(), 1),
                    ("get_timeline_state".to_owned(), 1),
                ]),
                input_tokens: 100,
                cached_input_tokens: Some(80),
                cache_creation_input_tokens: Some(5),
                output_tokens: 20,
                reasoning_output_tokens: Some(10),
                tool_surface: crate::ToolSurfaceMetrics {
                    tool_count: 7,
                    serialized_bytes: 2_048,
                    input_schema_bytes: 1_024,
                    description_bytes: 512,
                },
                cost_usd: Some(0.03),
                wall_time_ms: 10,
                errors: Vec::new(),
                interrupted: false,
            },
            operations: vec![Operation::SplitClip {
                clip: ClipId(1),
                at: TimeCode(30),
            }],
            undo_steps_to_original: Some(1),
        };
        let result = evaluate(&definition, &outcome);
        assert!(result.passed, "{:#?}", result.assertions);
        assert!(result.assertions.iter().all(|assertion| assertion.passed));
    }

    #[test]
    fn beat_aligned_cuts_accept_project_frame_boundaries_on_beats() {
        let mut context = FixtureContext::default();
        context.timeline_beat_sets.insert(
            "montage-beats".to_owned(),
            vec![TimeCode(60), TimeCode(120)],
        );
        let outcome = outcome_for(three_cut_document(), context);

        let result =
            evaluate_beat_aligned_cuts(TrackId(1), "montage-beats", TimeCode::ZERO, &outcome);
        assert!(result.passed, "{result:?}");
        assert!(result.detail.contains("inclusive tolerance"));
    }

    #[test]
    fn beat_aligned_cuts_reports_every_project_frame_miss() {
        let mut context = FixtureContext::default();
        context
            .timeline_beat_sets
            .insert("montage-beats".to_owned(), vec![TimeCode(10)]);
        let outcome = outcome_for(three_cut_document(), context);

        let result =
            evaluate_beat_aligned_cuts(TrackId(1), "montage-beats", TimeCode::ZERO, &outcome);
        assert!(!result.passed, "{result:?}");
        assert!(result.detail.contains("clips 1 and 2"));
        assert!(result.detail.contains("clips 2 and 3"));
    }

    #[test]
    fn required_assets_and_source_range_separation_are_track_scoped() {
        let mut final_document = document();
        let second_asset = MediaAsset {
            id: AssetId(2),
            path: PathBuf::from("second.mp4"),
            name: "second".to_owned(),
            duration: TimeCode(60),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Video,
            resolution: Some((320, 180)),
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
        };
        final_document.media_pool.push(second_asset);
        let mut second_clip = final_document.tracks[0].clips[0].clone();
        second_clip.id = ClipId(2);
        second_clip.asset = AssetId(2);
        second_clip.timeline_start = TimeCode(60);
        final_document.tracks[0].clips.push(second_clip);
        let mut context = FixtureContext::default();
        context.asset_aliases.insert("first".to_owned(), AssetId(1));
        context
            .asset_aliases
            .insert("second".to_owned(), AssetId(2));
        let outcome = outcome_for(final_document, context);

        let required = evaluate_required_assets_on_track(
            TrackId(1),
            &["first".to_owned(), "second".to_owned()],
            &outcome,
        );
        assert!(required.passed, "{required:?}");
        let separated = evaluate_source_ranges_separated(TrackId(1), TimeCode::ZERO, &outcome);
        assert!(separated.passed, "{separated:?}");
    }

    #[test]
    fn source_range_separation_rejects_overlap_and_reports_each_conflict() {
        let mut final_document = three_cut_document();
        final_document.tracks[0].clips[1].source_range = TimeCode(59)..TimeCode(90);
        final_document.tracks[0].clips[2].source_range = TimeCode(89)..TimeCode(120);
        let mut context = FixtureContext::default();
        context
            .asset_aliases
            .insert("montage".to_owned(), AssetId(1));
        let outcome = outcome_for(final_document, context);

        let result = evaluate_source_ranges_separated(TrackId(1), TimeCode::ZERO, &outcome);
        assert!(!result.passed, "{result:?}");
        assert!(result.detail.contains("clip 2"));
        assert!(result.detail.contains("clip 3"));
    }

    #[test]
    fn source_range_chronology_rejects_disjoint_but_shuffled_story_order() {
        let mut final_document = three_cut_document();
        final_document.tracks[0].clips[0].source_range = TimeCode(120)..TimeCode(180);
        final_document.tracks[0].clips[1].source_range = TimeCode::ZERO..TimeCode(60);
        final_document.tracks[0].clips[2].source_range = TimeCode(60)..TimeCode(120);
        let mut context = FixtureContext::default();
        context
            .asset_aliases
            .insert("narrative".to_owned(), AssetId(1));
        let shuffled = outcome_for(final_document.clone(), context.clone());

        let separated = evaluate_source_ranges_separated(TrackId(1), TimeCode::ZERO, &shuffled);
        assert!(separated.passed, "{separated:?}");
        let chronological =
            evaluate_source_ranges_chronological(TrackId(1), TimeCode::ZERO, &shuffled);
        assert!(!chronological.passed, "{chronological:?}");
        assert!(chronological.detail.contains("moves backward"));

        final_document.tracks[0].clips[0].source_range = TimeCode::ZERO..TimeCode(60);
        final_document.tracks[0].clips[1].source_range = TimeCode(60)..TimeCode(120);
        final_document.tracks[0].clips[2].source_range = TimeCode(120)..TimeCode(180);
        let ordered = outcome_for(final_document, context);
        let chronological =
            evaluate_source_ranges_chronological(TrackId(1), TimeCode::ZERO, &ordered);
        assert!(chronological.passed, "{chronological:?}");
    }

    #[test]
    fn music_fit_accepts_one_real_time_beat_anchored_audio_clip() {
        let mut context = FixtureContext::default();
        context.asset_aliases.insert("music".to_owned(), AssetId(1));
        context
            .source_beat_sets
            .insert("music-beats".to_owned(), vec![TimeCode(10), TimeCode(40)]);
        let outcome = outcome_for(music_document(), context);

        let result = evaluate_music_fit(
            TrackId(1),
            "music",
            "music-beats",
            TimeCode::ZERO,
            TimeCode(60),
            TimeCode::ZERO,
            &outcome,
        );
        assert!(result.passed, "{result:?}");
        assert!(result.detail.contains("repeat/looping is impossible"));
    }

    #[test]
    fn music_fit_rejects_retime_shaping_and_misaligned_source_start() {
        let mut final_document = music_document();
        let clip = &mut final_document.tracks[0].clips[0];
        clip.source_range = TimeCode(12)..TimeCode(72);
        clip.speed_percent = 200;
        clip.audio_gain_tenth_db = 10;
        clip.audio_fade_in_frames = TimeCode(2);
        clip.effects.push(kinewright_core::Effect {
            id: kinewright_core::EffectId(1),
            name: "compressor".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::new(),
        });
        clip.transition_in = Some(kinewright_core::Transition {
            name: "crossfade".to_owned(),
            duration: TimeCode(2),
        });
        let mut context = FixtureContext::default();
        context.asset_aliases.insert("music".to_owned(), AssetId(1));
        context
            .source_beat_sets
            .insert("music-beats".to_owned(), vec![TimeCode(10)]);
        let outcome = outcome_for(final_document, context);

        let result = evaluate_music_fit(
            TrackId(1),
            "music",
            "music-beats",
            TimeCode::ZERO,
            TimeCode(60),
            TimeCode::ZERO,
            &outcome,
        );
        assert!(!result.passed, "{result:?}");
        assert!(result.detail.contains("speed is 200%"));
        assert!(result.detail.contains("source start 12"));
        assert!(result.detail.contains("clip gain"));
        assert!(result.detail.contains("fade-in"));
        assert!(result.detail.contains("effect(s)"));
        assert!(result.detail.contains("transition"));
        assert!(result.detail.contains("time-stretch"));
    }

    #[test]
    fn music_source_end_pins_the_single_matching_clip() {
        let mut context = FixtureContext::default();
        context.asset_aliases.insert("music".to_owned(), AssetId(1));
        let outcome = outcome_for(music_document(), context);

        let passing =
            evaluate_music_source_end(TrackId(1), "music", TimeCode(70), TimeCode::ZERO, &outcome);
        assert!(passing.passed, "{passing:?}");

        let failing =
            evaluate_music_source_end(TrackId(1), "music", TimeCode(71), TimeCode::ZERO, &outcome);
        assert!(
            !failing.passed,
            "source endpoint unexpectedly passed: {failing:?}"
        );
        assert!(failing.detail.contains("distance 1"));
    }

    #[test]
    fn asset_use_minimum_requires_count_and_mapped_project_duration() {
        let mut final_document = document();
        final_document.media_pool[0].duration = TimeCode(200);
        final_document.tracks[0].clips.push(clip_fixture(
            2,
            AssetId(1),
            60,
            60,
            100,
            ClipContent::Media,
        ));
        final_document.duration = TimeCode(100);
        let mut context = FixtureContext::default();
        context
            .asset_aliases
            .insert("visual".to_owned(), AssetId(1));
        let outcome = outcome_for(final_document, context);

        let passing = evaluate_asset_use_minimum(TrackId(1), "visual", 2, TimeCode(100), &outcome);
        assert!(passing.passed, "{passing:?}");

        let failing_count =
            evaluate_asset_use_minimum(TrackId(1), "visual", 3, TimeCode(100), &outcome);
        assert!(
            !failing_count.passed,
            "minimum count unexpectedly passed: {failing_count:?}"
        );

        let failing_duration =
            evaluate_asset_use_minimum(TrackId(1), "visual", 2, TimeCode(101), &outcome);
        assert!(
            !failing_duration.passed,
            "minimum duration unexpectedly passed: {failing_duration:?}"
        );
    }

    #[test]
    fn asset_temporal_spread_requires_distinct_early_and_late_clips() {
        let mut final_document = document();
        final_document.media_pool[0].duration = TimeCode(200);
        final_document.tracks[0].clips[0] =
            clip_fixture(1, AssetId(1), 30, 0, 120, ClipContent::Media);
        final_document.duration = TimeCode(150);
        let mut context = FixtureContext::default();
        context
            .asset_aliases
            .insert("visual".to_owned(), AssetId(1));

        let single_clip = outcome_for(final_document.clone(), context.clone());
        let spanning = evaluate_asset_temporal_spread(
            TrackId(1),
            "visual",
            TimeCode(30),
            TimeCode(30),
            &single_clip,
        );
        assert!(
            !spanning.passed,
            "one clip spanning the split unexpectedly passed: {spanning:?}"
        );
        assert!(spanning.detail.contains("no distinct"), "{spanning:?}");

        let mut spread_document = final_document;
        spread_document.tracks[0].clips.push(clip_fixture(
            2,
            AssetId(1),
            80,
            120,
            180,
            ClipContent::Media,
        ));
        let spread = outcome_for(spread_document, context);
        let passing = evaluate_asset_temporal_spread(
            TrackId(1),
            "visual",
            TimeCode(30),
            TimeCode(80),
            &spread,
        );
        assert!(passing.passed, "{passing:?}");
    }

    #[test]
    fn asset_temporal_spread_rejects_invalid_thresholds_aliases_and_tracks() {
        let mut context = FixtureContext::default();
        context
            .asset_aliases
            .insert("visual".to_owned(), AssetId(1));
        let outcome = outcome_for(document(), context);

        let reversed = evaluate_asset_temporal_spread(
            TrackId(1),
            "visual",
            TimeCode(20),
            TimeCode(10),
            &outcome,
        );
        assert!(!reversed.passed, "reversed thresholds passed: {reversed:?}");
        assert!(reversed.detail.contains("reversed"), "{reversed:?}");

        let negative = evaluate_asset_temporal_spread(
            TrackId(1),
            "visual",
            TimeCode(-1),
            TimeCode(10),
            &outcome,
        );
        assert!(!negative.passed, "negative threshold passed: {negative:?}");
        assert!(negative.detail.contains("non-negative"), "{negative:?}");

        let unknown_alias = evaluate_asset_temporal_spread(
            TrackId(1),
            "missing",
            TimeCode::ZERO,
            TimeCode(10),
            &outcome,
        );
        assert!(
            !unknown_alias.passed,
            "unknown alias passed: {unknown_alias:?}"
        );
        assert!(unknown_alias.detail.contains("unknown asset alias"));

        let missing_track = evaluate_asset_temporal_spread(
            TrackId(99),
            "visual",
            TimeCode::ZERO,
            TimeCode(10),
            &outcome,
        );
        assert!(
            !missing_track.passed,
            "unknown track passed: {missing_track:?}"
        );
        assert!(missing_track.detail.contains("does not exist"));
    }

    #[test]
    fn clip_source_window_pins_a_timeline_role_without_pinning_exact_frames() {
        let mut final_document = document();
        final_document.tracks[0].clips[0] =
            clip_fixture(1, AssetId(1), 20, 30, 50, ClipContent::Media);
        let mut context = FixtureContext::default();
        context
            .asset_aliases
            .insert("visual".to_owned(), AssetId(1));
        let outcome = outcome_for(final_document, context);

        let passing = evaluate_clip_source_within(
            TrackId(1),
            TimeCode(20),
            "visual",
            &(TimeCode(25)..TimeCode(55)),
            &outcome,
        );
        assert!(passing.passed, "{passing:?}");

        let failing = evaluate_clip_source_within(
            TrackId(1),
            TimeCode(20),
            "visual",
            &(TimeCode(31)..TimeCode(55)),
            &outcome,
        );
        assert!(
            !failing.passed,
            "source escape unexpectedly passed: {failing:?}"
        );
        assert!(failing.detail.contains("source 30..50"), "{failing:?}");
    }

    #[test]
    fn edge_shot_holds_measure_individual_first_and_last_clips() {
        let mut final_document = document();
        final_document.media_pool[0].duration = TimeCode(200);
        final_document.tracks[0].clips = vec![
            clip_fixture(1, AssetId(1), 0, 0, 40, ClipContent::Media),
            clip_fixture(2, AssetId(1), 40, 40, 60, ClipContent::Media),
            clip_fixture(3, AssetId(1), 60, 60, 140, ClipContent::Media),
        ];
        final_document.duration = TimeCode(140);
        let outcome = outcome_for(final_document, FixtureContext::default());

        let passing = evaluate_edge_shot_holds(TrackId(1), TimeCode(40), TimeCode(80), &outcome);
        assert!(passing.passed, "{passing:?}");

        let failing = evaluate_edge_shot_holds(TrackId(1), TimeCode(41), TimeCode(81), &outcome);
        assert!(
            !failing.passed,
            "edge holds unexpectedly passed: {failing:?}"
        );
        assert!(failing.detail.contains("first media clip 1 holds 40"));
        assert!(failing.detail.contains("last media clip 3 holds 80"));
    }

    #[test]
    fn audio_presence_follows_the_real_mixer_contract() {
        let mut with_audio = document();
        assert!(!evaluate_audio_present(&with_audio).passed);
        with_audio.media_pool[0].kind = MediaKind::AudioVideo;
        assert!(evaluate_audio_present(&with_audio).passed);

        with_audio.tracks[0].clips[0].speed_percent = 200;
        assert!(!evaluate_audio_present(&with_audio).passed);
    }

    #[test]
    fn reframe_stability_rejects_eased_or_fast_virtual_camera_motion() {
        let mut final_document = document();
        let curve = |end: i64, interpolation| kinewright_core::AutomationCurve {
            keyframes: vec![
                kinewright_core::Keyframe {
                    at: TimeCode::ZERO,
                    value: 50,
                    interpolation,
                },
                kinewright_core::Keyframe {
                    at: TimeCode(12),
                    value: end,
                    interpolation,
                },
            ],
        };
        final_document.tracks[0].clips[0]
            .effects
            .push(kinewright_core::Effect {
                id: kinewright_core::EffectId(1),
                name: "reframe".to_owned(),
                parameters: BTreeMap::from([(
                    "target_aspect_basis_points".to_owned(),
                    ParamValue::Integer(5_625),
                )]),
                keyframes: BTreeMap::from([
                    (
                        "focus_x_percent".to_owned(),
                        curve(58, kinewright_core::KeyframeInterpolation::EaseInOut),
                    ),
                    (
                        "focus_y_percent".to_owned(),
                        curve(50, kinewright_core::KeyframeInterpolation::EaseInOut),
                    ),
                ]),
            });
        final_document.markers.push(provenance_marker(
            1,
            &[
                (0, 4_500, 5_500, 3_500, 6_500),
                (12, 4_500, 5_500, 3_500, 6_500),
                (59, 4_500, 5_500, 3_500, 6_500),
            ],
        ));
        let mut outcome = EvalOutcome {
            final_document,
            original_document: Document::default(),
            color: None,
            final_words: Vec::new(),
            final_timeline_words: Vec::new(),
            remaining_silences: Vec::new(),
            remaining_scenes: Vec::new(),
            context: FixtureContext::default(),
            session: SessionMetrics::default(),
            operations: Vec::new(),
            undo_steps_to_original: None,
        };

        let rejected = evaluate_reframe_stability(TrackId(1), 2, 25..=75, 20..=80, 2, &outcome);
        assert!(!rejected.passed);
        assert!(rejected.detail.contains("not linearly interpolated"));
        assert!(rejected.detail.contains("jumps 8 percent"));

        let effect = &mut outcome.final_document.tracks[0].clips[0].effects[0];
        effect.keyframes.insert(
            "focus_x_percent".to_owned(),
            curve(52, kinewright_core::KeyframeInterpolation::Linear),
        );
        effect.keyframes.insert(
            "focus_y_percent".to_owned(),
            curve(50, kinewright_core::KeyframeInterpolation::Linear),
        );
        assert!(evaluate_reframe_stability(TrackId(1), 2, 25..=75, 20..=80, 2, &outcome).passed);
    }

    #[test]
    fn dialogue_pause_assertion_catches_the_m38_short_boundary() {
        let word = |text: &str, asset: u64, start: i64, end: i64| TimelineTranscriptWord {
            text: text.to_owned(),
            speaker: None,
            asset: AssetId(asset),
            track: TrackId(1),
            clip: ClipId(asset),
            source_start: TimeCode(start),
            source_end: TimeCode(end),
            project_start: TimeCode(start),
            project_end: TimeCode(end),
        };
        let words = vec![
            word("rain", 1, 80, 100),
            word("Neighbors", 1, 112, 130),
            word("beds", 2, 280, 300),
            word("Then", 2, 307, 325),
            word("peppers.", 2, 380, 400),
            word("Now", 3, 412, 430),
        ];

        let rejected =
            evaluate_dialogue_pause_bounds(&words, &[], TimeCode(9), TimeCode(15), TimeCode(4));
        assert!(!rejected.passed);
        assert!(rejected.detail.contains("beds"));
        assert!(rejected.detail.contains("=7"));
    }

    #[test]
    fn exact_caption_words_catch_the_m37_material_error() {
        let mut final_document = document();
        final_document.tracks.push(Track {
            id: TrackId(2),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                id: ClipId(2),
                asset: AssetId::default(),
                source_range: TimeCode::ZERO..TimeCode(30),
                content: ClipContent::Title(CaptionPreset::Social.title("Map Steady the Exped")),
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            }],
        });
        let mut context = FixtureContext::default();
        context.word_sets.insert(
            "authored".to_owned(),
            normalize_word_sequence(["River map steadies the expedition"].into_iter()),
        );
        let mut outcome = EvalOutcome {
            final_document,
            original_document: Document::default(),
            color: None,
            final_words: Vec::new(),
            final_timeline_words: Vec::new(),
            remaining_silences: Vec::new(),
            remaining_scenes: Vec::new(),
            context,
            session: SessionMetrics::default(),
            operations: Vec::new(),
            undo_steps_to_original: None,
        };

        let rejected = evaluate_caption_words("authored", &outcome);
        assert!(!rejected.passed);
        assert!(rejected.detail.contains("expedition"));
        let ClipContent::Title(title) = &mut outcome.final_document.tracks[1].clips[0].content
        else {
            panic!("caption fixture should be a title");
        };
        title.text = "River map steadies the expedition".to_owned();
        assert!(evaluate_caption_words("authored", &outcome).passed);
    }

    #[test]
    fn caption_sentence_grouping_rejects_crossovers() {
        assert!(caption_contains_sentence_crossover(
            "and rainwater. Neighbors decided it could feed"
        ));
        assert!(!caption_contains_sentence_crossover(
            "Last spring this empty lot collected weeds and rainwater."
        ));
        assert!(!caption_contains_sentence_crossover(
            "Over three weekends volunteers"
        ));
    }

    #[test]
    fn rendered_dialogue_wer_uses_ordered_edits_and_rounds_up() {
        let expected = normalize_word_sequence(["river map steadies the expedition"].into_iter());
        let one_substitution =
            normalize_word_sequence(["river map steadies an expedition"].into_iter());
        let reordered = normalize_word_sequence(["map river steadies the expedition"].into_iter());

        assert_eq!(word_sequence_edit_distance(&expected, &one_substitution), 1);
        assert_eq!(word_error_rate_basis_points(1, expected.len()), 2_000);
        assert_eq!(word_sequence_edit_distance(&expected, &reordered), 2);
        assert_eq!(word_error_rate_basis_points(1, 3), 3_334);
    }

    #[test]
    fn encoded_audio_tail_rejects_loud_active_audio() {
        let measurement = AudioLoudness {
            integrated_lufs_hundredths: Some(-1_600),
            sample_peak_dbfs_hundredths: Some(-99),
            sample_rate: 48_000,
            channels: 2,
            sample_frames: 4_800,
        };

        assert!(!audio_tail_peak_passes(&measurement, -100));
    }

    #[test]
    fn encoded_audio_tail_accepts_quiet_and_silent_audio() {
        let quiet = AudioLoudness {
            integrated_lufs_hundredths: Some(-1_600),
            sample_peak_dbfs_hundredths: Some(-101),
            sample_rate: 48_000,
            channels: 2,
            sample_frames: 4_800,
        };
        let silent = AudioLoudness {
            integrated_lufs_hundredths: None,
            sample_peak_dbfs_hundredths: None,
            sample_rate: 48_000,
            channels: 2,
            sample_frames: 4_800,
        };

        assert!(audio_tail_peak_passes(&quiet, -100));
        assert!(audio_tail_peak_passes(&silent, -100));
        assert!(!audio_activity_loudness_passes(&quiet, -100));
        assert!(!audio_activity_loudness_passes(&silent, -100));
        assert!(audio_activity_loudness_passes(&quiet, -2_000));
    }

    #[test]
    fn encoded_audio_tail_requires_a_positive_bounded_window() {
        let invalid = EvalAudioTailSpec {
            terminal_window_frames: TimeCode::ZERO,
            maximum_sample_peak_dbfs_hundredths: -100,
            activity_window_frames: TimeCode(5),
            minimum_active_integrated_lufs_hundredths: -3_000,
            maximum_trailing_inactive_frames: TimeCode(30),
        };
        let oversized = EvalAudioTailSpec {
            terminal_window_frames: TimeCode(61),
            maximum_sample_peak_dbfs_hundredths: -100,
            activity_window_frames: TimeCode(5),
            minimum_active_integrated_lufs_hundredths: -3_000,
            maximum_trailing_inactive_frames: TimeCode(30),
        };

        assert!(
            audio_tail_range(TimeCode(60), invalid)
                .unwrap_err()
                .contains("positive")
        );
        assert!(
            audio_tail_range(TimeCode(60), oversized)
                .unwrap_err()
                .contains("exceeds")
        );
    }

    #[test]
    fn encoded_audio_tail_document_is_audio_only_and_uses_the_exact_range() {
        let source = music_document();
        let asset = &source.media_pool[0];
        let tail = audio_tail_document(asset, TimeCode(90)..TimeCode(120));

        assert_eq!(tail.duration, TimeCode(30));
        assert_eq!(tail.media_pool, vec![asset.clone()]);
        assert_eq!(tail.tracks.len(), 1);
        assert_eq!(tail.tracks[0].kind, TrackKind::Audio);
        assert_eq!(tail.tracks[0].clips.len(), 1);
        assert_eq!(
            tail.tracks[0].clips[0].source_range,
            TimeCode(90)..TimeCode(120)
        );
        assert_eq!(tail.tracks[0].clips[0].timeline_start, TimeCode::ZERO);
    }

    #[test]
    fn required_encoded_audio_tail_has_an_explicit_failure_assertion() {
        let deliverable = deliverable_shell(
            EvalDeliverableSpec {
                profile: DeliveryProfile::VerticalShort,
                focus_x_percent: 50,
                focus_y_percent: 50,
                proof_frames: 9,
                proof_cell_width: 240,
                require_audio: true,
                expected_transcript_word_set: None,
                maximum_word_error_rate_basis_points: 0,
                maximum_caption_word_error_rate_basis_points: None,
                loudness: None,
                audio_tail: Some(EvalAudioTailSpec {
                    terminal_window_frames: TimeCode(30),
                    maximum_sample_peak_dbfs_hundredths: -100,
                    activity_window_frames: TimeCode(5),
                    minimum_active_integrated_lufs_hundredths: -3_000,
                    maximum_trailing_inactive_frames: TimeCode(30),
                }),
                delivery_bit_depth: DeliveryEncodeDepth::Eight,
            },
            &document(),
            Path::new("artifacts/audio-tail"),
        );
        let assertion = deliverable_assertions(&deliverable)
            .into_iter()
            .find(|assertion| assertion.assertion == "encoded audio tail")
            .expect("required encoded audio-tail assertion");

        assert!(!assertion.passed);
        assert!(assertion.detail.contains("unavailable"));
    }

    #[test]
    fn finished_deliverable_requires_a_passing_encoded_audio_tail() {
        let spec = EvalDeliverableSpec {
            profile: DeliveryProfile::VerticalShort,
            focus_x_percent: 50,
            focus_y_percent: 50,
            proof_frames: 9,
            proof_cell_width: 240,
            require_audio: true,
            expected_transcript_word_set: None,
            maximum_word_error_rate_basis_points: 0,
            maximum_caption_word_error_rate_basis_points: None,
            loudness: None,
            audio_tail: Some(EvalAudioTailSpec {
                terminal_window_frames: TimeCode(30),
                maximum_sample_peak_dbfs_hundredths: -100,
                activity_window_frames: TimeCode(5),
                minimum_active_integrated_lufs_hundredths: -3_000,
                maximum_trailing_inactive_frames: TimeCode(30),
            }),
            delivery_bit_depth: DeliveryEncodeDepth::Eight,
        };
        let document = document();
        let existing_file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
        let mut deliverable = deliverable_shell(spec, &document, Path::new("artifacts/audio-tail"));
        deliverable.output_path = existing_file.clone();
        deliverable.document_path = existing_file.clone();
        deliverable.proof_path = existing_file;
        deliverable.conformance = Some(DeliveryConformanceReport {
            profile: DeliveryProfile::VerticalShort,
            delivery_bit_depth: DeliveryEncodeDepth::Eight,
            container: "mp4".to_owned(),
            resolution: (1_080, 1_920),
            delivery_color: document.color_context.delivery.clone(),
            video_codec: "h264".to_owned(),
            audio_codec: "aac".to_owned(),
            video_bitrate: 1,
            audio_bitrate: 1,
            issues: Vec::new(),
        });
        deliverable.output_bytes = Some(1);
        deliverable.output_sha256 = Some("a".repeat(64));
        deliverable.exported_frames = Some(60);
        deliverable.probed_resolution = Some((1_080, 1_920));
        deliverable.probed_duration_frames = Some(TimeCode(60));
        deliverable.probed_media_kind = Some(MediaKind::AudioVideo);
        deliverable.proof_sample_frames = vec![TimeCode::ZERO];
        deliverable.rendered_audio_tail = Some(RenderedAudioTailVerification {
            tail_start_frame: TimeCode(30),
            tail_end_frame: TimeCode(60),
            measurement: AudioLoudness {
                integrated_lufs_hundredths: Some(-1_600),
                sample_peak_dbfs_hundredths: Some(-99),
                sample_rate: 48_000,
                channels: 2,
                sample_frames: 4_800,
            },
            terminal_window_frames: TimeCode(30),
            maximum_sample_peak_dbfs_hundredths: -100,
            activity_window_frames: TimeCode(5),
            minimum_active_integrated_lufs_hundredths: -3_000,
            maximum_trailing_inactive_frames: TimeCode(30),
            observed_trailing_inactive_frames: TimeCode(35),
            latest_active_window_start_frame: None,
            latest_active_window_end_frame: None,
            passed: false,
        });

        assert!(!finish_deliverable(deliverable.clone()).machine_passed);
        deliverable
            .rendered_audio_tail
            .as_mut()
            .expect("tail verification")
            .passed = true;
        assert!(finish_deliverable(deliverable).machine_passed);
    }

    #[test]
    fn required_rendered_transcript_has_an_explicit_failure_assertion() {
        let mut deliverable = deliverable_shell(
            EvalDeliverableSpec {
                profile: DeliveryProfile::VerticalShort,
                focus_x_percent: 50,
                focus_y_percent: 50,
                proof_frames: 9,
                proof_cell_width: 240,
                require_audio: true,
                expected_transcript_word_set: Some("authored"),
                maximum_word_error_rate_basis_points: 1_500,
                maximum_caption_word_error_rate_basis_points: None,
                loudness: None,
                audio_tail: None,
                delivery_bit_depth: DeliveryEncodeDepth::Eight,
            },
            &document(),
            Path::new("artifacts/f2"),
        );
        deliverable
            .errors
            .push("post-render transcription failed deliberately".to_owned());
        let assertions = deliverable_assertions(&deliverable);
        let rendered = assertions
            .iter()
            .find(|assertion| assertion.assertion == "rendered dialogue accuracy")
            .expect("required rendered transcript assertion");
        assert!(!rendered.passed);
        assert!(rendered.detail.contains("unavailable"));
    }

    #[test]
    fn rendered_caption_alignment_detects_words_missing_from_the_screen() {
        let expected = "river map steadies the expedition"
            .split_whitespace()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let observed = "map steady the exped"
            .split_whitespace()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let verification = verify_word_sequences(&expected, &observed, 500);
        assert!(!verification.passed);
        assert!(verification.word_error_rate_basis_points > 500);
        assert!(!verification.missing_words.is_empty());
    }

    #[test]
    fn caption_sentence_checks_reject_the_first_interview_attempt_failures() {
        assert!(caption_contains_capitalized_sentence_crossover(
            "movies and I've been cleaning They"
        ));
        assert!(caption_ends_with_dangling_word(
            "But recently I was living in New Orleans and"
        ));
        assert!(caption_starts_likely_sentence(
            "And I've been cleaning them"
        ));
        assert!(!caption_ends_sentence("submerged in those floodwaters"));
        assert!(caption_boundary_breaks_phrase(
            "But recently I was living in New",
            "Orleans, and my house flooded,"
        ));
        assert!(caption_boundary_breaks_phrase("and a lot", "of my films,"));
    }

    #[test]
    fn scoreboard_and_jsonl_have_stable_machine_readable_shapes() {
        let definition = EvalDefinition {
            name: "fake-eval",
            rationale: "exercise reporting",
            fixture_builder: unused_fixture,
            prompts: &["edit it"],
            assertions: Vec::new(),
            budgets: budgets(),
            deliverable: None,
            color: None,
        };
        let mut result =
            EvalResult::execution_failure(&definition, &EvalError::Agent("deliberate".to_owned()));
        result.cost_usd = Some(0.0);
        let scoreboard = render_scoreboard(std::slice::from_ref(&result));
        assert!(scoreboard.contains("| fake-eval | FAIL |"));
        assert!(scoreboard.contains("| **TOTAL** | **FAIL** |"));

        let environment = EnvironmentStamp {
            timestamp_utc: "2026-08-10T12:00:00Z".to_owned(),
            timestamp_unix_ms: 0,
            harness: "fake".to_owned(),
            harness_version: Some("1.0".to_owned()),
            model: "fake-model".to_owned(),
            os: "test".to_owned(),
            architecture: "test".to_owned(),
            kinewright_version: "0.1.0".to_owned(),
        };
        let jsonl = render_jsonl(&environment, &[result]).unwrap();
        let records = jsonl
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0]["record_type"], "environment");
        assert_eq!(records[1]["record_type"], "eval_result");
        assert_eq!(records[2]["record_type"], "totals");
        assert_eq!(records[1]["result"]["name"], "fake-eval");
    }

    #[test]
    fn utc_timestamp_conversion_covers_the_milestone_date() {
        assert_eq!(format_utc_timestamp(1_786_291_200), "2026-08-09T16:00:00Z");
    }

    #[test]
    fn human_review_stays_pending_until_a_complete_person_score_exists() {
        let definition = EvalDefinition {
            name: "f1 finished cut",
            rationale: "review fixture",
            fixture_builder: unused_fixture,
            prompts: &["finish it"],
            assertions: Vec::new(),
            budgets: budgets(),
            deliverable: None,
            color: None,
        };
        let mut result =
            EvalResult::execution_failure(&definition, &EvalError::Agent("unused".to_owned()));
        let mut deliverable = deliverable_shell(
            EvalDeliverableSpec {
                profile: DeliveryProfile::VerticalShort,
                focus_x_percent: 50,
                focus_y_percent: 50,
                proof_frames: 9,
                proof_cell_width: 240,
                require_audio: true,
                expected_transcript_word_set: None,
                maximum_word_error_rate_basis_points: 0,
                maximum_caption_word_error_rate_basis_points: None,
                loudness: None,
                audio_tail: None,
                delivery_bit_depth: DeliveryEncodeDepth::Eight,
            },
            &document(),
            Path::new("artifacts/f1"),
        );
        deliverable.output_sha256 = Some("a".repeat(64));
        result.deliverable = Some(deliverable);

        let mut caption_required_result = result.clone();
        caption_required_result
            .deliverable
            .as_mut()
            .expect("caption fixture should have a deliverable")
            .rendered_caption_alignment_required = true;
        let sampled_review = human_review_template(
            "finished-v2",
            "run-sampled",
            &[result.clone(), caption_required_result],
        );
        assert_eq!(sampled_review.tasks[0].task_id, "f1");
        assert_eq!(sampled_review.tasks[1].task_id, "f1-sample-2");
        assert_eq!(
            sampled_review.tasks[0].not_applicable,
            vec![HumanRatingDimension::Captions]
        );
        assert!(sampled_review.tasks[1].not_applicable.is_empty());

        let mut review = human_review_template("finished-v2", "run-1", &[result]);
        let pending = summarize_human_review(&review).unwrap();
        assert_eq!(pending.tasks_reviewed, 0);
        assert_eq!(pending.tasks_pending, 1);
        assert_eq!(pending.acceptance_rate, None);

        review.reviewer = Some("human".to_owned());
        review.tasks[0].accepted = Some(true);
        review.tasks[0].not_applicable.clear();
        review.tasks[0].ratings = HumanRatings {
            story: Some(4.0),
            pacing: Some(2.5),
            visual_finish: Some(5.0),
            audio_finish: Some(4.0),
            captions: Some(5.0),
            delivery_readiness: Some(4.0),
        };
        let scored = summarize_human_review(&review).unwrap();
        assert_eq!(scored.tasks_reviewed, 1);
        assert_eq!(scored.tasks_accepted, 1);
        assert_eq!(scored.acceptance_rate, Some(1.0));
        assert_eq!(scored.mean_ratings.pacing, Some(2.5));
        assert_eq!(scored.overall_mean_rating, Some(24.5 / 6.0));
    }

    #[test]
    fn human_review_rejects_partial_or_out_of_range_scores() {
        let mut review = HumanReviewFile {
            schema_version: 1,
            benchmark_id: "finished-v2".to_owned(),
            run_id: "run-1".to_owned(),
            reviewer: None,
            tasks: vec![HumanTaskReview {
                task_id: "f1".to_owned(),
                blind_id: None,
                questions: Vec::new(),
                artifact_sha256: Some("b".repeat(64)),
                accepted: Some(false),
                ratings: HumanRatings {
                    story: Some(0.5),
                    ..HumanRatings::default()
                },
                not_applicable: Vec::new(),
                notes: None,
            }],
        };
        assert!(summarize_human_review(&review).is_err());
        review.tasks[0].ratings = HumanRatings {
            story: Some(1.0),
            pacing: Some(2.0),
            visual_finish: Some(3.0),
            audio_finish: Some(4.0),
            captions: Some(5.0),
            delivery_readiness: Some(6.0),
        };
        assert!(summarize_human_review(&review).is_err());

        review.tasks[0].ratings.delivery_readiness = Some(4.25);
        assert!(summarize_human_review(&review).is_err());
    }

    #[test]
    fn human_review_allows_captions_to_be_not_applicable_without_awarding_a_score() {
        let review = HumanReviewFile {
            schema_version: 1,
            benchmark_id: "finished-v2".to_owned(),
            run_id: "run-1".to_owned(),
            reviewer: Some("human".to_owned()),
            tasks: vec![HumanTaskReview {
                task_id: "g3".to_owned(),
                blind_id: None,
                questions: Vec::new(),
                artifact_sha256: None,
                accepted: Some(true),
                ratings: HumanRatings {
                    story: Some(4.0),
                    pacing: Some(4.0),
                    visual_finish: Some(4.0),
                    audio_finish: Some(4.0),
                    captions: None,
                    delivery_readiness: Some(4.0),
                },
                not_applicable: vec![HumanRatingDimension::Captions],
                notes: None,
            }],
        };
        let summary = summarize_human_review(&review).unwrap();
        assert_eq!(summary.tasks_reviewed, 1);
        assert_eq!(summary.mean_ratings.captions, None);
        assert_eq!(summary.overall_mean_rating, Some(4.0));
    }

    #[test]
    fn legacy_human_review_json_defaults_to_all_applicable_dimensions() {
        let json = r#"
        {
          "schema_version": 1,
          "benchmark_id": "finished-v2",
          "run_id": "run-1",
          "reviewer": "human",
          "tasks": [{
            "task_id": "g1",
            "artifact_sha256": null,
            "accepted": true,
            "ratings": {
              "story": 4.0,
              "pacing": 3.5,
              "visual_finish": 4.0,
              "audio_finish": 4.5,
              "captions": 3.0,
              "delivery_readiness": 4.0
            },
            "notes": null
          }]
        }
        "#;
        let review = serde_json::from_str::<HumanReviewFile>(json).unwrap();
        assert!(review.tasks[0].not_applicable.is_empty());
        let summary = summarize_human_review(&review).unwrap();
        assert_eq!(summary.tasks_reviewed, 1);
        assert_eq!(summary.mean_ratings.captions, Some(3.0));
    }

    #[test]
    fn human_review_rejects_not_applicable_overlap_and_duplicates() {
        let mut review = HumanReviewFile {
            schema_version: 1,
            benchmark_id: "finished-v2".to_owned(),
            run_id: "run-1".to_owned(),
            reviewer: None,
            tasks: vec![HumanTaskReview {
                task_id: "g3".to_owned(),
                blind_id: None,
                questions: Vec::new(),
                artifact_sha256: None,
                accepted: Some(true),
                ratings: HumanRatings {
                    story: Some(4.0),
                    pacing: Some(4.0),
                    visual_finish: Some(4.0),
                    audio_finish: Some(4.0),
                    captions: Some(4.0),
                    delivery_readiness: Some(4.0),
                },
                not_applicable: vec![HumanRatingDimension::Captions],
                notes: None,
            }],
        };
        let overlap = summarize_human_review(&review).unwrap_err().to_string();
        assert!(overlap.contains("both rated and not applicable"));

        review.tasks[0].ratings.captions = None;
        review.tasks[0].not_applicable = vec![
            HumanRatingDimension::Captions,
            HumanRatingDimension::Captions,
        ];
        let duplicate = summarize_human_review(&review).unwrap_err().to_string();
        assert!(duplicate.contains("more than once"));
    }

    #[test]
    fn human_review_rejects_partial_not_applicable_decisions() {
        let review = HumanReviewFile {
            schema_version: 1,
            benchmark_id: "finished-v2".to_owned(),
            run_id: "run-1".to_owned(),
            reviewer: None,
            tasks: vec![HumanTaskReview {
                task_id: "g3".to_owned(),
                blind_id: None,
                questions: Vec::new(),
                artifact_sha256: None,
                accepted: Some(true),
                ratings: HumanRatings {
                    story: Some(4.0),
                    pacing: Some(4.0),
                    visual_finish: Some(4.0),
                    audio_finish: Some(4.0),
                    captions: None,
                    delivery_readiness: Some(4.0),
                },
                not_applicable: Vec::new(),
                notes: None,
            }],
        };
        let error = summarize_human_review(&review).unwrap_err().to_string();
        assert!(error.contains("missing"));
        assert!(error.contains("captions"));
    }

    #[test]
    fn proof_sampling_is_uniform_and_includes_both_visible_edges() {
        assert_eq!(
            uniform_sample_frames(TimeCode(10), 4),
            vec![TimeCode(0), TimeCode(3), TimeCode(6), TimeCode(9)]
        );
        assert_eq!(uniform_sample_frames(TimeCode(10), 1), vec![TimeCode(4)]);
    }

    // -----------------------------------------------------------------------
    // CC7 §11.2.32 — the shared-runner half of the eval inventory.
    // -----------------------------------------------------------------------

    fn cc7_result_without_measurements() -> EvalResult {
        EvalResult {
            name: "g3 mixed footage".to_owned(),
            rationale: "a pre-CC7 record".to_owned(),
            passed: true,
            assertions: vec![AssertionResult {
                assertion: "timeline non-empty".to_owned(),
                passed: true,
                detail: "observed 3 clips".to_owned(),
            }],
            measurements: Vec::new(),
            turns: 1,
            tool_calls: BTreeMap::from([("commit_edit_plan".to_owned(), 2_u32)]),
            input_tokens: 10,
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            output_tokens: 20,
            reasoning_output_tokens: None,
            tool_surface: crate::ToolSurfaceMetrics::default(),
            cost_usd: Some(0.25),
            wall_time_ms: 1_234,
            operations_applied: 2,
            deliverable: None,
            execution_error: None,
        }
    }

    /// `EvalResult` derives `Serialize` only and `results.jsonl` is
    /// write-only, so there is no "a pre-CC7 record still parses" claim to
    /// make. What is guaranteed is that a record with no measurements still
    /// serialises byte for byte as it did before the field existed — which is
    /// what keeps every checked-in baseline JSON and every historic JSONL
    /// line exactly the shape it was recorded in.
    #[test]
    fn cc7_a_v5_result_serialises_byte_identically_without_measurements() {
        let result = cc7_result_without_measurements();
        let json = serde_json::to_string(&result).expect("v5-shaped result serialises");
        assert!(
            !json.contains("measurements"),
            "an empty measurement list must not reach the wire: {json}"
        );

        // The same record, with a measurement, does carry the key — so the
        // absence above is `skip_serializing_if` doing its job rather than a
        // field that never serialises at all.
        let mut measured = cc7_result_without_measurements();
        measured.measurements.push(EvalMeasurement {
            name: "neutral patch spread".to_owned(),
            observed: 2,
            budget: 5,
            unit: "monitoring_code".to_owned(),
            passed: true,
        });
        let measured_json = serde_json::to_string(&measured).expect("measured result serialises");
        assert!(measured_json.contains("\"measurements\""));
        assert!(measured_json.contains("\"monitoring_code\""));

        // And every other key is untouched: removing the measurements
        // recovers the original bytes exactly.
        measured.measurements.clear();
        assert_eq!(
            serde_json::to_string(&measured).expect("cleared result serialises"),
            json
        );
    }

    fn cc7_keyframed_document(parameter: &str, frames: &[i64]) -> Document {
        let mut document = document();
        let mut effect = kinewright_core::Effect {
            id: kinewright_core::EffectId(7),
            name: "primary_correction".to_owned(),
            parameters: BTreeMap::from([
                ("saturation_percent".to_owned(), ParamValue::Integer(40)),
                (MATTE_ENABLED_PARAMETER.to_owned(), ParamValue::Integer(1)),
            ]),
            keyframes: BTreeMap::new(),
        };
        effect.keyframes.insert(
            parameter.to_owned(),
            kinewright_core::AutomationCurve {
                keyframes: frames
                    .iter()
                    .map(|frame| kinewright_core::Keyframe {
                        at: TimeCode(*frame),
                        value: 5_000,
                        interpolation: kinewright_core::KeyframeInterpolation::default(),
                    })
                    .collect(),
            },
        );
        document.tracks[0].clips[0].effects.push(effect);
        document
    }

    /// The committed keyframes are the durable evidence of what the tracker
    /// did, which is why this variant replaces a tool-log assertion: there is
    /// no per-call tool log in `SessionMetrics` and CC7 adds none.
    #[test]
    fn cc7_track_keyframes_match_expected_reads_the_committed_document() {
        let parameter = "matte_window0_center_x_basis_points";
        let surviving = [0_i64, 4, 9, 14, 18, 23, 28, 32, 37, 42];
        let assertion = EvalAssertion::TrackKeyframesMatchExpected {
            parameter: parameter.to_owned(),
            expected_local_frames: surviving.to_vec(),
            absent_local_frames: vec![47],
        };

        let outcome = outcome_for(
            cc7_keyframed_document(parameter, &surviving),
            FixtureContext::default(),
        );
        let landed = color_assertion_outcome(&assertion, &outcome);
        assert!(landed.result.passed, "{}", landed.result.detail);
        assert_eq!(landed.measurement.observed, 10);
        assert_eq!(landed.measurement.budget, 10);
        assert_eq!(landed.measurement.unit, "keyframes");
        assert!(landed.measurement.passed);

        // A document that also keyframed the occluded sample fails, so the
        // absent list is load-bearing rather than decorative.
        let mut with_occluded = surviving.to_vec();
        with_occluded.push(47);
        let leaked = outcome_for(
            cc7_keyframed_document(parameter, &with_occluded),
            FixtureContext::default(),
        );
        let leaked = color_assertion_outcome(&assertion, &leaked);
        assert!(!leaked.result.passed);
        assert!(leaked.result.detail.contains("unexpectedly present [47]"));

        // And a document that dropped a surviving sample fails too.
        let dropped = outcome_for(
            cc7_keyframed_document(parameter, &surviving[..9]),
            FixtureContext::default(),
        );
        let dropped = color_assertion_outcome(&assertion, &dropped);
        assert!(!dropped.result.passed);
        assert_eq!(dropped.measurement.observed, 9);
    }

    /// A colour assertion with no evidence block reads as "not measured" and
    /// fails; it never passes by default.
    #[test]
    fn cc7_a_colour_assertion_without_evidence_fails_rather_than_passing() {
        let outcome = outcome_for(document(), FixtureContext::default());
        assert!(outcome.color.is_none());
        for assertion in [
            EvalAssertion::NeutralPatchSpreadAtMost {
                patch_rois: vec![NormalizedRoi::new(0, 2_000, 3_000, 888)],
                maximum_code: 5,
            },
            EvalAssertion::SkinHueWithinBand {
                roi: NormalizedRoi::new(0, 4_223, 1_500, 888),
                minimum_in_band_basis_points: 10_000,
            },
            EvalAssertion::LookBypassMatchesAbsent {
                clip_id: 1,
                effect_id: 7,
                frame: 0,
            },
            EvalAssertion::DeliveryVerificationWithinBudgets {
                depth: DeliveryEncodeDepth::Eight,
            },
            EvalAssertion::ColorQcTechnicalPass {
                clip_id: 1,
                frame: 0,
                checks: vec!["range".to_owned()],
            },
            EvalAssertion::MatteContainmentExact {
                roi: NormalizedRoi::new(1_500, 4_223, 375, 888),
                expected_covered_pixel_count: 192,
                expected_full_pixel_count: 192,
                expected_partial_pixel_count: 0,
            },
        ] {
            let landed = color_assertion_outcome(&assertion, &outcome);
            assert!(
                !landed.result.passed,
                "{} passed with no evidence: {}",
                landed.result.assertion, landed.result.detail
            );
            assert!(color_measurement(&assertion, &outcome).is_some());
        }

        // A partially measured quantity is not a measurement (R1-M3): nine of
        // twelve patches measuring 2 codes against a budget of 5 is a pass
        // until the three that did not resolve are on the record, at which
        // point the claim fails with the reason it could not be made.
        let spread = EvalAssertion::NeutralPatchSpreadAtMost {
            patch_rois: vec![NormalizedRoi::new(0, 2_000, 3_000, 888)],
            maximum_code: 5,
        };
        let measured = ColorEvalEvidence {
            neutral_spread_max_code: Some(2),
            ..ColorEvalEvidence::default()
        };
        let passing = EvalOutcome {
            color: Some(measured.clone()),
            ..outcome_for(document(), FixtureContext::default())
        };
        assert!(color_assertion_outcome(&spread, &passing).result.passed);

        let mut partial = measured;
        partial.record(
            ColorEvidenceQuantity::NeutralSpread,
            "the neutral patch chart09 resolved to no pixel".to_owned(),
        );
        // An error on a different quantity's inputs leaves this claim alone.
        partial.record(
            ColorEvidenceQuantity::Matte,
            "matte proof failed: not implemented".to_owned(),
        );
        let failing = EvalOutcome {
            color: Some(partial),
            ..outcome_for(document(), FixtureContext::default())
        };
        let landed = color_assertion_outcome(&spread, &failing);
        assert!(!landed.result.passed, "{}", landed.result.detail);
        assert!(
            landed.result.detail.contains("chart09"),
            "{}",
            landed.result.detail
        );
        assert!(
            !landed.result.detail.contains("matte proof"),
            "a matte failure must not be reported as the spread's reason: {}",
            landed.result.detail
        );
        assert!(!landed.measurement.passed);
    }

    /// Every colour assertion emits a measurement; no other assertion does.
    #[test]
    fn cc7_only_colour_assertions_emit_measurements() {
        let outcome = outcome_for(document(), FixtureContext::default());
        assert!(color_measurement(&EvalAssertion::TimelineNonEmpty, &outcome).is_none());
        assert!(color_measurement(&EvalAssertion::UndoIntegrity, &outcome).is_none());
        assert!(
            color_measurement(
                &EvalAssertion::ReferenceClipUntouched { clip_id: 1 },
                &outcome
            )
            .is_some()
        );
    }

    /// The request is derived from the assertions that gate it, so a region
    /// rectangle is written down once and a non-colour suite gets no request
    /// at all.
    #[test]
    fn cc7_a_colour_request_is_derived_from_the_assertions_only() {
        assert!(
            ColorEvalRequest::from_assertions(&[
                EvalAssertion::TimelineNonEmpty,
                EvalAssertion::UndoIntegrity
            ])
            .is_none()
        );
        let roi = NormalizedRoi::new(1_500, 4_223, 375, 888);
        let request = ColorEvalRequest::from_assertions(&[
            EvalAssertion::TimelineNonEmpty,
            EvalAssertion::ColorQcTechnicalPass {
                clip_id: 1,
                frame: 60,
                checks: vec!["range".to_owned(), "per_node".to_owned()],
            },
            EvalAssertion::MatteContainmentExact {
                roi,
                expected_covered_pixel_count: 192,
                expected_full_pixel_count: 192,
                expected_partial_pixel_count: 0,
            },
            EvalAssertion::DeliveryVerificationWithinBudgets {
                depth: DeliveryEncodeDepth::Ten,
            },
            EvalAssertion::TrackKeyframesMatchExpected {
                parameter: "matte_window0_center_x_basis_points".to_owned(),
                expected_local_frames: vec![0],
                absent_local_frames: vec![47],
            },
        ])
        .expect("a colour assertion yields a request");
        assert_eq!(request.project_frame, 60);
        assert_eq!(
            request.qc_checks,
            vec![ColorQcCheck::Range, ColorQcCheck::PerNode]
        );
        assert_eq!(request.matte_roi, Some(roi));
        assert_eq!(
            request.delivery_verification,
            Some(DeliveryEncodeDepth::Ten)
        );
        assert_eq!(
            request.keyframe_parameters,
            vec!["matte_window0_center_x_basis_points".to_owned()]
        );
    }

    fn cc7_reviewed_task(task_id: &str) -> HumanTaskReview {
        HumanTaskReview {
            task_id: task_id.to_owned(),
            blind_id: None,
            artifact_sha256: Some("a".repeat(64)),
            accepted: Some(true),
            ratings: HumanRatings {
                story: Some(4.0),
                pacing: Some(4.0),
                visual_finish: Some(4.5),
                audio_finish: Some(4.0),
                captions: Some(4.0),
                delivery_readiness: Some(4.5),
            },
            not_applicable: Vec::new(),
            questions: Vec::new(),
            notes: None,
        }
    }

    /// A schema version 1 file loads and scores exactly as it always has, and
    /// round-trips byte-identically through the version 2 code.
    #[test]
    fn cc7_human_review_v1_files_still_load_and_score() {
        let v1 = serde_json::json!({
            "schema_version": 1,
            "benchmark_id": "kinewright-finished-cut-v2",
            "run_id": "kinewright-eval-20260101T000000Z-claude-code",
            "reviewer": "riel",
            "tasks": [{
                "task_id": "f1",
                "artifact_sha256": "b".repeat(64),
                "accepted": true,
                "ratings": {
                    "story": 4.0, "pacing": 4.0, "visual_finish": 4.5,
                    "audio_finish": 4.0, "captions": 4.0, "delivery_readiness": 4.5
                },
                "not_applicable": [],
                "notes": null
            }]
        });
        let bytes = serde_json::to_vec(&v1).expect("v1 fixture serialises");
        let review: HumanReviewFile =
            serde_json::from_slice(&bytes).expect("a v1 review still parses");
        assert_eq!(review.schema_version, 1);
        assert_eq!(review.tasks[0].blind_id, None);
        assert!(review.tasks[0].questions.is_empty());

        let summary = summarize_human_review(&review).expect("a v1 review still scores");
        assert_eq!(summary.tasks_reviewed, 1);
        assert_eq!(summary.tasks_accepted, 1);

        // Byte-identical round trip: neither new field reaches the wire.
        let round_tripped = serde_json::to_value(&review).expect("v1 review re-serialises");
        assert_eq!(round_tripped, v1);
    }

    /// A schema version 2 file carries `blind_id` and `questions` and round
    /// trips through them.
    #[test]
    fn cc7_human_review_v2_round_trips_blind_id_and_questions() {
        let mut task = cc7_reviewed_task("c1");
        task.blind_id = Some("0f3a1c2d4e5b".to_owned());
        task.not_applicable = COLOR_WORKFLOW_NOT_APPLICABLE.to_vec();
        task.ratings = HumanRatings {
            visual_finish: Some(4.5),
            delivery_readiness: Some(4.0),
            ..HumanRatings::default()
        };
        task.questions = vec![HumanQuestion {
            id: "a".to_owned(),
            prompt: "Does the match preserve natural and intentional differences?".to_owned(),
            answer: Some(true),
            notes: None,
        }];
        let review = HumanReviewFile {
            schema_version: HUMAN_REVIEW_SCHEMA_VERSION,
            benchmark_id: COLOR_WORKFLOW_BENCHMARK_ID.to_owned(),
            run_id: "kinewright-eval-20260101T000000Z-claude-code".to_owned(),
            reviewer: Some("riel".to_owned()),
            tasks: vec![task],
        };
        let bytes = serde_json::to_vec_pretty(&review).expect("v2 review serialises");
        let parsed: HumanReviewFile = serde_json::from_slice(&bytes).expect("v2 review parses");
        assert_eq!(parsed, review);
        assert_eq!(parsed.schema_version, 2);
        assert_eq!(parsed.tasks[0].blind_id.as_deref(), Some("0f3a1c2d4e5b"));
        assert_eq!(parsed.tasks[0].questions[0].answer, Some(true));

        let summary = summarize_human_review(&parsed).expect("a v2 review scores");
        assert_eq!(summary.tasks_accepted, 1);
        // The four editorial dimensions are excluded rather than fabricated.
        assert_eq!(summary.mean_ratings.story, None);
        assert_eq!(summary.mean_ratings.visual_finish, Some(4.5));

        assert!(
            summarize_human_review(&HumanReviewFile {
                schema_version: 3,
                ..review
            })
            .is_err()
        );
    }

    /// Acceptance requires every question answered, in both directions.
    #[test]
    fn cc7_accepted_requires_every_question_answered() {
        let mut task = cc7_reviewed_task("c5");
        task.questions = vec![HumanQuestion {
            id: "e".to_owned(),
            prompt: "Does the look support the story?".to_owned(),
            answer: None,
            notes: None,
        }];
        let mut review = HumanReviewFile {
            schema_version: HUMAN_REVIEW_SCHEMA_VERSION,
            benchmark_id: COLOR_WORKFLOW_BENCHMARK_ID.to_owned(),
            run_id: "run-1".to_owned(),
            reviewer: None,
            tasks: vec![task],
        };
        let error = summarize_human_review(&review)
            .expect_err("an unanswered question blocks acceptance")
            .to_string();
        assert!(error.contains("question"), "{error}");
        assert!(error.contains("\"e\""), "{error}");

        review.tasks[0].questions[0].answer = Some(false);
        let summary = summarize_human_review(&review).expect("an answered question scores");
        assert_eq!(summary.tasks_reviewed, 1);

        // A pending task with an unanswered question is still merely pending.
        review.tasks[0].accepted = None;
        review.tasks[0].ratings = HumanRatings::default();
        review.tasks[0].questions[0].answer = None;
        let pending = summarize_human_review(&review).expect("a pending task stays pending");
        assert_eq!(pending.tasks_pending, 1);
        assert_eq!(pending.tasks_reviewed, 0);
    }

    /// The blind identifier is a hash prefix, never a random token, so two
    /// identical artefacts share one and the mapping is reproducible.
    #[test]
    fn cc7_blind_ids_are_derived_from_the_artifact_digest() {
        let hash = "0f3a1c2d4e5b".to_owned() + &"9".repeat(52);
        assert_eq!(
            blind_id_for_artifact(Some(&hash)).as_deref(),
            Some("0f3a1c2d4e5b")
        );
        assert_eq!(blind_id_for_artifact(None), None);
        assert_eq!(blind_id_for_artifact(Some("short")), None);
        assert_eq!(blind_id_for_artifact(Some(&"z".repeat(64))), None);
    }

    /// One deterministic analysis double, so the colour measurement path
    /// itself is exercised rather than merely compiled.
    struct Cc7StubAnalysis {
        monitor: BTreeMap<TimeCode, RgbaImage>,
        coverage: RgbaImage,
        working: kinewright_core::LinearRgbaImage,
    }

    impl Cc7StubAnalysis {
        fn frame(&self, at: TimeCode) -> RgbaImage {
            self.monitor
                .range(..=at)
                .next_back()
                .or_else(|| self.monitor.iter().next())
                .map(|(_, image)| image.clone())
                .expect("the stub carries at least one frame")
        }
    }

    impl Analysis for Cc7StubAnalysis {
        // Every fallible method returns `kinewright_core::MediaError`.
        fn probe(
            &self,
            _path: &Path,
        ) -> Result<kinewright_core::MediaAsset, kinewright_core::MediaError> {
            Err(kinewright_core::MediaError::NotImplemented)
        }

        fn thumbnail_at(
            &self,
            at: TimeCode,
            _max_width: u32,
        ) -> Result<RgbaImage, kinewright_core::MediaError> {
            Ok(self.frame(at))
        }

        fn monitor_proof_for_document(
            &self,
            _document: Arc<Document>,
            at: TimeCode,
        ) -> Result<kinewright_core::MonitorProof, kinewright_core::MediaError> {
            Ok(kinewright_core::MonitorProof {
                image: self.frame(at),
                metadata: kinewright_core::MonitorProofMetadata::test_double(),
            })
        }

        fn matte_proof_for_document(
            &self,
            _document: Arc<Document>,
            _at: TimeCode,
            clip: ClipId,
            effect: EffectId,
        ) -> Result<kinewright_core::MatteProof, kinewright_core::MediaError> {
            Ok(kinewright_core::MatteProof {
                metadata: kinewright_core::MatteProofMetadata {
                    render: kinewright_core::MonitorProofMetadata::test_double(),
                    clip,
                    effect,
                    node_kind: "primary_correction".to_owned(),
                    coverage_encoding: kinewright_core::MATTE_COVERAGE_ENCODING.to_owned(),
                    coverage_scale: kinewright_core::MATTE_COVERAGE_SCALE,
                    raster_aspect_millionths: 2_000_000,
                    matte_enabled: true,
                    window_count: 0,
                    qualifier_enabled: true,
                },
                coverage: self.coverage.clone(),
            })
        }

        fn working_proof_for_document(
            &self,
            _document: Arc<Document>,
            _at: TimeCode,
        ) -> Result<kinewright_core::WorkingProof, kinewright_core::MediaError> {
            Ok(kinewright_core::WorkingProof {
                metadata: kinewright_core::WorkingProofMetadata {
                    render: kinewright_core::MonitorProofMetadata::test_double(),
                    stage: kinewright_core::WORKING_PROOF_STAGE.to_owned(),
                    encoding: kinewright_core::WORKING_PROOF_ENCODING.to_owned(),
                    raster_aspect_millionths: 2_000_000,
                },
                image: self.working.clone(),
            })
        }

        fn request_transcription(&self, _asset: kinewright_core::MediaAsset) {}

        fn transcript_status(&self, _asset: &kinewright_core::MediaAsset) -> TranscriptStatus {
            TranscriptStatus::NotRequested
        }

        fn timeline_transcript(
            &self,
            _document: &Document,
            _range: Option<std::ops::Range<TimeCode>>,
        ) -> Result<Vec<TimelineTranscriptWord>, kinewright_core::MediaError> {
            Ok(Vec::new())
        }

        fn request_silence_detection(&self, _asset: kinewright_core::MediaAsset) {}

        fn silence_status(
            &self,
            _asset: &kinewright_core::MediaAsset,
        ) -> kinewright_core::SilenceStatus {
            kinewright_core::SilenceStatus::NotRequested
        }

        fn timeline_silences(
            &self,
            _document: &Document,
            _range: Option<std::ops::Range<TimeCode>>,
            _minimum_source_frames: TimeCode,
        ) -> Result<Vec<TimelineSilenceSpan>, kinewright_core::MediaError> {
            Ok(Vec::new())
        }

        fn request_scene_detection(&self, _asset: kinewright_core::MediaAsset) {}

        fn scene_status(
            &self,
            _asset: &kinewright_core::MediaAsset,
        ) -> kinewright_core::SceneStatus {
            kinewright_core::SceneStatus::NotRequested
        }

        fn timeline_scene_changes(
            &self,
            _document: &Document,
            _range: Option<std::ops::Range<TimeCode>>,
            _minimum_confidence_basis_points: u16,
        ) -> Result<Vec<TimelineSceneChange>, kinewright_core::MediaError> {
            Ok(Vec::new())
        }

        fn request_waveform(
            &self,
            _asset: kinewright_core::MediaAsset,
            _request_generation: u64,
        ) -> bool {
            false
        }

        fn request_thumbnail(
            &self,
            _asset: kinewright_core::MediaAsset,
            _source_at: TimeCode,
            _max_width: u32,
            _request_generation: u64,
        ) -> bool {
            false
        }

        fn visual_asset_results(&self) -> Receiver<kinewright_core::VisualAssetResult> {
            crossbeam_channel::never()
        }
    }

    // `PreparedFixture::new` takes one media handle that plays, analyses and
    // exports, so the analysis double carries the other two surfaces. Neither
    // is exercised by the project-path plumbing; they exist so the fixture
    // the runner builds can be built here without a real media engine, whose
    // process-exit teardown would make this lane flaky (F-E6).
    impl kinewright_core::Playback for Cc7StubAnalysis {
        fn set_document(&self, _document: Arc<Document>) {}
        fn request_frame(&self, _at: TimeCode) {}
        fn frames(&self) -> Receiver<(TimeCode, kinewright_core::FrameTexture)> {
            crossbeam_channel::never()
        }
        fn events(&self) -> Receiver<kinewright_core::MediaEvent> {
            crossbeam_channel::never()
        }
        fn play(&self, _from: TimeCode) {}
        fn pause(&self) {}
        fn seek(&self, _to: TimeCode) {}
        fn position(&self) -> TimeCode {
            TimeCode::ZERO
        }
        fn output_peaks(&self) -> [f32; 2] {
            [0.0, 0.0]
        }
    }

    impl kinewright_core::Export for Cc7StubAnalysis {
        fn export(
            &self,
            _out: &Path,
            _settings: kinewright_core::ExportSettings,
            _progress: kinewright_core::ProgressSink,
        ) -> Result<(), kinewright_core::MediaError> {
            Err(kinewright_core::MediaError::NotImplemented)
        }
    }

    fn cc7_stub_media() -> Cc7StubAnalysis {
        Cc7StubAnalysis {
            monitor: BTreeMap::from([(TimeCode(0), cc7_flat_raster(4, 4, 10))]),
            coverage: cc7_flat_raster(4, 4, 0),
            working: kinewright_core::LinearRgbaImage {
                width: 4,
                height: 4,
                pixels: [0.5, 0.5, 0.5, 1.0].repeat(16),
            },
        }
    }

    /// A 2x2x2 identity `.cube`: the smallest file CC4's parser accepts, so
    /// the import under test is the project-path branch and not a LUT.
    const CC7_IDENTITY_CUBE: &str = "TITLE \"CC7 eval identity\"\nLUT_3D_SIZE 2\n\
         0.0 0.0 0.0\n1.0 0.0 0.0\n0.0 1.0 0.0\n1.0 1.0 0.0\n\
         0.0 0.0 1.0\n1.0 0.0 1.0\n0.0 1.0 1.0\n1.0 1.0 1.0\n";

    async fn cc7_import_lut_asset(
        client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
        cube: &Path,
    ) -> rmcp::model::CallToolResult {
        client
            .call_tool(
                rmcp::model::CallToolRequestParams::new("invoke_capability").with_arguments(
                    serde_json::json!({
                        "name": "import_lut_asset",
                        "arguments": {
                            "expected_revision": 0,
                            "path": cube.display().to_string(),
                        },
                    })
                    .as_object()
                    .expect("the invocation is an object")
                    .clone(),
                ),
            )
            .await
            .expect("the tool call completes")
    }

    /// R-B2, both directions, through the shared runner's own helper.
    ///
    /// `apply_fixture_project_path` is the only reader of
    /// `PreparedFixture::project_path`, and c3 is unsatisfiable without it:
    /// `McpServer::start` begins with a fresh `None` path, so
    /// `import_lut_asset` refuses `project_not_saved` until a fixture hands
    /// one over. Both directions run against a real `McpServer` on a
    /// synthetic core, because the refusal and the acceptance are the
    /// server's, not the harness's.
    #[tokio::test(flavor = "multi_thread")]
    async fn cc7_a_fixture_project_path_reaches_the_server() {
        use rmcp::ServiceExt as _;

        let temporary = kinewright_media::test_support::TempDirectory::new("cc7-eval-project-path");
        let project = temporary.path("cc7-eval.kinewright");
        std::fs::write(&project, b"{}").expect("the saved project file is written");
        let cube = temporary.path("log-inverse.cube");
        std::fs::write(&cube, CC7_IDENTITY_CUBE).expect("the cube is written");

        // Direction one: `project_path: None`, exactly as v1-v5 pass it.
        let unsaved = PreparedFixture::new(
            document(),
            Arc::new(cc7_stub_media()),
            FixtureContext::default(),
            None,
            Vec::new(),
        )
        .expect("the fixture builds");
        assert_eq!(unsaved.project_path, None);
        let server = McpServer::start(
            unsaved.core.clone(),
            Arc::clone(&unsaved.playback),
            Arc::clone(&unsaved.analysis),
        )
        .expect("the server starts");
        apply_fixture_project_path(&server, &unsaved);
        let client = ()
            .serve(rmcp::transport::StreamableHttpClientTransport::from_uri(
                server.endpoint(),
            ))
            .await
            .expect("the client connects");
        let refused = cc7_import_lut_asset(&client, &cube).await;
        assert_eq!(refused.is_error, Some(true));
        assert_eq!(
            refused
                .structured_content
                .as_ref()
                .expect("a typed refusal")["code"],
            "project_not_saved"
        );
        // `tests/mcp_server.rs`'s teardown order: cancel the client before the
        // server goes away, or the transport's stream is left waiting on a
        // server that will never answer.
        client.cancel().await.expect("the client cancels");
        server.shutdown();

        // Direction two: the same fixture, saved. The confirmation is
        // approved beside the awaited call, because `import_lut_asset` blocks
        // on the broker before it reads a byte.
        let saved = PreparedFixture::new(
            document(),
            Arc::new(cc7_stub_media()),
            FixtureContext::default(),
            Some(project.clone()),
            Vec::new(),
        )
        .expect("the fixture builds");
        assert_eq!(saved.project_path.as_deref(), Some(project.as_path()));
        let server = McpServer::start(
            saved.core.clone(),
            Arc::clone(&saved.playback),
            Arc::clone(&saved.analysis),
        )
        .expect("the server starts");
        apply_fixture_project_path(&server, &saved);
        let broker = server.confirmations();
        let approvals = tokio::spawn(async move {
            loop {
                for request in broker.pending_requests() {
                    assert_eq!(request.tool_name, "import_lut_asset");
                    assert!(broker.approve(request.id));
                }
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }
        });
        let client = ()
            .serve(rmcp::transport::StreamableHttpClientTransport::from_uri(
                server.endpoint(),
            ))
            .await
            .expect("the client connects");
        let imported = cc7_import_lut_asset(&client, &cube).await;
        approvals.abort();
        assert_eq!(
            imported.is_error,
            Some(false),
            "{:?}",
            imported.structured_content
        );
        let imported = imported.structured_content.expect("a typed success");
        assert_eq!(imported["lut_asset"]["size"], 2);
        assert_eq!(
            query_document(&saved.core)
                .expect("the document")
                .lut_assets
                .len(),
            1
        );
        client.cancel().await.expect("the client cancels");
        server.shutdown();
    }

    fn cc7_flat_raster(width: u32, height: u32, code: u8) -> RgbaImage {
        RgbaImage {
            width,
            height,
            pixels: [code, code, code, 255].repeat((width * height) as usize),
        }
    }

    /// Left half achromatic, right half deliberately split by six codes.
    fn cc7_split_raster(width: u32, height: u32) -> RgbaImage {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..height {
            for x in 0..width {
                if x < width / 2 {
                    pixels.extend([10, 10, 10, 255]);
                } else {
                    pixels.extend([20, 16, 10, 255]);
                }
            }
        }
        RgbaImage {
            width,
            height,
            pixels,
        }
    }

    /// The patch statistics themselves, on rasters whose answers are
    /// arithmetic rather than rendered.
    #[test]
    fn cc7_patch_statistics_are_taken_on_a_two_pixel_inset() {
        let raster = cc7_split_raster(16, 8);
        let left = NormalizedRoi::new(0, 0, 5_000, 10_000);
        let right = NormalizedRoi::new(5_000, 0, 5_000, 10_000);
        assert_eq!(patch_spread_max_code(&raster, left).unwrap(), Some(0));
        assert_eq!(patch_spread_max_code(&raster, right).unwrap(), Some(6));
        // The left half is uniform code 10, whose BT.709 luma is exactly 10.
        assert_eq!(
            mean_luma_millionths(&raster, left).unwrap(),
            Some(10_000_000)
        );

        // A rectangle too small to inset is measured whole rather than
        // silently emptied.
        let tiny = NormalizedRoi::new(0, 0, 1_250, 2_500);
        assert_eq!(tiny.to_pixels(16, 8).unwrap().width, 2);
        assert_eq!(patch_spread_max_code(&raster, tiny).unwrap(), Some(0));

        assert_eq!(round_half_away_from_zero(0.5), 1);
        assert_eq!(round_half_away_from_zero(-0.5), -1);
        assert_eq!(round_half_away_from_zero(f64::NAN), 0);
    }

    /// Cropping is exact and never inset: the containment counts are counts.
    #[test]
    fn cc7_coverage_cropping_is_exact_and_never_inset() {
        let mut coverage = cc7_flat_raster(16, 8, 0);
        for y in 0..8_usize {
            for x in 8..16_usize {
                let index = (y * 16 + x) * 4;
                coverage.pixels[index] = 255;
                coverage.pixels[index + 1] = 255;
                coverage.pixels[index + 2] = 255;
            }
        }
        let right = NormalizedRoi::new(5_000, 0, 5_000, 10_000);
        let cropped = crop_rgba(&coverage, right).expect("the right half crops");
        assert_eq!((cropped.width, cropped.height), (8, 8));
        let statistics = matte_coverage_statistics(&cropped).expect("coverage statistics");
        assert_eq!(statistics.total_pixel_count, 64);
        assert_eq!(statistics.covered_pixel_count, 64);
        assert_eq!(statistics.full_pixel_count, 64);
        assert_eq!(statistics.partial_pixel_count, 0);
    }

    fn cc7_matte_document() -> Document {
        let mut document = document();
        document.tracks[0].clips[0]
            .effects
            .push(kinewright_core::Effect {
                id: EffectId(3),
                name: "primary_correction".to_owned(),
                parameters: BTreeMap::from([
                    ("saturation_percent".to_owned(), ParamValue::Integer(40)),
                    (MATTE_ENABLED_PARAMETER.to_owned(), ParamValue::Integer(1)),
                ]),
                keyframes: BTreeMap::new(),
            });
        // `bypass` is a colour-node control that `primary_correction` does
        // not declare, so the look leg uses a node that does.
        document.tracks[0].clips[0]
            .effects
            .push(kinewright_core::Effect {
                id: EffectId(4),
                name: "color_wheels".to_owned(),
                parameters: BTreeMap::new(),
                keyframes: BTreeMap::new(),
            });
        document
    }

    /// The measurement runs where the `Analysis` is alive, fills exactly the
    /// quantities the request asked for, leaves the rest `None`, and reaches
    /// `EvalOutcome::color` through the runner's own expression.
    ///
    /// The plumbing is the design (R-B1), so the test drives
    /// `measure_color_block` — the named function `run_eval_with_artifacts`
    /// calls — rather than a copy of it, and asserts the `None` direction a
    /// v1-v5 definition takes through the same call.
    // One measurement, both plumbing directions and the two ungated rows: the
    // body is long because each is a separate claim about the same call, not
    // because the test does several things.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn cc7_color_evidence_is_computed_where_the_analysis_is_alive() {
        let mut coverage = cc7_flat_raster(16, 8, 0);
        for y in 0..8_usize {
            for x in 8..16_usize {
                let index = (y * 16 + x) * 4;
                coverage.pixels[index] = 255;
                coverage.pixels[index + 1] = 255;
                coverage.pixels[index + 2] = 255;
            }
        }
        let analysis = Cc7StubAnalysis {
            monitor: BTreeMap::from([
                (TimeCode(0), cc7_flat_raster(16, 8, 10)),
                (TimeCode(60), cc7_split_raster(16, 8)),
            ]),
            coverage,
            working: kinewright_core::LinearRgbaImage {
                width: 16,
                height: 8,
                pixels: [0.5, 0.5, 0.5, 1.0].repeat(16 * 8),
            },
        };
        let document = Arc::new(cc7_matte_document());
        let right = NormalizedRoi::new(5_000, 0, 5_000, 10_000);
        let request = ColorEvalRequest {
            project_frame: 60,
            neutral_patch_rois: vec![NormalizedRoi::new(0, 0, 5_000, 10_000), right],
            chart_luma_roi: Some(right),
            chart_luma_reference_frame: 0,
            chart_luma_candidate_frame: 60,
            qc_checks: vec![ColorQcCheck::Range, ColorQcCheck::Gamut],
            gamut_roi: Some(right),
            matte_roi: Some(right),
            look_bypass: Some((ClipId(1), EffectId(4), 60)),
            ..ColorEvalRequest::default()
        };

        // The runner's own expression, not a copy of it: a definition that
        // carries a request measures one.
        let colored = EvalDefinition {
            name: "c1 Mixed-camera interview match",
            rationale: "exercise the colour plumbing",
            fixture_builder: unused_fixture,
            prompts: &["match them"],
            assertions: Vec::new(),
            budgets: budgets(),
            deliverable: None,
            color: Some(request.clone()),
        };
        let evidence = measure_color_block(&colored, &analysis, &document, None)
            .expect("a definition carrying a colour request measures one");
        assert!(evidence.errors.is_empty(), "{:?}", evidence.errors);
        assert_eq!(evidence.neutral_spread_max_code, Some(6));
        // Right half luma 0.2126·20 + 0.7152·16 + 0.0722·10 = 16.4172,
        // against a flat 10.0 at the reference frame. The tolerance absorbs
        // the last bit of an `f64` accumulation, not a measurement error.
        let delta = evidence
            .chart_luma_mean_delta_millionths
            .expect("the chart luma delta is measured");
        assert!((delta - 6_417_200).abs() <= 2, "observed {delta}");
        let matte = evidence.matte.expect("matte coverage is measured");
        assert_eq!(matte.covered_pixel_count, 64);
        assert_eq!(matte.full_pixel_count, 64);
        assert_eq!(matte.partial_pixel_count, 0);
        assert_eq!(evidence.gamut_pixel_count, Some(0));
        assert!(
            evidence
                .qc
                .as_ref()
                .expect("colour qc is measured")
                .technical_pass
        );
        // The stub renders the same raster with and without the node, which
        // is exactly what a lossless bypass looks like.
        assert_eq!(evidence.look_bypass_matches_absent, Some(true));
        assert_eq!(evidence.final_effects.len(), 2);
        assert_eq!(evidence.final_effects[0].effect, EffectId(3));
        assert_eq!(evidence.final_effects[0].name, "primary_correction");
        assert_eq!(evidence.final_effects[1].name, "color_wheels");
        // Nothing that was not requested was measured.
        assert_eq!(evidence.skin, None);
        assert_eq!(evidence.verification, None);

        // It lands on `EvalOutcome::color`, which is what every assertion
        // arm reads.
        let outcome = EvalOutcome {
            color: Some(evidence),
            ..outcome_for((*document).clone(), FixtureContext::default())
        };
        assert!(outcome.color.is_some());

        // R1-M2: the two quantities no variant gates still reach
        // `results.jsonl`, as `budget: 0, passed: true` rows. `colored` gates
        // nothing, so these are the only measurements it can produce.
        let measurements = evaluate(&colored, &outcome).measurements;
        let ungated = measurements
            .iter()
            .map(|measurement| {
                (
                    measurement.name.as_str(),
                    measurement.budget,
                    measurement.passed,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            ungated,
            vec![
                (CHART_LUMA_MEASUREMENT_NAME, 0, true),
                (GAMUT_MEASUREMENT_NAME, 0, true),
            ]
        );
        assert_eq!(measurements[0].observed, delta);
        assert_eq!(measurements[1].observed, 0);

        // The other direction, through the same call: a v1-v5 definition
        // carries no request, so no proof is rendered and the block stays
        // `None` — the other five suites are untouched.
        let uncolored = EvalDefinition {
            name: "v5 generalization",
            rationale: "a suite that is not a colour suite",
            fixture_builder: unused_fixture,
            prompts: &["cut it"],
            assertions: Vec::new(),
            budgets: budgets(),
            deliverable: None,
            color: None,
        };
        assert_eq!(
            measure_color_block(&uncolored, &analysis, &document, None),
            None
        );
        let plain = outcome_for((*document).clone(), FixtureContext::default());
        assert!(plain.color.is_none());
        assert!(evaluate(&uncolored, &plain).measurements.is_empty());
    }

    /// Delivery verification that was asked for and could not be taken is an
    /// error on the record, never a silent pass.
    #[test]
    fn cc7_delivery_verification_without_a_deliverable_is_recorded_as_an_error() {
        let analysis = Cc7StubAnalysis {
            monitor: BTreeMap::from([(TimeCode(0), cc7_flat_raster(4, 4, 10))]),
            coverage: cc7_flat_raster(4, 4, 0),
            working: kinewright_core::LinearRgbaImage {
                width: 4,
                height: 4,
                pixels: [0.5, 0.5, 0.5, 1.0].repeat(16),
            },
        };
        let document = Arc::new(document());
        let request = ColorEvalRequest {
            delivery_verification: Some(DeliveryEncodeDepth::Ten),
            ..ColorEvalRequest::default()
        };
        let evidence = measure_color_evidence(&request, &analysis, &document, None);
        assert_eq!(evidence.verification, None);
        assert_eq!(evidence.errors.len(), 1);
        assert_eq!(
            evidence.errors[0].quantity,
            ColorEvidenceQuantity::DeliveryVerification
        );
        assert!(
            evidence.errors[0].message.contains("no deliverable"),
            "{:?}",
            evidence.errors
        );

        let outcome = EvalOutcome {
            color: Some(evidence),
            ..outcome_for((*document).clone(), FixtureContext::default())
        };
        let landed = color_assertion_outcome(
            &EvalAssertion::DeliveryVerificationWithinBudgets {
                depth: DeliveryEncodeDepth::Ten,
            },
            &outcome,
        );
        assert!(!landed.result.passed);
        assert!(landed.result.detail.contains("could not be measured"));
    }

    /// The colour template rates only what a chart raster can be rated on,
    /// and attaches the scenario's question.
    #[test]
    fn cc7_the_colour_template_marks_the_editorial_dimensions_not_applicable() {
        let mut result = cc7_result_without_measurements();
        result.name = "c1 Mixed-camera interview match".to_owned();
        let mut deliverable = deliverable_shell(
            EvalDeliverableSpec {
                profile: DeliveryProfile::SourceMaster,
                focus_x_percent: 50,
                focus_y_percent: 50,
                proof_frames: 9,
                proof_cell_width: 240,
                require_audio: false,
                expected_transcript_word_set: None,
                maximum_word_error_rate_basis_points: 10_000,
                maximum_caption_word_error_rate_basis_points: None,
                loudness: None,
                audio_tail: None,
                delivery_bit_depth: DeliveryEncodeDepth::Eight,
            },
            &document(),
            Path::new("artifacts/c1-sample-1"),
        );
        deliverable.output_sha256 = Some("0f3a1c2d4e5b".to_owned() + &"7".repeat(52));
        result.deliverable = Some(deliverable);

        let questions = BTreeMap::from([(
            "c1".to_owned(),
            vec![HumanQuestion {
                id: "a".to_owned(),
                prompt: "Does the match preserve natural and intentional differences?".to_owned(),
                answer: None,
                notes: None,
            }],
        )]);
        let review = human_review_template_with_questions(
            COLOR_WORKFLOW_BENCHMARK_ID,
            "run-1",
            std::slice::from_ref(&result),
            &questions,
        );
        assert_eq!(review.schema_version, 2);
        assert_eq!(review.tasks[0].task_id, "c1");
        assert_eq!(review.tasks[0].blind_id.as_deref(), Some("0f3a1c2d4e5b"));
        assert_eq!(
            review.tasks[0].not_applicable,
            COLOR_WORKFLOW_NOT_APPLICABLE.to_vec()
        );
        assert_eq!(review.tasks[0].questions.len(), 1);
        assert_eq!(review.tasks[0].questions[0].id, "a");

        // The form the reviewer opens carries the question and the blind id
        // and nothing that names the task.
        let form = blind_review_form(&review);
        assert_eq!(form.entries.len(), 1);
        assert_eq!(form.entries[0].blind_id, "0f3a1c2d4e5b");
        let bytes = serde_json::to_string(&form).expect("the form serialises");
        assert!(!bytes.contains("c1"));
        assert!(!bytes.contains("task_id"));

        // A non-colour benchmark keeps its existing not-applicable rule.
        let editorial = human_review_template("kinewright-finished-cut-v2", "run-1", &[result]);
        assert_eq!(
            editorial.tasks[0].not_applicable,
            vec![HumanRatingDimension::Captions]
        );
    }
}
