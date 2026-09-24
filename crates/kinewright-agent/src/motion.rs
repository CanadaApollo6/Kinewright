//! MO1 R20: `plan_motion`, the one motion planner.
//!
//! Revision-gated and evidence-only: it returns exact operations and applies
//! nothing. Animated presets author whole-curve [`Operation::SetEffectKeyframes`]
//! on the R1 fine triple (coarse values stay put, fractions ride fine — the
//! canonical-writer rule), so every move is sub-pixel; `pip` writes static
//! [`Operation::SetEffectParam`] instead. A missing `transform` effect is added
//! neutral first. The planner never touches enable (R17).

use std::collections::BTreeMap;

use kinewright_core::{
    AutomationCurve, Clip, ClipId, Document, Effect, EffectId, Keyframe, KeyframeInterpolation,
    Operation, ParamValue, TimeCode, TimelineRevision, TrackKind, apply_batch, effect_descriptor,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;

/// MO1 R20: the six `plan_motion` presets, in registry order.
#[cfg(test)]
pub(crate) const MOTION_PRESET_NAMES: [&str; 6] = [
    "push_in",
    "pull_out",
    "pan_left",
    "pan_right",
    "ken_burns",
    "pip",
];

/// MO1 R25: the six presets in registry order, for the in-app plan dialog.
pub const MOTION_PRESETS: [(MotionPreset, &str); 6] = [
    (MotionPreset::PushIn, "push_in"),
    (MotionPreset::PullOut, "pull_out"),
    (MotionPreset::PanLeft, "pan_left"),
    (MotionPreset::PanRight, "pan_right"),
    (MotionPreset::KenBurns, "ken_burns"),
    (MotionPreset::Pip, "pip"),
];

/// MO1 R20: the response budget. A proposal serializing past this fails
/// closed with a summary, never a silently cut curve.
pub(crate) const MOTION_RESPONSE_BUDGET_BYTES: usize = 4096;

/// MO1 R20: the most keys one parameter curve in a proposal may carry. The
/// shipped presets emit two; anything past the cap is decimated, endpoints
/// kept, rather than refused.
pub(crate) const MOTION_MAX_KEYS_PER_CURVE: usize = 8;

/// MO1 R20: the effect every preset moves.
const MOTION_EFFECT_NAME: &str = "transform";

/// MO1 R20: `pip` writes a static bottom-right quarter frame. Positive `y` is
/// screen-down (the WGSL vertex stage negates `offset_y`), so both offsets
/// are positive; 37/50 = 0.74 NDC against the ideal 0.75, one coarse step off
/// the edge — a preset is a starting point, not a layout engine.
const PIP_STATICS: [(&str, i64); 3] = [("scale_percent", 25), ("x_percent", 37), ("y_percent", 37)];

/// MO1 R20: one `plan_motion` preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MotionPreset {
    PushIn,
    PullOut,
    PanLeft,
    PanRight,
    KenBurns,
    Pip,
}

impl MotionPreset {
    /// The wire name, matching [`MOTION_PRESET_NAMES`].
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PushIn => "push_in",
            Self::PullOut => "pull_out",
            Self::PanLeft => "pan_left",
            Self::PanRight => "pan_right",
            Self::KenBurns => "ken_burns",
            Self::Pip => "pip",
        }
    }
}

/// MO1 R20: arguments for `plan_motion`.
///
/// `deny_unknown_fields` so a misspelled `preset` is refused by name rather
/// than silently planned as the default — there is no default preset.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MotionPlanArgs {
    /// The timeline revision the proposal is computed against.
    pub expected_revision: TimelineRevision,
    /// The video-track clip the move lands on.
    pub clip_id: ClipId,
    /// Which of the six moves to author.
    pub preset: MotionPreset,
    /// Rebuild the target params' curves instead of refusing them.
    #[serde(default)]
    pub replace: bool,
}

/// MO1 R20: why a motion proposal was refused.
#[derive(Debug, Error)]
pub enum MotionPlanError {
    #[error("timeline revision conflict: expected {expected}, actual {actual}")]
    RevisionConflict {
        expected: TimelineRevision,
        actual: TimelineRevision,
    },
    #[error("clip {0} does not exist")]
    MissingClip(ClipId),
    #[error("clip {clip} is not on a video track (track={track:?})")]
    UnsupportedTrack { clip: ClipId, track: TrackKind },
    #[error("clip {clip} is {duration} frames long; a move needs at least 2 frames")]
    ClipTooShort { clip: ClipId, duration: i64 },
    #[error("clip {clip} already carries curves on {params}; clear them or pass replace: true")]
    ExistingCurves { clip: ClipId, params: String },
    #[error("the Core transform descriptor is unavailable")]
    MissingDescriptor,
    #[error("could not allocate a fresh transform effect id")]
    EffectIdExhausted,
    #[error("Core rejected the motion proposal: {0}")]
    CoreRejected(String),
}

