use std::{collections::BTreeMap, sync::Arc};

use eframe::egui;
use kinewright_core::{
    COLOR_CURVE_COORDINATE_MAX, COLOR_CURVE_COORDINATE_MIN, COLOR_CURVE_MAX_POINTS,
    COLOR_CURVE_MIN_POINTS, COLOR_CURVE_WHITE_BASIS_POINTS, COLOR_NODE_BYPASS_PARAMETER, Clip,
    ClipContent, ClipId, ColorCurveChannel, ColorNodeKind, ColorStage, ColorWheelChannel,
    ColorWheelControl, ColorWheelControlSet, ColorWheelsParams, Document, EFFECT_DESCRIPTORS,
    Effect, EffectId, LUT_ASSET_ID_PARAMETER, LUT_INPUT_ENCODING_PARAMETER,
    LUT_MIX_BASIS_POINTS_MAX, LUT_MIX_PARAMETER, LutAsset, LutAssetId, LutAssetSource,
    LutAvailabilityKind, LutAvailabilityStatus, LutNodeParams, MARKER_COLOR_TOKEN_COUNT,
    MATTE_MIX_BASIS_POINTS_MAX, MATTE_WINDOW_LIMIT, Marker, MarkerId, MatteParams,
    MatteQualifierParams, MatteWindowParams, MediaKind, Operation, ParamValue, ResolvedCurves,
    TITLE_COLORS, TITLE_FONT_SIZES, TRANSITION_DESCRIPTORS, TimeCode, Title, TitlePosition,
    Transition, color_node_inactive_reason, effect_compatibility_stage, is_audio_effect,
    is_legacy_display_effect, is_lut_color_node, is_matte_capable_color_node, is_matte_parameter,
};
use kinewright_media::BuiltinLook;

use crate::{
    app::KinewrightApp,
    color_wheel_widget::{self, ColorWheelState, color_wheel},
    curve_editor_widget::{self, curve_editor},
    matte_overlay_ui::{MatteHit, MatteTarget},
    media_workflow::{paint_source_status, source_display_state},
    theme::{self, color, space, type_size},
    timeline_ui::{is_internal_marker, linked_members, linked_transition_operations},
};

const INSPECTOR_MAX_HEIGHT: f32 = 360.0;

/// Edits gathered from one inspector frame.
///
/// A dragged slider emits one operation per frame so the preview stays live.
/// `coalesce_key` marks those frames so the whole drag lands in a single undo
/// entry; a frame that also carries a discrete edit (a button, a typed value)
/// drops the key and becomes an ordinary batch.
#[derive(Debug, Default)]
pub(crate) struct InspectorEdits {
    operations: Vec<Operation>,
    coalesce_key: Option<String>,
    /// Set on the frame a drag begins so the app opens a fresh gesture
    /// identity. Without it a second drag over the same control would merge
    /// into the previous drag's undo entry.
    gesture_started: bool,
    /// Look actions the card cannot express as operations because they need a
    /// file dialog, a worker thread, or a window (CC4 §7). Collected here so
    /// every card stays a pure function of the document and is testable
    /// without an egui context.
    look_requests: Vec<LookRequest>,
    /// The A/B hold that is live at the end of this frame, if any.
    ///
    /// The app mirrors it so a card that stops rendering mid-hold — the panel
    /// collapsed, the tab switched, the clip deselected — cannot leave
    /// `bypass = 1` written in the document with nothing left to release it
    /// (CC4 §7).
    ab_hold: Option<AbHoldRecord>,
    /// The card that released an A/B hold itself this frame, which retires the
    /// app's mirror without a second restore.
    ab_released: Option<(ClipId, EffectId)>,
    /// The matte section that was expanded this frame, if any (CC5 §6).
    ///
    /// The viewer renders before the inspector, so it reads the previous
    /// frame's report: a card that stops rendering therefore stops the overlay
    /// on the next frame rather than leaving the viewer eating pointer input.
    matte_expanded: Option<MatteTarget>,
    /// The window a card asked the overlay to select, with the window count
    /// the card could see, so the overlay can clamp the request (CC5 §6).
    matte_selected_window: Option<(usize, usize)>,
    /// Refusals a card produced while building a batch, for the app's error
    /// log.
    ///
    /// Drawing them inline inside the click branch showed them for exactly one
    /// frame — the frame the pointer was released — so a failed action looked
    /// like a control that simply did nothing. They travel out with the rest
    /// of the frame's edits instead, and land where every other failure does.
    errors: Vec<String>,
}

/// One live A/B hold, as the card reports it (CC4 §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AbHoldRecord {
    pub(crate) clip: ClipId,
    pub(crate) effect: EffectId,
    /// The `bypass` value captured on the press, which the release restores.
    pub(crate) restore: i64,
}

/// One live A/B hold as the app mirrors it, bound to the project it belongs to.
///
/// The session id is not decoration: a hold survives a project switch, and
/// restoring it into whatever project happens to be focused would write a
/// bypass into a document that never had one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MirroredAbHold {
    pub(crate) session: u64,
    pub(crate) record: AbHoldRecord,
}

/// One look action the inspector asks the app to perform after the frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LookRequest {
    /// Import a `.cube` and bind a new node of this stage to it.
    Import { clip: ClipId, stage: ColorStage },
    /// `Locate file…`: hash-checked restore of an asset's store bytes.
    Locate { lut_asset: LutAssetId },
    /// `Replace…`: import a different LUT and retarget this node.
    Replace { clip: ClipId, effect: EffectId },
    /// Open the look browser for one clip, optionally targeting a node.
    Browse {
        clip: ClipId,
        effect: Option<EffectId>,
    },
    /// Convert one legacy `cube_lut` whose external path must be imported
    /// into the store before the batch can be built (CC4 §9).
    ConvertLegacyCube {
        clip: ClipId,
        effect: EffectId,
        path: std::path::PathBuf,
    },
}

impl InspectorEdits {
    /// Record a discrete edit. Discrete edits are never coalesced.
    fn push(&mut self, operation: Operation) {
        self.coalesce_key = None;
        self.operations.push(operation);
    }

    fn extend(&mut self, operations: impl IntoIterator<Item = Operation>) {
        let before = self.operations.len();
        self.operations.extend(operations);
        if self.operations.len() != before {
            self.coalesce_key = None;
        }
    }

    /// Record one frame of a live control gesture.
    fn push_live(&mut self, operation: Operation, coalesce_key: String) {
        if self.operations.is_empty() {
            self.coalesce_key = Some(coalesce_key);
        }
        self.operations.push(operation);
    }

    /// Record one frame of a live control gesture that needs several
    /// operations to express a single value, such as a speed change that also
    /// retimes the linked audio.
    pub(crate) fn extend_live(
        &mut self,
        operations: impl IntoIterator<Item = Operation>,
        coalesce_key: String,
    ) {
        let before = self.operations.len();
        self.operations.extend(operations);
        if before == 0 && self.operations.len() != before {
            self.coalesce_key = Some(coalesce_key);
        }
    }

    pub(crate) fn begin_gesture(&mut self) {
        self.gesture_started = true;
    }

    /// Record a look action that needs the app: a dialog, a worker, a window.
    fn push_look(&mut self, request: LookRequest) {
        self.look_requests.push(request);
    }

    /// Mirror the A/B hold this frame's card reported.
    fn record_ab_hold(&mut self, record: AbHoldRecord) {
        self.ab_hold = Some(record);
    }

    /// Report that a card released its own hold, so the app's mirror retires.
    fn record_ab_release(&mut self, clip: ClipId, effect: EffectId) {
        self.ab_released = Some((clip, effect));
    }

    /// Report that this frame drew an expanded matte section (CC5 §6).
    fn record_matte_expanded(&mut self, target: MatteTarget) {
        self.matte_expanded = Some(target);
    }

    /// Ask the overlay to draw handles on one window (CC5 §6).
    ///
    /// `window_count` travels with the index: the overlay stores a bare `usize`
    /// and has no document of its own, so the clamp has to come from the card
    /// that read the count.
    fn record_matte_window_selection(&mut self, window: usize, window_count: usize) {
        self.matte_selected_window = Some((window, window_count));
    }

    #[cfg(test)]
    const fn matte_expanded(&self) -> Option<MatteTarget> {
        self.matte_expanded
    }

    #[cfg(test)]
    const fn matte_selected_window(&self) -> Option<(usize, usize)> {
        self.matte_selected_window
    }

    /// Record a refusal the app should surface through the error log.
    fn push_error(&mut self, message: impl Into<String>) {
        self.errors.push(message.into());
    }

    #[cfg(test)]
    fn errors(&self) -> &[String] {
        &self.errors
    }

    #[cfg(test)]
    const fn ab_hold(&self) -> Option<AbHoldRecord> {
        self.ab_hold
    }

    #[cfg(test)]
    fn operations(&self) -> &[Operation] {
        &self.operations
    }

    #[cfg(test)]
    fn coalesce_key(&self) -> Option<&str> {
        self.coalesce_key.as_deref()
    }

    #[cfg(test)]
    fn look_requests(&self) -> &[LookRequest] {
        &self.look_requests
    }
}

/// Stable coalesce key for one live look mix drag (CC4 §7).
fn look_mix_coalesce_key(clip: ClipId, effect: EffectId) -> String {
    format!("look:{}:{}:mix", clip.0, effect.0)
}

/// Stable coalesce key for one press-and-hold A/B comparison (CC4 §7).
///
/// The hold runs through the same coalesced gesture path as a slider drag, so
/// the whole comparison — the bypass on press and the restore on release — is
/// one undo entry and is the real, provably lossless bypass rather than a
/// preview shortcut.
fn look_ab_coalesce_key(clip: ClipId, effect: EffectId) -> String {
    format!("look:{}:{}:ab", clip.0, effect.0)
}

/// Stable per-parameter coalesce key for one live primary-correction drag.
fn primary_coalesce_key(clip: ClipId, effect: EffectId, parameter: &str) -> String {
    format!("primary:{}:{}:{parameter}", clip.0, effect.0)
}

/// Stable coalesce key for one live clip-speed drag.
fn speed_coalesce_key(clip: ClipId) -> String {
    format!("speed:{}", clip.0)
}

/// Stable coalesce key for one live audio-gain drag.
fn audio_gain_coalesce_key(clip: ClipId) -> String {
    format!("audio_gain:{}", clip.0)
}

// ---------------------------------------------------------------------------
// CC4 §7 look node operations
// ---------------------------------------------------------------------------

/// The first index in `effects` at which a node of `stage` satisfies the CC4
/// §3.2 stage-ordering rule.
///
/// The managed-node subsequence must have non-decreasing stage rank, so the
/// legal window for a new node opens just after the last managed node of an
/// equal-or-lower rank and closes at the first managed node of a higher rank.
/// This returns the start of that window, which is the same index the CC4 §8
/// planners emit, so an insert from a stage heading can never be rejected for
/// ordering. Non-colour effects are unconstrained and are stepped over without
/// moving the index.
#[must_use]
pub(crate) fn color_stage_insert_index(effects: &[Effect], stage: ColorStage) -> usize {
    let mut index = 0;
    for (position, effect) in effects.iter().enumerate() {
        let Some(kind) = ColorNodeKind::from_effect_name(&effect.name) else {
            continue;
        };
        if kind.stage().rank() <= stage.rank() {
            index = position + 1;
        }
    }
    index
}

/// The effect id a new node on this clip takes: one past the highest in use.
fn next_effect_id(clip: &Clip) -> EffectId {
    EffectId(
        clip.effects
            .iter()
            .map(|effect| effect.id.0)
            .max()
            .unwrap_or(0)
            .saturating_add(1),
    )
}

/// The `technical_lut` / `creative_look` node kind that occupies one stage.
const fn lut_kind_for_stage(stage: ColorStage) -> ColorNodeKind {
    match stage {
        ColorStage::Input => ColorNodeKind::TechnicalLut,
        // A correction stage has no LUT kind; the inspector never offers one,
        // and treating it as a creative look keeps this total rather than
        // panicking on an unreachable branch.
        ColorStage::Correction | ColorStage::Look => ColorNodeKind::CreativeLook,
    }
}

/// One `InsertEffect` that binds a new LUT node of `stage` to `asset` at the
/// first legal index (CC4 §2.7, §7).
///
/// Only the values the operator touched are written: `lut_asset_id` and, for a
/// creative look, nothing else, because CC4 §5 makes `mix_basis_points` neutral
/// at full strength so a look created with only a binding shows the look.
#[must_use]
pub(crate) fn insert_lut_node_operation(
    clip: &Clip,
    stage: ColorStage,
    asset: LutAssetId,
) -> Operation {
    Operation::InsertEffect {
        clip: clip.id,
        index: color_stage_insert_index(&clip.effects, stage),
        effect: Effect {
            id: next_effect_id(clip),
            name: lut_kind_for_stage(stage).effect_name().to_owned(),
            parameters: BTreeMap::from([(
                LUT_ASSET_ID_PARAMETER.to_owned(),
                ParamValue::Integer(lut_asset_parameter_value(asset)),
            )]),
            keyframes: BTreeMap::new(),
        },
    }
}

/// One `SetEffectParam` that retargets an existing LUT node onto `asset`.
#[must_use]
pub(crate) fn lut_asset_param_operation(
    clip: ClipId,
    effect: EffectId,
    asset: LutAssetId,
) -> Operation {
    effect_param_operation(
        clip,
        effect,
        LUT_ASSET_ID_PARAMETER,
        lut_asset_parameter_value(asset),
    )
}

/// `LutAssetId` as the integer a `ParamValue::Integer` carries.
///
/// Ids are bounded by `2^53 - 1`, so the saturation is unreachable; it exists
/// so a hand-built id can never wrap into a different asset.
fn lut_asset_parameter_value(asset: LutAssetId) -> i64 {
    i64::try_from(asset.0).unwrap_or(i64::MAX)
}

/// One `ConvertLegacyLook` for a legacy node already bound to `asset`.
///
/// `intensity_percent` maps to `mix_basis_points = percent * 100` (CC4 §9).
#[must_use]
pub(crate) fn convert_legacy_look_operation(
    clip: ClipId,
    legacy: &Effect,
    asset: LutAssetId,
) -> Operation {
    Operation::ConvertLegacyLook {
        clip,
        effect: legacy.id,
        lut_asset: asset,
        mix_basis_points: legacy_mix_basis_points(legacy),
    }
}

/// Whether converting the legacy node `legacy` into a managed creative look
/// *in place* keeps the CC4 §3.2 stage order.
///
/// `ConvertLegacyLook` rewrites the node where it stands, and a creative look
/// carries the highest stage rank, so the conversion is legal exactly when no
/// managed node of a lower rank — every correction, and the technical input
/// transform — sits after it. A legacy `look_lut` authored before a
/// `primary_correction` is the case this catches (CC4 §9).
#[must_use]
pub(crate) fn legacy_conversion_keeps_stage_order(effects: &[Effect], legacy: EffectId) -> bool {
    let Some(position) = effects.iter().position(|effect| effect.id == legacy) else {
        return false;
    };
    let converted = ColorStage::Look.rank();
    effects
        .iter()
        .enumerate()
        .skip(position + 1)
        .filter_map(|(_, effect)| ColorNodeKind::from_effect_name(&effect.name))
        .all(|kind| kind.stage().rank() >= converted)
}

/// The managed creative look one legacy node converts into, with the effect
/// id preserved so the conversion stays the *same* node to undo (CC4 §9).
///
/// Mirrors what `ConvertLegacyLook` writes in Core: the binding, the converted
/// mix, and nothing else.
fn converted_look_effect(legacy: &Effect, asset: LutAssetId) -> Effect {
    Effect {
        id: legacy.id,
        name: ColorNodeKind::CreativeLook.effect_name().to_owned(),
        parameters: BTreeMap::from([
            (
                LUT_ASSET_ID_PARAMETER.to_owned(),
                ParamValue::Integer(lut_asset_parameter_value(asset)),
            ),
            (
                LUT_MIX_PARAMETER.to_owned(),
                ParamValue::Integer(legacy_mix_basis_points(legacy)),
            ),
        ]),
        keyframes: BTreeMap::new(),
    }
}

/// The operations that turn one legacy look node into a managed creative look
/// (CC4 §9).
///
/// One `ConvertLegacyLook` when the node already stands somewhere a creative
/// look may stand. When it does not — a `look_lut` authored before a
/// `primary_correction` — the same conversion is expressed as
/// `[RemoveEffect, InsertEffect]` at the first legal Look index, which is the
/// only way to convert it at all: `ConvertLegacyLook` cannot move a node, and
/// converting in place would be rejected with `ColorStageOrderViolation`.
/// The effect id is preserved either way, so the operator's node keeps its
/// identity and the batch is one undo entry.
#[must_use]
pub(crate) fn legacy_conversion_operations(
    clip: &Clip,
    legacy: &Effect,
    asset: LutAssetId,
) -> Vec<Operation> {
    if legacy_conversion_keeps_stage_order(&clip.effects, legacy.id) {
        return vec![convert_legacy_look_operation(clip.id, legacy, asset)];
    }
    let remaining: Vec<Effect> = clip
        .effects
        .iter()
        .filter(|effect| effect.id != legacy.id)
        .cloned()
        .collect();
    vec![
        Operation::RemoveEffect {
            clip: clip.id,
            effect: legacy.id,
        },
        Operation::InsertEffect {
            clip: clip.id,
            index: color_stage_insert_index(&remaining, ColorStage::Look),
            effect: converted_look_effect(legacy, asset),
        },
    ]
}

/// The legacy `intensity_percent` as CC4 basis points, clamped to the managed
/// node's bounds so a hand-edited project cannot produce a rejected batch.
fn legacy_mix_basis_points(legacy: &Effect) -> i64 {
    stored_integer(legacy, "intensity_percent", 100)
        .saturating_mul(100)
        .clamp(0, LUT_MIX_BASIS_POINTS_MAX)
}

/// One effect parameter's stored integer, or `neutral` when it is absent or
/// is not an integer.
fn stored_integer(effect: &Effect, name: &str, neutral: i64) -> i64 {
    effect
        .parameters
        .get(name)
        .and_then(|value| match value {
            ParamValue::Integer(value) => Some(*value),
            ParamValue::Boolean(_) | ParamValue::Text(_) => None,
        })
        .unwrap_or(neutral)
}

/// One effect parameter's stored text, when it carries one.
fn stored_text<'a>(effect: &'a Effect, name: &str) -> Option<&'a str> {
    match effect.parameters.get(name) {
        Some(ParamValue::Text(value)) => Some(value.as_str()),
        _ => None,
    }
}

/// The `[AddLutAsset, ConvertLegacyLook]` batch that turns one legacy
/// `look_lut` into a managed creative look (CC4 §9).
///
/// The preset token resolves to the built-in generated asset of CC4 §2.6, and
/// the batch registers it only when the project does not already carry it —
/// the store is content-addressed, so importing the same look twice is one
/// record. `AddLutAsset` is always visible in the batch when it is needed,
/// because `ConvertLegacyLook` rejects an unregistered asset by design.
///
/// # Errors
///
/// Returns the human reason when the effect is not a `look_lut`, when its
/// `preset_token` names no built-in, or when the id space is exhausted.
pub(crate) fn convert_builtin_look_operations(
    document: &Document,
    clip: ClipId,
    legacy: &Effect,
) -> Result<Vec<Operation>, String> {
    if legacy.name != "look_lut" {
        return Err(format!(
            "{} is not a built-in legacy look; only look_lut converts from a preset token",
            legacy.name
        ));
    }
    let target = document
        .clip(clip)
        .ok_or_else(|| format!("clip {clip} no longer exists"))?;
    let token = stored_integer(legacy, "preset_token", 0);
    let builtin = BuiltinLook::from_preset_token(token)
        .ok_or_else(|| format!("preset_token {token} names no built-in look (allowed 0..=4)"))?;
    let mut operations = Vec::with_capacity(3);
    let asset = ensure_builtin_registered(document, builtin, &mut operations)?;
    operations.extend(legacy_conversion_operations(target, legacy, asset));
    Ok(operations)
}

/// Resolve one built-in to a registered asset id, emitting the `AddLutAsset`
/// that registers it when the project does not carry it yet (CC4 §2.6).
///
/// `AddLutAsset` is always visible in the batch when it is needed, because
/// `ConvertLegacyLook` and `validate_document` both reject an unregistered
/// asset by design.
fn ensure_builtin_registered(
    document: &Document,
    builtin: BuiltinLook,
    operations: &mut Vec<Operation>,
) -> Result<LutAssetId, String> {
    if let Some(existing) = registered_builtin(document, builtin) {
        return Ok(existing);
    }
    let id = document
        .next_lut_asset_id()
        .map_err(|error| error.to_string())?;
    operations.push(Operation::AddLutAsset {
        asset: builtin.to_lut_asset(id),
    });
    Ok(id)
}

/// The project's existing record for one built-in, matched on the pinned hash
/// as well as the name so a changed bake registers as a distinct asset rather
/// than silently re-rendering an older project (CC4 §2.3).
pub(crate) fn registered_builtin(document: &Document, builtin: BuiltinLook) -> Option<LutAssetId> {
    document
        .lut_assets
        .iter()
        .find(|asset| {
            matches!(&asset.source, LutAssetSource::Builtin { name } if name == builtin.name())
                && asset.sha256 == builtin.sha256()
        })
        .map(|asset| asset.id)
}

/// The `[AddLutAsset?, InsertEffect]` batch that stacks one more creative look
/// bound to `builtin` on a clip (CC4 §7 look browser).
///
/// # Errors
///
/// Returns the human reason when the LUT asset id space is exhausted.
pub(crate) fn builtin_look_operations(
    document: &Document,
    clip: &Clip,
    builtin: BuiltinLook,
    stage: ColorStage,
) -> Result<Vec<Operation>, String> {
    let mut operations = Vec::with_capacity(2);
    let asset = ensure_builtin_registered(document, builtin, &mut operations)?;
    operations.push(insert_lut_node_operation(clip, stage, asset));
    Ok(operations)
}

/// Retarget one existing LUT node onto a built-in, registering the asset first
/// when the project does not carry it (CC4 §7 look browser).
///
/// # Errors
///
/// Returns the human reason when the LUT asset id space is exhausted.
pub(crate) fn builtin_retarget_operations(
    document: &Document,
    clip: ClipId,
    effect: EffectId,
    builtin: BuiltinLook,
) -> Result<Vec<Operation>, String> {
    let mut operations = Vec::with_capacity(2);
    let asset = ensure_builtin_registered(document, builtin, &mut operations)?;
    operations.push(lut_asset_param_operation(clip, effect, asset));
    Ok(operations)
}

/// One press-and-hold A/B step, as a pure transition (CC4 §7).
///
/// The stored value to restore is captured on the press, not read back on the
/// release: the hold writes `bypass = 1` through the coalesced path, so by the
/// time the pointer comes up the live document already says `1` and reading it
/// then would make the comparison sticky.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AbHoldState {
    pub(crate) held: bool,
    pub(crate) restore: i64,
}

/// What one frame of the A/B control produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AbHoldStep {
    pub(crate) state: AbHoldState,
    pub(crate) operation: Option<Operation>,
    pub(crate) gesture_started: bool,
}

/// The context-wide id one look card's A/B hold state lives under.
///
/// Derived from the clip and effect rather than from the parent `Ui`, so the
/// frame loop can retire a stranded hold's state without the card that wrote
/// it ever rendering again (CC4 §7).
#[must_use]
pub(crate) fn ab_hold_id(clip: ClipId, effect: EffectId) -> egui::Id {
    egui::Id::new(("look-ab-hold", clip.0, effect.0))
}

/// Whether the press-and-hold A/B comparison is offered on one node (CC4 §7).
///
/// A keyframed `bypass` is evaluated per frame from its curve, so a hold would
/// write a static value the curve immediately overrides: the comparison would
/// change nothing visible while still filing an undo entry. The control is
/// disabled and badged instead, with "clear the keyframes first" as the
/// recovery.
#[must_use]
pub(crate) fn ab_hold_is_available(effect: &Effect) -> bool {
    !parameter_is_keyframed(effect, COLOR_NODE_BYPASS_PARAMETER)
}

/// Whether a mirrored A/B hold has to be restored by the frame loop rather
/// than by its card (CC4 §7).
///
/// Either the card did not render — the panel was collapsed, the material tab
/// switched, the clip deselected — so no release transition can ever be
/// observed, or the pointer is already up and the card missed the release.
/// Both leave `bypass = 1` in the document, which is a silent grade change.
#[must_use]
pub(crate) const fn ab_hold_needs_recovery(rendered_last_frame: bool, pointer_down: bool) -> bool {
    !rendered_last_frame || !pointer_down
}

/// The operation that returns one stranded A/B hold's node to its captured
/// value.
#[must_use]
pub(crate) fn ab_hold_restore_operation(record: AbHoldRecord) -> Operation {
    effect_param_operation(
        record.clip,
        record.effect,
        COLOR_NODE_BYPASS_PARAMETER,
        record.restore,
    )
}

/// Advance the A/B hold by one frame.
#[must_use]
pub(crate) fn ab_hold_step(
    clip: ClipId,
    effect: EffectId,
    previous: AbHoldState,
    pressed: bool,
    stored_bypass: i64,
) -> AbHoldStep {
    match (previous.held, pressed) {
        (false, true) => AbHoldStep {
            state: AbHoldState {
                held: true,
                restore: stored_bypass,
            },
            operation: Some(effect_param_operation(
                clip,
                effect,
                COLOR_NODE_BYPASS_PARAMETER,
                1,
            )),
            gesture_started: true,
        },
        (true, false) => AbHoldStep {
            state: AbHoldState::default(),
            operation: Some(effect_param_operation(
                clip,
                effect,
                COLOR_NODE_BYPASS_PARAMETER,
                previous.restore,
            )),
            gesture_started: false,
        },
        _ => AbHoldStep {
            state: previous,
            operation: None,
            gesture_started: false,
        },
    }
}

/// Whether a control change belongs to a drag gesture that is still one undo
/// entry.
///
/// Shared with the CC3 trackball and curve widgets so the rule has exactly one
/// definition.
///
/// egui reports the frame the pointer is released as `changed() == true` with
/// `dragged() == false`, so testing `dragged()` alone drops the final value out
/// of the gesture and files it as a second undo entry. `drag_stopped()` marks
/// exactly that release frame, and it carries the same coalesce key so the
/// whole drag — release included — stays one entry.
pub(crate) fn is_live_drag(slider: &egui::Response) -> bool {
    slider.dragged() || slider.drag_stopped()
}

impl KinewrightApp {
    /// Route one inspector frame's edits to the core actor.
    pub(crate) fn submit_inspector_edits(&mut self, edits: InspectorEdits) {
        // Mirror the A/B hold before the operations go out: the frame loop
        // needs a record even for the frame the hold opens, because that is
        // the frame after which the card may stop rendering (CC4 §7).
        if let Some(record) = edits.ab_hold {
            self.look_ab_hold = Some(MirroredAbHold {
                session: self.focused().id,
                record,
            });
            self.look_ab_hold_seen = true;
        } else if let Some((clip, effect)) = edits.ab_released
            && self
                .look_ab_hold
                .is_some_and(|held| held.record.clip == clip && held.record.effect == effect)
        {
            self.look_ab_hold = None;
        }
        // The overlay's input policy is the inspector's report, one frame old
        // (CC5 §6). Only an expansion is recorded: this path also carries the
        // viewer's own overlay drags, which draw no card, and reporting "no
        // section" from there would retire the report the drag is acting on.
        if let Some(target) = edits.matte_expanded {
            self.matte_overlay.report_expanded(target);
        }
        if let Some((window, window_count)) = edits.matte_selected_window {
            self.matte_overlay.select_window(window, window_count);
        }
        // Refusals go out before the operations so a card that both refused
        // one action and produced another still reports the refusal.
        for message in edits.errors {
            self.record_error("Look", message);
        }
        if edits.gesture_started {
            // Open the new gesture even when this frame produced no operation:
            // a mouse-down without movement still ends the previous gesture.
            self.begin_edit_gesture();
        }
        if !edits.operations.is_empty() {
            match edits.coalesce_key {
                Some(key) => {
                    let gesture = self.edit_gesture();
                    self.send_operations_coalesced(edits.operations, format!("{key}#{gesture}"));
                }
                None => self.send_operations(edits.operations),
            }
        }
        // Look actions run after the edits so a dialog can never block the
        // frame that produced them (CC4 §7).
        for request in edits.look_requests {
            self.handle_look_request(request);
        }
    }

    /// Restore a stranded A/B hold (CC4 §7).
    ///
    /// The hold writes a real `bypass = 1`, so a card that stops rendering
    /// while the pointer is down — a collapsed panel, a switched material tab,
    /// a deselected clip — would otherwise leave the look silently bypassed
    /// with no control left to release it. The restore goes out under the same
    /// coalesce key and the same gesture identity as the press, so the whole
    /// comparison is still exactly one undo entry.
    pub(crate) fn recover_stranded_ab_hold(&mut self, ctx: &egui::Context) {
        let Some(held) = self.look_ab_hold else {
            return;
        };
        let pointer_down = ctx.input(|input| input.pointer.primary_down());
        if !ab_hold_needs_recovery(self.look_ab_hold_seen, pointer_down) {
            // Consumed: the card has to report the hold again this frame.
            self.look_ab_hold_seen = false;
            return;
        }
        let record = held.record;
        self.look_ab_hold = None;
        self.look_ab_hold_seen = false;
        ctx.data_mut(|data| {
            data.insert_temp(
                ab_hold_id(record.clip, record.effect),
                AbHoldState::default(),
            );
        });
        // The hold belongs to one project, which may no longer be the focused
        // one — or may be closed. A node the operator deleted while holding
        // has no bypass left to restore, so the restore is dropped rather than
        // sent to be rejected.
        let Some(index) = crate::project::session_index_by_id(held.session, &self.projects) else {
            return;
        };
        if !self.projects[index]
            .document
            .clip(record.clip)
            .is_some_and(|clip| clip.effects.iter().any(|node| node.id == record.effect))
        {
            return;
        }
        let gesture = self.edit_gesture();
        let coalesce_key = format!(
            "{}#{gesture}",
            look_ab_coalesce_key(record.clip, record.effect)
        );
        if self.projects[index]
            .core
            .send(kinewright_core::Command::DoBatchCoalesced {
                operations: vec![ab_hold_restore_operation(record)],
                coalesce_key,
            })
            .is_err()
        {
            self.record_error("Look", "Core actor stopped while releasing the A/B hold");
        }
    }

    /// Perform one look action the inspector or the browser asked for.
    fn handle_look_request(&mut self, request: LookRequest) {
        match request {
            LookRequest::Import { clip, stage } => {
                let Some(path) = crate::media_workflow::choose_lut_file() else {
                    return;
                };
                self.start_lut_import(
                    path,
                    crate::media_workflow::LutImportIntent::Apply {
                        clip: Some(clip),
                        stage,
                    },
                );
            }
            LookRequest::Locate { lut_asset } => self.choose_lut_restore(lut_asset),
            LookRequest::Replace { clip, effect } => self.choose_lut_replacement(clip, effect),
            LookRequest::Browse { clip, effect } => self.look_browser.open_for(clip, effect),
            LookRequest::ConvertLegacyCube { clip, effect, path } => {
                self.start_legacy_cube_conversion(clip, effect, path);
            }
        }
    }