impl MotionPlanError {
    /// Stable machine-readable recovery code for structured agent responses.
    #[must_use]
    pub(crate) const fn code(&self) -> &'static str {
        match self {
            Self::RevisionConflict { .. } => "revision_conflict",
            Self::MissingClip(_) => "clip_not_found",
            Self::UnsupportedTrack { .. } => "unsupported_track_kind",
            Self::ClipTooShort { .. } => "clip_too_short",
            Self::ExistingCurves { .. } => "motion_curves_exist",
            Self::MissingDescriptor => "missing_transform_descriptor",
            Self::EffectIdExhausted => "effect_id_exhausted",
            Self::CoreRejected(_) => "motion_plan_core_rejected",
        }
    }

    /// Structured recovery evidence for the agent response.
    #[must_use]
    pub(crate) fn details(&self) -> Value {
        match self {
            Self::RevisionConflict { expected, actual } => {
                json!({"expected_revision": expected, "actual_revision": actual})
            }
            Self::MissingClip(clip) => json!({"clip_id": clip}),
            Self::UnsupportedTrack { clip, track } => {
                json!({"clip_id": clip, "track_kind": track})
            }
            Self::ClipTooShort { clip, duration } => {
                json!({"clip_id": clip, "duration_frames": duration})
            }
            Self::ExistingCurves { clip, params } => {
                json!({"clip_id": clip, "params": params})
            }
            Self::MissingDescriptor | Self::EffectIdExhausted => json!({}),
            Self::CoreRejected(error) => json!({"error": error}),
        }
    }
}

/// MO1 R20: one accepted motion proposal — exact operations, nothing applied.
#[derive(Debug)]
pub struct MotionPlan {
    pub expected_revision: TimelineRevision,
    pub clip_id: ClipId,
    pub preset: MotionPreset,
    pub operations: Vec<Operation>,
    pub target_effect_id: EffectId,
    pub created_new_effect: bool,
    /// Parameter name to key count, in emission order.
    pub key_counts: Vec<(String, usize)>,
}

/// MO1 R20: the animated ramps one preset authors — `(param, from, to)` over
/// the clip ends. `pan_left` slides the picture toward the left edge (`x`
/// decreases); `pan_right` mirrors it. `ken_burns` zooms 100 → 115% while
/// drifting from up-right to down-left. All values sit inside the R1
/// descriptor ranges, so the proposal cannot fail its own validation.
const fn preset_ramps(preset: MotionPreset) -> &'static [(&'static str, i64, i64)] {
    match preset {
        MotionPreset::PushIn => &[("scale_fine_hundredths", 10_000, 12_000)],
        MotionPreset::PullOut => &[("scale_fine_hundredths", 12_000, 10_000)],
        MotionPreset::PanLeft => &[("x_basis_points", 1_200, -1_200)],
        MotionPreset::PanRight => &[("x_basis_points", -1_200, 1_200)],
        MotionPreset::KenBurns => &[
            ("scale_fine_hundredths", 10_000, 11_500),
            ("x_basis_points", 800, -800),
            ("y_basis_points", -450, 450),
        ],
        MotionPreset::Pip => &[],
    }
}

/// MO1 R20: one two-key eased ramp from `from` at frame 0 to `to` at `end`.
/// The trailing key's interpolation never evaluates (no segment follows it);
/// it rides `EaseInOut` so the curve reads uniformly.
fn ramp_curve(from: i64, to: i64, end: TimeCode) -> AutomationCurve {
    AutomationCurve {
        keyframes: vec![
            Keyframe {
                at: TimeCode::ZERO,
                value: from,
                interpolation: KeyframeInterpolation::EaseInOut,
                tangent_in: 0,
                tangent_out: 0,
            },
            Keyframe {
                at: end,
                value: to,
                interpolation: KeyframeInterpolation::EaseInOut,
                tangent_in: 0,
                tangent_out: 0,
            },
        ],
    }
}

/// MO1 R20: thin `keys` to at most [`MOTION_MAX_KEYS_PER_CURVE`], keeping the
/// endpoints and spacing the survivors evenly. Pure; the shipped presets emit
/// two keys and never reach it, but the cap is enforced rather than assumed.
fn decimate_keys(keys: Vec<Keyframe>) -> Vec<Keyframe> {
    if keys.len() <= MOTION_MAX_KEYS_PER_CURVE {
        return keys;
    }
    let keep = MOTION_MAX_KEYS_PER_CURVE;
    let last = keys.len() - 1;
    (0..keep)
        .map(|slot| {
            // Non-negative by construction (`slot`, `last` and `keep - 1`
            // are all ≥ 0), so the float round-trip only spaces the slots.
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_precision_loss,
                clippy::cast_sign_loss
            )]
            let index = (slot as f64 * last as f64 / (keep - 1) as f64).round() as usize;
            keys[index.min(last)]
        })
        .collect()
}

/// MO1 R20: whether the response fits the byte budget — the text plus the
/// serialized structured body.
#[must_use]
pub(crate) fn motion_response_fits_budget(text: &str, body: &Value) -> bool {
    text.len().saturating_add(body.to_string().len()) <= MOTION_RESPONSE_BUDGET_BYTES
}

/// MO1 R20: what the registry serves for one accepted proposal — the text
/// plus the structured body, either the exact operations or the fail-closed
/// budget summary.
pub(crate) enum MotionProposal {
    Fits { text: String, body: Value },
    OverBudget { text: String, body: Value },
}

/// MO1 R20: render one accepted proposal. A body past the 4 KiB budget fails
/// closed with a summary naming the preset, the clip and the bytes — never a
/// silently cut curve.
pub(crate) fn motion_response(plan: &MotionPlan, operations: Value) -> MotionProposal {
    let key_counts = plan
        .key_counts
        .iter()
        .map(|(name, count)| (name.clone(), json!(*count)))
        .collect::<serde_json::Map<_, _>>();
    let total_keys: usize = plan.key_counts.iter().map(|(_, count)| count).sum();
    let mut body = json!({
        "timeline_revision": plan.expected_revision.0,
        "expected_revision": plan.expected_revision.0,
        "clip_id": plan.clip_id.0,
        "preset": plan.preset.as_str(),
        "target_effect_id": plan.target_effect_id.0,
        "created_new_effect": plan.created_new_effect,
        "key_counts": key_counts,
        "evidence_only": true,
        "applied": false,
        "next": "Review these exact operations; submit them through prepare_edit_plan at the same revision if the edit is requested.",
    });
    // Index assignment moves `operations` in; `json!` interpolation would
    // only borrow it through `to_value`.
    body["operations"] = operations;
    let text = format!(
        "prepared evidence-only {} proposal for clip {} at revision {} ({} key(s) across {} param(s)); no operation was applied",
        plan.preset.as_str(),
        plan.clip_id,
        plan.expected_revision,
        total_keys,
        plan.key_counts.len(),
    );
    if !motion_response_fits_budget(&text, &body) {
        let bytes = text.len().saturating_add(body.to_string().len());
        return MotionProposal::OverBudget {
            text: format!(
                "plan_motion {} proposal for clip {} exceeds the {} B response budget ({} B); no curve was emitted",
                plan.preset.as_str(),
                plan.clip_id,
                MOTION_RESPONSE_BUDGET_BYTES,
                bytes,
            ),
            body: json!({
                "code": "motion_response_over_budget",
                "preset": plan.preset.as_str(),
                "clip_id": plan.clip_id.0,
                "bytes": bytes,
                "budget_bytes": MOTION_RESPONSE_BUDGET_BYTES,
                "evidence_only": true,
                "applied": false,
            }),
        };
    }
    MotionProposal::Fits { text, body }
}

/// MO1 R20: one fresh effect id — one past the document-global maximum, the
/// colour planners' rule, so a planned id can never collide.
fn next_effect_id(document: &Document) -> Option<EffectId> {
    document
        .tracks
        .iter()
        .flat_map(|track| track.clips.iter())
        .flat_map(|clip| clip.effects.iter())
        .map(|effect| effect.id.0)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .map(EffectId)
}

/// MO1 R20: the clip plus its track kind, or why the clip cannot take a move.
fn motion_clip(
    document: &Document,
    clip_id: ClipId,
) -> Result<(&Clip, TrackKind), MotionPlanError> {
    for track in &document.tracks {
        if let Some(clip) = track.clips.iter().find(|clip| clip.id == clip_id) {
            if !matches!(track.kind, TrackKind::Video) {
                return Err(MotionPlanError::UnsupportedTrack {
                    clip: clip_id,
                    track: track.kind,
                });
            }
            return Ok((clip, track.kind));
        }
    }
    Err(MotionPlanError::MissingClip(clip_id))
}

/// MO1 R20: the effect one proposal targets — its id, whether the proposal
/// creates it, and which of the preset's params already carry curves.
struct MotionTarget {
    effect_id: EffectId,
    created: bool,
    curved: Vec<&'static str>,
}