    pub(crate) fn inspector_dock(&mut self, ui: &mut egui::Ui) {
        let id = ui.make_persistent_id("inspector-panel");
        let mut state =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true);
        if self.focused().title_text_focus.is_some() {
            state.set_open(true);
        }
        state
            .show_header(ui, |ui| {
                ui.label(egui::RichText::new("Inspector").font(theme::semibold(type_size::BODY)))
            })
            .body(|ui| {
                egui::ScrollArea::vertical()
                    .id_salt("inspector-scroll")
                    .max_height(INSPECTOR_MAX_HEIGHT)
                    .auto_shrink([false, true])
                    .show(ui, |ui| self.inspector(ui));
            });
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        if let Some(clip) = self
            .focused()
            .selected_clip
            .and_then(|id| self.focused().document.clip(id))
            .cloned()
        {
            match &clip.content {
                ClipContent::Media => self.media_clip_inspector(ui, &clip),
                ClipContent::Title(title) => self.title_inspector(ui, &clip, title),
                ClipContent::Freeze(freeze) => self.freeze_clip_inspector(ui, &clip, freeze),
            }
        } else if let Some(marker) = self
            .focused()
            .selected_marker
            .and_then(|id| self.focused().document.marker(id))
            .filter(|marker| !is_internal_marker(marker))
            .cloned()
        {
            self.marker_inspector(ui, &marker);
        } else {
            ui.add_space(space::THREE);
            ui.colored_label(color::TEXT_MUTED, "Select a clip, title, or marker.");
            ui.add_space(space::THREE);
        }
    }

    #[allow(clippy::too_many_lines)]
    fn media_clip_inspector(&mut self, ui: &mut egui::Ui, clip: &Clip) {
        let Some(asset) = self.focused().document.asset(clip.asset).cloned() else {
            ui.colored_label(color::STATUS_DANGER, "Media asset is missing");
            return;
        };
        ui.label(egui::RichText::new(&asset.name).font(theme::semibold(type_size::BODY)));
        ui.colored_label(color::TEXT_MUTED, format!("{:?}", asset.kind));
        ui.add_space(space::ONE);
        data_row(ui, "Path", &asset.path.display().to_string());
        let status = self.media_status_for_asset(&asset);
        let source_state = source_display_state(status.as_ref());
        ui.horizontal(|ui| {
            paint_source_status(ui, source_state);
            if ui
                .button("Relink…")
                .on_hover_text("Choose a replacement and verify its source fingerprint")
                .clicked()
            {
                self.choose_relink_for_asset(asset.id);
            }
        });
        ui.colored_label(
            if source_state.blocks_preview() {
                color::STATUS_DANGER
            } else {
                color::TEXT_MUTED
            },
            source_state.description(),
        );
        data_row(ui, "Source", &range_readout(&clip.source_range, asset.fps));
        let timeline_end = self
            .focused()
            .document
            .clip_duration(clip)
            .map_or(clip.timeline_start, |duration| {
                TimeCode(clip.timeline_start.0.saturating_add(duration.0))
            });
        data_row(
            ui,
            "Timeline",
            &range_readout(
                &(clip.timeline_start..timeline_end),
                self.focused().document.fps,
            ),
        );
        if let Some((width, height)) = asset.resolution {
            data_row(ui, "Raster", &format!("{width} × {height}"));
        }

        let mut pending = InspectorEdits::default();
        ui.add_space(space::TWO);
        ui.strong("Speed");
        let mut speed_percent = clip.speed_percent;
        let speed = ui.add(
            egui::Slider::new(&mut speed_percent, 10..=1000)
                .integer()
                .custom_formatter(|value, _| format!("{:.2}x", value / 100.0))
                .custom_parser(|text| {
                    text.trim()
                        .trim_end_matches(['x', 'X'])
                        .parse::<f64>()
                        .ok()
                        .map(|value| value * 100.0)
                }),
        );
        if speed.drag_started() {
            pending.begin_gesture();
        }
        if clip.speed_percent != 100 {
            ui.colored_label(
                color::TEXT_MUTED,
                "Audio is muted while the speed is not 1.00x",
            );
        }
        if speed.changed() {
            match crate::timeline_ui::clip_speed_operations(
                &self.focused().document,
                clip.id,
                speed_percent,
            ) {
                // A drag emits one batch per frame so the preview stays live;
                // the shared key files the whole drag as one undo entry
                // instead of one per frame.
                Ok(operations) if is_live_drag(&speed) => {
                    pending.extend_live(operations, speed_coalesce_key(clip.id));
                }
                Ok(operations) => pending.extend(operations),
                Err(error) => self.record_error("Operations", error),
            }
        }
        if let Some(audio_clip) = audio_target_clip(&self.focused().document, clip.id) {
            ui.add_space(space::TWO);
            ui.strong("Audio");
            let duration = self
                .focused()
                .document
                .clip_duration(&audio_clip)
                .map_or(0, |duration| duration.0.max(0));
            let mut gain_tenth_db = audio_clip.audio_gain_tenth_db;
            let mut fade_in_frames = audio_clip.audio_fade_in_frames.0;
            let mut fade_out_frames = audio_clip.audio_fade_out_frames.0;
            let gain = ui.add(
                egui::Slider::new(&mut gain_tenth_db, -600..=120)
                    .text("Gain")
                    .integer()
                    .custom_formatter(|value, _| format!("{:+.1} dB", value / 10.0)),
            );
            if gain.drag_started() {
                pending.begin_gesture();
            }
            let mut changed = gain.changed();
            ui.horizontal(|ui| {
                ui.label("Fade in");
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut fade_in_frames)
                            .range(0..=duration.saturating_sub(fade_out_frames))
                            .suffix(" f"),
                    )
                    .changed();
                ui.label("Fade out");
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut fade_out_frames)
                            .range(0..=duration.saturating_sub(fade_in_frames))
                            .suffix(" f"),
                    )
                    .changed();
            });
            if audio_clip.audio_gain_tenth_db != 0
                || audio_clip.audio_fade_in_frames != TimeCode::ZERO
                || audio_clip.audio_fade_out_frames != TimeCode::ZERO
            {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        color::TEXT_MUTED,
                        format!(
                            "gain:{:+.1} dB  fade_in:{}f  fade_out:{}f",
                            tenth_db_to_db(audio_clip.audio_gain_tenth_db),
                            audio_clip.audio_fade_in_frames.0,
                            audio_clip.audio_fade_out_frames.0
                        ),
                    );
                    if ui.small_button("Reset").clicked() {
                        gain_tenth_db = 0;
                        fade_in_frames = 0;
                        fade_out_frames = 0;
                        changed = true;
                    }
                });
            }
            if changed {
                let operation = clip_audio_operation(
                    audio_clip.id,
                    gain_tenth_db,
                    fade_in_frames,
                    fade_out_frames,
                );
                // Only a live gain drag coalesces. A fade edit or Reset click
                // cannot happen while the gain slider is dragged, so the gain
                // response alone decides.
                if is_live_drag(&gain) {
                    pending.push_live(operation, audio_gain_coalesce_key(audio_clip.id));
                } else {
                    pending.push(operation);
                }
            }
        }

        let document = Arc::clone(&self.focused().document);
        // Cloned rather than borrowed: the availability map has one entry per
        // LUT asset, and cloning it keeps `self` free for the dispatch that
        // follows the frame.
        let availability = self.focused().lut_availability.clone();
        let qc_clipping = self.color_qc.node_clipping();
        let looks = LookInspectorContext {
            document: &document,
            availability: &availability,
            qc_clipping: &qc_clipping,
            store_unavailable: self.focused().lut_store_unavailable_reason(),
        };
        effects_section(ui, clip, &looks, &mut pending);
        transition_section(ui, &document, clip, &mut pending);
        self.submit_inspector_edits(pending);
    }

    fn freeze_clip_inspector(
        &mut self,
        ui: &mut egui::Ui,
        clip: &Clip,
        freeze: &kinewright_core::FreezeFrame,
    ) {
        let Some(asset) = self.focused().document.asset(clip.asset).cloned() else {
            ui.colored_label(color::STATUS_DANGER, "Freeze source asset is missing");
            return;
        };
        ui.label(egui::RichText::new(&asset.name).font(theme::semibold(type_size::BODY)));
        ui.colored_label(color::TEXT_MUTED, "Freeze frame");
        ui.add_space(space::ONE);
        data_row(
            ui,
            "Frozen source",
            &frame_readout(freeze.source_frame, asset.fps),
        );
        let duration = self
            .focused()
            .document
            .clip_duration(clip)
            .unwrap_or(TimeCode::ZERO);
        data_row(
            ui,
            "Duration",
            &frame_readout(duration, self.focused().document.fps),
        );
        let mut pending = InspectorEdits::default();
        let document = Arc::clone(&self.focused().document);
        // Cloned rather than borrowed: the availability map has one entry per
        // LUT asset, and cloning it keeps `self` free for the dispatch that
        // follows the frame.
        let availability = self.focused().lut_availability.clone();
        let qc_clipping = self.color_qc.node_clipping();
        let looks = LookInspectorContext {
            document: &document,
            availability: &availability,
            qc_clipping: &qc_clipping,
            store_unavailable: self.focused().lut_store_unavailable_reason(),
        };
        effects_section(ui, clip, &looks, &mut pending);
        transition_section(ui, &document, clip, &mut pending);
        self.submit_inspector_edits(pending);
    }

    #[allow(clippy::too_many_lines)]
    fn title_inspector(&mut self, ui: &mut egui::Ui, clip: &Clip, title: &Title) {
        ui.strong("Title");
        let timeline_end = self
            .focused()
            .document
            .clip_duration(clip)
            .map_or(clip.timeline_start, |duration| {
                TimeCode(clip.timeline_start.0.saturating_add(duration.0))
            });
        data_row(
            ui,
            "Timeline",
            &range_readout(
                &(clip.timeline_start..timeline_end),
                self.focused().document.fps,
            ),
        );
        let focus_title = self.focused().title_text_focus == Some(clip.id);
        if focus_title {
            self.focused_mut().title_text_focus = None;
        }
        let draft = self
            .focused_mut()
            .title_text_draft
            .get_or_insert_with(|| (clip.id, title.text.clone()));
        if draft.0 != clip.id {
            *draft = (clip.id, title.text.clone());
        }
        let response = ui
            .scope(|ui| {
                theme::apply_input_visuals(ui);
                ui.add(
                    egui::TextEdit::multiline(&mut draft.1)
                        .desired_rows(2)
                        .hint_text("Title text"),
                )
            })
            .inner;
        if focus_title {
            response.request_focus();
        }
        let submit_text = response.lost_focus()
            || (response.has_focus()
                && ui
                    .input(|input| input.modifiers.command && input.key_pressed(egui::Key::Enter)));
        let mut pending = Vec::new();
        if submit_text && draft.1 != title.text {
            pending.push(title_param_operation(
                clip.id,
                "text",
                ParamValue::Text(draft.1.clone()),
            ));
        }

        let mut size_token = title.font_size_token;
        egui::ComboBox::from_id_salt(("title-size", clip.id.0))
            .selected_text(
                TITLE_FONT_SIZES
                    .iter()
                    .find(|item| item.token == size_token)
                    .map_or("Unknown", |item| item.name),
            )
            .show_ui(ui, |ui| {
                for item in TITLE_FONT_SIZES {
                    ui.selectable_value(&mut size_token, item.token, item.name);
                }
            });
        if size_token != title.font_size_token {
            pending.push(title_param_operation(
                clip.id,
                "font_size_token",
                ParamValue::Integer(i64::from(size_token)),
            ));
        }

        let mut color_token = title.color_token;
        egui::ComboBox::from_id_salt(("title-color", clip.id.0))
            .selected_text(
                TITLE_COLORS
                    .iter()
                    .find(|item| item.token == color_token)
                    .map_or("Unknown", |item| item.name),
            )
            .show_ui(ui, |ui| {
                for item in TITLE_COLORS {
                    ui.selectable_value(&mut color_token, item.token, item.name);
                }
            });
        if color_token != title.color_token {
            pending.push(title_param_operation(
                clip.id,
                "color_token",
                ParamValue::Integer(i64::from(color_token)),
            ));
        }

        let mut position = title.position;
        egui::ComboBox::from_id_salt(("title-position", clip.id.0))
            .selected_text(position.as_str())
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut position, TitlePosition::Top, "top");
                ui.selectable_value(&mut position, TitlePosition::Center, "center");
                ui.selectable_value(&mut position, TitlePosition::LowerThird, "lower third");
            });
        if position != title.position {
            pending.push(title_param_operation(
                clip.id,
                "position",
                ParamValue::Text(position.as_str().to_owned()),
            ));
        }

        let mut scrim = title.background_scrim;
        if ui.checkbox(&mut scrim, "Background scrim").changed() {
            pending.push(title_param_operation(
                clip.id,
                "background_scrim",
                ParamValue::Boolean(scrim),
            ));
        }
        let maximum = self
            .focused()
            .document
            .clip_duration(clip)
            .map_or(0, |value| value.0.max(0));
        for (name, label, current) in [
            ("fade_in_frames", "Fade in", title.fade_in_frames.0),
            ("fade_out_frames", "Fade out", title.fade_out_frames.0),
        ] {
            let mut value = current;
            if ui
                .add(
                    egui::Slider::new(&mut value, 0..=maximum)
                        .text(label)
                        .integer(),
                )
                .changed()
            {
                pending.push(title_param_operation(
                    clip.id,
                    name,
                    ParamValue::Integer(value),
                ));
            }
        }
        self.send_operations(pending);
    }

    fn marker_inspector(&mut self, ui: &mut egui::Ui, marker: &Marker) {
        ui.strong("Marker");
        data_row(
            ui,
            "Position",
            &frame_readout(marker.position, self.focused().document.fps),
        );
        let draft = self
            .focused_mut()
            .marker_label_draft
            .get_or_insert_with(|| (marker.id, marker.label.clone()));
        if draft.0 != marker.id {
            *draft = (marker.id, marker.label.clone());
        }
        let response = ui
            .scope(|ui| {
                theme::apply_input_visuals(ui);
                ui.text_edit_singleline(&mut draft.1)
            })
            .inner;
        let mut pending = Vec::new();
        if response.lost_focus() && draft.1 != marker.label {
            pending.push(marker_param_operation(
                marker.id,
                "label",
                ParamValue::Text(draft.1.clone()),
            ));
        }
        let mut color_token = marker.color_token;
        egui::ComboBox::from_id_salt(("marker-color", marker.id.0))
            .selected_text(format!("Color {}", color_token + 1))
            .show_ui(ui, |ui| {
                for token in 0..MARKER_COLOR_TOKEN_COUNT {
                    ui.selectable_value(&mut color_token, token, format!("Color {}", token + 1));
                }
            });
        if color_token != marker.color_token {
            pending.push(marker_param_operation(
                marker.id,
                "color_token",
                ParamValue::Integer(i64::from(color_token)),
            ));
        }
        let mut position = marker.position.0;
        if ui
            .add(
                egui::DragValue::new(&mut position)
                    .range(0..=i64::MAX)
                    .prefix("Frame "),
            )
            .changed()
        {
            pending.push(marker_param_operation(
                marker.id,
                "position",
                ParamValue::Integer(position),
            ));
        }
        self.send_operations(pending);
    }
}

/// Everything a look card needs that lives outside the document (CC4 §7).
///
/// Availability is runtime state, never project state, so it is injected here
/// exactly as M41 injects media availability rather than being read back out
/// of the `Document`.
pub(crate) struct LookInspectorContext<'a> {
    pub(crate) document: &'a Document,
    pub(crate) availability: &'a BTreeMap<LutAssetId, LutAvailabilityStatus>,
    /// The last `ColorQcReport`'s per-node clipping attribution (CC6 §8.3).
    ///
    /// A snapshot cloned out of the report rather than a borrow of the app,
    /// and a *report of the last measurement* rather than a live computation:
    /// the line prints the frame it was measured at so a stale reading is
    /// visible instead of misleading.
    pub(crate) qc_clipping: &'a crate::color_qc_ui::ColorQcNodeClipping,
    /// Why this project cannot own LUT bytes, or `None` when it can.
    ///
    /// Two different refusals reach the same disabled controls: a project that
    /// was never saved (`project_not_saved`) and a saved project whose derived
    /// store root is a symlink or a non-directory (`lut_store_root_invalid`).
    /// Carrying the reason rather than a bool keeps the second one from being
    /// reported as the first, which would send the operator round a "save the
    /// project first" loop on a project they just saved (CC4 §2.2).
    pub(crate) store_unavailable: Option<String>,
}

impl LookInspectorContext<'_> {
    /// Whether imports, restores, and conversions are available.
    fn has_store(&self) -> bool {
        self.store_unavailable.is_none()
    }

    /// The disabled-control reason, for a tooltip or an inline label.
    fn store_reason(&self) -> &str {
        self.store_unavailable.as_deref().unwrap_or_default()
    }
}

/// The inspector heading for one colour stage (CC4 §7).
const fn stage_heading(stage: ColorStage) -> &'static str {
    match stage {
        ColorStage::Input => "Input transform",
        ColorStage::Correction => "Correction",
        ColorStage::Look => "Creative look",
    }
}

/// What each stage is for, so the ordering rule is visible rather than
/// discovered through a rejection.
const fn stage_hint(stage: ColorStage) -> &'static str {
    match stage {
        ColorStage::Input => "Normalizes the source. Runs before every correction.",
        ColorStage::Correction => "Exposure, balance, wheels, and curves.",
        ColorStage::Look => "Runs after every correction.",
    }
}

fn effects_section(
    ui: &mut egui::Ui,
    clip: &Clip,
    looks: &LookInspectorContext<'_>,
    pending: &mut InspectorEdits,
) {
    ui.add_space(space::TWO);
    ui.strong("Colour");
    // The three stage headings render the managed nodes in `clip.effects`
    // order within each stage, which is also the execution order: the document
    // invariant forbids a vector order that contradicts the stage order, so
    // the inspector, the manifest, and the renderer cannot disagree (CC4 §3.2).
    for stage in ColorStage::ALL {
        color_stage_section(ui, clip, stage, looks, pending);
    }

    ui.add_space(space::TWO);
    ui.strong("Effects");
    for effect in &clip.effects {
        if ColorNodeKind::from_effect_name(&effect.name).is_some() {
            continue;
        }
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label(&effect.name);
                if ui.small_button("Remove").clicked() {
                    pending.push(Operation::RemoveEffect {
                        clip: clip.id,
                        effect: effect.id,
                    });
                }
            });
            if let Some(descriptor) = EFFECT_DESCRIPTORS
                .iter()
                .find(|descriptor| descriptor.name == effect.name)
            {
                for parameter in descriptor.parameters {
                    if !should_render_effect_parameter(descriptor, parameter.name) {
                        continue;
                    }
                    let mut value = effect
                        .parameters
                        .get(parameter.name)
                        .and_then(|value| match value {
                            ParamValue::Integer(value) => Some(*value),
                            ParamValue::Boolean(_) | ParamValue::Text(_) => None,
                        })
                        .unwrap_or(parameter.neutral);
                    if ui
                        .add(
                            egui::Slider::new(&mut value, parameter.min..=parameter.max)
                                .text(parameter.name)
                                .integer(),
                        )
                        .changed()
                    {
                        pending.push(effect_param_operation(
                            clip.id,
                            effect.id,
                            parameter.name,
                            value,
                        ));
                    }
                }
            }
            if let Some(stage) = effect_compatibility_stage(&effect.name) {
                ui.colored_label(color::STATUS_WARNING, stage.inspector_warning());
                legacy_look_conversion_row(ui, clip, effect, looks, pending);
            }
        });
    }
    ui.menu_button("+ Effect", |ui| {
        for descriptor in EFFECT_DESCRIPTORS {
            if !is_effect_insertable(descriptor.name) {
                continue;
            }
            if clip
                .effects
                .iter()
                .any(|effect| effect.name == descriptor.name)
            {
                continue;
            }
            if ui.button(effect_display_name(descriptor.name)).clicked() {
                // A LUT node cannot be added with descriptor neutrals: an
                // unbound `lut_asset_id` is rejected by design (CC4 §3.3), so
                // the menu routes it into the import that binds it.
                if let Some(kind) =
                    ColorNodeKind::from_effect_name(descriptor.name).filter(|kind| kind.is_lut())
                {
                    pending.push_look(LookRequest::Import {
                        clip: clip.id,
                        stage: kind.stage(),
                    });
                } else {
                    pending.push(add_effect_operation(clip, descriptor));
                }
                ui.close();
            }
        }
    });
}

/// One colour stage's heading, insert controls, and nodes (CC4 §7).
fn color_stage_section(
    ui: &mut egui::Ui,
    clip: &Clip,
    stage: ColorStage,
    looks: &LookInspectorContext<'_>,
    pending: &mut InspectorEdits,
) {
    ui.add_space(space::TWO);
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(stage_heading(stage)).font(theme::semibold(type_size::BODY)));
        ui.colored_label(color::TEXT_MUTED, stage_hint(stage));
        stage_insert_controls(ui, clip, stage, looks, pending);
    });
    let mut rendered = 0usize;
    for (stage_index, effect) in clip.effects.iter().enumerate() {
        let Some(kind) = ColorNodeKind::from_effect_name(&effect.name) else {
            continue;
        };
        if kind.stage() != stage {
            continue;
        }
        rendered += 1;
        match kind {
            ColorNodeKind::Primary => {
                primary_correction_section(ui, clip, effect, looks, pending);
            }
            ColorNodeKind::Wheels => {
                color_wheels_section(ui, clip, effect, stage_index, looks, pending);
            }
            ColorNodeKind::Curves => {
                color_curves_section(ui, clip, effect, stage_index, looks, pending);
            }
            ColorNodeKind::TechnicalLut | ColorNodeKind::CreativeLook => {
                lut_node_section(ui, clip, effect, kind, stage_index, looks, pending);
            }
        }
    }
    if rendered == 0 {
        ui.colored_label(color::TEXT_MUTED, "No nodes in this stage.");
    }
}

/// The per-stage insert controls, each computing the `InsertEffect` index that
/// satisfies the stage order so a human can never author a violation.
fn stage_insert_controls(
    ui: &mut egui::Ui,
    clip: &Clip,
    stage: ColorStage,
    looks: &LookInspectorContext<'_>,
    pending: &mut InspectorEdits,
) {
    let index = color_stage_insert_index(&clip.effects, stage);
    match stage {
        ColorStage::Input => {
            if lut_insert_button(ui, "+ Technical LUT…", index, looks).clicked() {
                pending.push_look(LookRequest::Import {
                    clip: clip.id,
                    stage,
                });
            }
        }
        ColorStage::Look => {
            if lut_insert_button(ui, "+ Import look…", index, looks).clicked() {
                pending.push_look(LookRequest::Import {
                    clip: clip.id,
                    stage,
                });
            }
            if ui
                .small_button("Browse looks…")
                .on_hover_text("Built-in looks and this project's imported LUT assets")
                .clicked()
            {
                pending.push_look(LookRequest::Browse {
                    clip: clip.id,
                    effect: None,
                });
            }
        }
        ColorStage::Correction => {
            ui.menu_button("+ Correction", |ui| {
                for descriptor in EFFECT_DESCRIPTORS {
                    let is_correction = ColorNodeKind::from_effect_name(descriptor.name)
                        .is_some_and(|kind| kind.stage() == ColorStage::Correction);
                    if !is_correction
                        || clip
                            .effects
                            .iter()
                            .any(|effect| effect.name == descriptor.name)
                    {
                        continue;
                    }
                    if ui.button(effect_display_name(descriptor.name)).clicked() {
                        pending.push(add_effect_operation(clip, descriptor));
                        ui.close();
                    }
                }
            });
        }
    }
}

/// A stage's import button, disabled with the `project_not_saved` reason when
/// the project cannot own LUT bytes yet (CC4 §2.2).
fn lut_insert_button(
    ui: &mut egui::Ui,
    label: &str,
    index: usize,
    looks: &LookInspectorContext<'_>,
) -> egui::Response {
    let button = ui.add_enabled(looks.has_store(), egui::Button::new(label).small());
    if looks.has_store() {
        button.on_hover_text(format!(
            "Inserts at position {index} in this clip's effect stack"
        ))
    } else {
        button.on_hover_text(looks.store_reason().to_owned())
    }
}

// ---------------------------------------------------------------------------
// CC4 §7 look card
// ---------------------------------------------------------------------------

/// One availability state rendered as the media card's warning treatment.
fn availability_chip(kind: Option<LutAvailabilityKind>) -> (&'static str, egui::Color32) {
    match kind {
        Some(LutAvailabilityKind::Verified) => ("verified", color::STATUS_SUCCESS),
        Some(LutAvailabilityKind::Missing) => ("missing", color::STATUS_DANGER),
        Some(LutAvailabilityKind::Changed) => ("changed", color::STATUS_DANGER),
        Some(LutAvailabilityKind::Unreadable) => ("unreadable", color::STATUS_DANGER),
        None => ("unchecked", color::STATUS_WARNING),
    }
}

/// One asset's provenance, as the browser and the manifest report it.
fn provenance_label(asset: &LutAsset) -> String {
    match &asset.source {
        LutAssetSource::Builtin { name } => format!("built-in · {name}"),
        LutAssetSource::Imported { source_path } => std::path::Path::new(source_path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map_or_else(|| source_path.clone(), std::borrow::ToOwned::to_owned),
    }
}

/// The CC4 §7 `technical_lut` / `creative_look` card.
///
/// Title, provenance, and availability; the mix slider on a creative look
/// only; a bypass toggle; the press-and-hold A/B; a reset that excludes the
/// binding; the keyframe rows; and the recovery banner for an asset whose
/// bytes are not where the project says they are.
#[allow(clippy::too_many_lines)]
fn lut_node_section(
    ui: &mut egui::Ui,
    clip: &Clip,
    effect: &Effect,
    kind: ColorNodeKind,
    stage_index: usize,
    looks: &LookInspectorContext<'_>,
    pending: &mut InspectorEdits,
) {
    let Some(descriptor) = kinewright_core::effect_descriptor(kind.effect_name()) else {
        return;
    };
    let params = LutNodeParams::from_effect(effect);
    let asset = looks.document.lut_asset(params.lut_asset_id);
    let availability = asset.map(|asset| looks.availability.get(&asset.id));
    ui.group(|ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(effect_display_name(kind.effect_name()))
                    .font(theme::semibold(type_size::BODY)),
            );
            ui.colored_label(
                color::TEXT_MUTED,
                format!("Stage {stage_index} · {}", kind.role()),
            );
            let mut bypass = params.bypass();
            if ui
                .checkbox(&mut bypass, "Bypass")
                .on_hover_text(
                    "A bypassed node keeps its position and every value and renders as the exact \
                     identity (CC4 §3.6).",
                )
                .changed()
            {
                pending.push(effect_param_operation(
                    clip.id,
                    effect.id,
                    COLOR_NODE_BYPASS_PARAMETER,
                    i64::from(bypass),
                ));
            }
            if ui
                .small_button("Reset look controls")
                .on_hover_text(
                    "Return the mix, encoding, and bypass to their neutrals. The binding is kept: \
                     unbinding a node is rejected (CC4 §6).",
                )
                .clicked()
            {
                pending.extend(color_node_reset_operations(clip.id, effect, &descriptor));
            }
            if ui.small_button("Remove").clicked() {
                pending.push(Operation::RemoveEffect {
                    clip: clip.id,
                    effect: effect.id,
                });
            }
        });

        match asset {
            Some(asset) => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(&asset.title).strong());
                    ui.colored_label(
                        color::TEXT_MUTED,
                        format!("{}³ · {}", asset.size, provenance_label(asset)),
                    );
                    let (chip_text, chip_color) =
                        availability_chip(availability.flatten().map(|status| status.kind));
                    ui.colored_label(chip_color, chip_text);
                    if ui
                        .small_button("Change…")
                        .on_hover_text("Pick another look for this node")
                        .clicked()
                    {
                        pending.push_look(LookRequest::Browse {
                            clip: clip.id,
                            effect: Some(effect.id),
                        });
                    }
                });
                ui.monospace(
                    egui::RichText::new(&asset.sha256[..16.min(asset.sha256.len())])
                        .size(type_size::CAPTION)
                        .color(color::TEXT_MUTED),
                );
                lut_recovery_banner(
                    ui,
                    clip,
                    effect,
                    asset,
                    availability.flatten(),
                    looks,
                    pending,
                );
            }
            None => {
                ui.colored_label(
                    color::STATUS_DANGER,
                    format!(
                        "This node references LUT asset {}, which this project does not record.",
                        params.lut_asset_id
                    ),
                );
            }
        }

        if kind == ColorNodeKind::CreativeLook {
            look_mix_row(ui, clip, effect, params, pending);
            look_ab_row(ui, clip, effect, params, pending);
        } else {
            ui.colored_label(
                color::TEXT_MUTED,
                "Mix is pinned at full strength: a partially applied technical normalization is \
                 not a meaningful state (CC4 §5.1).",
            );
        }
        ui.colored_label(
            color::TEXT_MUTED,
            format!(
                "Input encoding: {}",
                input_encoding_label(params.input_encoding_token)
            ),
        );
        if let Some(reason) = color_node_inactive_reason(effect) {
            ui.colored_label(
                color::TEXT_MUTED,
                format!("Inactive for this frame: {}", reason.as_str()),
            );
        }
        color_node_clipping_line(ui, clip, effect, looks);
        let names: Vec<&'static str> = descriptor
            .parameters
            .iter()
            .map(|parameter| parameter.name)
            .filter(|name| !is_matte_parameter(name))
            .collect();
        color_node_keyframe_rows(ui, clip.id, effect, &names, pending);
        // CC5 §2.1: a technical input transform normalizes the whole source,
        // so it carries no matte and gets no section.
        if kind == ColorNodeKind::CreativeLook {
            matte_section(ui, clip, effect, pending);
        }
    });
}

/// The `0..=100 %` mix slider, writing `mix_basis_points = percent * 100`.
///
/// A drag emits one operation per frame under the node's mix key so the
/// preview stays live; `is_live_drag` keeps the release frame inside the
/// gesture, so the whole drag is exactly one undo entry (CC4 §7).
fn look_mix_row(
    ui: &mut egui::Ui,
    clip: &Clip,
    effect: &Effect,
    params: LutNodeParams,
    pending: &mut InspectorEdits,
) {
    let mut percent = mix_percent(params.mix_basis_points, LUT_MIX_BASIS_POINTS_MAX);
    ui.horizontal(|ui| {
        let slider = ui.add(mix_slider(&mut percent, LUT_MIX_BASIS_POINTS_MAX, "Mix").suffix(" %"));
        if slider.drag_started() {
            pending.begin_gesture();
        }
        if slider.changed() {
            let operation = effect_param_operation(
                clip.id,
                effect.id,
                LUT_MIX_PARAMETER,
                percent.saturating_mul(100),
            );
            if is_live_drag(&slider) {
                pending.push_live(operation, look_mix_coalesce_key(clip.id, effect.id));
            } else {
                pending.push(operation);
            }
        }
        ui.monospace(format!("{} bp", params.mix_basis_points));
    });
}

/// A `*_mix_basis_points` as its slider's whole percent.
///
/// `max_basis_points` is the bound of the parameter being shown, not a shared
/// constant: `mix_basis_points` and `matte_mix_basis_points` are separate
/// contracts (CC4 §7, CC5 §2.2) that happen to agree on 10000 today, and
/// clamping one to the other's bound is a coincidence waiting to become a bug.
fn mix_percent(basis_points: i64, max_basis_points: i64) -> i64 {
    basis_points.clamp(0, max_basis_points) / 100
}

/// The whole-percent range a mix slider offers, derived from the same bound
/// [`mix_percent`] converts against.
///
/// Hard-coding `0..=100` beside a `mix_percent(bp, MAX)` call ties the control
/// to today's coincidence that both mix contracts stop at 10000 bp: raise one
/// descriptor's `max` and the slider would silently clamp the value the card
/// just read back down, which reads as a control that refuses to hold what the
/// document stores.
fn mix_percent_range(max_basis_points: i64) -> std::ops::RangeInclusive<i64> {
    0..=(max_basis_points.max(0) / 100)
}

/// A whole-percent mix slider bounded by its own parameter's max.
///
/// Shared by the look mix (CC4 §7) and the matte mix (CC5 §2.2) so neither can
/// be built with the other's bound.
fn mix_slider<'a>(percent: &'a mut i64, max_basis_points: i64, text: &str) -> egui::Slider<'a> {
    egui::Slider::new(percent, mix_percent_range(max_basis_points))
        .text(text.to_owned())
        .integer()
}

/// The press-and-hold A/B control (CC4 §7).
///
/// The hold writes the real `bypass = 1` through the coalesced gesture path
/// and restores the captured value on release, so the comparison is provably
/// lossless rather than a preview shortcut.
fn look_ab_row(
    ui: &mut egui::Ui,
    clip: &Clip,
    effect: &Effect,
    params: LutNodeParams,
    pending: &mut InspectorEdits,
) {
    // `LutNodeParams::from_effect` reads the *static* `bypass`. A keyframed
    // `bypass` is evaluated per frame from the curve, so a hold would write a
    // static `1` the curve immediately overrides: the comparison would do
    // nothing visible while still filing an undo entry. CC4 §7 wants the
    // keyframed state badged, not silently mis-served.
    let keyframed = !ab_hold_is_available(effect);
    let id = ab_hold_id(clip.id, effect.id);
    let previous: AbHoldState = ui.data(|data| data.get_temp(id)).unwrap_or_default();
    let response = ui.add_enabled(!keyframed, egui::Button::new("A / B (hold)").small());
    let response = if keyframed {
        response.on_disabled_hover_text(
            "bypass is keyframed on this node, so a hold would write a static value the curve \
             overrides. Clear the bypass keyframes first.",
        )
    } else {
        response.on_hover_text(
            "Hold to bypass this look and release to restore it. One hold is one undo entry.",
        )
    };
    let step = ab_hold_step(
        clip.id,
        effect.id,
        previous,
        !keyframed && response.is_pointer_button_down_on(),
        params.bypass_token,
    );
    ui.data_mut(|data| data.insert_temp(id, step.state));
    if step.gesture_started {
        pending.begin_gesture();
    }
    if let Some(operation) = step.operation {
        pending.push_live(operation, look_ab_coalesce_key(clip.id, effect.id));
    }
    if step.state.held {
        pending.record_ab_hold(AbHoldRecord {
            clip: clip.id,
            effect: effect.id,
            restore: step.state.restore,
        });
        ui.colored_label(color::STATUS_WARNING, "Bypassed while held");
    } else if previous.held {
        // The card released the hold itself, so the app's mirror retires
        // without a second restore.
        pending.record_ab_release(clip.id, effect.id);
    }
    if keyframed {
        ui.colored_label(color::STATUS_WARNING, "KEYFRAMED");
    }
}