/// MO1 R20: resolve the transform one preset moves and refuse the params it
/// must not overwrite. Several same-named transforms stack legally (R19);
/// the proposal targets the first in compositor order and names its id, so
/// the evidence is exact.
fn resolve_motion_target(
    document: &Document,
    clip: &Clip,
    preset: MotionPreset,
    replace: bool,
) -> Result<MotionTarget, MotionPlanError> {
    let existing = clip
        .effects
        .iter()
        .find(|effect| effect.name == MOTION_EFFECT_NAME);
    let (effect_id, created) = match existing {
        Some(effect) => (effect.id, false),
        None => (
            next_effect_id(document).ok_or(MotionPlanError::EffectIdExhausted)?,
            true,
        ),
    };
    let target_params: Vec<&str> = if preset == MotionPreset::Pip {
        PIP_STATICS.iter().map(|(name, _)| *name).collect()
    } else {
        preset_ramps(preset)
            .iter()
            .map(|(name, _, _)| *name)
            .collect()
    };
    let curved: Vec<&'static str> = target_params
        .iter()
        .copied()
        .filter(|name| {
            existing.is_some_and(|effect| {
                effect
                    .keyframes
                    .get(*name)
                    .is_some_and(|curve| !curve.keyframes.is_empty())
            })
        })
        .collect();
    if !curved.is_empty() && !replace {
        return Err(MotionPlanError::ExistingCurves {
            clip: clip.id,
            params: curved.join(", "),
        });
    }
    Ok(MotionTarget {
        effect_id,
        created,
        curved,
    })
}

/// MO1 R20: plan one preset against `document` at `actual_revision`. Pure over
/// the snapshot; the proposed operations are verified against a scratch copy
/// before they are returned, so a proposal the planner emits is one Core
/// accepts.
///
/// # Errors
///
/// Refuses revision conflicts, missing/non-video/too-short clips, existing
/// curves without `replace`, a missing transform descriptor, and proposals
/// Core itself rejects (see [`MotionPlanError`]).
pub fn plan_motion(
    document: &Document,
    actual_revision: TimelineRevision,
    args: &MotionPlanArgs,
) -> Result<MotionPlan, MotionPlanError> {
    if args.expected_revision != actual_revision {
        return Err(MotionPlanError::RevisionConflict {
            expected: args.expected_revision,
            actual: actual_revision,
        });
    }
    let (clip, _) = motion_clip(document, args.clip_id)?;
    let duration = document
        .clip_duration(clip)
        .map_err(|error| MotionPlanError::CoreRejected(error.to_string()))?;
    if duration.0 < 2 {
        return Err(MotionPlanError::ClipTooShort {
            clip: args.clip_id,
            duration: duration.0,
        });
    }
    let descriptor =
        effect_descriptor(MOTION_EFFECT_NAME).ok_or(MotionPlanError::MissingDescriptor)?;
    let target = resolve_motion_target(document, clip, args.preset, args.replace)?;
    let target_effect_id = target.effect_id;
    let created_new_effect = target.created;

    let mut operations = Vec::new();
    if created_new_effect {
        let parameters = descriptor
            .parameters
            .iter()
            .map(|parameter| {
                (
                    parameter.name.to_owned(),
                    ParamValue::Integer(parameter.neutral),
                )
            })
            .collect::<BTreeMap<_, _>>();
        operations.push(Operation::AddEffect {
            clip: args.clip_id,
            effect: Effect {
                enabled: true,
                enabled_curve: None,
                id: target_effect_id,
                name: MOTION_EFFECT_NAME.to_owned(),
                parameters,
                keyframes: BTreeMap::new(),
            },
        });
    }
    let mut key_counts = Vec::new();
    let end = TimeCode(duration.0 - 1);
    if args.preset == MotionPreset::Pip {
        for name in target.curved {
            operations.push(Operation::ClearEffectKeyframes {
                clip: args.clip_id,
                effect: target_effect_id,
                name: name.to_owned(),
            });
        }
        for (name, value) in PIP_STATICS {
            operations.push(Operation::SetEffectParam {
                clip: args.clip_id,
                effect: target_effect_id,
                name: name.to_owned(),
                value: ParamValue::Integer(value),
            });
        }
    } else {
        for (name, from, to) in preset_ramps(args.preset) {
            let curve = AutomationCurve {
                keyframes: decimate_keys(ramp_curve(*from, *to, end).keyframes),
            };
            key_counts.push(((*name).to_owned(), curve.keyframes.len()));
            operations.push(Operation::SetEffectKeyframes {
                clip: args.clip_id,
                effect: target_effect_id,
                name: (*name).to_owned(),
                curve,
            });
        }
    }
    debug_assert!(
        key_counts
            .iter()
            .all(|(_, count)| *count <= MOTION_MAX_KEYS_PER_CURVE),
        "decimation caps every emitted curve"
    );

    let mut candidate = document.clone();
    apply_batch(&mut candidate, &operations)
        .map_err(|error| MotionPlanError::CoreRejected(error.to_string()))?;
    Ok(MotionPlan {
        expected_revision: actual_revision,
        clip_id: args.clip_id,
        preset: args.preset,
        operations,
        target_effect_id,
        created_new_effect,
        key_counts,
    })
}

#[cfg(test)]
mod tests {
    use kinewright_core::{
        AssetId, AudioMix, ClipContent, ColorContext, ColorDescription, FreezeFrame, MediaAsset,
        MediaCatalog, MediaKind, MediaSourceFingerprint, Rational, Track, TrackId,
    };

    use super::*;

    /// One video- or audio-track document holding a single `duration`-frame
    /// freeze clip (source-range duration, so no asset is needed) carrying
    /// `effects`.
    fn motion_fixture(track_kind: TrackKind, duration: i64, effects: Vec<Effect>) -> Document {
        Document {
            investigator: None,
            catalog: MediaCatalog::default(),
            audio_mix: AudioMix::default(),
            tracks: vec![Track {
                id: TrackId(1),
                kind: track_kind,
                sync_lock: true,
                clips: vec![Clip {
                    enabled: true,
                    enabled_curve: None,
                    id: ClipId(1),
                    asset: AssetId(1),
                    source_range: TimeCode::ZERO..TimeCode(duration),
                    content: ClipContent::Freeze(FreezeFrame {
                        source_frame: TimeCode::ZERO,
                    }),
                    timeline_start: TimeCode::ZERO,
                    effects,
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                    audio_gain_curve: None,
                }],
            }],
            media_pool: vec![MediaAsset {
                id: AssetId(1),
                path: "fixture.mp4".into(),
                name: "fixture".into(),
                duration: TimeCode(duration),
                fps: Rational::new(30, 1).unwrap(),
                kind: MediaKind::Video,
                resolution: Some((1920, 1080)),
                source_fingerprint: MediaSourceFingerprint::default(),
                color_description: ColorDescription::default(),
                assumed_from: None,
            }],
            markers: Vec::new(),
            fps: Rational::new(30, 1).unwrap(),
            resolution: (1920, 1080),
            duration: TimeCode(duration),
            color_context: ColorContext::default(),
            lut_assets: Vec::new(),
        }
    }

    fn motion_args(preset: MotionPreset, replace: bool) -> MotionPlanArgs {
        MotionPlanArgs {
            expected_revision: TimelineRevision(7),
            clip_id: ClipId(1),
            preset,
            replace,
        }
    }

    fn neutral_transform(id: EffectId) -> Effect {
        Effect {
            enabled: true,
            enabled_curve: None,
            id,
            name: MOTION_EFFECT_NAME.to_owned(),
            parameters: BTreeMap::from([("scale_percent".to_owned(), ParamValue::Integer(100))]),
            keyframes: BTreeMap::new(),
        }
    }

    fn keyed_transform(id: EffectId, name: &str, value: i64) -> Effect {
        let mut effect = neutral_transform(id);
        effect.keyframes.insert(
            name.to_owned(),
            AutomationCurve {
                keyframes: vec![Keyframe {
                    at: TimeCode::ZERO,
                    value,
                    interpolation: KeyframeInterpolation::Hold,
                    tangent_in: 0,
                    tangent_out: 0,
                }],
            },
        );
        effect
    }

    /// MO1 R20: every preset name round-trips through the wire enum, so the
    /// registry order and the planner agree on the six.
    #[test]
    fn every_preset_name_round_trips() {
        for name in MOTION_PRESET_NAMES {
            let decoded: MotionPreset =
                serde_json::from_value(serde_json::json!(name)).expect("a preset decodes");
            assert_eq!(decoded.as_str(), name);
        }
    }

    /// MO1 R25: the in-app plan dialog lists the same six presets in the
    /// same registry order as the wire enum.
    #[test]
    fn gui_preset_list_matches_registry_order() {
        let listed: Vec<&str> = MOTION_PRESETS.iter().map(|preset| preset.1).collect();
        assert_eq!(listed, MOTION_PRESET_NAMES);
        for preset in MOTION_PRESETS {
            assert_eq!(preset.0.as_str(), preset.1);
        }
    }