/// The `display709` / `linear` / `grade709` token as its contract name.
const fn input_encoding_label(token: i64) -> &'static str {
    match token {
        1 => "linear",
        2 => "grade709",
        _ => "display709",
    }
}

/// The inline banner a `missing` or `changed` asset shows on every node that
/// references it, with its two typed recovery actions (CC4 §2.3, §7).
fn lut_recovery_banner(
    ui: &mut egui::Ui,
    clip: &Clip,
    effect: &Effect,
    asset: &LutAsset,
    availability: Option<&LutAvailabilityStatus>,
    looks: &LookInspectorContext<'_>,
    pending: &mut InspectorEdits,
) {
    let Some(status) = availability else {
        return;
    };
    if status.kind == LutAvailabilityKind::Verified {
        return;
    }
    ui.colored_label(
        color::STATUS_DANGER,
        format!(
            "{} is {:?}. Export and proof are blocked until it is restored or replaced.",
            asset.title, status.kind
        ),
    );
    if let Some(path) = &status.path {
        ui.colored_label(color::TEXT_MUTED, format!("Expected at {}", path.display()));
    }
    if let Some(reason) = &status.reason {
        ui.colored_label(color::TEXT_MUTED, reason);
    }
    ui.horizontal_wrapped(|ui| {
        let locate = ui.add_enabled(looks.has_store(), egui::Button::new("Locate file…").small());
        if locate
            .on_hover_text(
                "Point at the original file. Its bytes are hashed and accepted only on an exact \
                 match, so a restore can never substitute a different look.",
            )
            .clicked()
        {
            pending.push_look(LookRequest::Locate {
                lut_asset: asset.id,
            });
        }
        let replace = ui.add_enabled(looks.has_store(), egui::Button::new("Replace…").small());
        if replace
            .on_hover_text(
                "Import a different LUT and retarget this node. A different LUT is a different \
                 asset: no operation ever rewrites a hash in place.",
            )
            .clicked()
        {
            pending.push_look(LookRequest::Replace {
                clip: clip.id,
                effect: effect.id,
            });
        }
        if let Some(reason) = &looks.store_unavailable {
            ui.colored_label(color::TEXT_MUTED, reason);
        }
    });
}

/// Route one **Convert to managed look** click (CC4 §7, §9).
///
/// A `look_lut` resolves its `preset_token` to a built-in generated asset and
/// converts in one visible batch. A `cube_lut` names an external file the
/// import worker has to place in the store first.
///
/// Every refusal goes to the app's error log. Drawing it inside the click
/// branch showed it for exactly one frame — the frame the pointer came up — so
/// a conversion that could not be built was indistinguishable from a button
/// that did nothing.
fn request_legacy_conversion(
    document: &Document,
    clip: ClipId,
    effect: &Effect,
    pending: &mut InspectorEdits,
) {
    if effect.name == "look_lut" {
        match convert_builtin_look_operations(document, clip, effect) {
            Ok(operations) => pending.extend(operations),
            Err(reason) => pending.push_error(reason),
        }
        return;
    }
    let Some(path) = stored_text(effect, "path") else {
        pending.push_error(format!(
            "legacy {} node {} stores no path, so there is no .cube to import and convert",
            effect.name, effect.id
        ));
        return;
    };
    pending.push_look(LookRequest::ConvertLegacyCube {
        clip,
        effect: effect.id,
        path: std::path::PathBuf::from(path),
    });
}

/// The **Convert to managed look** control on a legacy `look_lut` / `cube_lut`
/// (CC4 §7, §9).
///
/// A `look_lut` resolves its `preset_token` to a built-in generated asset and
/// converts in one visible `[AddLutAsset, ConvertLegacyLook]` batch. A
/// `cube_lut` names an external file, which has to be imported into the store
/// first, so it routes through the import worker and converts on the response.
fn legacy_look_conversion_row(
    ui: &mut egui::Ui,
    clip: &Clip,
    effect: &Effect,
    looks: &LookInspectorContext<'_>,
    pending: &mut InspectorEdits,
) {
    if !matches!(effect.name.as_str(), "look_lut" | "cube_lut") {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        let convert = ui.add_enabled(
            looks.has_store(),
            egui::Button::new("Convert to managed look").small(),
        );
        // A legacy stage authored before a managed correction cannot become a
        // creative look where it stands, so the batch moves it to the first
        // legal Look position instead of being rejected (CC4 §3.2, §9).
        let reorders = !legacy_conversion_keeps_stage_order(&clip.effects, effect.id);
        let hover = if reorders {
            "Replaces this legacy stage with a managed creative look. It sits before a managed \
             correction, and a creative look runs after every correction, so the conversion also \
             moves it to the end of the colour stack. The result is not bit-identical: the legacy \
             path clamped to [0, 1] in display space and mixed in the encoded domain (CC4 §9). \
             Undoable."
        } else {
            "Replaces this legacy stage with a managed creative look at the same position. \
             The result is not bit-identical: the legacy path clamped to [0, 1] in display \
             space and mixed in the encoded domain (CC4 §9). Undoable."
        };
        if convert.on_hover_text(hover).clicked() {
            request_legacy_conversion(looks.document, clip.id, effect, pending);
        }
        if let Some(reason) = &looks.store_unavailable {
            ui.colored_label(color::TEXT_MUTED, reason);
        } else if reorders {
            ui.colored_label(
                color::TEXT_MUTED,
                "Converting moves this stage after the managed corrections.",
            );
        }
    });
}

fn primary_correction_section(
    ui: &mut egui::Ui,
    clip: &Clip,
    effect: &Effect,
    looks: &LookInspectorContext<'_>,
    pending: &mut InspectorEdits,
) {
    let Some(descriptor) = EFFECT_DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.name == "primary_correction")
    else {
        return;
    };

    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Primary correction").strong());
            ui.colored_label(color::TEXT_MUTED, "Managed SDR");
            if ui.small_button("Remove").clicked() {
                pending.push(Operation::RemoveEffect {
                    clip: clip.id,
                    effect: effect.id,
                });
            }
        });
        ui.horizontal(|ui| {
            ui.colored_label(
                color::TEXT_MUTED,
                "Exposure · white balance · tone · saturation",
            );
            if ui.small_button("Reset Primary").clicked() {
                pending.extend(color_node_reset_operations(clip.id, effect, descriptor));
            }
        });

        for parameter in descriptor.parameters {
            // CC5 §6: the 47 matte integers belong to the matte section, never
            // to a generic slider loop.
            if !should_render_effect_parameter(descriptor, parameter.name) {
                continue;
            }
            let mut value = effect
                .parameters
                .get(parameter.name)
                .and_then(|value| match value {
                    ParamValue::Integer(value) => Some(*value),
                    ParamValue::Boolean(_) | ParamValue::Text(_) => None,
                })
                .unwrap_or(parameter.neutral);
            let keyframed = parameter_is_keyframed(effect, parameter.name);
            ui.horizontal(|ui| {
                let mut slider = ui.add(
                    egui::Slider::new(&mut value, parameter.min..=parameter.max)
                        .text(primary_parameter_label(parameter.name))
                        .integer(),
                );
                if keyframed {
                    slider = slider.on_hover_text(
                        "Automation drives this parameter. The slider shows the static value; \
                         clear the keyframes to grade it directly.",
                    );
                }
                ui.monospace(primary_parameter_readout(parameter.name, value));
                if slider.drag_started() {
                    pending.begin_gesture();
                }
                if slider.changed() {
                    let operation =
                        effect_param_operation(clip.id, effect.id, parameter.name, value);
                    if is_live_drag(&slider) {
                        // One batch per frame keeps the preview live; the key
                        // keeps the whole drag as one undo entry.
                        pending.push_live(
                            operation,
                            primary_coalesce_key(clip.id, effect.id, parameter.name),
                        );
                    } else {
                        pending.push(operation);
                    }
                }
                if keyframed {
                    ui.colored_label(color::STATUS_WARNING, "KEYFRAMED");
                    if ui
                        .small_button("Clear keyframes")
                        .on_hover_text(
                            "Remove this parameter's automation so the slider value applies.",
                        )
                        .clicked()
                    {
                        pending.push(clear_keyframes_operation(
                            clip.id,
                            effect.id,
                            parameter.name,
                        ));
                    }
                }
            });
        }
        color_node_clipping_line(ui, clip, effect, looks);
        matte_section(ui, clip, effect, pending);
    });
}

/// Reset one effect: every descriptor parameter set to its neutral, plus
/// `ClearEffectKeyframes` for each parameter that carries automation, emitted
/// as one batch and therefore one undo entry.
///
/// CC3 §5 names this the `primary_reset_operations` pattern; it is the same
/// code for `primary_correction`, `color_wheels`, and `color_curves`, which is
/// why CC3 introduces no new operation kind.
///
/// `color_curves` is the one node whose parameters cannot be written in
/// descriptor order: core re-validates the strictly-increasing-`x` rule on
/// every intermediate document, and descriptor order writes `x0` before `x1`.
/// From a stored `x0 = -2000, x1 = -1000` that first write already crosses, so
/// the operation - and with it the whole reset batch - would be rejected. The
/// curve half is therefore routed through the same ordering strategy the curve
/// editor uses.
fn color_node_reset_operations(
    clip: ClipId,
    effect: &Effect,
    descriptor: &kinewright_core::EffectDescriptor,
) -> Vec<Operation> {
    let mut operations = Vec::with_capacity(descriptor.parameters.len() + effect.keyframes.len());
    if kinewright_core::ColorNodeKind::from_effect_name(descriptor.name)
        == Some(kinewright_core::ColorNodeKind::Curves)
    {
        for curve in ColorCurveChannel::ALL {
            operations.extend(curve_reset_parameter_operations(clip, effect, curve));
        }
        // `bypass` is node-owned rather than curve-owned; it is written after
        // every curve is back at the structural identity so the batch never
        // depends on the order the two halves happen to land in.
        operations.push(effect_param_operation(
            clip,
            effect.id,
            COLOR_NODE_BYPASS_PARAMETER,
            0,
        ));
        // CC5 §5: resetting a matte-capable node resets its matte too. The
        // matte parameters are independently bounded, so unlike the curve
        // points they need no ordering strategy.
        for parameter in descriptor.parameters {
            if is_matte_parameter(parameter.name) && matte_reset_needs_write(effect, parameter) {
                operations.push(effect_param_operation(
                    clip,
                    effect.id,
                    parameter.name,
                    parameter.neutral,
                ));
            }
        }
        for parameter in descriptor.parameters {
            if parameter_is_keyframed(effect, parameter.name) {
                operations.push(clear_keyframes_operation(clip, effect.id, parameter.name));
            }
        }
        return operations;
    }
    for parameter in descriptor.parameters {
        // CC4 §6: resetting a LUT node would unbind it (`lut_asset_id -> 0`),
        // which `validate_document` rejects, so the batch excludes that
        // parameter entirely — both its `SetEffectParam` and its
        // `ClearEffectKeyframes`, so a `Hold`-automated binding survives a
        // reset. The inspector labels this "Reset look controls".
        if parameter.name == LUT_ASSET_ID_PARAMETER && is_lut_color_node(descriptor.name) {
            continue;
        }
        if !is_matte_parameter(parameter.name) || matte_reset_needs_write(effect, parameter) {
            operations.push(effect_param_operation(
                clip,
                effect.id,
                parameter.name,
                parameter.neutral,
            ));
        }
        if parameter_is_keyframed(effect, parameter.name) {
            operations.push(clear_keyframes_operation(clip, effect.id, parameter.name));
        }
    }
    operations
}

/// Whether a reset has to write this matte parameter's neutral.
///
/// An omitted parameter already resolves to its neutral (CC5 §2.2), and one
/// stored *at* its neutral is already there, so writing either changes nothing
/// except to add an entry. Resetting a CC4-era node that never carried a matte
/// would otherwise grow its stored JSON by all 47, and a one-window node by the
/// 24 belonging to the three empty slots.
///
/// The keyframe clear is deliberately **not** gated on this: a static neutral
/// under an automation curve still renders the curve, so the clear is what
/// makes the reset a reset.
fn matte_reset_needs_write(
    effect: &Effect,
    parameter: &kinewright_core::EffectParameterDescriptor,
) -> bool {
    match effect.parameters.get(parameter.name) {
        None => false,
        Some(stored) => *stored != ParamValue::Integer(parameter.neutral),
    }
}

// ---------------------------------------------------------------------------
// CC5 §6 matte section
// ---------------------------------------------------------------------------

/// The `matte_*` node controls the section writes by name.
///
/// The names are the CC5 §2.2 contract strings rather than indices into
/// [`kinewright_core::matte_parameter_names`], so a reordering of that table
/// cannot silently retarget a control here; a test asserts every one of them is
/// a real matte parameter.
const MATTE_ENABLED_PARAMETER: &str = "matte_enabled";
const MATTE_WINDOW_COUNT_PARAMETER: &str = "matte_window_count";
const MATTE_COMBINE_PARAMETER: &str = "matte_combine_token";
const MATTE_INVERT_PARAMETER: &str = "matte_invert";
const MATTE_MIX_PARAMETER: &str = "matte_mix_basis_points";

/// The qualifier leg's ten parameters, enable first, in descriptor order.
const MATTE_QUALIFIER_PARAMETERS: [&str; 10] = [
    "matte_qualifier_enabled",
    "matte_hue_center_centidegrees",
    "matte_hue_width_centidegrees",
    "matte_hue_softness_centidegrees",
    "matte_saturation_low_basis_points",
    "matte_saturation_high_basis_points",
    "matte_saturation_softness_basis_points",
    "matte_luma_low_basis_points",
    "matte_luma_high_basis_points",
    "matte_luma_softness_basis_points",
];

/// Human labels for the qualifier scalars, in [`MATTE_QUALIFIER_PARAMETERS`]
/// order after the enable toggle.
const MATTE_QUALIFIER_LABELS: [&str; 9] = [
    "Hue centre (cd)",
    "Hue width (cd)",
    "Hue softness (cd)",
    "Sat low (bp)",
    "Sat high (bp)",
    "Sat softness (bp)",
    "Luma low (bp)",
    "Luma high (bp)",
    "Luma softness (bp)",
];

/// The label of the tracking control (CC5 §6).
pub(crate) const MATTE_TRACK_BUTTON_LABEL: &str = "Track window…";

/// Why the tracking control is present but disabled (CC5 §6).
pub(crate) const MATTE_TRACK_BUTTON_TOOLTIP: &str = "Tracking is agent-driven in CC5: ask the \
     agent to run track_matte_window. The app has no agent-tool call path, so this button would \
     pretend to work. Once the prepared plan is committed, its keyframes appear on this card.";

/// CC5 defers manual keyframe authoring, and says so rather than implying it.
const MATTE_KEYFRAME_NOTE: &str = "Automation is shown and clearable here. Setting keyframes by \
     hand is deferred in CC5 (§11); editing a keyframed control writes its static value.";

/// The Hold-only rule, surfaced where the tokens are (CC5 §5.1).
const MATTE_HOLD_ONLY_NOTE: &str =
    "Tokens and counts accept Hold keyframes only; core rejects any other interpolation.";

/// Stable coalesce key for one window-move gesture (CC5 §6).
pub(crate) fn matte_window_move_coalesce_key(
    clip: ClipId,
    effect: EffectId,
    window: usize,
) -> String {
    format!("matte_window_move:{}:{}:{window}", clip.0, effect.0)
}

/// Stable coalesce key for one window-resize gesture (CC5 §6).
pub(crate) fn matte_window_resize_coalesce_key(
    clip: ClipId,
    effect: EffectId,
    window: usize,
) -> String {
    format!("matte_window_resize:{}:{}:{window}", clip.0, effect.0)
}

/// Stable coalesce key for one window-rotate gesture (CC5 §6).
pub(crate) fn matte_window_rotate_coalesce_key(
    clip: ClipId,
    effect: EffectId,
    window: usize,
) -> String {
    format!("matte_window_rotate:{}:{}:{window}", clip.0, effect.0)
}

/// Stable coalesce key for one matte mix drag (CC5 §6).
pub(crate) fn matte_mix_coalesce_key(clip: ClipId, effect: EffectId) -> String {
    format!("matte_mix:{}:{}", clip.0, effect.0)
}

/// The coalesce key one overlay gesture belongs to (CC5 §6).
pub(crate) fn matte_gesture_coalesce_key(
    hit: MatteHit,
    clip: ClipId,
    effect: EffectId,
    window: usize,
) -> String {
    match hit {
        MatteHit::Move => matte_window_move_coalesce_key(clip, effect, window),
        MatteHit::Resize(_) => matte_window_resize_coalesce_key(clip, effect, window),
        MatteHit::Rotate => matte_window_rotate_coalesce_key(clip, effect, window),
    }
}

/// A per-control key for the matte scalars the overlay never drives.
fn matte_parameter_coalesce_key(clip: ClipId, effect: EffectId, name: &str) -> String {
    format!("matte_param:{}:{}:{name}", clip.0, effect.0)
}

/// One window's eight stored integers, in `matte_window_parameter_names` order.
fn matte_window_values(window: &MatteWindowParams) -> [i64; 8] {
    [
        window.shape_token,
        window.center_x_bp,
        window.center_y_bp,
        window.half_width_bp,
        window.half_height_bp,
        window.rotation_cd,
        window.feather_bp,
        window.invert,
    ]
}

/// Write window `index` to `values`: one `SetEffectParam` per control that
/// actually changes.
///
/// Every window control is independently bounded, so the batch is valid in any
/// order and every intermediate document is valid — which is what lets the
/// remove path shift a later window down before it decrements the count.
///
/// `stored` is what the slot holds now. A parameter already at its new value is
/// skipped, the same rule [`matte_reset_needs_write`] applies: re-stating a
/// value writes it into the stored map, which is how a shift down the window
/// list turns unstored neutrals into stored ones and grows the project file
/// with parameters nobody set.
#[must_use]
pub(crate) fn matte_window_edit_operations(
    clip: ClipId,
    effect: EffectId,
    index: usize,
    values: &MatteWindowParams,
    stored: &MatteWindowParams,
) -> Vec<Operation> {
    let Some(names) = kinewright_core::matte_window_parameter_names(index) else {
        return Vec::new();
    };
    names
        .iter()
        .zip(matte_window_values(values))
        .zip(matte_window_values(stored))
        .filter(|((_, value), previous)| value != previous)
        .map(|((name, value), _)| effect_param_operation(clip, effect, name, value))
        .collect()
}

/// Move window `from`'s automation onto window `to`, or clear `to`'s when
/// `from` is `None` (CC5 §5.1).
///
/// CC5 §5.1 makes a window's centre, half-extents, rotation and feather fully
/// keyframable and its tokens `Hold`-keyframable, so a slot rewrite that moved
/// only the eight stored integers would leave the rewritten window rendering
/// the *previous* occupant's curves — and a keyframed parameter is resolved from
/// its curve, so those curves override the values the rewrite just wrote. The
/// tracks travel with the values, which is also what makes a tracked window
/// survive the removal of the window above it.
///
/// Only parameters that actually carry automation produce an operation, so an
/// unkeyframed matte's Add and Remove batches are byte-identical to before.
fn matte_window_keyframe_shift_operations(
    clip: ClipId,
    effect: &Effect,
    to: usize,
    from: Option<usize>,
) -> Vec<Operation> {
    let Some(destination) = kinewright_core::matte_window_parameter_names(to) else {
        return Vec::new();
    };
    let source = from.and_then(kinewright_core::matte_window_parameter_names);
    let mut operations = Vec::new();
    for (control, name) in destination.iter().enumerate() {
        match source.and_then(|names| effect.keyframes.get(names[control])) {
            Some(curve) => operations.push(Operation::SetEffectKeyframes {
                clip,
                effect: effect.id,
                name: (*name).to_owned(),
                curve: curve.clone(),
            }),
            // Nothing to move: the destination must not keep its own curve, or
            // the freshly written statics would be overridden by the automation
            // of a window that no longer exists there.
            None if effect.keyframes.contains_key(*name) => {
                operations.push(clear_keyframes_operation(clip, effect.id, name));
            }
            None => {}
        }
    }
    operations
}

/// Add one geometric window (CC5 §6).
///
/// The new window is the descriptor neutral — a centred rect covering the
/// middle half of the frame — so only what the slot does not already store is
/// written; a fresh node therefore grows by exactly the count, and a slot left
/// behind by an earlier removal is reset instead of resurfacing. `matte_enabled`
/// is written first when the master switch is off, because a window on a
/// disabled matte would draw an outline over a node that ignores it. The count
/// is written last, so no intermediate document names a window whose parameters
/// have not landed yet.
#[must_use]
pub(crate) fn matte_add_window_operations(clip: ClipId, effect: &Effect) -> Vec<Operation> {
    let params = MatteParams::from_effect(effect);
    let index = params.window_count;
    if index >= MATTE_WINDOW_LIMIT {
        return Vec::new();
    }
    let Some(names) = kinewright_core::matte_window_parameter_names(index) else {
        return Vec::new();
    };
    let mut operations = Vec::with_capacity(names.len() + 2);
    if !params.is_enabled() {
        operations.push(effect_param_operation(
            clip,
            effect.id,
            MATTE_ENABLED_PARAMETER,
            1,
        ));
    }
    let stored = params.windows[index];
    let fresh = MatteWindowParams::NEUTRAL;
    for ((name, value), previous) in names
        .iter()
        .zip(matte_window_values(&fresh))
        .zip(matte_window_values(&stored))
    {
        if value != previous {
            operations.push(effect_param_operation(clip, effect.id, name, value));
        }
    }
    // A recycled slot may still carry the automation of the window that used to
    // live in it, which would animate the "fresh" window off its neutral on the
    // very first frame (CC5 §5.1).
    operations.extend(matte_window_keyframe_shift_operations(
        clip, effect, index, None,
    ));
    operations.push(effect_param_operation(
        clip,
        effect.id,
        MATTE_WINDOW_COUNT_PARAMETER,
        i64::try_from(index + 1).unwrap_or(0),
    ));
    operations
}

/// Remove window `index` (CC5 §6).
///
/// Later windows shift down first and the count is decremented last, so every
/// intermediate document is a legal project: the count never names a slot whose
/// values have not been written yet, and the windows keep their order.
#[must_use]
pub(crate) fn matte_remove_window_operations(
    clip: ClipId,
    effect: &Effect,
    index: usize,
) -> Vec<Operation> {
    let params = MatteParams::from_effect(effect);
    if index >= params.window_count {
        return Vec::new();
    }
    let mut operations = Vec::new();
    let vacated = params.window_count - 1;
    for slot in index..vacated {
        operations.extend(matte_window_edit_operations(
            clip,
            effect.id,
            slot,
            &params.windows[slot + 1],
            &params.windows[slot],
        ));
        // The automation moves with the values it belongs to (CC5 §5.1).
        operations.extend(matte_window_keyframe_shift_operations(
            clip,
            effect,
            slot,
            Some(slot + 1),
        ));
    }
    // The slot the shift emptied keeps its stale statics — `Add window` resets
    // those — but it must not keep automation, or the next Add would resurrect
    // the removed window's motion under a neutral-looking card.
    operations.extend(matte_window_keyframe_shift_operations(
        clip, effect, vacated, None,
    ));
    operations.push(effect_param_operation(
        clip,
        effect.id,
        MATTE_WINDOW_COUNT_PARAMETER,
        i64::try_from(vacated).unwrap_or(0),
    ));
    operations
}

/// Write the whole qualifier leg from one resolved value (CC5 §2.2).
#[must_use]
pub(crate) fn matte_qualifier_operations(
    clip: ClipId,
    effect: EffectId,
    qualifier: &MatteQualifierParams,
) -> Vec<Operation> {
    let values = [
        qualifier.enabled,
        qualifier.hue_center_cd,
        qualifier.hue_width_cd,
        qualifier.hue_softness_cd,
        qualifier.sat_low_bp,
        qualifier.sat_high_bp,
        qualifier.sat_softness_bp,
        qualifier.luma_low_bp,
        qualifier.luma_high_bp,
        qualifier.luma_softness_bp,
    ];
    MATTE_QUALIFIER_PARAMETERS
        .iter()
        .zip(values)
        .map(|(name, value)| effect_param_operation(clip, effect, name, value))
        .collect()
}

/// The parameters one overlay gesture owns, and nothing else (CC5 §6).
///
/// A move writes exactly two parameters, which is why the overlay uses the
/// multi-operation live push; a rotate writes one; a resize writes the axes its
/// handle drives.
#[must_use]
pub(crate) fn matte_window_drag_operations(
    clip: ClipId,
    effect: EffectId,
    index: usize,
    hit: MatteHit,
    values: &MatteWindowParams,
) -> Vec<Operation> {
    let Some(names) = kinewright_core::matte_window_parameter_names(index) else {
        return Vec::new();
    };
    let mut operations = Vec::with_capacity(2);
    let mut write = |slot: usize, value: i64| {
        operations.push(effect_param_operation(clip, effect, names[slot], value));
    };
    match hit {
        MatteHit::Move => {
            write(1, values.center_x_bp);
            write(2, values.center_y_bp);
        }
        MatteHit::Resize(handle) => {
            if handle.drives_width() {
                write(3, values.half_width_bp);
            }
            if handle.drives_height() {
                write(4, values.half_height_bp);
            }
        }
        MatteHit::Rotate => write(5, values.rotation_cd),
    }
    operations
}

/// The CC5 §6 matte section, collapsed by default so a CC4 project's inspector
/// is unchanged.
///
/// The section reports its own expansion through [`InspectorEdits`]; the viewer
/// reads that report on the following frame to decide whether it takes pointer
/// input at all (CC5 §6, §12).
fn matte_section(ui: &mut egui::Ui, clip: &Clip, effect: &Effect, pending: &mut InspectorEdits) {
    let params = MatteParams::from_effect(effect);
    let id = matte_section_id(clip.id, effect.id);
    // `CollapsingState` rather than `CollapsingHeader` so the open state has an
    // id the rest of the app — and a test without a window — can name, and so
    // the report is the stored state rather than an animation frame.
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
        .show_header(ui, |ui| {
            ui.label(egui::RichText::new("Matte (this correction)").strong());
            ui.colored_label(color::TEXT_MUTED, "Secondary");
        })
        .body(|ui| {
            matte_section_body(ui, clip, effect, &params, pending);
        });
    if matte_section_is_open(ui, id) {
        pending.record_matte_expanded(MatteTarget::new(clip.id, effect.id));
    }
}

/// The persistent id of one node's matte section.
///
/// Derived from the node's own identity alone, not from the `Ui` it happens to
/// be nested in: a clip id and an effect id already name exactly one node, and
/// an id that also depended on the surrounding groups would silently collapse
/// everybody's open sections the next time a card's layout changed. It also
/// means the section's open state can be named from outside the card — by the
/// rest of the app, and by a test without a window.
fn matte_section_id(clip: ClipId, effect: EffectId) -> egui::Id {
    egui::Id::new(("matte-section", clip.0, effect.0))
}

/// Whether the section is expanded, read after the header so a click this
/// frame is already reflected.
fn matte_section_is_open(ui: &egui::Ui, id: egui::Id) -> bool {
    egui::collapsing_header::CollapsingState::load(ui.ctx(), id)
        .is_some_and(|state| state.is_open())
}

#[allow(clippy::too_many_lines)]
fn matte_section_body(
    ui: &mut egui::Ui,
    clip: &Clip,
    effect: &Effect,
    params: &MatteParams,
    pending: &mut InspectorEdits,
) {
    ui.colored_label(
        color::TEXT_MUTED,
        "Gates this correction only. The layer Mask (layer alpha) is a compositing operation and \
         is not a secondary (CC5 §1).",
    );
    let mut enabled = params.is_enabled();
    if ui
        .checkbox(&mut enabled, "Enable matte")
        .on_hover_text(MATTE_HOLD_ONLY_NOTE)
        .changed()
    {
        pending.push(effect_param_operation(
            clip.id,
            effect.id,
            MATTE_ENABLED_PARAMETER,
            i64::from(enabled),
        ));
    }

    let degenerate = params.degenerate_bands();
    if !degenerate.is_empty() {
        ui.colored_label(
            color::STATUS_WARNING,
            format!(
                "matte_band_inverted_by_automation: {} resolves with its low edge above its high \
                 edge, so that band evaluates to 0 (CC5 §2.6).",
                degenerate.join(", ")
            ),
        );
    }

    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "Windows {}/{MATTE_WINDOW_LIMIT}",
            params.window_count
        ));
        if ui
            .add_enabled(
                params.window_count < MATTE_WINDOW_LIMIT,
                egui::Button::new("Add window"),
            )
            .on_hover_text("Add a centred rect window and enable the matte.")
            .clicked()
        {
            pending.extend(matte_add_window_operations(clip.id, effect));
        }
        ui.colored_label(color::TEXT_MUTED, "Combine");
        for (token, label) in [(0_i64, "Union"), (1, "Intersection")] {
            if ui
                .selectable_label(params.combine_token == token, label)
                .on_hover_text(MATTE_HOLD_ONLY_NOTE)
                .clicked()
                && params.combine_token != token
            {
                pending.push(effect_param_operation(
                    clip.id,
                    effect.id,
                    MATTE_COMBINE_PARAMETER,
                    token,
                ));
            }
        }
        ui.add_enabled(
            matte_track_button_enabled(),
            egui::Button::new(MATTE_TRACK_BUTTON_LABEL),
        )
        .on_disabled_hover_text(MATTE_TRACK_BUTTON_TOOLTIP);
    });

    for index in 0..params.window_count {
        matte_window_row(
            ui,
            clip,
            effect,
            index,
            params.window_count,
            &params.windows[index],
            pending,
        );
    }
    if params.window_count == 0 {
        ui.colored_label(
            color::TEXT_MUTED,
            "No windows: the geometric leg selects the whole frame.",
        );
    }

    matte_qualifier_rows(ui, clip, effect, params, pending);

    let mut invert = params.is_inverted();
    if ui
        .checkbox(&mut invert, "Invert matte")
        .on_hover_text(MATTE_HOLD_ONLY_NOTE)
        .changed()
    {
        pending.push(effect_param_operation(
            clip.id,
            effect.id,
            MATTE_INVERT_PARAMETER,
            i64::from(invert),
        ));
    }

    let mut percent = mix_percent(params.mix_bp, MATTE_MIX_BASIS_POINTS_MAX);
    ui.horizontal(|ui| {
        let slider = ui.add(mix_slider(
            &mut percent,
            MATTE_MIX_BASIS_POINTS_MAX,
            "Matte mix",
        ));
        if slider.drag_started() {
            pending.begin_gesture();
        }
        if slider.changed() {
            let operation = effect_param_operation(
                clip.id,
                effect.id,
                MATTE_MIX_PARAMETER,
                percent.saturating_mul(100),
            );
            if is_live_drag(&slider) {
                pending.push_live(operation, matte_mix_coalesce_key(clip.id, effect.id));
            } else {
                pending.push(operation);
            }
        }
        // The slider is whole percent, as the look mix is (CC4 §7), so the
        // stored value is shown beside it: an agent may author any basis point
        // and a 6050 that reads as "60 %" would look like a rounding bug.
        ui.monospace(format!("{} bp", params.mix_bp));
        ui.colored_label(
            color::TEXT_MUTED,
            "Scales the coverage: 0 % makes the node inactive while the matte is enabled.",
        );
    });

    let matte_names: Vec<&'static str> = kinewright_core::matte_parameter_names().to_vec();
    color_node_keyframe_rows(ui, clip.id, effect, &matte_names, pending);
    ui.colored_label(color::TEXT_MUTED, MATTE_KEYFRAME_NOTE);
}

/// Whether the tracking control is interactive. It never is in CC5 (§6).
#[must_use]
pub(crate) const fn matte_track_button_enabled() -> bool {
    false
}