    /// MO1 R20: the push-in golden — a missing transform is added neutral,
    /// then the fine scale ramps 100 → 120% across the clip ends, eased.
    #[test]
    fn push_in_adds_a_neutral_transform_then_ramps_fine_scale() {
        let document = motion_fixture(TrackKind::Video, 60, Vec::new());
        let plan = plan_motion(
            &document,
            TimelineRevision(7),
            &motion_args(MotionPreset::PushIn, false),
        )
        .expect("a clean clip plans");
        assert_eq!(plan.target_effect_id, EffectId(1));
        assert!(plan.created_new_effect);
        assert_eq!(plan.key_counts, [("scale_fine_hundredths".to_owned(), 2)]);
        let Operation::AddEffect { clip, effect } = &plan.operations[0] else {
            panic!("first op adds the transform: {:?}", plan.operations[0]);
        };
        assert_eq!(*clip, ClipId(1));
        assert_eq!(effect.name, "transform");
        assert_eq!(effect.parameters.len(), 11, "all neutrals, explicit");
        let Operation::SetEffectKeyframes {
            clip,
            effect: effect_id,
            name,
            curve,
        } = &plan.operations[1]
        else {
            panic!("second op writes the curve: {:?}", plan.operations[1]);
        };
        assert_eq!(
            (*clip, *effect_id, name.as_str()),
            (ClipId(1), EffectId(1), "scale_fine_hundredths")
        );
        assert_eq!(plan.operations.len(), 2);
        let keys = &curve.keyframes;
        assert_eq!(keys.len(), 2);
        assert_eq!((keys[0].at, keys[0].value), (TimeCode::ZERO, 10_000));
        assert_eq!((keys[1].at, keys[1].value), (TimeCode(59), 12_000));
        assert!(
            keys.iter()
                .all(|key| key.interpolation == KeyframeInterpolation::EaseInOut),
            "eased, both ends"
        );
    }

    /// MO1 R20: the remaining animated preset goldens — values and ramps only;
    /// the push-in pins the op shapes.
    #[test]
    fn animated_preset_ramps_carry_their_documented_values() {
        let document = motion_fixture(TrackKind::Video, 48, Vec::new());
        for (preset, expected) in [
            (
                MotionPreset::PullOut,
                vec![("scale_fine_hundredths", 12_000, 10_000)],
            ),
            (
                MotionPreset::PanLeft,
                vec![("x_basis_points", 1_200, -1_200)],
            ),
            (
                MotionPreset::PanRight,
                vec![("x_basis_points", -1_200, 1_200)],
            ),
            (
                MotionPreset::KenBurns,
                vec![
                    ("scale_fine_hundredths", 10_000, 11_500),
                    ("x_basis_points", 800, -800),
                    ("y_basis_points", -450, 450),
                ],
            ),
        ] {
            let plan = plan_motion(&document, TimelineRevision(7), &motion_args(preset, false))
                .expect("a clean clip plans");
            let curves: Vec<(&str, i64, i64)> = plan.operations[1..]
                .iter()
                .map(|operation| {
                    let Operation::SetEffectKeyframes { name, curve, .. } = operation else {
                        panic!("{preset:?} emits whole curves: {operation:?}");
                    };
                    let keys = &curve.keyframes;
                    assert_eq!(keys.len(), 2, "{preset:?} {name}");
                    assert_eq!((keys[0].at, keys[1].at), (TimeCode::ZERO, TimeCode(47)));
                    (name.as_str(), keys[0].value, keys[1].value)
                })
                .collect();
            assert_eq!(curves, expected, "{preset:?}");
        }
    }

    /// MO1 R20: `pip` is static — three `SetEffectParam` on the transform it
    /// adds, no curves anywhere.
    #[test]
    fn pip_writes_static_params_and_no_curves() {
        let document = motion_fixture(TrackKind::Video, 60, Vec::new());
        let plan = plan_motion(
            &document,
            TimelineRevision(7),
            &motion_args(MotionPreset::Pip, false),
        )
        .expect("a clean clip plans");
        assert!(plan.created_new_effect);
        assert!(plan.key_counts.is_empty());
        assert_eq!(plan.operations.len(), 4);
        assert!(matches!(plan.operations[0], Operation::AddEffect { .. }));
        let statics: Vec<(&str, i64)> = plan.operations[1..]
            .iter()
            .map(|operation| {
                let Operation::SetEffectParam { name, value, .. } = operation else {
                    panic!("pip emits statics: {operation:?}");
                };
                let ParamValue::Integer(value) = value else {
                    panic!("integer statics: {value:?}");
                };
                (name.as_str(), *value)
            })
            .collect();
        assert_eq!(
            statics,
            [("scale_percent", 25), ("x_percent", 37), ("y_percent", 37)]
        );
    }