/// One window's row: shape, geometry, feather, invert, select, remove.
#[allow(clippy::too_many_lines)]
fn matte_window_row(
    ui: &mut egui::Ui,
    clip: &Clip,
    effect: &Effect,
    index: usize,
    window_count: usize,
    window: &MatteWindowParams,
    pending: &mut InspectorEdits,
) {
    let Some(names) = kinewright_core::matte_window_parameter_names(index) else {
        return;
    };
    ui.group(|ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("W{index}"));
            for (token, label) in [(1_i64, "Rect"), (2, "Ellipse")] {
                if ui
                    .selectable_label(window.shape_token == token, label)
                    .on_hover_text(MATTE_HOLD_ONLY_NOTE)
                    .clicked()
                    && window.shape_token != token
                {
                    pending.push(effect_param_operation(clip.id, effect.id, names[0], token));
                }
            }
            if ui
                .small_button("Select in viewer")
                .on_hover_text("Draw this window's handles on the Program viewer.")
                .clicked()
            {
                pending.record_matte_window_selection(index, window_count);
            }
            if ui.small_button("Remove").clicked() {
                pending.extend(matte_remove_window_operations(clip.id, effect, index));
            }
        });
        let move_key = matte_window_move_coalesce_key(clip.id, effect.id, index);
        let resize_key = matte_window_resize_coalesce_key(clip.id, effect.id, index);
        let rotate_key = matte_window_rotate_coalesce_key(clip.id, effect.id, index);
        ui.horizontal_wrapped(|ui| {
            matte_integer_control(
                ui,
                clip.id,
                effect,
                names[1],
                "Centre X",
                window.center_x_bp,
                &move_key,
                pending,
            );
            matte_integer_control(
                ui,
                clip.id,
                effect,
                names[2],
                "Centre Y",
                window.center_y_bp,
                &move_key,
                pending,
            );
        });
        ui.horizontal_wrapped(|ui| {
            matte_integer_control(
                ui,
                clip.id,
                effect,
                names[3],
                "Half width",
                window.half_width_bp,
                &resize_key,
                pending,
            );
            matte_integer_control(
                ui,
                clip.id,
                effect,
                names[4],
                "Half height",
                window.half_height_bp,
                &resize_key,
                pending,
            );
        });
        ui.horizontal_wrapped(|ui| {
            matte_integer_control(
                ui,
                clip.id,
                effect,
                names[5],
                "Rotation (cd)",
                window.rotation_cd,
                &rotate_key,
                pending,
            );
            matte_integer_control(
                ui,
                clip.id,
                effect,
                names[6],
                "Feather",
                window.feather_bp,
                &matte_parameter_coalesce_key(clip.id, effect.id, names[6]),
                pending,
            );
            let mut invert = window.is_inverted();
            if ui
                .checkbox(&mut invert, "Invert")
                .on_hover_text(MATTE_HOLD_ONLY_NOTE)
                .changed()
            {
                pending.push(effect_param_operation(
                    clip.id,
                    effect.id,
                    names[7],
                    i64::from(invert),
                ));
            }
        });
    });
}

/// The inclusive bounds one matte control is stored under (CC5 §2.2).
///
/// Read from the node's own `EFFECT_DESCRIPTORS` entry rather than transcribed
/// at the call site: the descriptor is what `SetEffectParam` validates against,
/// so a slider built from anything else can offer a value core will reject — or
/// refuse one core would accept. An unregistered name yields an inert control
/// rather than an invented range; the section only draws registered names, and
/// a test pins that.
fn matte_parameter_range(effect: &Effect, name: &str, value: i64) -> std::ops::RangeInclusive<i64> {
    let Some(parameter) = kinewright_core::effect_descriptor(&effect.name)
        .and_then(|descriptor| descriptor.parameter(name))
    else {
        // A release build keeps the inert control; the test lane fails loudly,
        // so a control retargeted at a name the node does not register is
        // caught here rather than shipping as a `DragValue` that cannot move.
        debug_assert!(
            !is_matte_capable_color_node(&effect.name),
            "unregistered matte control {name} on {}",
            effect.name
        );
        return value..=value;
    };
    parameter.min..=parameter.max
}

/// One bounded integer matte control, live-coalesced under `coalesce_key`.
#[allow(clippy::too_many_arguments)]
fn matte_integer_control(
    ui: &mut egui::Ui,
    clip: ClipId,
    effect: &Effect,
    name: &str,
    label: &str,
    value: i64,
    coalesce_key: &str,
    pending: &mut InspectorEdits,
) {
    let mut edited = value;
    ui.label(label);
    let response =
        ui.add(egui::DragValue::new(&mut edited).range(matte_parameter_range(effect, name, value)));
    if response.drag_started() {
        pending.begin_gesture();
    }
    if response.changed() && edited != value {
        let operation = effect_param_operation(clip, effect.id, name, edited);
        if is_live_drag(&response) {
            pending.push_live(operation, coalesce_key.to_owned());
        } else {
            pending.push(operation);
        }
    }
}

/// The HSL qualifier controls (CC5 §2.4).
fn matte_qualifier_rows(
    ui: &mut egui::Ui,
    clip: &Clip,
    effect: &Effect,
    params: &MatteParams,
    pending: &mut InspectorEdits,
) {
    let qualifier = params.qualifier;
    ui.horizontal_wrapped(|ui| {
        let mut enabled = qualifier.is_enabled();
        if ui
            .checkbox(&mut enabled, "Qualifier (HSL)")
            .on_hover_text(
                "Judged on the value entering this node, in the grade709 encoding (CC5 §2.4).",
            )
            .changed()
        {
            pending.push(effect_param_operation(
                clip.id,
                effect.id,
                MATTE_QUALIFIER_PARAMETERS[0],
                i64::from(enabled),
            ));
        }
        if ui
            .small_button("Reset qualifier")
            .on_hover_text("Return every qualifier control to its neutral.")
            .clicked()
        {
            pending.extend(matte_qualifier_operations(
                clip.id,
                effect.id,
                &MatteQualifierParams::NEUTRAL,
            ));
        }
        if qualifier.hue_leg_disabled() {
            ui.colored_label(
                color::TEXT_MUTED,
                "Hue leg disabled at 180°: achromatic pixels are included.",
            );
        }
    });
    let values = [
        qualifier.hue_center_cd,
        qualifier.hue_width_cd,
        qualifier.hue_softness_cd,
        qualifier.sat_low_bp,
        qualifier.sat_high_bp,
        qualifier.sat_softness_bp,
        qualifier.luma_low_bp,
        qualifier.luma_high_bp,
        qualifier.luma_softness_bp,
    ];
    // No transcribed bounds here: `matte_integer_control` reads each one from
    // the node's descriptor, which is what core validates the write against.
    for (index, ((name, label), value)) in MATTE_QUALIFIER_PARAMETERS
        .iter()
        .skip(1)
        .zip(MATTE_QUALIFIER_LABELS)
        .zip(values)
        .enumerate()
    {
        if index % 3 == 0 {
            ui.label(
                egui::RichText::new(["Hue", "Saturation", "Luma"][index / 3])
                    .size(type_size::CAPTION)
                    .strong(),
            );
        }
        ui.horizontal(|ui| {
            matte_integer_control(
                ui,
                clip.id,
                effect,
                name,
                label,
                value,
                &matte_parameter_coalesce_key(clip.id, effect.id, name),
                pending,
            );
        });
    }
}

/// Stable per-wheel coalesce key for one live trackball or master drag.
fn wheels_coalesce_key(clip: ClipId, effect: EffectId, control: ColorWheelControl) -> String {
    format!(
        "wheels:{}:{}:{}",
        clip.0,
        effect.0,
        color_wheel_widget::control_token(control)
    )
}

/// Stable per-curve coalesce key for one live curve-point drag.
fn curves_coalesce_key(clip: ClipId, effect: EffectId, curve: ColorCurveChannel) -> String {
    format!("curves:{}:{}:{}", clip.0, effect.0, curve.name())
}

/// The CC3 §7 `color_wheels` card: three trackballs, a bypass toggle, a reset,
/// and the keyframe state of every control.
fn color_wheels_section(
    ui: &mut egui::Ui,
    clip: &Clip,
    effect: &Effect,
    stage_index: usize,
    looks: &LookInspectorContext<'_>,
    pending: &mut InspectorEdits,
) {
    let Some(descriptor) = kinewright_core::effect_descriptor("color_wheels") else {
        return;
    };
    let params = ColorWheelsParams::from_effect(effect);
    ui.group(|ui| {
        color_node_header(
            ui,
            clip,
            effect,
            &descriptor,
            stage_index,
            "Colour wheels",
            looks,
            pending,
        );
        // Wrapped so a narrow inspector stacks the balls instead of clipping
        // the third one out of reach.
        ui.horizontal_wrapped(|ui| {
            for control in ColorWheelControl::ALL {
                let state = wheel_state(effect, params, control);
                let response = color_wheel(ui, &state);
                apply_wheel_response(clip.id, effect, control, &response, pending);
            }
        });
        let names: Vec<&'static str> = descriptor
            .parameters
            .iter()
            .map(|parameter| parameter.name)
            .filter(|name| !is_matte_parameter(name))
            .collect();
        color_node_keyframe_rows(ui, clip.id, effect, &names, pending);
        matte_section(ui, clip, effect, pending);
    });
}

/// One trackball's document state, read from the stored integers.
fn wheel_state(
    effect: &Effect,
    params: ColorWheelsParams,
    control: ColorWheelControl,
) -> ColorWheelState {
    ColorWheelState {
        control,
        values: ColorWheelControlSet {
            master: params.control(control, ColorWheelChannel::Master),
            red: params.control(control, ColorWheelChannel::Red),
            green: params.control(control, ColorWheelChannel::Green),
            blue: params.control(control, ColorWheelChannel::Blue),
        },
        keyframed: ColorWheelChannel::ALL
            .map(|channel| parameter_is_keyframed(effect, control.parameter_name(channel))),
    }
}

/// Turn one frame of trackball interaction into operations.
///
/// A drag emits one batch per frame under the wheel's coalesce key, so the
/// preview stays live while the whole gesture collapses to a single undo entry
/// (CC3 §7). A double-click is a discrete reset of that wheel's four controls.
fn apply_wheel_response(
    clip: ClipId,
    effect: &Effect,
    control: ColorWheelControl,
    response: &crate::color_wheel_widget::ColorWheelResponse,
    pending: &mut InspectorEdits,
) {
    if response.gesture_started {
        pending.begin_gesture();
    }
    if response.reset {
        pending.extend(wheel_reset_operations(clip, effect, control));
        return;
    }
    if response.changes.is_empty() {
        return;
    }
    let operations = response.changes.iter().map(|(channel, value)| {
        effect_param_operation(clip, effect.id, control.parameter_name(*channel), *value)
    });
    if response.live {
        pending.extend_live(operations, wheels_coalesce_key(clip, effect.id, control));
    } else {
        pending.extend(operations);
    }
}

/// Reset one wheel: its four controls to their neutrals, plus a keyframe clear
/// for each that carries automation.
fn wheel_reset_operations(
    clip: ClipId,
    effect: &Effect,
    control: ColorWheelControl,
) -> Vec<Operation> {
    let (_, _, neutral) = control.bounds();
    let mut operations = Vec::with_capacity(ColorWheelChannel::ALL.len());
    for channel in ColorWheelChannel::ALL {
        let name = control.parameter_name(channel);
        operations.push(effect_param_operation(clip, effect.id, name, neutral));
        if parameter_is_keyframed(effect, name) {
            operations.push(clear_keyframes_operation(clip, effect.id, name));
        }
    }
    operations
}

/// The CC3 §7 `color_curves` card: a channel selector, the curve editor, a
/// per-curve reset, and the automation-truncation warning.
fn color_curves_section(
    ui: &mut egui::Ui,
    clip: &Clip,
    effect: &Effect,
    stage_index: usize,
    looks: &LookInspectorContext<'_>,
    pending: &mut InspectorEdits,
) {
    let Some(descriptor) = kinewright_core::effect_descriptor("color_curves") else {
        return;
    };
    let resolved = ResolvedCurves::from_effect(effect);
    ui.group(|ui| {
        color_node_header(
            ui,
            clip,
            effect,
            &descriptor,
            stage_index,
            "Colour curves",
            looks,
            pending,
        );

        let truncated = automation_truncated_curves_cached(ui, effect, &resolved);
        if !truncated.is_empty() {
            let names = truncated
                .iter()
                .map(|curve| curve.name())
                .collect::<Vec<_>>()
                .join(", ");
            ui.colored_label(
                color::STATUS_WARNING,
                format!(
                    "curve_truncated_by_automation: {names} resolves without strictly increasing \
                     x, so the curve renders as its longest valid prefix (CC3 §3.4)."
                ),
            );
        }

        let selection_id = ui.make_persistent_id(("color-curves-channel", clip.id.0, effect.id.0));
        let mut selected = ui
            .data(|data| data.get_temp::<ColorCurveChannel>(selection_id))
            .unwrap_or(ColorCurveChannel::Master);
        ui.horizontal(|ui| {
            for curve in ColorCurveChannel::ALL {
                if ui
                    .selectable_label(selected == curve, curve_editor_widget::curve_label(curve))
                    .clicked()
                {
                    selected = curve;
                }
            }
            if ui
                .small_button("Reset curve")
                .on_hover_text("Restore this curve to (0, 0) and (10000, 10000).")
                .clicked()
            {
                pending.extend(curve_reset_operations(clip.id, effect, selected));
            }
        });
        ui.data_mut(|data| data.insert_temp(selection_id, selected));

        let points = resolved.curve(selected).points.clone();
        let editor_id = ui.make_persistent_id(("color-curve-editor", clip.id.0, effect.id.0));
        let response = curve_editor(ui, &points, selected, editor_id);
        apply_curve_response(clip.id, effect, selected, &points, &response, pending);
        ui.colored_label(
            color::TEXT_MUTED,
            format!(
                "{} points · click to add · drag to shape · right-click or Delete to remove · \
                 double-click to reset",
                points.len()
            ),
        );

        color_curve_keyframe_rows(ui, clip.id, effect, pending);
        matte_section(ui, clip, effect, pending);
    });
}

/// The keyframe indicators of a `color_curves` card (CC3 §7).
///
/// §7 requires an indicator per keyframed control, not per keyframed control
/// *of the curve that happens to be selected*: automation on the red curve
/// must stay visible while the master curve is on screen, otherwise switching
/// tabs is the only way to discover it. Rows are grouped by owning curve so a
/// bare `red_y3` is still attributable.
fn color_curve_keyframe_rows(
    ui: &mut egui::Ui,
    clip: ClipId,
    effect: &Effect,
    pending: &mut InspectorEdits,
) {
    let groups = color_curve_keyframe_groups(effect);
    if groups.is_empty() {
        return;
    }
    ui.colored_label(color::STATUS_WARNING, KEYFRAME_ROWS_NOTE);
    for (label, names) in groups {
        ui.label(egui::RichText::new(label).size(type_size::CAPTION).strong());
        for name in names {
            keyframe_row(ui, clip, effect, name, pending);
        }
    }
}

/// The keyframed controls of a `color_curves` node, grouped by owner in
/// `ColorCurveChannel::ALL` order with the node-owned `bypass` last.
fn color_curve_keyframe_groups(effect: &Effect) -> Vec<(&'static str, Vec<&'static str>)> {
    let mut groups: Vec<(&'static str, Vec<&'static str>)> = Vec::new();
    for curve in ColorCurveChannel::ALL {
        let keyframed: Vec<&'static str> = curve
            .parameter_names()
            .iter()
            .copied()
            .filter(|name| parameter_is_keyframed(effect, name))
            .collect();
        if !keyframed.is_empty() {
            groups.push((curve_group_label(curve), keyframed));
        }
    }
    if parameter_is_keyframed(effect, COLOR_NODE_BYPASS_PARAMETER) {
        groups.push(("Node", vec![COLOR_NODE_BYPASS_PARAMETER]));
    }
    groups
}

/// The heading of one keyframe group. The tab strip abbreviates the three
/// channels to `R`/`G`/`B`; a group heading standing above a bare `red_y3` has
/// to spell the curve out.
const fn curve_group_label(curve: ColorCurveChannel) -> &'static str {
    match curve {
        ColorCurveChannel::Master => "Master",
        ColorCurveChannel::Red => "Red",
        ColorCurveChannel::Green => "Green",
        ColorCurveChannel::Blue => "Blue",
    }
}

/// Turn one frame of curve interaction into operations.
fn apply_curve_response(
    clip: ClipId,
    effect: &Effect,
    curve: ColorCurveChannel,
    current: &[(i32, i32)],
    response: &crate::curve_editor_widget::CurveEditorResponse,
    pending: &mut InspectorEdits,
) {
    if response.gesture_started {
        pending.begin_gesture();
    }
    if response.reset {
        pending.extend(curve_reset_operations(clip, effect, curve));
        return;
    }
    let Some(points) = response.points.as_deref() else {
        return;
    };
    let operations = curve_edit_operations(clip, effect.id, curve, current, points);
    if response.live {
        pending.extend_live(operations, curves_coalesce_key(clip, effect.id, curve));
    } else {
        pending.extend(operations);
    }
}

/// The operations that turn one curve's stored points into `next`.
///
/// CC3 §2.4: only `{curve}_point_count` and the active points' coordinates are
/// written, because an omitted parameter resolves to its neutral.
///
/// Core validates every `SetEffectParam` against the document the change would
/// produce, so the *order* inside the batch is load-bearing: an intermediate
/// state whose active prefix is not strictly increasing in `x` would be
/// rejected even though both the start and the end state are legal. Inserting a
/// point moves every later coordinate to a smaller `x`, so ascending writes are
/// safe; removing one moves them to a larger `x`, so the count shrinks first
/// and the writes run descending. Anything else collapses the active prefix to
/// two points, rewrites, and restores the count.
fn curve_edit_operations(
    clip: ClipId,
    effect: EffectId,
    curve: ColorCurveChannel,
    current: &[(i32, i32)],
    next: &[(i32, i32)],
) -> Vec<Operation> {
    let mut operations = Vec::with_capacity(2 + next.len() * 2);
    let count = |operations: &mut Vec<Operation>, value: usize| {
        operations.push(effect_param_operation(
            clip,
            effect,
            curve.point_count_parameter(),
            i64::try_from(value).unwrap_or(i64::MAX),
        ));
    };
    let point = |operations: &mut Vec<Operation>, index: usize| {
        let (Some(x_name), Some(y_name)) = (curve.x_parameter(index), curve.y_parameter(index))
        else {
            return;
        };
        let (x, y) = next[index];
        operations.push(effect_param_operation(clip, effect, x_name, i64::from(x)));
        operations.push(effect_param_operation(clip, effect, y_name, i64::from(y)));
    };

    let moves_left = next
        .iter()
        .zip(current)
        .all(|(next, current)| next.0 <= current.0);
    // The descending branch writes `{curve}_point_count` *first*, so it must
    // never grow the active prefix: growing it would expose the colliding
    // `(10000, 10000)` neutrals of the points that are still unwritten, and
    // core would reject the count. `zip` stops at the shorter list, so the
    // length guard is not implied by the coordinate comparison and has to be
    // stated.
    let moves_right = next.len() <= current.len()
        && next
            .iter()
            .zip(current)
            .all(|(next, current)| next.0 >= current.0);
    debug_assert!(
        !moves_right || next.len() <= current.len(),
        "the count-first curve branch must never grow the active prefix",
    );
    if moves_left {
        for index in 0..next.len() {
            point(&mut operations, index);
        }
        count(&mut operations, next.len());
    } else if moves_right {
        count(&mut operations, next.len());
        for index in (0..next.len()).rev() {
            point(&mut operations, index);
        }
    } else {
        count(&mut operations, kinewright_core::COLOR_CURVE_MIN_POINTS);
        for index in kinewright_core::COLOR_CURVE_MIN_POINTS..next.len() {
            point(&mut operations, index);
        }
        if next
            .first()
            .is_some_and(|first| current.get(1).is_some_and(|second| first.0 < second.0))
        {
            point(&mut operations, 0);
            point(&mut operations, 1);
        } else {
            point(&mut operations, 1);
            point(&mut operations, 0);
        }
        count(&mut operations, next.len());
    }
    operations
}

/// The structural identity a curve reset targets: `(0, 0)` and
/// `(10000, 10000)` (CC3 §2.3).
const CURVE_RESET_POINTS: [(i32, i32); COLOR_CURVE_MIN_POINTS] = [
    (0, 0),
    (
        COLOR_CURVE_WHITE_BASIS_POINTS,
        COLOR_CURVE_WHITE_BASIS_POINTS,
    ),
];

/// Reset one curve: its 33 parameters to their neutrals, plus a keyframe clear
/// for each that carries automation (CC3 §5).
fn curve_reset_operations(
    clip: ClipId,
    effect: &Effect,
    curve: ColorCurveChannel,
) -> Vec<Operation> {
    let mut operations = curve_reset_parameter_operations(clip, effect, curve);
    for parameter in curve.parameters() {
        if parameter_is_keyframed(effect, parameter.name) {
            operations.push(clear_keyframes_operation(clip, effect.id, parameter.name));
        }
    }
    operations
}

/// The `SetEffectParam`s of one curve reset, in an order core accepts.
///
/// Descriptor order is *not* accepted: it writes `x0` first, and from a stored
/// `x0 = -2000, x1 = -1000` the intermediate `x0 = 0, x1 = -1000` is not
/// strictly increasing, so core rejects the operation and `apply_batch`
/// discards the entire reset. The active pair therefore goes through
/// [`curve_edit_operations`], which already owns the proof that one of the two
/// write orders is always legal; the remaining points are written afterwards,
/// while `{curve}_point_count` is back at two and they are inactive.
fn curve_reset_parameter_operations(
    clip: ClipId,
    effect: &Effect,
    curve: ColorCurveChannel,
) -> Vec<Operation> {
    let stored = stored_curve_points(effect, curve);
    let mut operations =
        curve_edit_operations(clip, effect.id, curve, &stored, &CURVE_RESET_POINTS);
    let descriptors = curve.parameters();
    let neutral = |name: &str| {
        descriptors
            .iter()
            .find(|parameter| parameter.name == name)
            .map_or(i64::from(COLOR_CURVE_WHITE_BASIS_POINTS), |parameter| {
                parameter.neutral
            })
    };
    // Points 2..16 are inactive once the count is back at two, so their
    // deliberately colliding `(10000, 10000)` neutrals are never examined by
    // the strict-`x` check (CC3 §2.3).
    for index in COLOR_CURVE_MIN_POINTS..COLOR_CURVE_MAX_POINTS {
        let (Some(x_name), Some(y_name)) = (curve.x_parameter(index), curve.y_parameter(index))
        else {
            break;
        };
        operations.push(effect_param_operation(
            clip,
            effect.id,
            x_name,
            neutral(x_name),
        ));
        operations.push(effect_param_operation(
            clip,
            effect.id,
            y_name,
            neutral(y_name),
        ));
    }
    operations
}

/// The *stored*, untruncated point list of one curve.
///
/// Reset ordering must reason about the prefix core validates, which is
/// `{curve}_point_count` raw parameters, not about the §3.4-truncated list the
/// editor draws.
fn stored_curve_points(effect: &Effect, curve: ColorCurveChannel) -> Vec<(i32, i32)> {
    let descriptors = curve.parameters();
    let stored = |index: usize| -> i64 {
        let descriptor = &descriptors[index];
        effect
            .parameters
            .get(descriptor.name)
            .and_then(|value| match value {
                ParamValue::Integer(value) => Some(*value),
                ParamValue::Boolean(_) | ParamValue::Text(_) => None,
            })
            .unwrap_or(descriptor.neutral)
    };
    let minimum = i64::try_from(COLOR_CURVE_MIN_POINTS).unwrap_or(2);
    let maximum = i64::try_from(COLOR_CURVE_MAX_POINTS).unwrap_or(16);
    let count =
        usize::try_from(stored(0).clamp(minimum, maximum)).unwrap_or(COLOR_CURVE_MIN_POINTS);
    let coordinate = |value: i64| {
        i32::try_from(value.clamp(COLOR_CURVE_COORDINATE_MIN, COLOR_CURVE_COORDINATE_MAX))
            .unwrap_or(COLOR_CURVE_WHITE_BASIS_POINTS)
    };
    (0..count)
        .map(|index| {
            (
                coordinate(stored(1 + index * 2)),
                coordinate(stored(2 + index * 2)),
            )
        })
        .collect()
}

/// The curves CC3 §3.4 truncation shortens at any keyframe boundary.
///
/// Truncation is a property of the *resolved* curve, so the scan evaluates the
/// node at frame zero and at every curve keyframe. The list is bounded so a
/// pathological automation curve cannot stall the inspector.
fn automation_truncated_curves(effect: &Effect) -> Vec<ColorCurveChannel> {
    const SCAN_LIMIT: usize = 64;
    let mut frames = vec![TimeCode::ZERO];
    for (name, curve) in &effect.keyframes {
        // `bypass` is node-owned rather than curve-owned, but it decides
        // whether a truncation is visible at all. A node that is bypassed at
        // frame zero and live from frame ten would otherwise never be scanned
        // at a frame where its truncation matters.
        if ColorCurveChannel::owning(name).is_none() && *name != COLOR_NODE_BYPASS_PARAMETER {
            continue;
        }
        frames.extend(curve.keyframes.iter().map(|keyframe| keyframe.at));
    }
    frames.sort_unstable();
    frames.dedup();
    frames.truncate(SCAN_LIMIT);
    let mut truncated = Vec::new();
    for at in frames {
        let resolved = ResolvedCurves::from_effect(&effect.evaluated_at(at));
        if resolved.bypass() {
            continue;
        }
        for curve in resolved.truncated_curves() {
            if !truncated.contains(&curve) {
                truncated.push(curve);
            }
        }
    }
    truncated
}

/// [`automation_truncated_curves`] at UI cost.
///
/// The scan clones the node's 133-parameter map once per scanned frame, so
/// running it unconditionally every frame costs up to 64 clones for a warning
/// that changes only when the automation does. A node with no automation needs
/// no scan at all: `resolved` is already the answer. A node with automation is
/// scanned once and memoised in egui's temporary store under a fingerprint of
/// its keyframes, so editing them invalidates the entry.
fn automation_truncated_curves_cached(
    ui: &egui::Ui,
    effect: &Effect,
    resolved: &ResolvedCurves,
) -> Vec<ColorCurveChannel> {
    if effect.keyframes.is_empty() {
        return if resolved.bypass() {
            Vec::new()
        } else {
            resolved.truncated_curves()
        };
    }
    let id = ui.make_persistent_id(("color-curves-truncation", effect.id.0));
    let fingerprint = keyframe_fingerprint(effect);
    if let Some((cached, curves)) =
        ui.data(|data| data.get_temp::<(u64, Vec<ColorCurveChannel>)>(id))
        && cached == fingerprint
    {
        return curves;
    }
    let curves = automation_truncated_curves(effect);
    ui.data_mut(|data| data.insert_temp(id, (fingerprint, curves.clone())));
    curves
}

/// A cheap hash of everything the truncation scan reads out of a node's
/// automation.
///
/// Interpolation is deliberately excluded: the scan only evaluates the effect
/// *at* keyframe frames, where every interpolation mode agrees.
fn keyframe_fingerprint(effect: &Effect) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (name, curve) in &effect.keyframes {
        name.hash(&mut hasher);
        curve.keyframes.len().hash(&mut hasher);
        for keyframe in &curve.keyframes {
            keyframe.at.0.hash(&mut hasher);
            keyframe.value.hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// The shared header of a CC3 colour-node card: name, stage index, bypass,
/// reset, and remove, plus the CC6 §8.3 clipping-contribution line.
// Every argument is a distinct thing the header draws; bundling them into a
// struct would only move the same list one line up.
#[allow(clippy::too_many_arguments)]
fn color_node_header(
    ui: &mut egui::Ui,
    clip: &Clip,
    effect: &Effect,
    descriptor: &kinewright_core::EffectDescriptor,
    stage_index: usize,
    title: &str,
    looks: &LookInspectorContext<'_>,
    pending: &mut InspectorEdits,
) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).strong());
        ui.colored_label(color::TEXT_MUTED, format!("Stage {stage_index}"));
        let mut bypass = bypass_token(effect) >= 1;
        if ui
            .checkbox(&mut bypass, "Bypass")
            .on_hover_text(
                "A bypassed node keeps its position and every value and renders as the exact \
                 identity (CC3 §5).",
            )
            .changed()
        {
            pending.push(effect_param_operation(
                clip.id,
                effect.id,
                COLOR_NODE_BYPASS_PARAMETER,
                i64::from(bypass),
            ));
        }
        if ui
            .small_button("Reset")
            .on_hover_text("Return every control to its neutral and clear its automation.")
            .clicked()
        {
            pending.extend(color_node_reset_operations(clip.id, effect, descriptor));
        }
        if ui.small_button("Remove").clicked() {
            pending.push(Operation::RemoveEffect {
                clip: clip.id,
                effect: effect.id,
            });
        }
    });
    if let Some(reason) = color_node_inactive_reason(effect) {
        ui.colored_label(
            color::TEXT_MUTED,
            format!("Inactive for this frame: {}", reason.as_str()),
        );
    }
    color_node_clipping_line(ui, clip, effect, looks);
}

/// The CC6 §8.3 clipping-contribution line, in the same muted slot the
/// inactive reason occupies.
///
/// Absent when there is no report, when the node is not in it, or when both
/// deltas are `<= 0`. It never computes anything: it reads the last
/// measurement the Colour QC window took, and names the frame it was taken at.
fn color_node_clipping_line(
    ui: &mut egui::Ui,
    clip: &Clip,
    effect: &Effect,
    looks: &LookInspectorContext<'_>,
) {
    if let Some(line) = looks.qc_clipping.line_for(clip.id, effect.id) {
        ui.add(egui::Label::new(egui::RichText::new(line).color(color::TEXT_MUTED)).wrap())
            .on_hover_text(
                "Measured at working_linear_post_composite by removing this node from a scratch \
             clone and re-measuring (CC6 §3.7). Evidence from the last Colour QC measurement, \
             not a live reading.",
            );
    }
}

/// The stored `bypass` token of a colour node.
fn bypass_token(effect: &Effect) -> i64 {
    effect
        .parameters
        .get(COLOR_NODE_BYPASS_PARAMETER)
        .and_then(|value| match value {
            ParamValue::Integer(value) => Some(*value),
            ParamValue::Boolean(_) | ParamValue::Text(_) => None,
        })
        .unwrap_or(0)
}