    /// MO1 R20: an existing transform is reused in place — no second node —
    /// and the proposal names its id.
    #[test]
    fn an_existing_transform_is_reused_and_named() {
        let document = motion_fixture(TrackKind::Video, 60, vec![neutral_transform(EffectId(4))]);
        let plan = plan_motion(
            &document,
            TimelineRevision(7),
            &motion_args(MotionPreset::PushIn, false),
        )
        .expect("a static transform plans");
        assert_eq!(plan.target_effect_id, EffectId(4));
        assert!(!plan.created_new_effect);
        assert_eq!(plan.operations.len(), 1);
        assert!(matches!(
            plan.operations[0],
            Operation::SetEffectKeyframes { .. }
        ));
    }

    /// MO1 R20: target params that already carry curves refuse by name unless
    /// `replace` rebuilds them.
    #[test]
    fn existing_curves_refuse_by_name_unless_replace_rebuilds() {
        let document = motion_fixture(
            TrackKind::Video,
            60,
            vec![keyed_transform(
                EffectId(2),
                "scale_fine_hundredths",
                11_000,
            )],
        );
        let refused = plan_motion(
            &document,
            TimelineRevision(7),
            &motion_args(MotionPreset::PushIn, false),
        )
        .expect_err("curves without replace refuse");
        assert_eq!(refused.code(), "motion_curves_exist");
        let message = refused.to_string();
        assert!(message.contains("scale_fine_hundredths"), "{message}");
        assert!(message.contains("replace: true"), "{message}");
        let rebuilt = plan_motion(
            &document,
            TimelineRevision(7),
            &motion_args(MotionPreset::PushIn, true),
        )
        .expect("replace rebuilds");
        assert_eq!(
            rebuilt.operations.len(),
            1,
            "whole-curve overwrite, no clear needed"
        );
    }

    /// MO1 R20: `pip` with `replace` clears the curved params it then writes
    /// statically; unrelated curves are untouched.
    #[test]
    fn pip_replace_clears_only_the_params_it_writes() {
        let mut effect = keyed_transform(EffectId(2), "scale_percent", 150);
        effect.keyframes.insert(
            "rotation_centidegrees".to_owned(),
            AutomationCurve {
                keyframes: vec![Keyframe {
                    at: TimeCode::ZERO,
                    value: 900,
                    interpolation: KeyframeInterpolation::Hold,
                    tangent_in: 0,
                    tangent_out: 0,
                }],
            },
        );
        let document = motion_fixture(TrackKind::Video, 60, vec![effect]);
        let plan = plan_motion(
            &document,
            TimelineRevision(7),
            &motion_args(MotionPreset::Pip, true),
        )
        .expect("pip replace plans");
        let Operation::ClearEffectKeyframes { name, .. } = &plan.operations[0] else {
            panic!("first op clears the curved param: {:?}", plan.operations[0]);
        };
        assert_eq!(name, "scale_percent");
        assert_eq!(plan.operations.len(), 4, "one clear plus three statics");
        let mut landed = document.clone();
        apply_batch(&mut landed, &plan.operations).expect("the proposal applies");
        let landed_effect = landed.tracks[0].clips[0]
            .effects
            .iter()
            .find(|effect| effect.id == EffectId(2))
            .unwrap();
        assert!(!landed_effect.keyframes.contains_key("scale_percent"));
        assert!(
            landed_effect
                .keyframes
                .contains_key("rotation_centidegrees")
        );
    }

    /// MO1 R20: `ken_burns` is a move preset and is NOT refused on video.
    #[test]
    fn ken_burns_is_allowed_on_video() {
        let document = motion_fixture(TrackKind::Video, 120, Vec::new());
        let plan = plan_motion(
            &document,
            TimelineRevision(7),
            &motion_args(MotionPreset::KenBurns, false),
        )
        .expect("ken_burns plans on video");
        assert_eq!(plan.key_counts.len(), 3);
    }