/// The keyframe indicator rows of a colour-node card (CC3 §7).
///
/// A keyframed control is badged, is clearable in one click, and carries the
/// note that direct editing writes the static value rather than a keyframe.
fn color_node_keyframe_rows(
    ui: &mut egui::Ui,
    clip: ClipId,
    effect: &Effect,
    names: &[&'static str],
    pending: &mut InspectorEdits,
) {
    let keyframed: Vec<&&str> = names
        .iter()
        .filter(|name| parameter_is_keyframed(effect, name))
        .collect();
    if keyframed.is_empty() {
        return;
    }
    ui.colored_label(color::STATUS_WARNING, KEYFRAME_ROWS_NOTE);
    for name in keyframed {
        keyframe_row(ui, clip, effect, name, pending);
    }
}

const KEYFRAME_ROWS_NOTE: &str =
    "Automation drives these controls. Editing here writes the static value, not a keyframe.";

/// One keyframed control's badge and its one-click clear.
fn keyframe_row(
    ui: &mut egui::Ui,
    clip: ClipId,
    effect: &Effect,
    name: &str,
    pending: &mut InspectorEdits,
) {
    ui.horizontal(|ui| {
        ui.monospace(egui::RichText::new(name).size(type_size::CAPTION));
        ui.colored_label(color::STATUS_WARNING, "KEYFRAMED");
        if ui
            .small_button("Clear keyframes")
            .on_hover_text("Remove this parameter's automation so the edited value applies.")
            .clicked()
        {
            pending.push(clear_keyframes_operation(clip, effect.id, name));
        }
    });
}

/// True when automation, not the static parameter value, drives the render.
/// The inspector badges these parameters so a slider that appears inert is
/// explained instead of looking broken.
fn parameter_is_keyframed(effect: &Effect, parameter: &str) -> bool {
    effect.keyframes.contains_key(parameter)
}

fn clear_keyframes_operation(clip: ClipId, effect: EffectId, name: &str) -> Operation {
    Operation::ClearEffectKeyframes {
        clip,
        effect,
        name: name.to_owned(),
    }
}

fn primary_parameter_label(name: &str) -> &str {
    match name {
        "exposure_milli_stops" => "Exposure",
        "temperature_percent" => "Temperature",
        "tint_percent" => "Tint",
        "contrast_percent" => "Contrast",
        "contrast_pivot_basis_points" => "Pivot",
        "blacks_percent" => "Blacks",
        "shadows_percent" => "Shadows",
        "highlights_percent" => "Highlights",
        "whites_percent" => "Whites",
        "saturation_percent" => "Saturation",
        _ => name,
    }
}

#[allow(clippy::cast_precision_loss)]
fn primary_parameter_readout(name: &str, value: i64) -> String {
    match name {
        "exposure_milli_stops" => format!("{:+.3} stops", value as f64 / 1_000.0),
        "contrast_pivot_basis_points" => format!("{:.4}", value as f64 / 10_000.0),
        _ => format!("{value:+}%"),
    }
}

fn effect_display_name(name: &str) -> &str {
    match name {
        "primary_correction" => "Primary correction",
        "color_wheels" => "Colour wheels",
        "color_curves" => "Colour curves",
        "technical_lut" => "Technical LUT",
        "creative_look" => "Creative look",
        _ => name,
    }
}

/// Whether the `+ Effect` menu offers one effect for new insertion.
///
/// CC4 §7 adds `look_lut` to the exclusions: the managed `technical_lut` and
/// `creative_look` kinds now cover every look, so a new project never grows a
/// legacy stage. The legacy nodes already in a project stay visible, keep
/// rendering, and offer **Convert to managed look**.
fn is_effect_insertable(name: &str) -> bool {
    !is_audio_effect(name)
        && !is_legacy_display_effect(name)
        && !matches!(name, "color_grade" | "cube_lut" | "look_lut")
}

/// Keep internal, high-precision reframe storage out of the generic inspector
/// when the matching percent control is available. The basis-point parameters
/// remain in the core descriptor for agent-authored edits and rendering.
fn should_render_effect_parameter(
    descriptor: &kinewright_core::EffectDescriptor,
    parameter_name: &str,
) -> bool {
    // CC5 §6: the matte section owns all 47 matte integers on all four
    // matte-capable kinds. 47 raw sliders is not a workflow, and the two
    // parameters a window move writes must land as one gesture rather than as
    // two unrelated sliders.
    if is_matte_parameter(parameter_name) {
        return false;
    }
    // CC3 §7: the wheels and curves nodes own dedicated cards. Their 13 and 133
    // integers must never reach the generic slider loop, and `AddEffect` must
    // insert them with no parameters at all, because an omitted parameter
    // resolves to its neutral (CC3 §2.4).
    if matches!(descriptor.name, "color_wheels" | "color_curves") {
        return false;
    }
    // CC4 §7: the LUT nodes own dedicated cards. `lut_asset_id` is set by the
    // browser and the import, `input_encoding_token` by the encoding picker,
    // and `mix_basis_points` is pinned on a `technical_lut` (min = max), so a
    // generic slider over any of them would be either wrong or inert.
    if is_lut_color_node(descriptor.name) {
        return !matches!(
            (descriptor.name, parameter_name),
            (_, LUT_ASSET_ID_PARAMETER | LUT_INPUT_ENCODING_PARAMETER)
                | ("technical_lut", LUT_MIX_PARAMETER)
        );
    }
    let Some(legacy_name) = (match (descriptor.name, parameter_name) {
        ("reframe", "focus_x_basis_points") => Some("focus_x_percent"),
        ("reframe", "focus_y_basis_points") => Some("focus_y_percent"),
        _ => None,
    }) else {
        return true;
    };

    !descriptor
        .parameters
        .iter()
        .any(|parameter| parameter.name == legacy_name)
}

fn transition_section(
    ui: &mut egui::Ui,
    document: &kinewright_core::Document,
    clip: &Clip,
    pending: &mut InspectorEdits,
) {
    ui.add_space(space::TWO);
    ui.strong("Transition in");
    if let Some(transition) = &clip.transition_in {
        let maximum = document
            .clip_duration(clip)
            .map_or(1, |value| value.0.max(1));
        let mut name = transition.name.clone();
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("Type");
            egui::ComboBox::from_id_salt(("transition-type", clip.id.0))
                .selected_text(&name)
                .show_ui(ui, |ui| {
                    for descriptor in TRANSITION_DESCRIPTORS {
                        changed |= ui
                            .selectable_value(
                                &mut name,
                                descriptor.name.to_owned(),
                                descriptor.name,
                            )
                            .on_hover_text(descriptor.description)
                            .changed();
                    }
                });
        });
        let mut duration = transition.duration.0;
        changed |= ui
            .add(
                egui::Slider::new(&mut duration, 1..=maximum)
                    .text("frames")
                    .integer(),
            )
            .changed();
        if changed {
            pending.extend(transition_duration_operations(
                document, clip.id, &name, duration,
            ));
        }
        if ui.small_button("Remove transition").clicked() {
            pending.extend(linked_transition_operations(document, clip.id, None));
        }
    } else {
        let duration = document
            .clip_duration(clip)
            .map_or(1, |value| value.0.clamp(1, 15));
        ui.menu_button("+ Transition", |ui| {
            for descriptor in TRANSITION_DESCRIPTORS {
                if ui
                    .button(descriptor.name)
                    .on_hover_text(descriptor.description)
                    .clicked()
                {
                    let transition = Transition {
                        name: descriptor.name.to_owned(),
                        duration: TimeCode(duration),
                    };
                    pending.extend(linked_transition_operations(
                        document,
                        clip.id,
                        Some(&transition),
                    ));
                    ui.close();
                }
            }
        });
    }
}

fn title_param_operation(clip: ClipId, name: &str, value: ParamValue) -> Operation {
    Operation::SetTitleParam {
        clip,
        name: name.to_owned(),
        value,
    }
}

fn audio_target_clip(document: &kinewright_core::Document, selected: ClipId) -> Option<Clip> {
    let mut members = linked_members(document, selected);
    members.sort_by_key(|(_, clip)| clip.id != selected);
    members
        .into_iter()
        .map(|(_, clip)| clip)
        .find(|clip| clip_carries_audio(document, clip))
}

fn clip_carries_audio(document: &kinewright_core::Document, clip: &Clip) -> bool {
    clip.content.is_media()
        && document
            .asset(clip.asset)
            .is_some_and(|asset| matches!(asset.kind, MediaKind::Audio | MediaKind::AudioVideo))
}

const fn clip_audio_operation(
    clip: ClipId,
    gain_tenth_db: i32,
    fade_in_frames: i64,
    fade_out_frames: i64,
) -> Operation {
    Operation::SetClipAudio {
        clip,
        gain_tenth_db,
        fade_in_frames: TimeCode(fade_in_frames),
        fade_out_frames: TimeCode(fade_out_frames),
    }
}