    /// MO1 R20: audio-track clips, missing clips, stale revisions and
    /// sub-2-frame clips refuse with typed codes.
    #[test]
    fn unplannable_clips_refuse_typed() {
        let audio = motion_fixture(TrackKind::Audio, 60, Vec::new());
        let refused = plan_motion(
            &audio,
            TimelineRevision(7),
            &motion_args(MotionPreset::PushIn, false),
        )
        .expect_err("audio-track clips refuse");
        assert_eq!(refused.code(), "unsupported_track_kind");

        let document = motion_fixture(TrackKind::Video, 60, Vec::new());
        let missing = MotionPlanArgs {
            clip_id: ClipId(99),
            ..motion_args(MotionPreset::PushIn, false)
        };
        assert_eq!(
            plan_motion(&document, TimelineRevision(7), &missing)
                .expect_err("missing clips refuse")
                .code(),
            "clip_not_found"
        );
        assert!(matches!(
            plan_motion(
                &document,
                TimelineRevision(9),
                &motion_args(MotionPreset::PushIn, false)
            )
            .expect_err("stale revisions refuse"),
            MotionPlanError::RevisionConflict { .. }
        ));

        let single = motion_fixture(TrackKind::Video, 1, Vec::new());
        assert_eq!(
            plan_motion(
                &single,
                TimelineRevision(7),
                &motion_args(MotionPreset::PushIn, false)
            )
            .expect_err("1-frame clips refuse")
            .code(),
            "clip_too_short"
        );
    }

    /// MO1 R20: decimation keeps the endpoints, spaces the survivors evenly,
    /// and leaves short curves alone.
    #[test]
    fn decimation_keeps_endpoints_and_leaves_short_curves_alone() {
        let keys: Vec<Keyframe> = (0..20)
            .map(|at| Keyframe {
                at: TimeCode(at),
                value: at * 100,
                interpolation: KeyframeInterpolation::Linear,
                tangent_in: 0,
                tangent_out: 0,
            })
            .collect();
        let thinned = decimate_keys(keys.clone());
        assert_eq!(thinned.len(), MOTION_MAX_KEYS_PER_CURVE);
        assert_eq!(thinned.first().unwrap().at, TimeCode::ZERO);
        assert_eq!(thinned.last().unwrap().at, TimeCode(19));
        let mut frames: Vec<i64> = thinned.iter().map(|key| key.at.0).collect();
        frames.dedup();
        assert_eq!(frames.len(), MOTION_MAX_KEYS_PER_CURVE, "no doubled frames");

        let short: Vec<Keyframe> = keys[..8].to_vec();
        assert_eq!(decimate_keys(short.clone()), short);
    }

    /// MO1 R20: the budget helper draws the line at exactly 4 KiB.
    #[test]
    fn the_budget_helper_draws_the_line_at_4kib() {
        let body = json!({"operations": []});
        let room = MOTION_RESPONSE_BUDGET_BYTES - body.to_string().len();
        assert!(motion_response_fits_budget(&"t".repeat(room), &body));
        assert!(!motion_response_fits_budget(&"t".repeat(room + 1), &body));
    }

    /// MO1 R20: a real proposal renders with its exact operations inside the
    /// budget, and carries the evidence-only markers.
    #[test]
    fn a_real_proposal_renders_inside_the_budget() {
        let document = motion_fixture(TrackKind::Video, 60, Vec::new());
        let plan = plan_motion(
            &document,
            TimelineRevision(7),
            &motion_args(MotionPreset::KenBurns, false),
        )
        .expect("ken_burns plans");
        let operations = serde_json::to_value(&plan.operations).unwrap();
        let MotionProposal::Fits { text, body } = motion_response(&plan, operations) else {
            panic!("the largest preset fits the budget");
        };
        let bytes = text.len() + body.to_string().len();
        assert!(
            bytes < MOTION_RESPONSE_BUDGET_BYTES,
            "ken_burns renders {bytes} B against a 4 KiB budget"
        );
        assert_eq!(body["applied"], json!(false));
        assert_eq!(body["evidence_only"], json!(true));
        assert_eq!(body["operations"].as_array().unwrap().len(), 4);
    }

    /// MO1 R20: an over-budget proposal fails closed with a summary — the
    /// code, the preset, the clip, the bytes — and no operations.
    #[test]
    fn an_over_budget_proposal_fails_closed_with_a_summary() {
        let document = motion_fixture(TrackKind::Video, 60, Vec::new());
        let plan = plan_motion(
            &document,
            TimelineRevision(7),
            &motion_args(MotionPreset::PushIn, false),
        )
        .expect("push_in plans");
        let operations = json!([Value::String("o".repeat(MOTION_RESPONSE_BUDGET_BYTES))]);
        let MotionProposal::OverBudget { text, body } = motion_response(&plan, operations) else {
            panic!("a 4 KiB operation fails closed");
        };
        assert!(text.contains("push_in"), "{text}");
        assert!(text.contains("clip 1"), "{text}");
        assert!(text.contains("no curve was emitted"), "{text}");
        assert_eq!(body["code"], json!("motion_response_over_budget"));
        assert_eq!(body["applied"], json!(false));
        assert!(
            body.get("operations").is_none(),
            "no cut curve leaks: {body}"
        );
    }
}