fn tenth_db_to_db(value: i32) -> f64 {
    f64::from(value) / 10.0
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
fn db_to_tenth_db(value: f64) -> i32 {
    (value * 10.0).round() as i32
}

fn marker_param_operation(marker: MarkerId, name: &str, value: ParamValue) -> Operation {
    Operation::SetMarkerParam {
        marker,
        name: name.to_owned(),
        value,
    }
}

fn effect_param_operation(clip: ClipId, effect: EffectId, name: &str, value: i64) -> Operation {
    Operation::SetEffectParam {
        clip,
        effect,
        name: name.to_owned(),
        value: ParamValue::Integer(value),
    }
}

/// The operation that places one new effect on a clip.
///
/// A managed colour node is *inserted* at the first index its stage allows
/// rather than appended: appending a correction onto
/// `[primary_correction, creative_look]` puts it after the look, which Core
/// rejects with `ColorStageOrderViolation` (CC4 §3.2, §7). Every other effect
/// is unconstrained and still appends.
fn add_effect_operation(clip: &Clip, descriptor: &kinewright_core::EffectDescriptor) -> Operation {
    let id = clip
        .effects
        .iter()
        .map(|effect| effect.id.0)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let parameters = descriptor
        .parameters
        .iter()
        .filter(|parameter| should_render_effect_parameter(descriptor, parameter.name))
        .map(|parameter| {
            (
                parameter.name.to_owned(),
                ParamValue::Integer(parameter.neutral),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let effect = Effect {
        id: EffectId(id),
        name: descriptor.name.to_owned(),
        parameters,
        keyframes: BTreeMap::new(),
    };
    match ColorNodeKind::from_effect_name(descriptor.name) {
        Some(kind) => Operation::InsertEffect {
            clip: clip.id,
            index: color_stage_insert_index(&clip.effects, kind.stage()),
            effect,
        },
        None => Operation::AddEffect {
            clip: clip.id,
            effect,
        },
    }
}

fn transition_duration_operations(
    document: &kinewright_core::Document,
    clip: ClipId,
    name: &str,
    duration: i64,
) -> Vec<Operation> {
    let transition = Transition {
        name: name.to_owned(),
        duration: TimeCode(duration),
    };
    linked_transition_operations(document, clip, Some(&transition))
}

fn data_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(color::TEXT_MUTED, label);
        ui.monospace(value);
    });
}

#[allow(clippy::cast_precision_loss)]
fn frame_readout(frame: TimeCode, fps: kinewright_core::Rational) -> String {
    let seconds = frame.0 as f64 * f64::from(fps.denominator()) / f64::from(fps.numerator());
    format!("{}f · {seconds:.3}s", frame.0)
}

fn range_readout(range: &std::ops::Range<TimeCode>, fps: kinewright_core::Rational) -> String {
    format!(
        "{} → {}",
        frame_readout(range.start, fps),
        frame_readout(range.end, fps)
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use kinewright_core::{
        AssetId, AutomationCurve, Document, EffectDescriptor, EffectParameterDescriptor,
        EffectUniform, Keyframe, KeyframeInterpolation, LinkId, MediaAsset, Rational, Track,
        TrackId, TrackKind,
    };

    use super::*;
    use crate::{color_wheel_widget::ColorWheelResponse, curve_editor_widget::CurveEditorResponse};

    #[test]
    fn inspector_control_builders_emit_only_operations() {
        assert_eq!(
            title_param_operation(ClipId(3), "text", ParamValue::Text("New".to_owned())),
            Operation::SetTitleParam {
                clip: ClipId(3),
                name: "text".to_owned(),
                value: ParamValue::Text("New".to_owned()),
            }
        );
        assert_eq!(
            marker_param_operation(MarkerId(4), "position", ParamValue::Integer(90)),
            Operation::SetMarkerParam {
                marker: MarkerId(4),
                name: "position".to_owned(),
                value: ParamValue::Integer(90),
            }
        );
        let mut document = Document::default();
        document.tracks.push(Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                id: ClipId(3),
                asset: AssetId(1),
                source_range: TimeCode(0)..TimeCode(30),
                content: ClipContent::Media,
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: Some(Transition {
                    name: "crossfade".to_owned(),
                    duration: TimeCode(6),
                }),
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            }],
        });
        assert_eq!(
            transition_duration_operations(&document, ClipId(3), "fade_from_black", 12),
            vec![
                Operation::RemoveTransition { clip: ClipId(3) },
                Operation::AddTransition {
                    clip: ClipId(3),
                    transition: Transition {
                        name: "fade_from_black".to_owned(),
                        duration: TimeCode(12),
                    },
                },
            ]
        );
    }

    #[test]
    fn descriptor_driven_add_effect_uses_neutral_integer_values() {
        static PARAMETERS: &[EffectParameterDescriptor] = &[EffectParameterDescriptor {
            name: "percent",
            min: -100,
            max: 100,
            neutral: 0,
            uniform: EffectUniform::Brightness,
        }];
        let descriptor = EffectDescriptor {
            name: "brightness",
            parameters: PARAMETERS,
        };
        let clip = Clip {
            id: ClipId(1),
            asset: AssetId(1),
            source_range: TimeCode(0)..TimeCode(30),
            content: ClipContent::Media,
            timeline_start: TimeCode::ZERO,
            effects: vec![Effect {
                id: EffectId(8),
                name: "contrast".to_owned(),
                parameters: BTreeMap::new(),
                keyframes: BTreeMap::new(),
            }],
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        };
        assert_eq!(
            add_effect_operation(&clip, &descriptor),
            Operation::AddEffect {
                clip: ClipId(1),
                effect: Effect {
                    id: EffectId(9),
                    name: "brightness".to_owned(),
                    parameters: BTreeMap::from([("percent".to_owned(), ParamValue::Integer(0),)]),
                    keyframes: BTreeMap::new(),
                },
            }
        );
    }

    #[test]
    fn primary_correction_card_uses_contract_defaults_and_reset_batch() {
        let descriptor = EFFECT_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.name == "primary_correction")
            .expect("CC1 descriptor");
        let clip = Clip {
            id: ClipId(3),
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
        };

        // A managed colour node is inserted at its stage's first legal index,
        // never appended (CC4 §3.2).
        let Operation::InsertEffect { effect, index, .. } = add_effect_operation(&clip, descriptor)
        else {
            panic!("primary correction must emit InsertEffect");
        };
        assert_eq!(index, 0);
        // CC5 §8: the matte parameters are omitted entirely, which is what
        // makes a node inserted after CC5 render bit-identically to a CC4 one —
        // an omitted parameter resolves to its neutral and an all-neutral matte
        // is inactive.
        assert_eq!(
            effect.parameters.len(),
            descriptor.parameters.len() - kinewright_core::MATTE_PARAMETER_COUNT
        );
        assert!(
            effect
                .parameters
                .keys()
                .all(|name| !is_matte_parameter(name)),
            "an inserted node stores no matte parameter"
        );
        assert!(effect.parameters.iter().all(|(name, value)| value
            == &ParamValue::Integer(descriptor.parameter(name).unwrap().neutral)));

        let reset_effect = Effect {
            id: EffectId(8),
            name: "primary_correction".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::from([(
                "shadows_percent".to_owned(),
                AutomationCurve {
                    keyframes: vec![Keyframe {
                        at: TimeCode::ZERO,
                        value: 40,
                        interpolation: KeyframeInterpolation::Linear,
                    }],
                },
            )]),
        };
        let reset = color_node_reset_operations(clip.id, &reset_effect, descriptor);
        // Every non-matte parameter, plus the one keyframe clear. The 47 matte
        // parameters this node never stored already resolve to their neutrals
        // (CC5 §2.2), so the reset leaves them unstored instead of writing them.
        let non_matte = descriptor
            .parameters
            .iter()
            .filter(|parameter| !is_matte_parameter(parameter.name))
            .count();
        assert_eq!(reset.len(), non_matte + 1);
        let reset_values = reset
            .iter()
            .filter(|operation| matches!(operation, Operation::SetEffectParam { .. }));
        for (operation, parameter) in reset_values.zip(
            descriptor
                .parameters
                .iter()
                .filter(|parameter| !is_matte_parameter(parameter.name)),
        ) {
            assert_eq!(
                operation,
                &Operation::SetEffectParam {
                    clip: clip.id,
                    effect: EffectId(8),
                    name: parameter.name.to_owned(),
                    value: ParamValue::Integer(parameter.neutral),
                }
            );
        }
        assert!(reset.contains(&Operation::ClearEffectKeyframes {
            clip: clip.id,
            effect: EffectId(8),
            name: "shadows_percent".to_owned(),
        }));
    }

    #[test]
    fn live_slider_frames_coalesce_while_discrete_edits_stay_separate() {
        let key = primary_coalesce_key(ClipId(3), EffectId(8), "exposure_milli_stops");
        assert_eq!(key, "primary:3:8:exposure_milli_stops");

        let mut edits = InspectorEdits::default();
        edits.begin_gesture();
        for value in [10, 20, 30] {
            edits.push_live(
                effect_param_operation(ClipId(3), EffectId(8), "exposure_milli_stops", value),
                key.clone(),
            );
        }
        assert_eq!(edits.operations().len(), 3);
        assert_eq!(edits.coalesce_key(), Some(key.as_str()));
        assert!(edits.gesture_started);

        // A discrete edit in the same frame is never folded into a drag.
        edits.push(clear_keyframes_operation(
            ClipId(3),
            EffectId(8),
            "exposure_milli_stops",
        ));
        assert_eq!(edits.coalesce_key(), None);
        assert_eq!(edits.operations().len(), 4);

        // A frame with no drag stays an ordinary batch.
        let mut typed = InspectorEdits::default();
        typed.push(effect_param_operation(
            ClipId(3),
            EffectId(8),
            "exposure_milli_stops",
            40,
        ));
        assert_eq!(typed.coalesce_key(), None);
        assert!(!typed.gesture_started);
    }

    /// egui reports the release frame of a drag as `changed() == true` with
    /// `dragged() == false`. Gating coalescing on `dragged()` alone therefore
    /// files the final value of every drag as a second undo entry.
    #[test]
    fn the_release_frame_of_a_drag_keeps_the_gesture_key() {
        let key = primary_coalesce_key(ClipId(3), EffectId(8), "exposure_milli_stops");

        // Frames 1..n of the drag, then the release frame, all share one key.
        let mut edits = InspectorEdits::default();
        edits.begin_gesture();
        for value in [10, 20] {
            edits.push_live(
                effect_param_operation(ClipId(3), EffectId(8), "exposure_milli_stops", value),
                key.clone(),
            );
        }
        edits.push_live(
            effect_param_operation(ClipId(3), EffectId(8), "exposure_milli_stops", 30),
            key.clone(),
        );
        assert_eq!(edits.operations().len(), 3);
        assert_eq!(
            edits.coalesce_key(),
            Some(key.as_str()),
            "the release frame must not open a second undo entry"
        );
    }

    /// The Speed and Audio-Gain sliders coalesce like the primary controls, on
    /// their own keys so two different controls never merge.
    #[test]
    fn speed_and_audio_gain_drags_coalesce_on_their_own_keys() {
        assert_eq!(speed_coalesce_key(ClipId(3)), "speed:3");
        assert_eq!(audio_gain_coalesce_key(ClipId(7)), "audio_gain:7");
        assert_ne!(
            speed_coalesce_key(ClipId(3)),
            audio_gain_coalesce_key(ClipId(3)),
            "two controls on one clip must not merge into one undo entry"
        );
        assert_ne!(
            speed_coalesce_key(ClipId(3)),
            primary_coalesce_key(ClipId(3), EffectId(8), "exposure_milli_stops")
        );

        // A speed change is several operations per frame; they still form one
        // coalesced batch.
        let mut edits = InspectorEdits::default();
        edits.begin_gesture();
        edits.extend_live(
            [
                Operation::SetClipSpeed {
                    clip: ClipId(3),
                    speed_percent: 200,
                },
                Operation::SetClipSpeed {
                    clip: ClipId(4),
                    speed_percent: 200,
                },
            ],
            speed_coalesce_key(ClipId(3)),
        );
        assert_eq!(edits.operations().len(), 2);
        assert_eq!(edits.coalesce_key(), Some("speed:3"));

        // Yielding no operation leaves no key behind to attach to a later edit.
        let mut empty = InspectorEdits::default();
        empty.extend_live(Vec::new(), speed_coalesce_key(ClipId(3)));
        assert_eq!(empty.coalesce_key(), None);
        assert!(empty.operations().is_empty());

        let mut gain = InspectorEdits::default();
        gain.push_live(
            clip_audio_operation(ClipId(7), -120, 0, 0),
            audio_gain_coalesce_key(ClipId(7)),
        );
        assert_eq!(gain.coalesce_key(), Some("audio_gain:7"));
    }

    #[test]
    fn keyframed_primary_parameters_are_badged_and_clearable() {
        let effect = Effect {
            id: EffectId(8),
            name: "primary_correction".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::from([(
                "exposure_milli_stops".to_owned(),
                AutomationCurve {
                    keyframes: vec![Keyframe {
                        at: TimeCode::ZERO,
                        value: 250,
                        interpolation: KeyframeInterpolation::Linear,
                    }],
                },
            )]),
        };

        assert!(parameter_is_keyframed(&effect, "exposure_milli_stops"));
        assert!(!parameter_is_keyframed(&effect, "saturation_percent"));
        assert_eq!(
            clear_keyframes_operation(ClipId(3), effect.id, "exposure_milli_stops"),
            Operation::ClearEffectKeyframes {
                clip: ClipId(3),
                effect: EffectId(8),
                name: "exposure_milli_stops".to_owned(),
            }
        );
    }

    #[test]
    fn primary_correction_readouts_use_human_units() {
        assert_eq!(primary_parameter_label("exposure_milli_stops"), "Exposure");
        assert_eq!(
            primary_parameter_label("contrast_pivot_basis_points"),
            "Pivot"
        );
        assert_eq!(primary_parameter_label("blacks_percent"), "Blacks");
        assert_eq!(primary_parameter_label("saturation_percent"), "Saturation");
        assert_eq!(
            primary_parameter_readout("exposure_milli_stops", 1_250),
            "+1.250 stops"
        );
        assert_eq!(
            primary_parameter_readout("contrast_pivot_basis_points", 5_000),
            "0.5000"
        );
        assert_eq!(primary_parameter_readout("whites_percent", -25), "-25%");
    }

    #[test]
    fn legacy_display_effects_are_visible_but_not_offered_for_new_insertion() {
        for name in ["brightness", "contrast", "saturation"] {
            assert!(!is_effect_insertable(name));
            assert!(is_legacy_display_effect(name));
        }
        assert!(is_effect_insertable("primary_correction"));
        // CC4 §7 moved `look_lut` into the exclusions: the managed kinds cover
        // it, so a new project never grows a legacy stage. An existing one
        // stays visible and offers "Convert to managed look".
        assert!(!is_effect_insertable("look_lut"));
        assert!(!is_effect_insertable("color_grade"));
        assert!(!is_effect_insertable("cube_lut"));
        for name in ["look_lut", "cube_lut"] {
            assert_eq!(
                effect_compatibility_stage(name)
                    .expect("LUT compatibility stage")
                    .issue_code(),
                "legacy_lut_stage"
            );
        }
    }

    #[test]
    fn inspector_hides_reframe_basis_points_only_when_percent_control_exists() {
        const BASIS_ONLY_PARAMETERS: &[EffectParameterDescriptor] = &[EffectParameterDescriptor {
            name: "focus_x_basis_points",
            min: 0,
            max: 10_000,
            neutral: 5_000,
            uniform: EffectUniform::ReframeFocusX,
        }];
        let descriptor = EFFECT_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.name == "reframe")
            .expect("reframe descriptor");

        assert!(should_render_effect_parameter(
            descriptor,
            "focus_x_percent"
        ));
        assert!(!should_render_effect_parameter(
            descriptor,
            "focus_x_basis_points"
        ));
        assert!(!should_render_effect_parameter(
            descriptor,
            "focus_y_basis_points"
        ));
        assert!(should_render_effect_parameter(
            descriptor,
            "target_aspect_basis_points"
        ));

        let basis_only_descriptor = EffectDescriptor {
            name: "reframe",
            parameters: BASIS_ONLY_PARAMETERS,
        };
        assert!(should_render_effect_parameter(
            &basis_only_descriptor,
            "focus_x_basis_points"
        ));

        let clip = Clip {
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
        };
        let Operation::AddEffect { effect, .. } = add_effect_operation(&clip, descriptor) else {
            panic!("expected add effect operation");
        };
        assert!(!effect.parameters.contains_key("focus_x_basis_points"));
        assert!(!effect.parameters.contains_key("focus_y_basis_points"));
    }

    #[test]
    fn crop_neutral_add_and_shared_freeze_controls_emit_media_style_ops() {
        let descriptor = EFFECT_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.name == "crop")
            .unwrap();
        let media = Clip {
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
        };
        let mut freeze = media.clone();
        freeze.content = ClipContent::Freeze(kinewright_core::FreezeFrame {
            source_frame: TimeCode(12),
        });

        let media_effect = add_effect_operation(&media, descriptor);
        let freeze_effect = add_effect_operation(&freeze, descriptor);
        assert_eq!(media_effect, freeze_effect);
        let Operation::AddEffect { effect, .. } = freeze_effect else {
            panic!("crop control must emit AddEffect");
        };
        assert_eq!(
            effect.parameters,
            BTreeMap::from([
                ("bottom_percent".to_owned(), ParamValue::Integer(0)),
                ("left_percent".to_owned(), ParamValue::Integer(0)),
                ("right_percent".to_owned(), ParamValue::Integer(0)),
                ("top_percent".to_owned(), ParamValue::Integer(0)),
            ])
        );

        let document = Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![freeze],
            }],
            color_context: kinewright_core::ColorContext::default(),
            ..Document::default()
        };
        assert_eq!(
            transition_duration_operations(&document, ClipId(1), "crossfade", 6),
            vec![Operation::AddTransition {
                clip: ClipId(1),
                transition: Transition {
                    name: "crossfade".to_owned(),
                    duration: TimeCode(6),
                },
            }]
        );
    }

    #[test]
    fn audio_controls_route_to_the_linked_audio_member() {
        let link = Some(LinkId(7));
        let document = Document {
            media_pool: vec![
                MediaAsset {
                    id: AssetId(1),
                    path: PathBuf::from("picture.mov"),
                    name: "Picture".to_owned(),
                    duration: TimeCode(30),
                    fps: Rational::new(30, 1).expect("valid fps"),
                    kind: MediaKind::Video,
                    resolution: Some((1920, 1080)),
                    source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
                    color_description: kinewright_core::ColorDescription::default(),
                },
                MediaAsset {
                    id: AssetId(2),
                    path: PathBuf::from("sound.wav"),
                    name: "Sound".to_owned(),
                    duration: TimeCode(30),
                    fps: Rational::new(30, 1).expect("valid fps"),
                    kind: MediaKind::Audio,
                    resolution: None,
                    source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
                    color_description: kinewright_core::ColorDescription::default(),
                },
            ],
            tracks: vec![
                Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![media_clip(ClipId(10), AssetId(1), link)],
                },
                Track {
                    id: TrackId(2),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![media_clip(ClipId(11), AssetId(2), link)],
                },
            ],
            color_context: kinewright_core::ColorContext::default(),
            ..Document::default()
        };

        let target = audio_target_clip(&document, ClipId(10)).expect("linked audio target");
        assert_eq!(target.id, ClipId(11));
        assert_eq!(
            clip_audio_operation(target.id, -60, 12, 4),
            Operation::SetClipAudio {
                clip: ClipId(11),
                gain_tenth_db: -60,
                fade_in_frames: TimeCode(12),
                fade_out_frames: TimeCode(4),
            }
        );
        assert_eq!(
            audio_target_clip(&document, ClipId(11)).map(|clip| clip.id),
            Some(ClipId(11))
        );
    }

    #[test]
    fn gain_slider_boundaries_round_trip_through_tenth_decibels() {
        for value in [-600, 120] {
            assert_eq!(db_to_tenth_db(tenth_db_to_db(value)), value);
        }
    }

    /// CC3 §7 makes both nodes first-class inserts, and CC3 §2.4 makes an
    /// omitted parameter resolve to its neutral: a fresh node therefore carries
    /// no parameters at all rather than 13 or 133 redundant neutrals.
    #[test]
    fn colour_nodes_insert_with_no_parameters_and_legacy_effects_stay_excluded() {
        assert!(is_effect_insertable("color_wheels"));
        assert!(is_effect_insertable("color_curves"));
        assert!(!is_effect_insertable("color_grade"));
        assert!(!is_effect_insertable("cube_lut"));
        for name in ["brightness", "contrast", "saturation"] {
            assert!(!is_effect_insertable(name));
        }

        let clip = media_clip(ClipId(10), AssetId(1), None);
        // CC5 §2.2 grew both descriptors by the 47 matte parameters. The counts
        // are written as the CC3 control count plus that constant so the
        // arithmetic, not a transcribed literal, is what this asserts.
        for (name, parameter_count) in [
            ("color_wheels", 13 + kinewright_core::MATTE_PARAMETER_COUNT),
            ("color_curves", 133 + kinewright_core::MATTE_PARAMETER_COUNT),
        ] {
            let descriptor = kinewright_core::effect_descriptor(name).expect("CC3 descriptor");
            assert_eq!(descriptor.parameters.len(), parameter_count);
            let Operation::InsertEffect { effect, .. } = add_effect_operation(&clip, &descriptor)
            else {
                panic!("{name} must emit InsertEffect");
            };
            assert_eq!(effect.name, name);
            assert!(
                effect.parameters.is_empty(),
                "{name} must insert at neutral by omission"
            );
            // The raw integers never reach the generic slider loop.
            for parameter in descriptor.parameters {
                assert!(!should_render_effect_parameter(&descriptor, parameter.name));
            }
        }
    }

    /// CC3 §5: a node reset is one `SetEffectParam` per descriptor parameter at
    /// its neutral plus a `ClearEffectKeyframes` for each automated one, in one
    /// batch and therefore one undo entry. `bypass` is an ordinary parameter and
    /// resets to `0` with everything else.
    ///
    /// CC5 §5 widens it: a matte-capable node's reset resets its matte too. The
    /// matte parameters this node never stored already resolve to their
    /// neutrals (CC5 §2.2), so the batch is the 13 CC3 controls and no matte
    /// write at all — resetting a CC4-era node must not grow its JSON by 47
    /// entries.
    #[test]
    fn a_wheels_reset_writes_every_neutral_and_clears_automation() {
        let descriptor = kinewright_core::effect_descriptor("color_wheels").expect("descriptor");
        let effect = keyframed_effect("color_wheels", "gain_red_thousandths", 1_800);
        let reset = color_node_reset_operations(ClipId(3), &effect, &descriptor);

        let sets: Vec<&Operation> = reset
            .iter()
            .filter(|operation| matches!(operation, Operation::SetEffectParam { .. }))
            .collect();
        assert_eq!(sets.len(), 13, "the 13 CC3 controls, and nothing else");
        assert_eq!(reset.len(), 14);
        assert!(
            !sets.iter().any(|operation| matches!(
                operation,
                Operation::SetEffectParam { name, .. } if is_matte_parameter(name)
            )),
            "a node with no stored matte needs no matte write"
        );
        for (operation, parameter) in sets.iter().zip(
            descriptor
                .parameters
                .iter()
                .filter(|parameter| !is_matte_parameter(parameter.name)),
        ) {
            assert_eq!(
                **operation,
                Operation::SetEffectParam {
                    clip: ClipId(3),
                    effect: EffectId(8),
                    name: parameter.name.to_owned(),
                    value: ParamValue::Integer(parameter.neutral),
                }
            );
        }
        assert!(reset.contains(&Operation::SetEffectParam {
            clip: ClipId(3),
            effect: EffectId(8),
            name: COLOR_NODE_BYPASS_PARAMETER.to_owned(),
            value: ParamValue::Integer(0),
        }));
        assert!(reset.contains(&Operation::ClearEffectKeyframes {
            clip: ClipId(3),
            effect: EffectId(8),
            name: "gain_red_thousandths".to_owned(),
        }));
    }

    /// Double-clicking one ball resets only that wheel's four controls.
    #[test]
    fn a_wheel_reset_touches_only_its_own_four_controls() {
        let effect = keyframed_effect("color_wheels", "lift_red_basis_points", 900);
        let reset = wheel_reset_operations(ClipId(3), &effect, ColorWheelControl::Lift);
        assert_eq!(reset.len(), 5);
        for channel in ColorWheelChannel::ALL {
            assert!(reset.contains(&Operation::SetEffectParam {
                clip: ClipId(3),
                effect: EffectId(8),
                name: ColorWheelControl::Lift.parameter_name(channel).to_owned(),
                value: ParamValue::Integer(0),
            }));
        }
        assert!(reset.contains(&Operation::ClearEffectKeyframes {
            clip: ClipId(3),
            effect: EffectId(8),
            name: "lift_red_basis_points".to_owned(),
        }));
        assert!(
            wheel_reset_operations(ClipId(3), &effect, ColorWheelControl::Gain)
                .iter()
                .all(|operation| !format!("{operation:?}").contains("lift_"))
        );
    }

    /// CC3 §5: a curve reset is the node reset restricted to one curve's 33
    /// parameters.
    #[test]
    fn a_curve_reset_covers_exactly_its_thirty_three_parameters() {
        let effect = keyframed_effect("color_curves", "master_y1", 8_000);
        let reset = curve_reset_operations(ClipId(3), &effect, ColorCurveChannel::Master);
        let sets: Vec<&Operation> = reset
            .iter()
            .filter(|operation| matches!(operation, Operation::SetEffectParam { .. }))
            .collect();
        assert_eq!(sets.len(), 33);
        assert_eq!(reset.len(), 34);
        for operation in &reset {
            let name = match operation {
                Operation::SetEffectParam { name, .. }
                | Operation::ClearEffectKeyframes { name, .. } => name.clone(),
                other => panic!("unexpected reset operation {other:?}"),
            };
            assert!(
                name.starts_with("master_"),
                "{name} is not a master control"
            );
        }
        // The neutrals are the structural identity: (0, 0) and (10000, 10000).
        assert!(reset.contains(&Operation::SetEffectParam {
            clip: ClipId(3),
            effect: EffectId(8),
            name: "master_x0".to_owned(),
            value: ParamValue::Integer(0),
        }));
        assert!(reset.contains(&Operation::SetEffectParam {
            clip: ClipId(3),
            effect: EffectId(8),
            name: "master_x1".to_owned(),
            value: ParamValue::Integer(10_000),
        }));
        assert!(reset.contains(&Operation::SetEffectParam {
            clip: ClipId(3),
            effect: EffectId(8),
            name: "master_point_count".to_owned(),
            value: ParamValue::Integer(2),
        }));
    }

    /// CC3 §2.4: an edit writes `{curve}_point_count` and the coordinates of the
    /// active points only. Nothing at index `>= point_count` is touched.
    #[test]
    fn a_curve_edit_writes_point_count_and_only_the_active_points() {
        let operations = curve_edit_operations(
            ClipId(10),
            EffectId(4),
            ColorCurveChannel::Master,
            &[(0, 0), (10_000, 10_000)],
            &[(0, 0), (5_000, 6_000), (10_000, 10_000)],
        );
        let written: Vec<(String, i64)> = operations
            .iter()
            .map(|operation| match operation {
                Operation::SetEffectParam { name, value, .. } => {
                    let ParamValue::Integer(value) = value else {
                        panic!("curves are integer-only");
                    };
                    (name.clone(), *value)
                }
                other => panic!("unexpected curve operation {other:?}"),
            })
            .collect();
        assert_eq!(
            written,
            vec![
                ("master_x0".to_owned(), 0),
                ("master_y0".to_owned(), 0),
                ("master_x1".to_owned(), 5_000),
                ("master_y1".to_owned(), 6_000),
                ("master_x2".to_owned(), 10_000),
                ("master_y2".to_owned(), 10_000),
                ("master_point_count".to_owned(), 3),
            ]
        );
        for index in 3..16 {
            let inactive = format!("master_x{index}");
            assert!(
                written.iter().all(|(name, _)| name != &inactive),
                "{inactive} must stay omitted so its neutral resolves"
            );
        }
    }

    /// Core validates every `SetEffectParam` against the document the change
    /// would produce, so a batch whose *intermediate* state has a non-increasing
    /// `x` is rejected even when its start and end states are legal. Every edit
    /// the widget can produce must therefore apply cleanly.
    #[test]
    fn every_curve_edit_batch_is_accepted_by_core_in_order() {
        let mut document = curves_document();
        let mut points = vec![(0, 0), (10_000, 10_000)];
        for next in [
            // Add a point: later coordinates move left, so writes run ascending.
            vec![(0, 0), (5_000, 6_000), (10_000, 10_000)],
            // Drag it left, then right, inside its neighbours.
            vec![(0, 0), (2_000, 6_000), (10_000, 10_000)],
            vec![(0, 0), (7_000, 6_000), (10_000, 10_000)],
            vec![(0, 0), (7_000, 6_000), (8_000, 9_000), (10_000, 10_000)],
            // Remove one: later coordinates move right, so the count shrinks
            // first and the writes run descending.
            vec![(0, 0), (8_000, 9_000), (10_000, 10_000)],
            // A mixed edit moves points in both directions at once.
            vec![(0, 0), (3_000, 1_000), (12_000, 12_000)],
            vec![(-2_000, -2_000), (10_000, 10_000)],
        ] {
            let operations = curve_edit_operations(
                ClipId(10),
                EffectId(4),
                ColorCurveChannel::Master,
                &points,
                &next,
            );
            kinewright_core::apply_batch(&mut document, &operations)
                .unwrap_or_else(|error| panic!("core rejected {next:?}: {error}"));
            let effect = &document.tracks[0].clips[0].effects[0];
            assert_eq!(ResolvedCurves::from_effect(effect).master.points, next);
            points = next;
        }
    }

    /// Stored states a user can reach through the curve editor, each legal on
    /// its own but hostile to a descriptor-ordered reset.
    fn adversarial_curve_states() -> Vec<(&'static str, Vec<(i32, i32)>)> {
        let mut sixteen = Vec::new();
        for index in 0..16 {
            let x = -2_000 + index * 500;
            sixteen.push((x, x));
        }
        vec![
            // The reported repro: both active points sit left of zero, so a
            // descriptor-ordered `master_x0 = 0` crosses `master_x1 = -1000`.
            ("negative pair", vec![(-2_000, -1_000), (-1_000, 500)]),
            // Both active points sit right of white, so `master_x1 = 10000`
            // would cross if it were written first instead.
            ("far right", vec![(9_000, 9_000), (11_000, 11_500)]),
            (
                "far right, three points",
                vec![(9_000, 0), (11_000, 5_000), (12_000, 12_000)],
            ),
            // One point must move right and the other left, so neither the
            // ascending nor the descending branch applies.
            ("reversed pair", vec![(-2_000, 4_000), (12_000, 1_000)]),
            ("sixteen points", sixteen),
        ]
    }

    /// Install a stored point list directly, bypassing the editor, so the reset
    /// is exercised against states the widget can reach but the batch builder
    /// has never seen produced.
    fn store_curve(document: &mut Document, curve: ColorCurveChannel, points: &[(i32, i32)]) {
        let effect = &mut document.tracks[0].clips[0].effects[0];
        effect.parameters.insert(
            curve.point_count_parameter().to_owned(),
            ParamValue::Integer(i64::try_from(points.len()).expect("a point count fits in i64")),
        );
        for (index, (x, y)) in points.iter().enumerate() {
            let x_name = curve.x_parameter(index).expect("an active x parameter");
            let y_name = curve.y_parameter(index).expect("an active y parameter");
            effect
                .parameters
                .insert(x_name.to_owned(), ParamValue::Integer(i64::from(*x)));
            effect
                .parameters
                .insert(y_name.to_owned(), ParamValue::Integer(i64::from(*y)));
        }
    }

    /// Core validates the strictly-increasing-`x` rule against every
    /// intermediate document, so descriptor order - `point_count`, `x0`, `y0`,
    /// `x1`, ... - is *not* a legal reset order: from a stored
    /// `x0 = -2000, x1 = -1000` the very first write crosses and `apply_batch`
    /// discards the whole reset without a visible failure.
    #[test]
    fn a_curve_reset_is_accepted_from_every_adversarial_stored_state() {
        for (label, stored) in adversarial_curve_states() {
            let mut document = curves_document();
            store_curve(&mut document, ColorCurveChannel::Master, &stored);
            document
                .validate()
                .unwrap_or_else(|error| panic!("{label} must be a legal stored state: {error}"));

            let effect = document.tracks[0].clips[0].effects[0].clone();
            let reset = curve_reset_operations(ClipId(10), &effect, ColorCurveChannel::Master);
            kinewright_core::apply_batch(&mut document, &reset)
                .unwrap_or_else(|error| panic!("core rejected the {label} reset: {error}"));

            let effect = &document.tracks[0].clips[0].effects[0];
            let resolved = ResolvedCurves::from_effect(effect);
            assert!(
                resolved.master.is_structural_identity(),
                "the {label} reset must restore (0, 0) and (10000, 10000)",
            );
            assert!(!resolved.master.truncated);
            // Every one of the curve's 33 parameters ends at its neutral,
            // including the inactive points 2..16.
            for parameter in ColorCurveChannel::Master.parameters() {
                assert_eq!(
                    effect.parameters.get(parameter.name),
                    Some(&ParamValue::Integer(parameter.neutral)),
                    "{} must end at its neutral after the {label} reset",
                    parameter.name,
                );
            }
        }
    }

    /// The regression the ordering fix exists for: writing the same neutrals in
    /// descriptor order is rejected, so a reset that did so would be silently
    /// discarded by `apply_batch`.
    #[test]
    fn a_descriptor_ordered_curve_reset_would_be_rejected() {
        let mut document = curves_document();
        store_curve(
            &mut document,
            ColorCurveChannel::Master,
            &[(-2_000, -1_000), (-1_000, 500)],
        );
        let descriptor_order: Vec<Operation> = ColorCurveChannel::Master
            .parameters()
            .iter()
            .map(|parameter| {
                effect_param_operation(ClipId(10), EffectId(4), parameter.name, parameter.neutral)
            })
            .collect();
        let error = kinewright_core::apply_batch(&mut document.clone(), &descriptor_order)
            .expect_err("descriptor order must cross x0 over x1");
        assert!(
            format!("{error}").contains("strictly increasing"),
            "unexpected rejection: {error}"
        );
    }

    /// The whole-node reset covers all four curves plus the node-owned
    /// `bypass`, and must be accepted from the same hostile stored states.
    #[test]
    fn a_whole_node_curves_reset_is_accepted_from_every_adversarial_stored_state() {
        let descriptor = kinewright_core::effect_descriptor("color_curves").expect("descriptor");
        for (label, stored) in adversarial_curve_states() {
            let mut document = curves_document();
            for curve in ColorCurveChannel::ALL {
                store_curve(&mut document, curve, &stored);
            }
            document.tracks[0].clips[0].effects[0].parameters.insert(
                COLOR_NODE_BYPASS_PARAMETER.to_owned(),
                ParamValue::Integer(1),
            );
            document
                .validate()
                .unwrap_or_else(|error| panic!("{label} must be a legal stored state: {error}"));

            let effect = document.tracks[0].clips[0].effects[0].clone();
            let reset = color_node_reset_operations(ClipId(10), &effect, &descriptor);
            kinewright_core::apply_batch(&mut document, &reset)
                .unwrap_or_else(|error| panic!("core rejected the {label} node reset: {error}"));

            let effect = &document.tracks[0].clips[0].effects[0];
            let resolved = ResolvedCurves::from_effect(effect);
            assert!(resolved.is_neutral(), "the {label} node reset must be flat");
            assert!(
                !resolved.bypass(),
                "the {label} node reset must clear bypass"
            );
            for parameter in descriptor.parameters {
                // A matte parameter this node never stored already *resolves*
                // to its neutral, and the reset deliberately leaves it unstored
                // rather than adding 47 entries to a CC4-era node (CC5 §2.2);
                // what must hold either way is the resolved value.
                assert_eq!(
                    effect
                        .integer_parameter_at(parameter.name, TimeCode::ZERO)
                        .unwrap_or(parameter.neutral),
                    parameter.neutral,
                    "{} must resolve to its neutral after the {label} node reset",
                    parameter.name,
                );
                if is_matte_parameter(parameter.name) {
                    assert!(
                        !effect.parameters.contains_key(parameter.name),
                        "{} was never stored, so the reset must not store it",
                        parameter.name,
                    );
                } else {
                    assert_eq!(
                        effect.parameters.get(parameter.name),
                        Some(&ParamValue::Integer(parameter.neutral)),
                        "{} must end at its neutral after the {label} node reset",
                        parameter.name,
                    );
                }
            }
        }
    }

    /// The descending branch writes `{curve}_point_count` before the points, so
    /// it must never grow the active prefix: growing it would expose the
    /// colliding `(10000, 10000)` neutrals of the points still unwritten. This
    /// edit satisfies the coordinate half of the test - every shared point moves
    /// right - and would be rejected without the length guard.
    #[test]
    fn a_growing_curve_edit_never_takes_the_count_first_branch() {
        let mut document = curves_document();
        let current = [(0, 0), (11_000, 11_000)];
        let next = [(0, 0), (11_500, 11_500), (11_800, 11_800)];
        kinewright_core::apply_batch(
            &mut document,
            &curve_edit_operations(
                ClipId(10),
                EffectId(4),
                ColorCurveChannel::Master,
                &[(0, 0), (10_000, 10_000)],
                &current,
            ),
        )
        .expect("the setup edit must apply");

        let operations = curve_edit_operations(
            ClipId(10),
            EffectId(4),
            ColorCurveChannel::Master,
            &current,
            &next,
        );
        let first_count = operations
            .iter()
            .find_map(|operation| match operation {
                Operation::SetEffectParam { name, value, .. } if name == "master_point_count" => {
                    Some(value.clone())
                }
                _ => None,
            })
            .expect("a curve edit always writes its point count");
        assert_eq!(
            first_count,
            ParamValue::Integer(2),
            "a growing edit must shrink the prefix first, never grow it"
        );
        kinewright_core::apply_batch(&mut document, &operations)
            .expect("core must accept a growing edit past white");
        assert_eq!(
            ResolvedCurves::from_effect(&document.tracks[0].clips[0].effects[0])
                .master
                .points,
            next.to_vec()
        );
    }

    /// CC3 §7 requires an indicator per keyframed control, not per keyframed
    /// control of the selected curve. Automation on the red curve stays visible
    /// while the master curve is on screen.
    #[test]
    fn keyframe_badges_cover_every_curve_and_the_node_bypass() {
        let mut effect = keyframed_effect("color_curves", "master_y1", 8_000);
        for name in ["red_x1", "red_point_count", "blue_y0"] {
            effect.keyframes.insert(
                name.to_owned(),
                AutomationCurve {
                    keyframes: vec![Keyframe {
                        at: TimeCode::ZERO,
                        value: 2,
                        interpolation: KeyframeInterpolation::Hold,
                    }],
                },
            );
        }
        effect.keyframes.insert(
            COLOR_NODE_BYPASS_PARAMETER.to_owned(),
            AutomationCurve {
                keyframes: vec![Keyframe {
                    at: TimeCode::ZERO,
                    value: 1,
                    interpolation: KeyframeInterpolation::Hold,
                }],
            },
        );
        // Every keyframed control appears exactly once, grouped under the curve
        // that owns it, in `ColorCurveChannel::ALL` order with the node-owned
        // `bypass` last. Green carries no automation and is omitted.
        assert_eq!(
            color_curve_keyframe_groups(&effect),
            vec![
                ("Master", vec!["master_y1"]),
                ("Red", vec!["red_point_count", "red_x1"]),
                ("Blue", vec!["blue_y0"]),
                ("Node", vec![COLOR_NODE_BYPASS_PARAMETER]),
            ]
        );
        assert!(
            color_curve_keyframe_groups(&Effect {
                keyframes: BTreeMap::new(),
                ..effect
            })
            .is_empty(),
            "a node with no automation shows no badges"
        );
    }

    /// CC3 §7: the card reports `curve_truncated_by_automation` even when the
    /// only automation is the node-owned `bypass`. `bypass` is not curve-owned,
    /// so a scan built from curve keyframes alone would only ever look at frame
    /// zero, where this node is still bypassed.
    #[test]
    fn a_keyframed_bypass_is_part_of_the_truncation_scan() {
        let mut effect = Effect {
            id: EffectId(4),
            name: "color_curves".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::from([
                // A whole-curve step to sixteen points whose coordinates are
                // omitted: every point past the first resolves to the colliding
                // `(10000, 10000)` neutral, so the curve truncates to two.
                (
                    "master_point_count".to_owned(),
                    AutomationCurve {
                        keyframes: vec![Keyframe {
                            at: TimeCode::ZERO,
                            value: 16,
                            interpolation: KeyframeInterpolation::Hold,
                        }],
                    },
                ),
            ]),
        };
        effect.keyframes.insert(
            COLOR_NODE_BYPASS_PARAMETER.to_owned(),
            AutomationCurve {
                keyframes: vec![
                    Keyframe {
                        at: TimeCode::ZERO,
                        value: 1,
                        interpolation: KeyframeInterpolation::Hold,
                    },
                    Keyframe {
                        at: TimeCode(10),
                        value: 0,
                        interpolation: KeyframeInterpolation::Hold,
                    },
                ],
            },
        );
        assert_eq!(
            automation_truncated_curves(&effect),
            vec![ColorCurveChannel::Master],
            "frame 10 releases the bypass and must be scanned"
        );
    }

    /// CC3 §7: a drag applies one batch per frame under a stable gesture key so
    /// the preview stays live and the whole gesture is one undo entry. A
    /// discrete edit never carries a key.
    #[test]
    fn wheel_and_curve_gestures_coalesce_under_their_own_keys() {
        assert_eq!(
            wheels_coalesce_key(ClipId(3), EffectId(8), ColorWheelControl::Lift),
            "wheels:3:8:lift"
        );
        assert_eq!(
            wheels_coalesce_key(ClipId(3), EffectId(8), ColorWheelControl::Gain),
            "wheels:3:8:gain"
        );
        assert_eq!(
            curves_coalesce_key(ClipId(3), EffectId(8), ColorCurveChannel::Red),
            "curves:3:8:red"
        );
        assert_ne!(
            wheels_coalesce_key(ClipId(3), EffectId(8), ColorWheelControl::Lift),
            wheels_coalesce_key(ClipId(3), EffectId(9), ColorWheelControl::Lift),
            "two nodes on one clip must never merge into one undo entry"
        );

        let effect = keyframed_effect("color_wheels", "gain_red_thousandths", 1_800);
        let mut edits = InspectorEdits::default();
        for (frame, value) in [1_100, 1_200, 1_300].into_iter().enumerate() {
            let response = ColorWheelResponse {
                changes: vec![(ColorWheelChannel::Red, value)],
                live: true,
                gesture_started: frame == 0,
                reset: false,
            };
            apply_wheel_response(
                ClipId(3),
                &effect,
                ColorWheelControl::Gain,
                &response,
                &mut edits,
            );
        }
        assert_eq!(edits.operations().len(), 3);
        assert_eq!(edits.coalesce_key(), Some("wheels:3:8:gain"));
        assert!(edits.gesture_started);

        // A double-click reset in the same frame drops the key: it is discrete.
        apply_wheel_response(
            ClipId(3),
            &effect,
            ColorWheelControl::Gain,
            &ColorWheelResponse {
                reset: true,
                ..ColorWheelResponse::default()
            },
            &mut edits,
        );
        assert_eq!(edits.coalesce_key(), None);

        // A click that adds a curve point is discrete; a point drag is live.
        let curves = keyframed_effect("color_curves", "master_y1", 8_000);
        let mut click = InspectorEdits::default();
        apply_curve_response(
            ClipId(3),
            &curves,
            ColorCurveChannel::Master,
            &[(0, 0), (10_000, 10_000)],
            &CurveEditorResponse {
                points: Some(vec![(0, 0), (5_000, 6_000), (10_000, 10_000)]),
                ..CurveEditorResponse::default()
            },
            &mut click,
        );
        assert_eq!(click.operations().len(), 7);
        assert_eq!(click.coalesce_key(), None);
        assert!(!click.gesture_started);

        let mut drag = InspectorEdits::default();
        for (frame, x) in [4_000, 4_500, 5_000].into_iter().enumerate() {
            apply_curve_response(
                ClipId(3),
                &curves,
                ColorCurveChannel::Master,
                &[(0, 0), (3_000, 6_000), (10_000, 10_000)],
                &CurveEditorResponse {
                    points: Some(vec![(0, 0), (x, 6_000), (10_000, 10_000)]),
                    live: true,
                    gesture_started: frame == 0,
                    reset: false,
                },
                &mut drag,
            );
        }
        assert_eq!(drag.coalesce_key(), Some("curves:3:8:master"));
        assert!(drag.gesture_started);
        assert_eq!(drag.operations().len(), 21);
    }

    /// CC3 §5: bypass is an ordinary integer parameter set with one
    /// `SetEffectParam`, never a UI-only flag and never a node removal.
    #[test]
    fn the_bypass_toggle_emits_exactly_one_set_effect_param() {
        let mut edits = InspectorEdits::default();
        edits.push(effect_param_operation(
            ClipId(3),
            EffectId(8),
            COLOR_NODE_BYPASS_PARAMETER,
            1,
        ));
        assert_eq!(edits.operations().len(), 1);
        assert_eq!(edits.coalesce_key(), None);
        assert_eq!(
            edits.operations()[0],
            Operation::SetEffectParam {
                clip: ClipId(3),
                effect: EffectId(8),
                name: "bypass".to_owned(),
                value: ParamValue::Integer(1),
            }
        );

        let mut effect = keyframed_effect("color_wheels", "gain_red_thousandths", 1_800);
        assert_eq!(bypass_token(&effect), 0, "an omitted bypass resolves to 0");
        effect
            .parameters
            .insert("bypass".to_owned(), ParamValue::Integer(1));
        assert_eq!(bypass_token(&effect), 1);
        assert_eq!(
            color_node_inactive_reason(&effect),
            Some(kinewright_core::ColorNodeInactiveReason::Bypassed)
        );
    }

    /// CC3 §7: the card reports `curve_truncated_by_automation` when keyframe
    /// evaluation leaves a curve without strictly increasing `x` (CC3 §3.4).
    #[test]
    fn automation_that_crosses_points_is_reported_as_truncation() {
        let mut effect = Effect {
            id: EffectId(4),
            name: "color_curves".to_owned(),
            parameters: BTreeMap::from([
                ("master_point_count".to_owned(), ParamValue::Integer(3)),
                ("master_x1".to_owned(), ParamValue::Integer(5_000)),
                ("master_y1".to_owned(), ParamValue::Integer(6_000)),
                ("master_x2".to_owned(), ParamValue::Integer(10_000)),
                ("master_y2".to_owned(), ParamValue::Integer(10_000)),
            ]),
            keyframes: BTreeMap::new(),
        };
        assert!(automation_truncated_curves(&effect).is_empty());

        effect.keyframes.insert(
            "master_x1".to_owned(),
            AutomationCurve {
                keyframes: vec![
                    Keyframe {
                        at: TimeCode::ZERO,
                        value: 5_000,
                        interpolation: KeyframeInterpolation::Linear,
                    },
                    Keyframe {
                        at: TimeCode(5),
                        value: 11_000,
                        interpolation: KeyframeInterpolation::Linear,
                    },
                ],
            },
        );
        assert_eq!(
            automation_truncated_curves(&effect),
            vec![ColorCurveChannel::Master]
        );

        // A bypassed node is the exact identity, so it reports nothing.
        effect
            .parameters
            .insert("bypass".to_owned(), ParamValue::Integer(1));
        assert!(automation_truncated_curves(&effect).is_empty());
    }

    /// The ball reads the stored integers, not a cached float, and badges the
    /// controls automation drives.
    #[test]
    fn a_wheel_reads_its_stored_integers_and_keyframe_state() {
        let mut effect = keyframed_effect("color_wheels", "gain_red_thousandths", 1_800);
        effect.parameters.insert(
            "gain_master_thousandths".to_owned(),
            ParamValue::Integer(1_400),
        );
        effect
            .parameters
            .insert("gain_red_thousandths".to_owned(), ParamValue::Integer(900));
        let params = ColorWheelsParams::from_effect(&effect);
        let state = wheel_state(&effect, params, ColorWheelControl::Gain);
        assert_eq!(state.values.master, 1_400);
        assert_eq!(state.values.red, 900);
        assert_eq!(state.values.green, 1_000);
        assert_eq!(state.keyframed, [false, true, false, false]);

        let lift = wheel_state(&effect, params, ColorWheelControl::Lift);
        assert_eq!(lift.values.master, 0);
        assert_eq!(lift.keyframed, [false; 4]);
    }

    fn keyframed_effect(name: &str, parameter: &str, value: i64) -> Effect {
        Effect {
            id: EffectId(8),
            name: name.to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::from([(
                parameter.to_owned(),
                AutomationCurve {
                    keyframes: vec![Keyframe {
                        at: TimeCode::ZERO,
                        value,
                        interpolation: KeyframeInterpolation::Hold,
                    }],
                },
            )]),
        }
    }

    /// A one-clip document carrying an all-neutral `color_curves` node.
    fn curves_document() -> Document {
        let mut clip = media_clip(ClipId(10), AssetId(1), None);
        clip.effects = vec![Effect {
            id: EffectId(4),
            name: "color_curves".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::new(),
        }];
        Document {
            media_pool: vec![MediaAsset {
                id: AssetId(1),
                path: PathBuf::from("picture.mov"),
                name: "Picture".to_owned(),
                duration: TimeCode(30),
                fps: Rational::new(30, 1).expect("valid fps"),
                kind: MediaKind::Video,
                resolution: Some((1920, 1080)),
                source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
                color_description: kinewright_core::ColorDescription::default(),
            }],
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![clip],
            }],
            color_context: kinewright_core::ColorContext::default(),
            duration: TimeCode(30),
            ..Document::default()
        }
    }

    fn media_clip(id: ClipId, asset: AssetId, link: Option<LinkId>) -> Clip {
        Clip {
            id,
            asset,
            source_range: TimeCode(0)..TimeCode(30),
            content: ClipContent::Media,
            timeline_start: TimeCode::ZERO,
            effects: Vec::new(),
            transition_in: None,
            link,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        }
    }

    // -----------------------------------------------------------------------
    // CC4 §7 look workflow
    // -----------------------------------------------------------------------

    fn colour_effect(id: u64, name: &str) -> Effect {
        Effect {
            id: EffectId(id),
            name: name.to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::new(),
        }
    }

    fn lut_effect(id: u64, name: &str, asset: u64, mix: Option<i64>) -> Effect {
        let mut parameters = BTreeMap::from([(
            LUT_ASSET_ID_PARAMETER.to_owned(),
            ParamValue::Integer(i64::try_from(asset).expect("fixture id")),
        )]);
        if let Some(mix) = mix {
            parameters.insert(LUT_MIX_PARAMETER.to_owned(), ParamValue::Integer(mix));
        }
        Effect {
            id: EffectId(id),
            name: name.to_owned(),
            parameters,
            keyframes: BTreeMap::new(),
        }
    }

    fn look_clip(effects: Vec<Effect>) -> Clip {
        let mut clip = media_clip(ClipId(10), AssetId(1), None);
        clip.effects = effects;
        clip
    }

    /// A document carrying one clip and one registered built-in asset.
    fn look_document(effects: Vec<Effect>, assets: Vec<LutAsset>) -> Document {
        let mut document = curves_document();
        document.tracks[0].clips[0].effects = effects;
        document.lut_assets = assets;
        document
    }

    #[test]
    fn the_insert_index_puts_a_technical_lut_before_every_correction() {
        // Interleaved non-colour effects are unconstrained and are stepped
        // over without moving the index.
        let effects = vec![
            colour_effect(1, "crop"),
            colour_effect(2, "primary_correction"),
            colour_effect(3, "mask"),
            colour_effect(4, "color_curves"),
            lut_effect(5, "creative_look", 1, None),
            colour_effect(6, "reframe"),
        ];
        assert_eq!(color_stage_insert_index(&effects, ColorStage::Input), 0);
        // A creative look lands after the last managed node, which is the
        // existing look at index 4.
        assert_eq!(color_stage_insert_index(&effects, ColorStage::Look), 5);
        // A correction lands after the last correction, before the look.
        assert_eq!(
            color_stage_insert_index(&effects, ColorStage::Correction),
            4
        );
    }

    #[test]
    fn the_insert_index_appends_a_second_technical_lut_after_the_first() {
        let effects = vec![
            lut_effect(1, "technical_lut", 1, None),
            colour_effect(2, "crop"),
            colour_effect(3, "primary_correction"),
        ];
        assert_eq!(color_stage_insert_index(&effects, ColorStage::Input), 1);
        assert_eq!(color_stage_insert_index(&effects, ColorStage::Look), 3);
    }

    #[test]
    fn an_empty_stack_inserts_every_stage_at_the_front() {
        assert_eq!(color_stage_insert_index(&[], ColorStage::Input), 0);
        assert_eq!(color_stage_insert_index(&[], ColorStage::Look), 0);
    }

    #[test]
    fn every_computed_insert_index_is_accepted_by_core() {
        // The whole point of the computed index: no ordinary path can produce
        // a `ColorStageOrderViolation`.
        let mut document = look_document(
            vec![
                colour_effect(1, "crop"),
                colour_effect(2, "primary_correction"),
                colour_effect(3, "color_curves"),
            ],
            vec![BuiltinLook::Warm.to_lut_asset(LutAssetId(1))],
        );
        for stage in [ColorStage::Look, ColorStage::Input] {
            let clip = document.clip(ClipId(10)).expect("fixture clip").clone();
            let operation = insert_lut_node_operation(&clip, stage, LutAssetId(1));
            kinewright_core::apply_batch(&mut document, std::slice::from_ref(&operation))
                .unwrap_or_else(|error| panic!("{stage:?} insert rejected: {error}"));
        }
        let effects = &document.clip(ClipId(10)).expect("clip").effects;
        assert_eq!(effects[0].name, "technical_lut");
        assert_eq!(effects.last().expect("look").name, "creative_look");
        document.validate().expect("the stage order holds");
    }

    #[test]
    fn an_inserted_look_writes_only_the_binding() {
        let clip = look_clip(vec![colour_effect(1, "primary_correction")]);
        let Operation::InsertEffect {
            clip: target,
            index,
            effect,
        } = insert_lut_node_operation(&clip, ColorStage::Look, LutAssetId(7))
        else {
            panic!("insert_lut_node_operation must emit InsertEffect");
        };
        assert_eq!(target, ClipId(10));
        assert_eq!(index, 1);
        assert_eq!(effect.id, EffectId(2));
        assert_eq!(effect.name, "creative_look");
        // CC3 §2.4's convention: only the values the operator touched. The
        // neutral mix of a creative look is full strength, so a node created
        // with only a binding shows the look.
        assert_eq!(
            effect.parameters,
            BTreeMap::from([(LUT_ASSET_ID_PARAMETER.to_owned(), ParamValue::Integer(7))])
        );
        assert!(effect.keyframes.is_empty());
    }

    #[test]
    fn the_mix_slider_writes_percent_times_one_hundred_under_its_own_key() {
        let mut pending = InspectorEdits::default();
        pending.push_live(
            effect_param_operation(ClipId(10), EffectId(2), LUT_MIX_PARAMETER, 65 * 100),
            look_mix_coalesce_key(ClipId(10), EffectId(2)),
        );
        assert_eq!(pending.coalesce_key(), Some("look:10:2:mix"));
        assert_eq!(
            pending.operations(),
            [effect_param_operation(
                ClipId(10),
                EffectId(2),
                LUT_MIX_PARAMETER,
                6_500
            )]
        );
        assert_eq!(mix_percent(6_500, LUT_MIX_BASIS_POINTS_MAX), 65);
        assert_eq!(mix_percent(10_000, LUT_MIX_BASIS_POINTS_MAX), 100);
        assert_eq!(mix_percent(0, LUT_MIX_BASIS_POINTS_MAX), 0);
    }

    #[test]
    fn the_release_frame_of_a_mix_drag_stays_in_the_gesture() {
        // egui reports the release frame as `changed() && !dragged()`, so the
        // gate that decides whether a frame coalesces has to accept it.
        let mut pending = InspectorEdits::default();
        for value in [10, 40, 55] {
            pending.push_live(
                effect_param_operation(ClipId(10), EffectId(2), LUT_MIX_PARAMETER, value * 100),
                look_mix_coalesce_key(ClipId(10), EffectId(2)),
            );
        }
        assert_eq!(pending.operations().len(), 3);
        assert_eq!(pending.coalesce_key(), Some("look:10:2:mix"));
    }

    #[test]
    fn a_mix_drag_and_a_discrete_edit_in_one_frame_stop_coalescing() {
        let mut pending = InspectorEdits::default();
        pending.push_live(
            effect_param_operation(ClipId(10), EffectId(2), LUT_MIX_PARAMETER, 5_000),
            look_mix_coalesce_key(ClipId(10), EffectId(2)),
        );
        pending.push(effect_param_operation(
            ClipId(10),
            EffectId(2),
            COLOR_NODE_BYPASS_PARAMETER,
            1,
        ));
        assert_eq!(pending.coalesce_key(), None);
    }

    #[test]
    fn the_ab_hold_emits_bypass_one_then_zero_through_the_coalesced_path() {
        let mut pending = InspectorEdits::default();
        let mut state = AbHoldState::default();

        // Press: capture the stored value and write the real bypass.
        let press = ab_hold_step(ClipId(10), EffectId(2), state, true, 0);
        assert!(press.gesture_started);
        state = press.state;
        assert_eq!(
            state,
            AbHoldState {
                held: true,
                restore: 0
            }
        );
        let press_operation = press.operation.expect("a press writes bypass = 1");
        assert_eq!(
            press_operation,
            effect_param_operation(ClipId(10), EffectId(2), COLOR_NODE_BYPASS_PARAMETER, 1)
        );
        pending.begin_gesture();
        pending.push_live(
            press_operation,
            look_ab_coalesce_key(ClipId(10), EffectId(2)),
        );

        // Held frames emit nothing at all, so a hold is not one entry per frame.
        let held = ab_hold_step(ClipId(10), EffectId(2), state, true, 1);
        assert!(held.operation.is_none());
        assert!(!held.gesture_started);
        state = held.state;

        // Release: restore the value captured on the press, not the live `1`.
        let release = ab_hold_step(ClipId(10), EffectId(2), state, false, 1);
        let release_operation = release.operation.expect("a release restores bypass");
        assert_eq!(
            release_operation,
            effect_param_operation(ClipId(10), EffectId(2), COLOR_NODE_BYPASS_PARAMETER, 0)
        );
        assert_eq!(release.state, AbHoldState::default());
        pending.push_live(
            release_operation,
            look_ab_coalesce_key(ClipId(10), EffectId(2)),
        );

        // One hold is one undo entry.
        assert_eq!(pending.coalesce_key(), Some("look:10:2:ab"));
        assert_eq!(pending.operations().len(), 2);
    }

    #[test]
    fn an_ab_hold_over_an_already_bypassed_node_restores_the_bypass() {
        let press = ab_hold_step(ClipId(10), EffectId(2), AbHoldState::default(), true, 1);
        let release = ab_hold_step(ClipId(10), EffectId(2), press.state, false, 1);
        assert_eq!(
            release.operation.expect("release"),
            effect_param_operation(ClipId(10), EffectId(2), COLOR_NODE_BYPASS_PARAMETER, 1)
        );
    }

    #[test]
    fn a_look_reset_excludes_the_binding_and_its_keyframe_clear() {
        let descriptor =
            kinewright_core::effect_descriptor("creative_look").expect("the CC4 descriptor exists");
        let mut effect = lut_effect(2, "creative_look", 9, Some(4_000));
        // Both the binding and the mix carry automation, so the reset has a
        // `ClearEffectKeyframes` to omit and one to keep.
        effect.keyframes.insert(
            LUT_ASSET_ID_PARAMETER.to_owned(),
            AutomationCurve {
                keyframes: vec![Keyframe {
                    at: TimeCode::ZERO,
                    value: 9,
                    interpolation: KeyframeInterpolation::Hold,
                }],
            },
        );
        effect.keyframes.insert(
            LUT_MIX_PARAMETER.to_owned(),
            AutomationCurve {
                keyframes: vec![Keyframe {
                    at: TimeCode::ZERO,
                    value: 4_000,
                    interpolation: KeyframeInterpolation::Hold,
                }],
            },
        );

        let operations = color_node_reset_operations(ClipId(10), &effect, &descriptor);

        assert!(
            !operations.iter().any(|operation| matches!(
                operation,
                Operation::SetEffectParam { name, .. } | Operation::ClearEffectKeyframes { name, .. }
                    if name == LUT_ASSET_ID_PARAMETER
            )),
            "resetting the binding would unbind the node, which Core rejects (CC4 §6): {operations:?}"
        );
        assert!(operations.contains(&effect_param_operation(
            ClipId(10),
            EffectId(2),
            LUT_MIX_PARAMETER,
            LUT_MIX_BASIS_POINTS_MAX
        )));
        assert!(operations.contains(&clear_keyframes_operation(
            ClipId(10),
            EffectId(2),
            LUT_MIX_PARAMETER
        )));
        assert!(operations.contains(&effect_param_operation(
            ClipId(10),
            EffectId(2),
            COLOR_NODE_BYPASS_PARAMETER,
            0
        )));
    }

    #[test]
    fn a_look_reset_is_accepted_by_core_and_keeps_the_binding() {
        let mut document = look_document(
            vec![lut_effect(2, "creative_look", 1, Some(2_500))],
            vec![BuiltinLook::Warm.to_lut_asset(LutAssetId(1))],
        );
        let descriptor = kinewright_core::effect_descriptor("creative_look").expect("descriptor");
        let effect = document.clip(ClipId(10)).expect("clip").effects[0].clone();
        for operation in color_node_reset_operations(ClipId(10), &effect, &descriptor) {
            kinewright_core::apply_batch(&mut document, std::slice::from_ref(&operation))
                .unwrap_or_else(|error| panic!("reset rejected: {error}"));
        }
        let reset = &document.clip(ClipId(10)).expect("clip").effects[0];
        assert_eq!(
            LutNodeParams::from_effect(reset).lut_asset_id,
            LutAssetId(1)
        );
        assert_eq!(
            LutNodeParams::from_effect(reset).mix_basis_points,
            LUT_MIX_BASIS_POINTS_MAX
        );
        document.validate().expect("the reset document is valid");
    }

    #[test]
    fn a_technical_lut_reset_also_keeps_its_binding() {
        let descriptor = kinewright_core::effect_descriptor("technical_lut").expect("descriptor");
        let effect = lut_effect(3, "technical_lut", 4, None);
        let operations = color_node_reset_operations(ClipId(10), &effect, &descriptor);
        assert!(!operations.iter().any(|operation| matches!(
            operation,
            Operation::SetEffectParam { name, .. } if name == LUT_ASSET_ID_PARAMETER
        )));
        // The pinned mix resets to its own neutral, which is full strength.
        assert!(operations.contains(&effect_param_operation(
            ClipId(10),
            EffectId(3),
            LUT_MIX_PARAMETER,
            LUT_MIX_BASIS_POINTS_MAX
        )));
    }

    #[test]
    fn every_preset_token_converts_to_its_built_in_with_the_mapped_intensity() {
        for token in 0..=4 {
            let mut legacy = colour_effect(3, "look_lut");
            legacy
                .parameters
                .insert("preset_token".to_owned(), ParamValue::Integer(token));
            legacy
                .parameters
                .insert("intensity_percent".to_owned(), ParamValue::Integer(60));
            let document = look_document(vec![legacy.clone()], Vec::new());

            let operations = convert_builtin_look_operations(&document, ClipId(10), &legacy)
                .unwrap_or_else(|error| panic!("token {token} did not convert: {error}"));

            // CC4 §9: the visible batch is exactly `[AddLutAsset, ConvertLegacyLook]`.
            assert_eq!(operations.len(), 2, "token {token}: {operations:?}");
            let expected =
                BuiltinLook::from_preset_token(token).expect("token 0..=4 is a built-in");
            let Operation::AddLutAsset { asset } = &operations[0] else {
                panic!("token {token}: the batch must lead with AddLutAsset");
            };
            assert_eq!(asset.id, LutAssetId(1));
            assert_eq!(asset.sha256, expected.sha256());
            assert_eq!(asset.title, expected.title());
            assert_eq!(
                asset.source,
                LutAssetSource::Builtin {
                    name: expected.name().to_owned()
                }
            );
            assert_eq!(
                operations[1],
                Operation::ConvertLegacyLook {
                    clip: ClipId(10),
                    effect: EffectId(3),
                    lut_asset: LutAssetId(1),
                    // intensity_percent 60 -> 6000 basis points.
                    mix_basis_points: 6_000,
                }
            );
        }
    }

    #[test]
    fn converting_a_registered_built_in_emits_only_the_conversion() {
        let mut legacy = colour_effect(3, "look_lut");
        legacy
            .parameters
            .insert("preset_token".to_owned(), ParamValue::Integer(1));
        let document = look_document(
            vec![legacy.clone()],
            vec![BuiltinLook::Warm.to_lut_asset(LutAssetId(4))],
        );
        let operations =
            convert_builtin_look_operations(&document, ClipId(10), &legacy).expect("converts");
        assert_eq!(
            operations,
            vec![Operation::ConvertLegacyLook {
                clip: ClipId(10),
                effect: EffectId(3),
                lut_asset: LutAssetId(4),
                // An omitted intensity_percent resolves to its neutral, 100 %.
                mix_basis_points: 10_000,
            }]
        );
    }

    #[test]
    fn a_conversion_batch_is_accepted_by_core_in_order() {
        let mut legacy = colour_effect(3, "look_lut");
        legacy
            .parameters
            .insert("preset_token".to_owned(), ParamValue::Integer(2));
        legacy
            .parameters
            .insert("intensity_percent".to_owned(), ParamValue::Integer(75));
        let mut document = look_document(
            vec![colour_effect(1, "primary_correction"), legacy.clone()],
            Vec::new(),
        );
        for operation in convert_builtin_look_operations(&document, ClipId(10), &legacy)
            .expect("the batch builds")
        {
            kinewright_core::apply_batch(&mut document, std::slice::from_ref(&operation))
                .unwrap_or_else(|error| panic!("conversion rejected: {error}"));
        }
        let effects = &document.clip(ClipId(10)).expect("clip").effects;
        // The managed node replaces the legacy stage at its exact position.
        assert_eq!(effects.len(), 2);
        assert_eq!(effects[1].name, "creative_look");
        assert_eq!(effects[1].id, EffectId(3));
        let params = LutNodeParams::from_effect(&effects[1]);
        assert_eq!(params.lut_asset_id, LutAssetId(1));
        assert_eq!(params.mix_basis_points, 7_500);
        document
            .validate()
            .expect("the converted document is valid");
    }

    /// CC4 §3.2, §7: the stage headings' insert controls emit an
    /// `InsertEffect` at the first index the stage allows. Appending a
    /// correction onto a stack that already carries a creative look would be
    /// rejected with `ColorStageOrderViolation`, which is a dead end for the
    /// operator: the node they asked for simply never appears.
    #[test]
    fn adding_a_correction_under_an_existing_look_inserts_before_it() {
        let mut document = look_document(
            vec![
                colour_effect(1, "primary_correction"),
                lut_effect(2, "creative_look", 1, None),
            ],
            vec![BuiltinLook::Warm.to_lut_asset(LutAssetId(1))],
        );
        let clip = document.clip(ClipId(10)).expect("fixture clip").clone();
        let descriptor = kinewright_core::effect_descriptor("color_curves").expect("CC3 curves");

        let operation = add_effect_operation(&clip, &descriptor);

        let Operation::InsertEffect {
            clip: target,
            index,
            effect,
        } = operation.clone()
        else {
            panic!("a managed correction must insert, never append: {operation:?}");
        };
        assert_eq!(target, ClipId(10));
        assert_eq!(index, 1, "the curves node belongs before the creative look");
        assert_eq!(effect.name, "color_curves");
        assert_eq!(effect.id, EffectId(3));

        // The same operation the `+ Correction` menu emits, accepted by Core.
        kinewright_core::apply_batch(&mut document, std::slice::from_ref(&operation))
            .expect("Core accepts the computed correction index");
        let names: Vec<&str> = document
            .clip(ClipId(10))
            .expect("clip")
            .effects
            .iter()
            .map(|effect| effect.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["primary_correction", "color_curves", "creative_look"]
        );
        document.validate().expect("the stage order holds");
    }

    /// The same clip, appended instead of inserted: the rejection this fix
    /// removes, pinned so the insert cannot quietly regress to an append.
    #[test]
    fn appending_the_same_correction_is_rejected_by_core() {
        let mut document = look_document(
            vec![
                colour_effect(1, "primary_correction"),
                lut_effect(2, "creative_look", 1, None),
            ],
            vec![BuiltinLook::Warm.to_lut_asset(LutAssetId(1))],
        );
        let appended = Operation::AddEffect {
            clip: ClipId(10),
            effect: colour_effect(3, "color_curves"),
        };
        let error = kinewright_core::apply_batch(&mut document, std::slice::from_ref(&appended))
            .expect_err("appending a correction after a look violates the stage order");
        assert!(
            error.to_string().contains("non-decreasing stage rank"),
            "{error}"
        );
    }

    /// Every insertable effect the two menus offer, applied to a stack that
    /// already carries a look: none of them may be rejected.
    #[test]
    fn every_menu_insert_is_accepted_over_an_existing_creative_look() {
        for name in ["primary_correction", "color_wheels", "color_curves"] {
            let mut document = look_document(
                vec![lut_effect(1, "creative_look", 1, None)],
                vec![BuiltinLook::Warm.to_lut_asset(LutAssetId(1))],
            );
            let clip = document.clip(ClipId(10)).expect("clip").clone();
            let descriptor = kinewright_core::effect_descriptor(name).expect("descriptor");
            let operation = add_effect_operation(&clip, &descriptor);
            kinewright_core::apply_batch(&mut document, std::slice::from_ref(&operation))
                .unwrap_or_else(|error| panic!("{name} rejected: {error}"));
            assert_eq!(
                document.clip(ClipId(10)).expect("clip").effects[0].name,
                name
            );
            document.validate().expect("the stage order holds");
        }
    }

    /// CC4 §3.2, §9: a legacy `look_lut` authored *before* a managed
    /// correction cannot become a creative look where it stands, so the
    /// conversion moves it instead of being rejected with no explanation.
    #[test]
    fn converting_a_legacy_look_that_precedes_a_correction_reorders_it() {
        let mut legacy = colour_effect(3, "look_lut");
        legacy
            .parameters
            .insert("preset_token".to_owned(), ParamValue::Integer(2));
        legacy
            .parameters
            .insert("intensity_percent".to_owned(), ParamValue::Integer(75));
        let mut document = look_document(
            vec![legacy.clone(), colour_effect(1, "primary_correction")],
            Vec::new(),
        );
        assert!(!legacy_conversion_keeps_stage_order(
            &document.clip(ClipId(10)).expect("clip").effects,
            EffectId(3)
        ));

        let operations =
            convert_builtin_look_operations(&document, ClipId(10), &legacy).expect("batch builds");
        // `[AddLutAsset, RemoveEffect, InsertEffect]`, applied atomically.
        assert_eq!(operations.len(), 3);
        assert!(matches!(operations[0], Operation::AddLutAsset { .. }));
        assert!(matches!(
            operations[1],
            Operation::RemoveEffect {
                effect: EffectId(3),
                ..
            }
        ));
        let Operation::InsertEffect { index, effect, .. } = &operations[2] else {
            panic!("the reordering conversion must insert the managed look");
        };
        assert_eq!(*index, 1, "after the correction it used to precede");
        // The node keeps its identity across the move.
        assert_eq!(effect.id, EffectId(3));

        kinewright_core::apply_batch(&mut document, &operations)
            .expect("Core accepts the reordering conversion");
        let effects = &document.clip(ClipId(10)).expect("clip").effects;
        assert_eq!(effects.len(), 2);
        assert_eq!(effects[0].name, "primary_correction");
        assert_eq!(effects[1].name, "creative_look");
        assert_eq!(effects[1].id, EffectId(3));
        let params = LutNodeParams::from_effect(&effects[1]);
        assert_eq!(params.lut_asset_id, LutAssetId(1));
        assert_eq!(params.mix_basis_points, 7_500);
        document
            .validate()
            .expect("the converted document is valid");
    }

    /// The other position converts in place, as a single `ConvertLegacyLook`.
    #[test]
    fn converting_a_legacy_look_that_follows_a_correction_stays_in_place() {
        let mut legacy = colour_effect(3, "look_lut");
        legacy
            .parameters
            .insert("preset_token".to_owned(), ParamValue::Integer(2));
        let document = look_document(
            vec![colour_effect(1, "primary_correction"), legacy.clone()],
            Vec::new(),
        );
        assert!(legacy_conversion_keeps_stage_order(
            &document.clip(ClipId(10)).expect("clip").effects,
            EffectId(3)
        ));
        let operations =
            convert_builtin_look_operations(&document, ClipId(10), &legacy).expect("batch builds");
        assert_eq!(operations.len(), 2);
        assert!(matches!(
            operations[1],
            Operation::ConvertLegacyLook {
                effect: EffectId(3),
                ..
            }
        ));
    }

    /// CC4 §7: a hold that loses its card — the panel collapsed, the material
    /// tab switched, the clip deselected — must still be released, or the
    /// document keeps `bypass = 1` with no control left to clear it.
    #[test]
    fn a_hold_whose_card_stops_rendering_is_restored_by_the_frame_loop() {
        let press = ab_hold_step(ClipId(10), EffectId(2), AbHoldState::default(), true, 0);
        let record = AbHoldRecord {
            clip: ClipId(10),
            effect: EffectId(2),
            restore: press.state.restore,
        };

        // Rendering with the pointer still down is the ordinary held frame.
        assert!(!ab_hold_needs_recovery(true, true));
        // Either half failing strands the hold.
        assert!(ab_hold_needs_recovery(false, true));
        assert!(ab_hold_needs_recovery(true, false));
        assert!(ab_hold_needs_recovery(false, false));

        // The recovery writes exactly what the card's release would have.
        let release = ab_hold_step(ClipId(10), EffectId(2), press.state, false, 1);
        assert_eq!(
            ab_hold_restore_operation(record),
            release.operation.expect("a release restores bypass")
        );

        // The card mirrors the hold out to the app on the frame it opens, so
        // the record exists before the card can stop rendering.
        let mut pending = InspectorEdits::default();
        pending.record_ab_hold(record);
        assert_eq!(pending.ab_hold(), Some(record));

        // The mirror is bound to the project that owns it: a hold that
        // survives a project switch must not be restored into whatever
        // document happens to be focused.
        let mirrored = MirroredAbHold { session: 3, record };
        assert_ne!(mirrored, MirroredAbHold { session: 4, record });

        // The hold state is addressable without the card that wrote it.
        assert_eq!(
            ab_hold_id(ClipId(10), EffectId(2)),
            ab_hold_id(ClipId(10), EffectId(2))
        );
        assert_ne!(
            ab_hold_id(ClipId(10), EffectId(2)),
            ab_hold_id(ClipId(10), EffectId(3))
        );
    }

    /// CC4 §7: `LutNodeParams::from_effect` reads the *static* `bypass`, so a
    /// node whose `bypass` is keyframed would have the hold write a value its
    /// curve overrides — an undo entry for no visible change.
    #[test]
    fn the_ab_control_is_withheld_from_a_node_whose_bypass_is_keyframed() {
        let plain = lut_effect(2, "creative_look", 1, None);
        assert!(ab_hold_is_available(&plain));

        let mut keyframed = plain.clone();
        keyframed.keyframes.insert(
            COLOR_NODE_BYPASS_PARAMETER.to_owned(),
            AutomationCurve {
                keyframes: vec![Keyframe {
                    at: TimeCode::ZERO,
                    value: 1,
                    interpolation: KeyframeInterpolation::Hold,
                }],
            },
        );
        assert!(!ab_hold_is_available(&keyframed));
        // The badge the card shows comes from the same predicate the keyframe
        // rows use, so the two can never disagree.
        assert!(parameter_is_keyframed(
            &keyframed,
            COLOR_NODE_BYPASS_PARAMETER
        ));
        // A withheld control never presses, so no operation is produced.
        let step = ab_hold_step(ClipId(10), EffectId(2), AbHoldState::default(), false, 0);
        assert!(step.operation.is_none());
        assert!(!step.state.held);
    }

    #[test]
    fn a_non_legacy_effect_and_an_out_of_range_token_are_refused() {
        let document = look_document(Vec::new(), Vec::new());
        let not_legacy = colour_effect(3, "primary_correction");
        assert!(
            convert_builtin_look_operations(&document, ClipId(10), &not_legacy)
                .expect_err("only look_lut converts from a token")
                .contains("primary_correction")
        );
        let mut bad_token = colour_effect(3, "look_lut");
        bad_token
            .parameters
            .insert("preset_token".to_owned(), ParamValue::Integer(9));
        let error = convert_builtin_look_operations(&document, ClipId(10), &bad_token)
            .expect_err("token 9 names no built-in");
        assert!(error.contains('9') && error.contains("0..=4"), "{error}");
    }

    /// CC4 §9: a conversion the card cannot build is reported through the
    /// app's error log, not as a `colored_label` drawn for the single frame
    /// the pointer came up on.
    #[test]
    fn a_refused_legacy_conversion_leaves_the_frame_as_an_error_not_a_one_frame_label() {
        let document = look_document(Vec::new(), Vec::new());
        let mut bad_token = colour_effect(3, "look_lut");
        bad_token
            .parameters
            .insert("preset_token".to_owned(), ParamValue::Integer(9));

        let mut pending = InspectorEdits::default();
        request_legacy_conversion(&document, ClipId(10), &bad_token, &mut pending);
        assert!(
            pending.operations().is_empty(),
            "a refused conversion produces no operation"
        );
        assert!(pending.look_requests().is_empty());
        assert_eq!(pending.errors().len(), 1);
        assert!(
            pending.errors()[0].contains('9') && pending.errors()[0].contains("0..=4"),
            "{:?}",
            pending.errors()
        );

        // A legacy `cube_lut` with no stored path has no file to import, which
        // used to be a silently dead button.
        let mut pending = InspectorEdits::default();
        request_legacy_conversion(
            &document,
            ClipId(10),
            &colour_effect(4, "cube_lut"),
            &mut pending,
        );
        assert!(pending.look_requests().is_empty());
        assert_eq!(pending.errors().len(), 1);
        assert!(
            pending.errors()[0].contains("no path"),
            "{:?}",
            pending.errors()
        );

        // The path that works still routes to the import worker, with nothing
        // in the error log.
        let mut cube = colour_effect(4, "cube_lut");
        cube.parameters.insert(
            "path".to_owned(),
            ParamValue::Text("/looks/legacy.cube".to_owned()),
        );
        let mut pending = InspectorEdits::default();
        request_legacy_conversion(&document, ClipId(10), &cube, &mut pending);
        assert!(pending.errors().is_empty());
        assert_eq!(
            pending.look_requests(),
            [LookRequest::ConvertLegacyCube {
                clip: ClipId(10),
                effect: EffectId(4),
                path: std::path::PathBuf::from("/looks/legacy.cube"),
            }]
        );
    }

    #[test]
    fn a_look_request_is_collected_without_disturbing_a_live_gesture() {
        // The card records dialogs and workers separately from operations, so
        // a request in the same frame as a drag neither breaks the drag's
        // coalescing nor turns into an edit of its own.
        let mut pending = InspectorEdits::default();
        pending.push_live(
            effect_param_operation(ClipId(10), EffectId(2), LUT_MIX_PARAMETER, 3_000),
            look_mix_coalesce_key(ClipId(10), EffectId(2)),
        );
        pending.push_look(LookRequest::Locate {
            lut_asset: LutAssetId(1),
        });
        pending.push_look(LookRequest::Import {
            clip: ClipId(10),
            stage: ColorStage::Look,
        });
        assert_eq!(pending.coalesce_key(), Some("look:10:2:mix"));
        assert_eq!(pending.operations().len(), 1);
        assert_eq!(
            pending.look_requests(),
            [
                LookRequest::Locate {
                    lut_asset: LutAssetId(1)
                },
                LookRequest::Import {
                    clip: ClipId(10),
                    stage: ColorStage::Look
                },
            ]
        );
    }

    #[test]
    fn the_effect_menu_excludes_legacy_looks_and_offers_the_managed_kinds() {
        // CC4 §7: `look_lut` joins `color_grade` and `cube_lut` in the
        // exclusions because the managed kinds now cover it.
        assert!(!is_effect_insertable("look_lut"));
        assert!(!is_effect_insertable("cube_lut"));
        assert!(!is_effect_insertable("color_grade"));
        assert!(is_effect_insertable("technical_lut"));
        assert!(is_effect_insertable("creative_look"));
        assert!(is_effect_insertable("primary_correction"));
        assert!(is_effect_insertable("crop"));
    }

    #[test]
    fn the_generic_loop_hides_the_binding_the_encoding_and_the_pinned_mix() {
        let creative =
            kinewright_core::effect_descriptor("creative_look").expect("the descriptor exists");
        let technical =
            kinewright_core::effect_descriptor("technical_lut").expect("the descriptor exists");
        for descriptor in [&creative, &technical] {
            assert!(!should_render_effect_parameter(
                descriptor,
                LUT_ASSET_ID_PARAMETER
            ));
            assert!(!should_render_effect_parameter(
                descriptor,
                LUT_INPUT_ENCODING_PARAMETER
            ));
        }
        // The mix is pinned on a technical LUT (min = max = 10000), so a
        // slider over it would be inert; it stays visible on a creative look,
        // which is where the dedicated control writes it.
        assert!(!should_render_effect_parameter(
            &technical,
            LUT_MIX_PARAMETER
        ));
        assert!(should_render_effect_parameter(&creative, LUT_MIX_PARAMETER));
    }

    #[test]
    fn a_retarget_is_one_set_effect_param_on_the_binding() {
        assert_eq!(
            lut_asset_param_operation(ClipId(10), EffectId(2), LutAssetId(5)),
            Operation::SetEffectParam {
                clip: ClipId(10),
                effect: EffectId(2),
                name: LUT_ASSET_ID_PARAMETER.to_owned(),
                value: ParamValue::Integer(5),
            }
        );
    }

    #[test]
    fn the_availability_chip_names_every_state() {
        assert_eq!(
            availability_chip(Some(LutAvailabilityKind::Verified)).0,
            "verified"
        );
        assert_eq!(
            availability_chip(Some(LutAvailabilityKind::Missing)).0,
            "missing"
        );
        assert_eq!(
            availability_chip(Some(LutAvailabilityKind::Changed)).0,
            "changed"
        );
        assert_eq!(
            availability_chip(Some(LutAvailabilityKind::Unreadable)).0,
            "unreadable"
        );
        assert_eq!(availability_chip(None).0, "unchecked");
    }

    #[test]
    fn the_encoding_readout_uses_the_contract_names() {
        assert_eq!(input_encoding_label(0), "display709");
        assert_eq!(input_encoding_label(1), "linear");
        assert_eq!(input_encoding_label(2), "grade709");
    }

    // -----------------------------------------------------------------------
    // CC5 §6 matte section
    // -----------------------------------------------------------------------

    /// The four matte-capable kinds (CC5 §2.1). `technical_lut` is not one.
    const MATTE_CAPABLE_KINDS: [&str; 4] = [
        "primary_correction",
        "color_wheels",
        "color_curves",
        "creative_look",
    ];

    /// A one-clip document carrying exactly one matte-capable node with
    /// effect id 1 and nothing stored on it.
    ///
    /// `creative_look` is the exception: it must be bound to a registered LUT
    /// asset or `validate_document` rejects it (CC4 §2), so it gets one — the
    /// binding is the only stored parameter, and it is not a matte parameter.
    fn matte_document(name: &str) -> Document {
        let effect = if name == "creative_look" {
            lut_effect(1, name, 4, None)
        } else {
            colour_effect(1, name)
        };
        let document = look_document(
            vec![effect],
            vec![BuiltinLook::Warm.to_lut_asset(LutAssetId(4))],
        );
        document
            .validate()
            .expect("a bare matte-capable node is legal");
        document
    }

    fn stored_matte(document: &Document) -> MatteParams {
        MatteParams::from_effect(&document.tracks[0].clips[0].effects[0])
    }

    fn apply(document: &mut Document, operations: &[Operation], what: &str) {
        kinewright_core::apply_batch(document, operations)
            .unwrap_or_else(|error| panic!("core rejected {what}: {error}"));
        document
            .validate()
            .unwrap_or_else(|error| panic!("{what} left an invalid document: {error}"));
    }

    /// CC5 §2.2: every matte control is bounded by *its own* descriptor entry,
    /// so one control's range can never be applied to another's.
    ///
    /// The expected bounds are hand-transcribed from the §2.2 tables rather
    /// than read back from the descriptor, and they deliberately disagree with
    /// each other: a single literal range applied across the section — the
    /// shape the transcribed bounds used to have — cannot satisfy all of them.
    #[test]
    fn every_matte_control_is_bounded_by_its_own_descriptor_entry() {
        // (parameter, min, max), CC5 §2.2.
        let expected: [(&str, i64, i64); 8] = [
            ("matte_window_count", 0, 4),
            ("matte_mix_basis_points", 0, 10_000),
            ("matte_hue_center_centidegrees", 0, 35_999),
            ("matte_hue_width_centidegrees", 0, 18_000),
            ("matte_saturation_low_basis_points", 0, 10_000),
            ("matte_window0_center_x_basis_points", -10_000, 20_000),
            ("matte_window0_half_width_basis_points", 1, 10_000),
            ("matte_window3_rotation_centidegrees", -18_000, 18_000),
        ];
        for kind in MATTE_CAPABLE_KINDS {
            let effect = colour_effect(1, kind);
            for (parameter, min, max) in expected {
                assert_eq!(
                    matte_parameter_range(&effect, parameter, 0),
                    min..=max,
                    "{kind}.{parameter} must be bounded by its descriptor"
                );
            }
        }

        // A name the node does not register yields an inert control rather than
        // an invented range that would offer a value core rejects. A
        // matte-*capable* node asked for an unregistered name is a retargeted
        // control, and `matte_parameter_range` now trips a `debug_assert` for
        // that case, so it is pinned by its own `#[should_panic]` test below
        // rather than here.
        assert_eq!(
            matte_parameter_range(&colour_effect(1, "technical_lut"), "matte_invert", 7),
            7..=7,
            "a node with no matte registers none of its names"
        );
    }

    /// A control pointed at a name its own node does not register would draw as
    /// a `DragValue` that cannot move — silently, in release. The test lane
    /// fails loudly instead, so a rename or a typo is caught here.
    #[test]
    #[should_panic(expected = "unregistered matte control")]
    #[cfg(debug_assertions)]
    fn a_matte_control_on_an_unregistered_name_fails_loudly_in_the_test_lane() {
        let _ = matte_parameter_range(
            &colour_effect(1, "color_wheels"),
            "matte_window0_feather_bp",
            42,
        );
    }

    /// The slider's range and the value it shows come from the same bound. A
    /// hard-coded `0..=100` beside a `mix_percent(bp, MAX)` conversion reads as
    /// correct only while both contracts happen to stop at 10000 bp: raise one
    /// and the widget would clamp away the value the card just resolved.
    #[test]
    fn a_mix_slider_offers_exactly_the_percent_its_own_bound_allows() {
        assert_eq!(mix_percent_range(LUT_MIX_BASIS_POINTS_MAX), 0..=100);
        assert_eq!(mix_percent_range(MATTE_MIX_BASIS_POINTS_MAX), 0..=100);
        // The range reaches whatever the conversion can produce, at any bound.
        for max in [0_i64, 5_000, 10_000, 12_345, 20_000] {
            assert_eq!(*mix_percent_range(max).end(), mix_percent(max, max));
        }

        // And the widget itself: at a 20000 bp max, 150 % is a legal stored
        // value and the slider has to keep it — while still clamping at its own
        // bound.
        let ctx = egui::Context::default();
        let mut inside = 150;
        let mut beyond = 250;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.add(mix_slider(&mut inside, 20_000, "Mix"));
            ui.add(mix_slider(&mut beyond, 20_000, "Mix"));
        });
        assert_eq!(
            inside, 150,
            "the range comes from the parameter's own max, not from 100 %"
        );
        assert_eq!(beyond, 200, "and it still clamps at that max");
    }

    /// CC5 §2.2 and CC4 §7 are separate mix contracts. `mix_percent` takes the
    /// bound of the control it is showing, so the two can never clamp each
    /// other, and the matte slider is handed the matte descriptor's bound.
    #[test]
    fn the_matte_mix_and_the_look_mix_are_bounded_separately() {
        // The bound is a parameter, not a constant baked into the conversion.
        assert_eq!(mix_percent(12_000, 10_000), 100, "clamped at its own max");
        assert_eq!(mix_percent(12_000, 20_000), 120, "not at somebody else's");
        assert_eq!(mix_percent(-500, 10_000), 0);

        // And the constant the matte card passes is the matte descriptor's own
        // bound, on every kind that carries a matte.
        for kind in MATTE_CAPABLE_KINDS {
            let descriptor = kinewright_core::effect_descriptor(kind).expect("descriptor");
            assert_eq!(
                descriptor
                    .parameter(MATTE_MIX_PARAMETER)
                    .expect("the matte mix control")
                    .max,
                MATTE_MIX_BASIS_POINTS_MAX,
                "{kind}: the matte slider's bound must be the matte contract's"
            );
        }
    }

    /// CC5 §6: the 47 matte integers never reach the generic slider loop, on
    /// any of the four kinds that carry them.
    #[test]
    fn the_generic_loop_hides_every_matte_parameter() {
        for name in MATTE_CAPABLE_KINDS {
            let descriptor = kinewright_core::effect_descriptor(name).expect("CC5 descriptor");
            let mut hidden = 0;
            for parameter in descriptor.parameters {
                if is_matte_parameter(parameter.name) {
                    assert!(
                        !should_render_effect_parameter(&descriptor, parameter.name),
                        "{name}.{} must be owned by the matte section",
                        parameter.name
                    );
                    hidden += 1;
                }
            }
            assert_eq!(
                hidden,
                kinewright_core::MATTE_PARAMETER_COUNT,
                "{name} must carry all 47 matte parameters"
            );
        }
        // The node's own controls are untouched by the matte rule.
        let primary = kinewright_core::effect_descriptor("primary_correction").expect("descriptor");
        assert!(should_render_effect_parameter(
            &primary,
            "exposure_milli_stops"
        ));
        // `technical_lut` carries no matte parameter to hide.
        let technical = kinewright_core::effect_descriptor("technical_lut").expect("descriptor");
        assert!(
            !technical
                .parameters
                .iter()
                .any(|parameter| is_matte_parameter(parameter.name)),
            "a technical input transform carries no matte (CC5 §2.1)"
        );
    }

    /// CC5 §5: resetting a matte-capable node resets its matte too — every
    /// parameter it stores off-neutral returned to its neutral, plus a keyframe
    /// clear for each that carries automation — and core accepts the batch.
    #[test]
    fn a_matte_capable_reset_neutralizes_the_whole_matte_and_clears_it() {
        for name in MATTE_CAPABLE_KINDS {
            let descriptor = kinewright_core::effect_descriptor(name).expect("descriptor");
            let mut document = matte_document(name);
            let mut effect = document.tracks[0].clips[0].effects[0].clone();
            effect.keyframes.insert(
                "matte_window0_center_x_basis_points".to_owned(),
                AutomationCurve {
                    keyframes: vec![Keyframe {
                        at: TimeCode::ZERO,
                        value: 8_000,
                        interpolation: KeyframeInterpolation::Linear,
                    }],
                },
            );
            effect
                .parameters
                .insert("matte_enabled".to_owned(), ParamValue::Integer(1));
            effect
                .parameters
                .insert("matte_window_count".to_owned(), ParamValue::Integer(2));
            effect.parameters.insert(
                "matte_mix_basis_points".to_owned(),
                ParamValue::Integer(3_000),
            );
            document.tracks[0].clips[0].effects[0] = effect.clone();
            document
                .validate()
                .expect("a stored, keyframed matte is a legal project");
            let reset = color_node_reset_operations(ClipId(10), &effect, &descriptor);

            // Every matte parameter the node actually *stores* off-neutral is
            // returned to its neutral; the ones it never stored already resolve
            // to neutral, so writing them would only add entries (CC5 §2.2).
            for stored in [
                "matte_enabled",
                "matte_window_count",
                "matte_mix_basis_points",
            ] {
                let neutral = descriptor
                    .parameter(stored)
                    .expect("a matte control on a matte-capable node")
                    .neutral;
                assert!(
                    reset.contains(&Operation::SetEffectParam {
                        clip: ClipId(10),
                        effect: effect.id,
                        name: stored.to_owned(),
                        value: ParamValue::Integer(neutral),
                    }),
                    "{name}.{stored} must be reset to its neutral"
                );
            }
            for untouched in descriptor.parameters {
                if !is_matte_parameter(untouched.name)
                    || effect.parameters.contains_key(untouched.name)
                {
                    continue;
                }
                assert!(
                    !reset.iter().any(|operation| matches!(
                        operation,
                        Operation::SetEffectParam { name, .. } if name == untouched.name
                    )),
                    "{name}.{} already resolves to its neutral: resetting must not \
                     store it",
                    untouched.name
                );
            }
            assert!(
                reset.contains(&Operation::ClearEffectKeyframes {
                    clip: ClipId(10),
                    effect: effect.id,
                    name: "matte_window0_center_x_basis_points".to_owned(),
                }),
                "{name} must clear the matte's automation with it, whatever its \
                 static value resolves to"
            );

            // And the reset really is a reset: applying it leaves an all-neutral
            // matte, which is the property the removed "47 sets" count stood in
            // for.
            apply(&mut document, &reset, "the matte reset");
            let matte = stored_matte(&document);
            assert!(!matte.is_enabled());
            assert_eq!(matte.window_count, 0);
            assert_eq!(matte.mix_bp, kinewright_core::MATTE_MIX_BASIS_POINTS_MAX);
        }
    }

    /// CC5 §2.2: an omitted matte parameter already resolves to its neutral, so
    /// resetting a node that never carried a matte must not write 47 entries
    /// into a CC4-era project's JSON. The bound the reviewer cared about is the
    /// stored size, so that is what this measures.
    #[test]
    fn resetting_a_matteless_node_stores_no_matte_entries() {
        for name in MATTE_CAPABLE_KINDS {
            let descriptor = kinewright_core::effect_descriptor(name).expect("descriptor");
            let mut document = matte_document(name);
            let effect = document.tracks[0].clips[0].effects[0].clone();
            assert!(
                effect.parameters.keys().all(|key| !is_matte_parameter(key)),
                "the fixture node starts with no matte parameter stored"
            );

            let reset = color_node_reset_operations(ClipId(10), &effect, &descriptor);
            assert!(
                !reset.iter().any(|operation| matches!(
                    operation,
                    Operation::SetEffectParam { name, .. } if is_matte_parameter(name)
                )),
                "{name}: a node with no stored matte needs no matte writes"
            );
            apply(&mut document, &reset, "resetting a matteless node");
            let stored = &document.tracks[0].clips[0].effects[0].parameters;
            assert!(
                stored.keys().all(|key| !is_matte_parameter(key)),
                "{name}: the reset grew the stored node by {:?}",
                stored
                    .keys()
                    .filter(|key| is_matte_parameter(key))
                    .collect::<Vec<_>>()
            );
        }
    }

    /// CC5 §6 and §5.1: a slot rewrite moves the automation with the values.
    /// Without that, the shifted window renders the removed window's curve —
    /// and a keyframed parameter resolves from its curve, so the shifted
    /// statics would never be seen at all.
    #[test]
    fn removing_a_window_moves_the_keyframe_tracks_down_with_the_values() {
        let mut document = matte_document("color_wheels");

        // Two windows, each with its own keyframed centre.
        for step in 0..2 {
            let effect = document.tracks[0].clips[0].effects[0].clone();
            let add = matte_add_window_operations(ClipId(10), &effect);
            apply(&mut document, &add, "Add window");
            let _ = step;
        }
        assert_eq!(stored_matte(&document).window_count, 2);

        let curve = |value: i64| AutomationCurve {
            keyframes: vec![Keyframe {
                at: TimeCode::ZERO,
                value,
                interpolation: KeyframeInterpolation::Linear,
            }],
        };
        apply(
            &mut document,
            &[
                Operation::SetEffectKeyframes {
                    clip: ClipId(10),
                    effect: EffectId(1),
                    name: "matte_window0_center_x_basis_points".to_owned(),
                    curve: curve(1_000),
                },
                Operation::SetEffectKeyframes {
                    clip: ClipId(10),
                    effect: EffectId(1),
                    name: "matte_window1_center_x_basis_points".to_owned(),
                    curve: curve(9_000),
                },
            ],
            "two keyframed centres",
        );

        let effect = document.tracks[0].clips[0].effects[0].clone();
        let removal = matte_remove_window_operations(ClipId(10), &effect, 0);
        apply(&mut document, &removal, "Remove window 0");

        let effect = &document.tracks[0].clips[0].effects[0];
        assert_eq!(
            effect
                .keyframes
                .get("matte_window0_center_x_basis_points")
                .map(|curve| curve.keyframes[0].value),
            Some(9_000),
            "window 1's curve moved down into window 0"
        );
        assert!(
            !effect
                .keyframes
                .contains_key("matte_window1_center_x_basis_points"),
            "the vacated slot keeps no automation"
        );
        assert_eq!(
            effect.integer_parameter_at("matte_window0_center_x_basis_points", TimeCode::ZERO),
            Some(9_000),
            "and the window renders the curve it was given, not the removed one's"
        );
    }

    /// CC5 §5.1: a window's `shape_token` and `invert` are `Hold`-keyframable
    /// too, and they travel with the values on a removal exactly as the
    /// geometry does. They are the easiest pair to leave behind — the shift
    /// writes eight statics and only the six continuous controls have obvious
    /// curves — and a stranded `Hold` token would resolve the *removed*
    /// window's shape under the shifted window's card.
    #[test]
    fn removing_a_window_moves_the_hold_only_token_curves_too() {
        let mut document = matte_document("color_wheels");
        for _ in 0..2 {
            let effect = document.tracks[0].clips[0].effects[0].clone();
            let add = matte_add_window_operations(ClipId(10), &effect);
            apply(&mut document, &add, "Add window");
        }
        assert_eq!(stored_matte(&document).window_count, 2);

        let hold = |value: i64| AutomationCurve {
            keyframes: vec![Keyframe {
                at: TimeCode::ZERO,
                value,
                interpolation: KeyframeInterpolation::Hold,
            }],
        };
        let curve = |name: &str, value: i64| Operation::SetEffectKeyframes {
            clip: ClipId(10),
            effect: EffectId(1),
            name: name.to_owned(),
            curve: hold(value),
        };
        apply(
            &mut document,
            &[
                // W0 is a held rect, uninverted; W1 a held ellipse, inverted.
                curve("matte_window0_shape_token", 1),
                curve("matte_window0_invert", 0),
                curve("matte_window1_shape_token", 2),
                curve("matte_window1_invert", 1),
            ],
            "held tokens on both windows",
        );

        let effect = document.tracks[0].clips[0].effects[0].clone();
        apply(
            &mut document,
            &matte_remove_window_operations(ClipId(10), &effect, 0),
            "Remove window 0",
        );

        let effect = &document.tracks[0].clips[0].effects[0];
        for (name, expected) in [
            ("matte_window0_shape_token", 2),
            ("matte_window0_invert", 1),
        ] {
            let moved = effect.keyframes.get(name).expect("the curve moved down");
            assert_eq!(
                moved.keyframes[0].value, expected,
                "{name} must carry W1's held value"
            );
            assert_eq!(
                moved.keyframes[0].interpolation,
                KeyframeInterpolation::Hold,
                "{name} stays Hold-only, which is what core accepts"
            );
        }
        for name in ["matte_window1_shape_token", "matte_window1_invert"] {
            assert!(
                !effect.keyframes.contains_key(name),
                "{name}: the vacated slot keeps no automation"
            );
        }

        // And it is what renders: the shifted window resolves as the inverted
        // ellipse it was, not as W0's rect.
        let resolved = MatteParams::from_effect(&effect.evaluated_at(TimeCode::ZERO)).windows[0];
        assert!(
            resolved.is_ellipse() && resolved.is_inverted(),
            "the shifted window resolves W1's held tokens: {resolved:?}"
        );
    }

    /// A recycled slot must not resurrect the removed window's motion under a
    /// card that reads as neutral (CC5 §5.1).
    #[test]
    fn an_added_window_carries_no_inherited_automation() {
        let mut document = matte_document("color_wheels");
        let effect = document.tracks[0].clips[0].effects[0].clone();
        apply(
            &mut document,
            &matte_add_window_operations(ClipId(10), &effect),
            "Add window",
        );
        apply(
            &mut document,
            &[Operation::SetEffectKeyframes {
                clip: ClipId(10),
                effect: EffectId(1),
                name: "matte_window0_feather_basis_points".to_owned(),
                curve: AutomationCurve {
                    keyframes: vec![Keyframe {
                        at: TimeCode::ZERO,
                        value: 4_000,
                        interpolation: KeyframeInterpolation::Linear,
                    }],
                },
            }],
            "a keyframed feather",
        );

        // Remove it, then add a window back into the slot it vacated.
        let effect = document.tracks[0].clips[0].effects[0].clone();
        apply(
            &mut document,
            &matte_remove_window_operations(ClipId(10), &effect, 0),
            "Remove window 0",
        );
        let effect = document.tracks[0].clips[0].effects[0].clone();
        let add = matte_add_window_operations(ClipId(10), &effect);
        apply(&mut document, &add, "Add window into the recycled slot");

        let effect = &document.tracks[0].clips[0].effects[0];
        assert!(
            !effect
                .keyframes
                .contains_key("matte_window0_feather_basis_points"),
            "a fresh window is at its neutral, not on the last window's curve"
        );
        assert_eq!(
            MatteParams::from_effect(&effect.evaluated_at(TimeCode::ZERO)).windows[0],
            MatteWindowParams::NEUTRAL,
            "and it resolves to the descriptor neutral at every frame"
        );
    }

    /// An unkeyframed matte's Add and Remove batches are untouched by the
    /// automation handling: no `SetEffectKeyframes`, no `ClearEffectKeyframes`.
    #[test]
    fn window_batches_stay_keyframe_free_when_the_matte_is() {
        let mut document = matte_document("color_curves");
        let effect = document.tracks[0].clips[0].effects[0].clone();
        let add = matte_add_window_operations(ClipId(10), &effect);
        assert!(
            add.iter()
                .all(|operation| matches!(operation, Operation::SetEffectParam { .. }))
        );
        apply(&mut document, &add, "Add window");
        let effect = document.tracks[0].clips[0].effects[0].clone();
        assert!(
            matte_remove_window_operations(ClipId(10), &effect, 0)
                .iter()
                .all(|operation| matches!(operation, Operation::SetEffectParam { .. })),
        );
    }

    /// Adding and removing windows keeps every intermediate document valid, and
    /// a removal shifts the later windows down rather than leaving a hole
    /// (CC5 §6).
    #[test]
    fn window_add_and_remove_batches_are_accepted_in_order_by_core() {
        let mut document = matte_document("color_wheels");
        let effect = document.tracks[0].clips[0].effects[0].clone();
        let first = matte_add_window_operations(ClipId(10), &effect);
        assert!(
            first.contains(&effect_param_operation(
                ClipId(10),
                EffectId(1),
                "matte_enabled",
                1
            )),
            "the first window enables the matte"
        );
        apply(&mut document, &first, "the first Add window");
        assert_eq!(stored_matte(&document).window_count, 1);
        assert!(stored_matte(&document).is_enabled());

        let effect = document.tracks[0].clips[0].effects[0].clone();
        let second = matte_add_window_operations(ClipId(10), &effect);
        assert!(
            !second.contains(&effect_param_operation(
                ClipId(10),
                EffectId(1),
                "matte_enabled",
                1
            )),
            "an already-enabled matte is not re-enabled"
        );
        apply(&mut document, &second, "the second Add window");
        assert_eq!(stored_matte(&document).window_count, 2);

        // Give the second window an identity so the shift is observable.
        apply(
            &mut document,
            &[
                effect_param_operation(
                    ClipId(10),
                    EffectId(1),
                    "matte_window1_center_x_basis_points",
                    8_000,
                ),
                effect_param_operation(ClipId(10), EffectId(1), "matte_window1_shape_token", 2),
            ],
            "the second window's own values",
        );

        let effect = document.tracks[0].clips[0].effects[0].clone();
        let removal = matte_remove_window_operations(ClipId(10), &effect, 0);
        assert_eq!(
            removal.last(),
            Some(&effect_param_operation(
                ClipId(10),
                EffectId(1),
                "matte_window_count",
                1
            )),
            "the count is decremented last so no document names an unwritten window"
        );
        apply(&mut document, &removal, "Remove window 0");
        let matte = stored_matte(&document);
        assert_eq!(matte.window_count, 1);
        assert_eq!(
            (matte.windows[0].center_x_bp, matte.windows[0].shape_token),
            (8_000, 2),
            "the second window shifts down to index 0"
        );

        // Removing the last window empties the list; removing from an empty
        // list is a no-op rather than an operation core would reject.
        let effect = document.tracks[0].clips[0].effects[0].clone();
        let removal = matte_remove_window_operations(ClipId(10), &effect, 0);
        apply(&mut document, &removal, "Remove the last window");
        assert_eq!(stored_matte(&document).window_count, 0);
        let effect = document.tracks[0].clips[0].effects[0].clone();
        assert!(matte_remove_window_operations(ClipId(10), &effect, 0).is_empty());

        // A fourth window is the limit; a fifth Add is refused by the helper,
        // not by core.
        for index in 0..MATTE_WINDOW_LIMIT {
            let effect = document.tracks[0].clips[0].effects[0].clone();
            let batch = matte_add_window_operations(ClipId(10), &effect);
            assert!(!batch.is_empty(), "window {index} must be addable");
            apply(&mut document, &batch, "an Add window up to the limit");
        }
        let effect = document.tracks[0].clips[0].effects[0].clone();
        assert!(
            matte_add_window_operations(ClipId(10), &effect).is_empty(),
            "a fifth window is not offered"
        );
        assert_eq!(stored_matte(&document).window_count, MATTE_WINDOW_LIMIT);
    }

    /// An added window resets a slot an earlier removal left behind, so a new
    /// window is never the ghost of a deleted one.
    #[test]
    fn an_added_window_never_inherits_a_removed_window_s_values() {
        let mut document = matte_document("primary_correction");
        let effect = document.tracks[0].clips[0].effects[0].clone();
        apply(
            &mut document,
            &matte_add_window_operations(ClipId(10), &effect),
            "Add window",
        );
        apply(
            &mut document,
            &[
                effect_param_operation(
                    ClipId(10),
                    EffectId(1),
                    "matte_window0_center_x_basis_points",
                    -4_000,
                ),
                effect_param_operation(
                    ClipId(10),
                    EffectId(1),
                    "matte_window0_feather_basis_points",
                    6_000,
                ),
            ],
            "a hand-placed window",
        );
        let effect = document.tracks[0].clips[0].effects[0].clone();
        apply(
            &mut document,
            &matte_remove_window_operations(ClipId(10), &effect, 0),
            "Remove window 0",
        );
        let effect = document.tracks[0].clips[0].effects[0].clone();
        apply(
            &mut document,
            &matte_add_window_operations(ClipId(10), &effect),
            "Add window again",
        );
        let window = stored_matte(&document).windows[0];
        assert_eq!(
            window,
            MatteWindowParams::NEUTRAL,
            "the re-added window is the descriptor neutral, not the removed one"
        );
    }

    /// The qualifier writer covers the whole leg and core accepts it.
    #[test]
    fn the_qualifier_writer_covers_every_qualifier_parameter() {
        for name in MATTE_QUALIFIER_PARAMETERS {
            assert!(
                is_matte_parameter(name),
                "{name} must be a real CC5 matte parameter"
            );
        }
        let mut document = matte_document("color_curves");
        let tuned = MatteQualifierParams {
            enabled: 1,
            hue_center_cd: 3_000,
            hue_width_cd: 1_500,
            hue_softness_cd: 1_000,
            sat_low_bp: 2_000,
            sat_high_bp: 9_000,
            sat_softness_bp: 1_000,
            luma_low_bp: 1_000,
            luma_high_bp: 8_000,
            luma_softness_bp: 500,
        };
        let operations = matte_qualifier_operations(ClipId(10), EffectId(1), &tuned);
        assert_eq!(operations.len(), MATTE_QUALIFIER_PARAMETERS.len());
        apply(&mut document, &operations, "a tuned qualifier");
        assert_eq!(stored_matte(&document).qualifier, tuned);

        let effect = document.tracks[0].clips[0].effects[0].clone();
        let reset =
            matte_qualifier_operations(ClipId(10), effect.id, &MatteQualifierParams::NEUTRAL);
        apply(&mut document, &reset, "Reset qualifier");
        assert_eq!(
            stored_matte(&document).qualifier,
            MatteQualifierParams::NEUTRAL
        );
    }

    /// CC5 §6's coalesce keys, verbatim, and the parameters each gesture owns.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn overlay_gestures_use_the_contract_keys_and_write_only_their_own_parameters() {
        assert_eq!(
            matte_window_move_coalesce_key(ClipId(3), EffectId(8), 2),
            "matte_window_move:3:8:2"
        );
        assert_eq!(
            matte_window_resize_coalesce_key(ClipId(3), EffectId(8), 2),
            "matte_window_resize:3:8:2"
        );
        assert_eq!(
            matte_window_rotate_coalesce_key(ClipId(3), EffectId(8), 2),
            "matte_window_rotate:3:8:2"
        );
        assert_eq!(
            matte_mix_coalesce_key(ClipId(3), EffectId(8)),
            "matte_mix:3:8"
        );
        assert_eq!(
            matte_gesture_coalesce_key(MatteHit::Move, ClipId(3), EffectId(8), 0),
            "matte_window_move:3:8:0"
        );
        assert_eq!(
            matte_gesture_coalesce_key(
                MatteHit::Resize(crate::matte_overlay_ui::MatteHandle::TopLeft),
                ClipId(3),
                EffectId(8),
                1
            ),
            "matte_window_resize:3:8:1"
        );
        assert_eq!(
            matte_gesture_coalesce_key(MatteHit::Rotate, ClipId(3), EffectId(8), 3),
            "matte_window_rotate:3:8:3"
        );
        assert_ne!(
            matte_window_move_coalesce_key(ClipId(3), EffectId(8), 0),
            matte_window_move_coalesce_key(ClipId(3), EffectId(8), 1),
            "two windows of one node must never merge into one undo entry"
        );

        let window = MatteWindowParams {
            center_x_bp: 6_000,
            center_y_bp: 4_000,
            half_width_bp: 1_000,
            half_height_bp: 900,
            rotation_cd: 1_234,
            ..MatteWindowParams::NEUTRAL
        };
        let moved =
            matte_window_drag_operations(ClipId(3), EffectId(8), 0, MatteHit::Move, &window);
        assert_eq!(
            moved,
            vec![
                effect_param_operation(
                    ClipId(3),
                    EffectId(8),
                    "matte_window0_center_x_basis_points",
                    6_000
                ),
                effect_param_operation(
                    ClipId(3),
                    EffectId(8),
                    "matte_window0_center_y_basis_points",
                    4_000
                ),
            ],
            "a move writes exactly two parameters (CC5 §6)"
        );
        assert_eq!(
            matte_window_drag_operations(
                ClipId(3),
                EffectId(8),
                0,
                MatteHit::Resize(crate::matte_overlay_ui::MatteHandle::Right),
                &window
            ),
            vec![effect_param_operation(
                ClipId(3),
                EffectId(8),
                "matte_window0_half_width_basis_points",
                1_000
            )],
            "an edge handle writes one axis"
        );
        assert_eq!(
            matte_window_drag_operations(
                ClipId(3),
                EffectId(8),
                0,
                MatteHit::Resize(crate::matte_overlay_ui::MatteHandle::BottomRight),
                &window
            )
            .len(),
            2,
            "a corner handle writes both axes"
        );
        assert_eq!(
            matte_window_drag_operations(ClipId(3), EffectId(8), 0, MatteHit::Rotate, &window),
            vec![effect_param_operation(
                ClipId(3),
                EffectId(8),
                "matte_window0_rotation_centidegrees",
                1_234
            )]
        );
    }

    /// One gesture is one undo entry: the release frame stays inside it, and a
    /// discrete edit in the same frame drops the key.
    #[test]
    fn a_matte_gesture_is_one_undo_entry_including_its_release_frame() {
        let window = MatteWindowParams::NEUTRAL;
        let key = matte_window_move_coalesce_key(ClipId(3), EffectId(8), 0);
        let mut edits = InspectorEdits::default();
        edits.begin_gesture();
        for centre in [5_100, 5_200, 5_300] {
            let moved = MatteWindowParams {
                center_x_bp: centre,
                ..window
            };
            edits.extend_live(
                matte_window_drag_operations(ClipId(3), EffectId(8), 0, MatteHit::Move, &moved),
                key.clone(),
            );
        }
        assert_eq!(edits.operations().len(), 6, "two parameters per frame");
        assert_eq!(
            edits.coalesce_key(),
            Some(key.as_str()),
            "the release frame must not open a second undo entry"
        );
        assert!(edits.gesture_started);

        // A mix drag rides its own key, so a matte move and a mix drag never
        // merge.
        let mut mix = InspectorEdits::default();
        mix.push_live(
            effect_param_operation(ClipId(3), EffectId(8), "matte_mix_basis_points", 6_000),
            matte_mix_coalesce_key(ClipId(3), EffectId(8)),
        );
        assert_eq!(mix.coalesce_key(), Some("matte_mix:3:8"));

        // A discrete edit in the same frame is not part of the gesture.
        mix.push(effect_param_operation(
            ClipId(3),
            EffectId(8),
            "matte_invert",
            1,
        ));
        assert_eq!(mix.coalesce_key(), None);
    }

    /// CC5 §6: the tracking control exists so the workflow is discoverable, and
    /// is disabled so it cannot pretend to work.
    #[test]
    fn the_track_window_control_is_present_but_disabled() {
        assert!(
            !matte_track_button_enabled(),
            "CC5 has no agent-tool call path from the app"
        );
        assert_eq!(MATTE_TRACK_BUTTON_LABEL, "Track window…");
        assert!(
            MATTE_TRACK_BUTTON_TOOLTIP.contains("agent-driven")
                && MATTE_TRACK_BUTTON_TOOLTIP.contains("track_matte_window")
                && MATTE_TRACK_BUTTON_TOOLTIP.contains("committed"),
            "the tooltip must say tracking is agent-driven and that committed keyframes appear here"
        );
    }

    /// The window writer and the parameter table agree on order, so a shift
    /// cannot silently write a centre into a half-extent.
    #[test]
    fn the_window_writer_matches_the_descriptor_order() {
        let window = MatteWindowParams {
            shape_token: 2,
            center_x_bp: 1,
            center_y_bp: 2,
            half_width_bp: 3,
            half_height_bp: 4,
            rotation_cd: 5,
            feather_bp: 6,
            invert: 1,
        };
        let names = kinewright_core::matte_window_parameter_names(1).expect("window 1");
        // Every one of the eight differs from the neutral the slot holds, so
        // the whole batch is written and the order is observable.
        let operations = matte_window_edit_operations(
            ClipId(10),
            EffectId(1),
            1,
            &window,
            &MatteWindowParams::NEUTRAL,
        );
        assert_eq!(operations.len(), names.len());
        for (operation, (name, value)) in operations
            .iter()
            .zip(names.iter().zip(matte_window_values(&window)))
        {
            assert_eq!(
                operation,
                &effect_param_operation(ClipId(10), EffectId(1), name, value)
            );
        }
        for (name, suffix) in names.iter().zip([
            "_shape_token",
            "_center_x_basis_points",
            "_center_y_basis_points",
            "_half_width_basis_points",
            "_half_height_basis_points",
            "_rotation_centidegrees",
            "_feather_basis_points",
            "_invert",
        ]) {
            assert!(name.ends_with(suffix), "{name} must end with {suffix}");
        }
        assert!(
            matte_window_edit_operations(
                ClipId(10),
                EffectId(1),
                MATTE_WINDOW_LIMIT,
                &window,
                &MatteWindowParams::NEUTRAL,
            )
            .is_empty()
        );

        // A parameter already at the value being written is not restated: a
        // slot rewrite that agrees with what is stored writes nothing, and one
        // that differs in a single control writes exactly that control.
        assert!(
            matte_window_edit_operations(ClipId(10), EffectId(1), 1, &window, &window).is_empty(),
            "an unchanged slot is not rewritten"
        );
        let moved = MatteWindowParams {
            center_x_bp: window.center_x_bp + 100,
            ..window
        };
        assert_eq!(
            matte_window_edit_operations(ClipId(10), EffectId(1), 1, &moved, &window),
            [effect_param_operation(
                ClipId(10),
                EffectId(1),
                names[1],
                moved.center_x_bp
            )],
            "only the control that moved is written"
        );
    }

    /// CC5 §6: the section is collapsed by default, so a CC4 project's
    /// inspector is unchanged and the viewer keeps today's behaviour; an
    /// expanded section reports itself, which is the only thing that gives the
    /// viewer pointer input.
    #[test]
    fn the_matte_section_is_collapsed_by_default_and_reports_its_expansion() {
        let ctx = egui::Context::default();
        let clip = look_clip(vec![colour_effect(1, "color_wheels")]);
        let effect = clip.effects[0].clone();
        let mut header_id = None;

        let mut edits = InspectorEdits::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            header_id = Some(matte_section_id(clip.id, effect.id));
            matte_section(ui, &clip, &effect, &mut edits);
        });
        assert_eq!(
            edits.matte_expanded(),
            None,
            "a collapsed section leaves the viewer alone"
        );
        assert_eq!(edits.matte_selected_window(), None);
        assert!(
            edits.operations().is_empty(),
            "drawing the card writes nothing to the document"
        );

        let header_id = header_id.expect("the section drew a header");
        let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
            &ctx, header_id, false,
        );
        state.set_open(true);
        state.store(&ctx);

        // The header animates open, so the report lands within a few frames.
        let mut reported = None;
        for _ in 0..20 {
            let mut edits = InspectorEdits::default();
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                matte_section(ui, &clip, &effect, &mut edits);
            });
            if let Some(target) = edits.matte_expanded() {
                assert!(
                    edits.operations().is_empty(),
                    "an expanded section still writes nothing on its own"
                );
                reported = Some(target);
                break;
            }
        }
        assert_eq!(
            reported,
            Some(MatteTarget::new(clip.id, effect.id)),
            "an expanded section names the node it belongs to"
        );
    }

    /// Draw one colour node's card exactly as `color_stage_section` dispatches
    /// it, with its matte section — if it has one — forced open, and report
    /// what the card told the viewer.
    ///
    /// The section is opened by storing the `CollapsingState` under the id the
    /// card will build, then re-running until the header's open animation has
    /// settled, which is how `the_matte_section_is_collapsed_by_default...`
    /// already drives it.
    fn matte_expansion_reported_by_card(name: &str) -> Option<MatteTarget> {
        let ctx = egui::Context::default();
        // The cards ask for the app's own font families, so the theme has to be
        // installed before one can be laid out.
        crate::theme::install(&ctx);
        let effect = colour_effect(1, name);
        let clip = look_clip(vec![effect.clone()]);
        let kind = ColorNodeKind::from_effect_name(name).expect("a registered colour node");
        let document = look_document(vec![effect.clone()], Vec::new());
        let availability = BTreeMap::new();
        let qc_clipping = crate::color_qc_ui::ColorQcNodeClipping::default();

        let draw = |ui: &mut egui::Ui, edits: &mut InspectorEdits| {
            let looks = LookInspectorContext {
                document: &document,
                availability: &availability,
                qc_clipping: &qc_clipping,
                store_unavailable: None,
            };
            match kind {
                ColorNodeKind::Primary => {
                    primary_correction_section(ui, &clip, &effect, &looks, edits);
                }
                ColorNodeKind::Wheels => {
                    color_wheels_section(ui, &clip, &effect, 0, &looks, edits);
                }
                ColorNodeKind::Curves => {
                    color_curves_section(ui, &clip, &effect, 0, &looks, edits);
                }
                ColorNodeKind::TechnicalLut | ColorNodeKind::CreativeLook => {
                    lut_node_section(ui, &clip, &effect, kind, 0, &looks, edits);
                }
            }
        };

        // The section's id names the node, not the layout, so it can be forced
        // open from here without the card's cooperation. A card that draws no
        // matte section simply never reads it.
        let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
            &ctx,
            matte_section_id(clip.id, effect.id),
            false,
        );
        state.set_open(true);
        state.store(&ctx);

        // The header animates open, so the report lands within a few frames.
        for _ in 0..20 {
            let mut edits = InspectorEdits::default();
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| draw(ui, &mut edits));
            if let Some(target) = edits.matte_expanded() {
                return Some(target);
            }
        }
        None
    }

    /// Every string one colour node's card paints, with the QC clipping
    /// snapshot it was given.
    fn painted_colour_node_card(
        name: &str,
        qc_clipping: &crate::color_qc_ui::ColorQcNodeClipping,
    ) -> Vec<String> {
        let ctx = egui::Context::default();
        // The cards ask for the app's own font families, so the theme has to be
        // installed before one can be laid out.
        crate::theme::install(&ctx);
        let effect = colour_effect(1, name);
        let clip = look_clip(vec![effect.clone()]);
        let kind = ColorNodeKind::from_effect_name(name).expect("a registered colour node");
        let document = look_document(vec![effect.clone()], Vec::new());
        let availability = BTreeMap::new();
        let mut edits = InspectorEdits::default();

        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let looks = LookInspectorContext {
                document: &document,
                availability: &availability,
                qc_clipping,
                store_unavailable: None,
            };
            match kind {
                ColorNodeKind::Primary => {
                    primary_correction_section(ui, &clip, &effect, &looks, &mut edits);
                }
                ColorNodeKind::Wheels => {
                    color_wheels_section(ui, &clip, &effect, 0, &looks, &mut edits);
                }
                ColorNodeKind::Curves => {
                    color_curves_section(ui, &clip, &effect, 0, &looks, &mut edits);
                }
                ColorNodeKind::TechnicalLut | ColorNodeKind::CreativeLook => {
                    lut_node_section(ui, &clip, &effect, kind, 0, &looks, &mut edits);
                }
            }
        });
        assert!(
            edits.operations().is_empty(),
            "drawing a card writes nothing to the document"
        );
        crate::theme::painted_text(&output)
    }

    /// CC6 §8.3: the clipping contribution reaches the colour node's own card,
    /// with the frame it was measured at, and only when there is one.
    ///
    /// Rendered headless rather than asserted through `line_for` alone: the
    /// line has to be *drawn* by the card the operator is looking at, and a
    /// card that never calls it would pass every test of the string.
    #[test]
    fn a_colour_node_card_draws_its_clipping_contribution_line() {
        let clipping = crate::color_qc_ui::ColorQcNodeClipping::from_entries(
            12,
            vec![
                (ClipId(10), EffectId(1), 903, 41),
                // A node whose removal changed nothing, on the same report.
                (ClipId(10), EffectId(2), 0, 0),
            ],
        );
        let expected = "Clipping contribution: +903 bp range · +41 bp gamut (frame 12)";

        for name in ["primary_correction", "color_wheels", "color_curves"] {
            let painted = painted_colour_node_card(name, &clipping);
            assert!(
                painted.iter().any(|line| line == expected),
                "{name}'s card does not draw the clipping line:\n{painted:#?}"
            );

            // And with no measurement, the card is exactly as it was: a report
            // of nothing is not a line saying zero.
            let quiet =
                painted_colour_node_card(name, &crate::color_qc_ui::ColorQcNodeClipping::default());
            assert!(
                !quiet
                    .iter()
                    .any(|line| line.starts_with("Clipping contribution")),
                "{name}'s card invents a line with no report behind it:\n{quiet:#?}"
            );
        }
    }

    /// Every matte-capable card draws the section, and `technical_lut` does
    /// not: a partially applied source normalization is not a meaningful state
    /// (CC5 §2.1).
    ///
    /// Driven through the real cards rather than asserted against the core
    /// predicate: what matters is that the four kinds actually *render* a
    /// section the viewer can act on, and that the fifth renders none however
    /// hard the open state is forced.
    #[test]
    fn only_the_matte_capable_cards_render_a_section() {
        for name in MATTE_CAPABLE_KINDS {
            assert_eq!(
                matte_expansion_reported_by_card(name),
                Some(MatteTarget::new(ClipId(10), EffectId(1))),
                "{name}'s card must draw a matte section and report its expansion"
            );
        }
        assert_eq!(
            matte_expansion_reported_by_card("technical_lut"),
            None,
            "a technical input transform has no matte section to expand, so \
             forcing the id open reports nothing and the viewer stays inert"
        );
    }
}
