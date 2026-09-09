use std::{borrow::Cow, fmt::Write, sync::Arc};

use kinewright_core::{
    COLOR_CURVE_COORDINATE_MAX, COLOR_CURVE_COORDINATE_MIN, COLOR_CURVE_MAX_POINTS,
    COLOR_CURVE_MIN_POINTS, COLOR_CURVE_WHITE_BASIS_POINTS, ColorCurveChannel, EFFECT_DESCRIPTORS,
    EffectParameterDescriptor, LUT_ASSET_ID_PARAMETER, LUT_INPUT_ENCODING_PARAMETER,
    MATTE_HUE_WIDTH_DISABLE_CENTIDEGREES, MATTE_MIX_BASIS_POINTS_MAX, MATTE_PARAMETER_COUNT,
    MATTE_WINDOW_LIMIT, Operation, TITLE_PARAMETER_DESCRIPTORS, TRANSITION_DESCRIPTORS,
    TimelineRevision, is_lut_color_node, is_matte_capable_color_node, is_matte_parameter,
    matte_parameters, matte_window_parameters,
};
use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use serde_json::{Map, Value};
use thiserror::Error;

pub const INSPECTOR_TOOL_NAMES: [&str; 78] = [
    "get_timeline_state",
    "search_capabilities",
    "get_capability",
    "invoke_capability",
    "prepare_edit_plan",
    "commit_edit_plan",
    "discard_edit_plan",
    "get_color_context",
    "plan_primary_correction",
    "plan_color_wheels",
    "plan_color_curves",
    "plan_technical_lut",
    "plan_creative_look",
    "list_look_assets",
    "import_lut_asset",
    "convert_legacy_look",
    "render_color_proof",
    // CC5 §7: the three matte surfaces. `inspect_grade_matte` needs a
    // `CAPABILITY_KIND_OVERRIDES` entry because `inspect_` matches no
    // name-prefix inference rule.
    "inspect_grade_matte",
    "track_matte_window",
    "plan_secondary_correction",
    // CC6 §7: the working-stage QC surface. `get_` already infers
    // `CapabilityKind::Inspector`, so no `CAPABILITY_KIND_OVERRIDES` entry is
    // needed; that omission is a decision, not an oversight.
    "get_color_qc",
    "get_media_status",
    "get_cache_status",
    "clear_media_cache",
    "relink_media",
    "get_clip_info",
    "get_source_info",
    "plan_source_program_edit",
    "get_source_storyboard",
    "get_source_shot_board",
    "get_cut_neighborhoods",
    "search_media",
    "get_frame_at",
    "get_video_scopes",
    "get_video_scopes_v2",
    "analyze_color_shot",
    "plan_shot_match",
    "track_mask_region",
    "track_reframe_subject",
    "get_timeline_storyboard",
    "get_transcript",
    "get_transcripts",
    "get_timeline_transcript",
    "get_dialogue_pacing",
    "get_silences",
    "get_timeline_silences",
    "get_scene_changes",
    "get_timeline_scene_changes",
    "get_beats",
    "get_timeline_beats",
    "get_music_structure",
    // AU1 §6.2: the read-only mix measurement surface, beside the other audio
    // inspectors. `get_` already infers `CapabilityKind::Inspector`, so no
    // `CAPABILITY_KIND_OVERRIDES` entry is needed; that omission is a
    // decision, not an oversight, exactly as for `get_color_qc` above.
    "get_audio_levels",
    // AU2 §6.2: the third-octave evidence surface, beside the levels
    // measurement it mirrors. `get_` infers `CapabilityKind::Inspector`, so
    // no `CAPABILITY_KIND_OVERRIDES` entry is needed here either.
    "get_audio_spectrum",
    // AU3 §4.1: the evidence-only audio QC surface, beside the two
    // measurements it extends. `get_` infers `CapabilityKind::Inspector`, so
    // no `CAPABILITY_KIND_OVERRIDES` entry is needed here either.
    "get_audio_qc",
    "plan_dialogue_assembly",
    "plan_beat_pacing",
    "plan_beat_montage",
    "plan_music_fit",
    "plan_audio_normalization",
    "get_analysis_status",
    "get_caption_presets",
    "get_captions",
    "plan_caption_corrections",
    "add_styled_captions",
    "get_qa_report",
    "get_delivery_variants",
    "get_delivery_profiles",
    "get_delivery_conformance",
    "get_delivery_variant_storyboard",
    "get_editorial_readiness",
    "plan_speaker_multicam",
    "queue_export",
    "get_export_jobs",
    "cancel_export",
    "request_analysis",
    "cancel_analysis",
    "apply_edit_plan",
    "import_media",
];

/// Operation variants deliberately absent from the generated mutator tools.
///
/// Each one is owned by exactly one hand-written capability that does work the
/// generated single-operation tool cannot express:
///
/// - `relink_media` for `RelinkAsset`: the replacement must be probed and
///   hashed by the filesystem-owning media layer first;
/// - `import_lut_asset` for `AddLutAsset`: the `.cube` bytes must be parsed,
///   hashed, and written into the project store first (CC4 §8);
/// - `convert_legacy_look` for `ConvertLegacyLook`: a legacy look whose asset
///   is not registered yet needs the `[AddLutAsset, ConvertLegacyLook]` batch
///   of CC4 §9, and `AddLutAsset` is refused on every plan path by design, so
///   the generated single-operation tool could only ever describe the half of
///   the conversion that happens to already be submittable. The raw
///   `ConvertLegacyLook` operation stays legal in `prepare_edit_plan` for a
///   node whose asset is already registered; nothing new is blocked.
pub const UNGENERATED_OPERATION_VARIANTS: [&str; 3] =
    ["RelinkAsset", "AddLutAsset", "ConvertLegacyLook"];

/// The shared serde default for a flag whose absence means `true`.
///
/// Shared rather than repeated per module: a second copy is a second place for
/// a default to drift, and every caller means exactly the same thing by it.
#[must_use]
pub(crate) const fn default_true() -> bool {
    true
}

#[derive(Debug, Clone)]
pub struct OperationToolDefinition {
    pub variant: String,
    pub tool: Tool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionedOperation {
    pub expected_revision: TimelineRevision,
    pub operation: Operation,
}

#[derive(Debug, Error)]
pub enum SchemaError {
    #[error("Operation schema has no oneOf variants")]
    MissingVariants,
    #[error("Operation schema variant is not an externally tagged object")]
    InvalidVariant,
    #[error("Operation schema for {0} is not an object")]
    InvalidInput(String),
    #[error("could not decode {tool} arguments: {error}")]
    InvalidArguments { tool: String, error: String },
    #[error("unknown Kinewright operation tool: {0}")]
    UnknownTool(String),
}

/// Build one MCP tool definition for every operation variant.
///
/// # Errors
///
/// Returns a schema error when the generated operation schema has an unexpected shape.
///
/// # Panics
///
/// Panics if the statically generated operation schema cannot be serialized.
pub fn operation_tools() -> Result<Vec<OperationToolDefinition>, SchemaError> {
    let root = serde_json::to_value(schemars::schema_for!(Operation))
        .expect("schemars Operation output must serialize");
    let variants = root
        .get("oneOf")
        .and_then(Value::as_array)
        .ok_or(SchemaError::MissingVariants)?;
    let definitions = root.get("$defs").cloned();

    variants
        .iter()
        // Relinking and LUT-asset registration are intentionally omitted from
        // generated mutator tools.
        //
        // A raw Operation::RelinkAsset cannot prove that the replacement path
        // was probed and hashed by the filesystem-owning media layer, and a
        // raw Operation::AddLutAsset cannot prove that the recorded sha256
        // names bytes that actually exist in the project LUT store (CC4 §8).
        // The explicit `relink_media` and `import_lut_asset` capabilities
        // perform that filesystem work before entering Core; keeping the enum
        // variants in the core schema preserves journal/serde compatibility
        // without exposing an unsafe shortcut. `ConvertLegacyLook` joins them
        // because the CC4 §9 conversion is a batch whose first half is
        // `AddLutAsset`; see UNGENERATED_OPERATION_VARIANTS.
        .filter(|variant_schema| {
            variant_schema
                .get("properties")
                .and_then(Value::as_object)
                .and_then(|properties| properties.keys().next())
                .is_none_or(|variant| !UNGENERATED_OPERATION_VARIANTS.contains(&variant.as_str()))
        })
        .map(|variant_schema| operation_tool(variant_schema, definitions.as_ref()))
        .collect()
}

pub fn decode_operation(
    tool_name: &str,
    mut arguments: JsonObject,
) -> Result<RevisionedOperation, SchemaError> {
    let expected_revision =
        arguments
            .remove("expected_revision")
            .ok_or_else(|| SchemaError::InvalidArguments {
                tool: tool_name.to_owned(),
                error: "missing expected_revision from get_timeline_state".to_owned(),
            })?;
    let expected_revision = serde_json::from_value(expected_revision).map_err(|error| {
        SchemaError::InvalidArguments {
            tool: tool_name.to_owned(),
            error: format!("invalid expected_revision: {error}"),
        }
    })?;
    let definition = operation_tools()?
        .into_iter()
        .find(|definition| definition.tool.name == tool_name)
        .ok_or_else(|| SchemaError::UnknownTool(tool_name.to_owned()))?;
    let tagged = Value::Object(Map::from_iter([(
        definition.variant,
        Value::Object(arguments),
    )]));
    serde_json::from_value(tagged)
        .map(|operation| RevisionedOperation {
            expected_revision,
            operation,
        })
        .map_err(|error| SchemaError::InvalidArguments {
            tool: tool_name.to_owned(),
            error: error.to_string(),
        })
}

/// Return every operation and hand-written capability name in the internal registry.
///
/// # Errors
///
/// Returns a schema error when operation tool generation fails.
pub fn capability_tool_names() -> Result<Vec<String>, SchemaError> {
    let mut names = operation_tools()?
        .into_iter()
        .map(|definition| definition.tool.name.into_owned())
        .collect::<Vec<_>>();
    names.extend(INSPECTOR_TOOL_NAMES.into_iter().map(str::to_owned));
    Ok(names)
}

#[must_use]
pub fn operation_tool_name(operation: &Operation) -> &'static str {
    match operation {
        Operation::AddAsset { .. } => "add_asset",
        // This name remains stable for journal/plan diagnostics, but the
        // generated operation tool is deliberately not emitted (see above).
        Operation::RelinkAsset { .. } => "relink_asset",
        Operation::SetAssetColorDescription { .. } => "set_asset_color_description",
        Operation::SetColorContext { .. } => "set_color_context",
        Operation::UpsertBin { .. } => "upsert_bin",
        Operation::RemoveBin { .. } => "remove_bin",
        Operation::SetAssetBin { .. } => "set_asset_bin",
        Operation::UpsertStringOut { .. } => "upsert_string_out",
        Operation::RemoveStringOut { .. } => "remove_string_out",
        Operation::UpsertSyncGroup { .. } => "upsert_sync_group",
        Operation::RemoveSyncGroup { .. } => "remove_sync_group",
        Operation::UpsertAudioBus { .. } => "upsert_audio_bus",
        Operation::RemoveAudioBus { .. } => "remove_audio_bus",
        Operation::SetAudioMaster { .. } => "set_audio_master",
        Operation::SetPanLaw { .. } => "set_pan_law",
        Operation::AddTrack { .. } => "add_track",
        Operation::RemoveTrack { .. } => "remove_track",
        Operation::SetTrackSyncLock { .. } => "set_track_sync_lock",
        Operation::SetTrackMix { .. } => "set_track_mix",
        Operation::AddClip { .. } => "add_clip",
        Operation::AddTitle { .. } => "add_title",
        Operation::SplitClip { .. } => "split_clip",
        Operation::TrimClip { .. } => "trim_clip",
        Operation::MoveClip { .. } => "move_clip",
        Operation::ThreePointEdit { .. } => "three_point_edit",
        Operation::PatchedThreePointEdit { .. } => "patched_three_point_edit",
        Operation::SlipClip { .. } => "slip_clip",
        Operation::RollEdit { .. } => "roll_edit",
        Operation::SlideClip { .. } => "slide_clip",
        Operation::ReplaceClip { .. } => "replace_clip",
        Operation::FitToFill { .. } => "fit_to_fill",
        Operation::DeleteClip { .. } => "delete_clip",
        Operation::RippleDeleteClip { .. } => "ripple_delete_clip",
        Operation::RippleInsertGap { .. } => "ripple_insert_gap",
        Operation::LinkClips { .. } => "link_clips",
        Operation::UnlinkClips { .. } => "unlink_clips",
        Operation::AddMarker { .. } => "add_marker",
        Operation::RemoveMarker { .. } => "remove_marker",
        Operation::MoveMarker { .. } => "move_marker",
        Operation::AddEffect { .. } => "add_effect",
        Operation::InsertEffect { .. } => "insert_effect",
        Operation::RemoveEffect { .. } => "remove_effect",
        Operation::SetEffectParam { .. } => "set_effect_param",
        Operation::SetEffectKeyframes { .. } => "set_effect_keyframes",
        Operation::ClearEffectKeyframes { .. } => "clear_effect_keyframes",
        // Owned by the hand-written `convert_legacy_look` capability, which
        // registers the asset and converts in one batch (CC4 §9). The name is
        // stable for journal and plan diagnostics.
        Operation::ConvertLegacyLook { .. } => "convert_legacy_look",
        // This name remains stable for journal/plan diagnostics, but the
        // generated operation tool is deliberately not emitted (see above):
        // only `import_lut_asset` can create a `LutAsset` record, because only
        // it can write the hashed bytes into the project store (CC4 §8).
        Operation::AddLutAsset { .. } => "add_lut_asset",
        Operation::RemoveLutAsset { .. } => "remove_lut_asset",
        Operation::SetTitleParam { .. } => "set_title_param",
        Operation::SetClipAudio { .. } => "set_clip_audio",
        Operation::AddTransition { .. } => "add_transition",
        Operation::RemoveTransition { .. } => "remove_transition",
        Operation::SetMarkerParam { .. } => "set_marker_param",
        Operation::AddFreezeFrame { .. } => "add_freeze_frame",
        Operation::SetClipSpeed { .. } => "set_clip_speed",
    }
}

pub fn schema_object<T: schemars::JsonSchema>() -> Arc<JsonObject> {
    let value =
        serde_json::to_value(schemars::schema_for!(T)).expect("schemars output must serialize");
    Arc::new(
        value
            .as_object()
            .expect("root JSON schema must be an object")
            .clone(),
    )
}

#[allow(clippy::too_many_lines)]
fn operation_tool(
    variant_schema: &Value,
    definitions: Option<&Value>,
) -> Result<OperationToolDefinition, SchemaError> {
    let properties = variant_schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or(SchemaError::InvalidVariant)?;
    if properties.len() != 1 {
        return Err(SchemaError::InvalidVariant);
    }
    let (variant, input) = properties
        .iter()
        .next()
        .ok_or(SchemaError::InvalidVariant)?;
    let input = input
        .as_object()
        .cloned()
        .ok_or_else(|| SchemaError::InvalidInput(variant.clone()))?;
    let operation_schema = Value::Object(input);
    let mut revisioned_input = serde_json::Map::from_iter([
        ("type".to_owned(), Value::String("object".to_owned())),
        ("allOf".to_owned(), Value::Array(vec![operation_schema])),
        (
            "properties".to_owned(),
            serde_json::json!({
                "expected_revision": {
                    "type": "integer",
                    "format": "uint64",
                    "minimum": 0,
                    "description": "Exact revision returned by get_timeline_state before planning this edit."
                }
            }),
        ),
        (
            "required".to_owned(),
            serde_json::json!(["expected_revision"]),
        ),
    ]);
    if let Some(definitions) = definitions {
        revisioned_input.insert("$defs".to_owned(), definitions.clone());
    }
    let input = revisioned_input;
    let name = camel_to_snake(variant);
    let annotations = ToolAnnotations::new()
        .read_only(false)
        .destructive(matches!(
            name.as_str(),
            "delete_clip" | "ripple_delete_clip" | "remove_track"
        ))
        .idempotent(matches!(
            name.as_str(),
            "set_clip_audio"
                | "set_track_mix"
                | "upsert_audio_bus"
                | "set_audio_master"
                | "set_pan_law"
        ))
        .open_world(false);
    let mut description = format!(
        "Apply Operation::{variant} to the live timeline only at expected_revision from get_timeline_state. All frame values are exact integers."
    );
    if matches!(
        variant.as_str(),
        "AddEffect"
            | "InsertEffect"
            | "SetEffectParam"
            | "SetEffectKeyframes"
            | "ClearEffectKeyframes"
    ) {
        write!(description, " {}", effect_documentation())
            .expect("writing effect documentation to a String cannot fail");
    }
    if matches!(variant.as_str(), "AddTitle" | "SetTitleParam") {
        description.push_str(" Titles are video-track clips, so move, trim, split, ripple, link, and undo apply normally. Parameters: ");
        for (index, parameter) in TITLE_PARAMETER_DESCRIPTORS.iter().enumerate() {
            if index != 0 {
                description.push_str(", ");
            }
            description.push_str(parameter.name);
        }
        description.push('.');
    }
    if matches!(variant.as_str(), "AddTransition" | "RemoveTransition") {
        write!(description, " {}", transition_documentation())
            .expect("writing transition documentation to a String cannot fail");
    }
    match variant.as_str() {
        "RelinkAsset" => description.push_str(
            " This operation is intentionally not exposed as a generated tool; use relink_media so the media layer probes and hashes the replacement before Core applies it.",
        ),
        "AddLutAsset" => description.push_str(
            " This operation is intentionally not exposed as a generated tool; use import_lut_asset so the media layer parses, hashes, and stores the .cube bytes before Core registers the record.",
        ),
        "InsertEffect" => description.push_str(
            " The positional sibling of AddEffect: index may equal the current effect count, which appends. Managed colour nodes must stay in non-decreasing stage order (technical_lut, then primary_correction/color_wheels/color_curves, then creative_look); a violating index is rejected, never reordered. Prefer plan_technical_lut or plan_creative_look, which compute a legal index for you.",
        ),
        "RemoveLutAsset" => description.push_str(
            " Removes one project LUT asset record. It is rejected while any effect still references the id, including a bypassed node and a Hold keyframe value, and it never deletes the content-addressed store file.",
        ),
        "SetAssetColorDescription" => description.push_str(
            " Source colour overrides must provide the complete typed colour description, nonzero confidence, and user_override provenance. The operation is revision-gated and undoable.",
        ),
        "SetColorContext" => description.push_str(
            " Replaces the project working, monitoring, and delivery colour context as one journaled, undoable edit. Use the current managed SDR context to reset an incompatible legacy or future target explicitly; loading a project never performs this rewrite silently.",
        ),
        "RippleDeleteClip" | "RippleInsertGap" => description.push_str(
            " Ripple shifts the edited track and every other sync-locked track. Unlocked tracks remain fixed. Project markers at or after the ripple point always shift regardless of track sync locks. The delete ripple point is the removed clip's pre-edit end; insert uses its explicit at frame. Only clips starting at or after that point shift, and a straddling clip remains unchanged.",
        ),
        "SetTrackSyncLock" => description.push_str(
            " Sync lock is enabled by default. Disable it only when a track should run free during ripple edits on other tracks.",
        ),
        "SetTrackMix" => description.push_str(
            " gain_tenth_db is an integer number of tenths of a decibel in -600..=120; pan_percent is an integer in -100..=100; -100 is hard left and 100 is hard right. How a position becomes per-channel gains is a document-level setting, set_pan_law: under the default balance law 0 is an exact identity, and under constant_power 0 is -3.01 dB on both channels. mute silences the track everywhere including ducking sidechains; any solo silences every non-solo track. The operation replaces all four values; sending neutral values removes the track's entry.",
        ),
        "LinkClips" | "UnlinkClips" => description.push_str(
            " Links are metadata: moving, trimming, or deleting a member requires an atomic plan covering its whole link group.",
        ),
        "UpsertBin" | "RemoveBin" | "SetAssetBin" => description.push_str(
            " Bins are hierarchical project metadata. An asset can belong to at most one bin; removing a bin moves its assets back to the unfiled root and rejects bins that still have children.",
        ),
        "UpsertStringOut" | "RemoveStringOut" => description.push_str(
            " A string-out is an ordered set of labeled, exact source-frame selects and does not mutate the timeline.",
        ),
        "UpsertSyncGroup" | "RemoveSyncGroup" => description.push_str(
            " A sync group requires at least two distinct assets with named angles and exact frame offsets relative to group zero. This is reusable multicam foundation metadata.",
        ),
        "ThreePointEdit" => description.push_str(
            " Mark exactly three of source_in, source_out, timeline_in, and timeline_out; the fourth boundary is derived with exact frame-rate mapping. Insert opens time on the target and sync-locked tracks. Overwrite replaces only the selected target-track range.",
        ),
        "SlipClip" => description.push_str(
            " Preserves the clip's timeline start, duration, speed, effects, and links while changing its source in/out by an equal amount.",
        ),
        "RollEdit" => description.push_str(
            " The clips must be butt-joined media neighbors. Moving their shared boundary preserves the sequence duration.",
        ),
        "SlideClip" => description.push_str(
            " The clip must have butt-joined media neighbors on both sides. Its source remains fixed while its neighbors absorb the move and the sequence duration stays fixed.",
        ),
        "ReplaceClip" => description.push_str(
            " The replacement source must map to the clip's exact current project duration. Timeline placement, clip id, effects, audio shaping, links, and transition metadata are preserved.",
        ),
        "FitToFill" => description.push_str(
            " Finds an exact integer speed from 10% through 1000% for the replacement source to occupy the clip's current timeline slot. It rejects unrepresentable fits instead of drifting a boundary.",
        ),
        "AddMarker" | "MoveMarker" | "RemoveMarker" | "SetMarkerParam" => description.push_str(
            " Markers are non-destructive editorial suggestions and are preferred when reviewing footage.",
        ),
        "SetClipAudio" => description.push_str(
            " gain_tenth_db is an integer number of tenths of a decibel in -600..=120. Fade values are non-negative project frames whose sum cannot exceed the clip duration. Fade-out anchors to the clip's project end. Gain and clip fades compose multiplicatively with transition audio ramps.",
        ),
        "UpsertAudioBus" | "RemoveAudioBus" => description.push_str(
            " Audio buses route each track to at most one bus and carry an ordered effect chain. Each bus also carries a post-effects fader, gain_tenth_db, in tenths of a decibel in -600..=120. Bus effects must use one of audio_gain, audio_eq, audio_compressor, audio_ducking, audio_limiter, audio_parametric_eq, audio_gate, or audio_true_peak_limiter. Prefer audio_parametric_eq (high-pass, low and high shelves, four peaking bands), audio_compressor (peak or RMS detection, soft knee, lookahead), audio_gate, and audio_true_peak_limiter, whose true_peak mode measures the inter-sample peak with a 4x oversampled detector. audio_eq and audio_limiter are legacy fixed-crossover and hard-clamp nodes kept for existing projects; do not add them to a new chain. Every node takes bypass, 0 or 1; bypass, detector and true_peak accept only hold keyframes. The nodes of one chain may declare at most 20 ms of lookahead_milliseconds in total, and that parameter cannot be keyframed at all. Every other numeric control supports the same fixed-point keyframe curves as clip effects, on project frames; frequency and Q interpolate linearly in their integer unit, not logarithmically. Ducking reads the listed sidechain tracks before bus processing. Unrouted tracks feed the master directly.",
        ),
        "SetAudioMaster" => description.push_str(
            " The master chain sits after the bus sum: master gain, then the effects in order, then the single hard clamp to -1.0..=1.0. gain_tenth_db is an integer number of tenths of a decibel in -600..=120. The same audio effect names, ranges, curves, keyframe rules and 20 ms lookahead budget apply as on a bus, except audio_ducking, which has no sidechain at master and is rejected. The operation replaces the whole master chain; sending a zero gain and an empty effect list removes it.",
        ),
        "SetPanLaw" => description.push_str(
            " The pan law decides how each track's pan_percent becomes per-channel gains. balance is the default and leaves centre an exact identity, so a centred document renders bit-identically. constant_power places centre at -3.01 dB on both channels and keeps L^2 + R^2 equal to 1 at every position; switching to it changes the output of every panned and centred track. A one-channel output device applies no pan under either law, and channels beyond the first two take the unpanned gain. The law is a document-level setting; there is no per-track override.",
        ),
        "SetEffectKeyframes" => description.push_str(
            " The curve uses clip-local integer frame offsets and fixed-point parameter values. Keyframes must be non-negative, strictly ordered, inside the clip, and inside the parameter's documented range. Interpolation is hold, linear, ease_in, ease_out, or ease_in_out and applies from each keyframe to the next.",
        ),
        "ClearEffectKeyframes" => description.push_str(
            " Removes automation from one registered parameter and restores its static parameter value.",
        ),
        "AddFreezeFrame" => description.push_str(
            " Freeze clips hold one source frame from a real video-capable asset for a project-frame duration. They are video-track clips and remain silent.",
        ),
        "SetClipSpeed" => description.push_str(
            " speed_percent is an integer percentage in 10..=1000; 100 is real time. Speed scales the media clip's effective source rate, so 50 doubles its project duration and 200 halves it. The operation fails if the new duration would overlap a later clip - ripple-insert a gap first when slowing a clip down. Audio is muted at any speed other than 100. Titles and freeze frames have no speed.",
        ),
        _ => {}
    }
    let tool =
        Tool::new(name, Cow::Owned(description), Arc::new(input)).with_annotations(annotations);
    Ok(OperationToolDefinition {
        variant: variant.clone(),
        tool,
    })
}

fn effect_documentation() -> String {
    let mut documentation = String::from("Supported effects: ");
    for (effect_index, effect) in EFFECT_DESCRIPTORS.iter().enumerate() {
        if effect_index != 0 {
            documentation.push_str("; ");
        }
        write!(documentation, "{}(", effect.name)
            .expect("writing effect documentation to a String cannot fail");
        // CC3 §2.4: `color_curves` owns 133 generated parameters. Enumerating
        // them adds several kilobytes to every AddEffect/SetEffectParam tool
        // description and measurably degrades M36 runtime efficiency, so the
        // three generating patterns are described instead. Every bound below
        // is read from the Core descriptor, so the summary cannot drift.
        if effect.name == COLOR_CURVES_EFFECT_NAME {
            documentation.push_str(&color_curves_pattern_documentation());
        } else if is_lut_color_node(effect.name) {
            // CC4 §8/M36: `lut_asset_id` spans 0..=2^53-1. Printing that range
            // on every AddEffect/SetEffectParam description is pure noise — the
            // only usable ids come from the project, so the description names
            // the tool that lists them instead.
            documentation.push_str(&lut_node_pattern_documentation(effect.parameters));
        } else {
            // CC5 §2.2/M36: the 47 matte parameters are emitted once as a
            // shared legend below, never enumerated per kind. AU2 §4.1/M36:
            // the twelve `audio_parametric_eq` band rows are emitted once as
            // one generating pattern below, for the same reason.
            let parametric_eq = effect.name == PARAMETRIC_EQ_EFFECT_NAME;
            for (parameter_index, parameter) in effect
                .parameters
                .iter()
                .filter(|parameter| {
                    let summarised = is_matte_parameter(parameter.name)
                        || (parametric_eq && is_parametric_eq_band_parameter(parameter.name));
                    !summarised
                })
                .enumerate()
            {
                if parameter_index != 0 {
                    documentation.push_str(", ");
                }
                write!(
                    documentation,
                    "{}={}..={}, neutral {}",
                    parameter.name, parameter.min, parameter.max, parameter.neutral
                )
                .expect("writing effect documentation to a String cannot fail");
            }
        }
        // CC5 §2.2, normative: one shared legend per matte-capable kind.
        if is_matte_capable_color_node(effect.name) {
            documentation.push_str("; ");
            documentation.push_str(&matte_pattern_documentation());
        }
        // AU2 §4.1, normative: one generating pattern for the four peaking
        // bands, never twelve enumerated rows.
        if effect.name == PARAMETRIC_EQ_EFFECT_NAME {
            documentation.push_str("; ");
            documentation.push_str(&audio_parametric_eq_pattern_documentation(
                effect.parameters,
            ));
        }
        documentation.push(')');
        // Legacy compatibility stages remain loadable but are outside the CC1
        // managed conformance claim; advertising them as ordinary effects
        // invites an agent to reach for them instead of primary_correction.
        // `color_grade` is a wire alias Core canonicalises to
        // `primary_correction` on load, so it is labelled as such rather than
        // as a separate effect.
        if effect.name == "color_grade" {
            documentation.push_str(" [alias of primary_correction]");
        } else if kinewright_core::effect_compatibility_stage(effect.name).is_some() {
            documentation
                .push_str(" [legacy - outside CC1 managed conformance; prefer primary_correction]");
        }
    }
    documentation.push_str(
        ". cube_lut additionally requires parameters.path as a non-empty text path to a 3D .cube file; intensity_percent remains integer-automatable.",
    );
    documentation
}

/// The canonical CC3 curves effect name, kept local so the compact-description
/// special case is greppable from the schema module.
const COLOR_CURVES_EFFECT_NAME: &str = "color_curves";

/// A compact description of the four CC4 LUT-node controls (CC4 §8, M36).
///
/// Every bound is read from the Core descriptor so the summary cannot drift;
/// only `lut_asset_id`'s `0..=9007199254740991` range is replaced, because
/// enumerating it teaches an agent nothing it can act on.
fn lut_node_pattern_documentation(parameters: &[EffectParameterDescriptor]) -> String {
    let mut documentation = String::new();
    for (index, parameter) in parameters.iter().enumerate() {
        if index != 0 {
            documentation.push_str(", ");
        }
        if parameter.name == LUT_ASSET_ID_PARAMETER {
            documentation.push_str(
                "lut_asset_id (project LUT asset id; see list_look_assets, neutral 0 = unbound)",
            );
            continue;
        }
        write!(
            documentation,
            "{}={}..={}, neutral {}",
            parameter.name, parameter.min, parameter.max, parameter.neutral
        )
        .expect("writing effect documentation to a String cannot fail");
        if parameter.name == LUT_INPUT_ENCODING_PARAMETER {
            documentation.push_str(" (0 display709, 1 linear, 2 grade709)");
        }
    }
    documentation
}

/// The 47 CC5 matte parameters in one compact legend (CC5 §2.2, M36).
///
/// **Normative:** this must never enumerate the 32 `matte_window{j}_*`
/// parameters. Four matte-capable descriptors × 47 entries is several
/// kilobytes on every `AddEffect`/`SetEffectParam` tool description — the same
/// M36 runtime-efficiency argument that gave `color_curves` its pattern form.
///
/// Every bound is read from the Core descriptors so the legend cannot drift
/// from the values Core actually validates.
fn matte_pattern_documentation() -> String {
    let control = |name: &str| {
        matte_parameters()
            .iter()
            .find(|parameter| parameter.name == name)
            .map_or_else(
                || "?".to_owned(),
                |parameter| format!("{}..={}", parameter.min, parameter.max),
            )
    };
    // Window 0's descriptors carry the bounds every window shares; the table is
    // generated from one macro, so reading window 0 reads all four.
    let window = |suffix: &str| {
        matte_window_parameters(0)
            .and_then(|table| {
                table
                    .iter()
                    .find(|parameter| parameter.name.ends_with(suffix))
                    .map(|parameter| format!("{}..={}", parameter.min, parameter.max))
            })
            .unwrap_or_else(|| "?".to_owned())
    };
    let last_window = MATTE_WINDOW_LIMIT.saturating_sub(1);
    format!(
        "matte_* is the CC5 secondary, {MATTE_PARAMETER_COUNT} parameters summarised once: \
matte_enabled/matte_qualifier_enabled/matte_invert/matte_combine_token={token}, neutral 0, combine 0 union 1 intersection; \
matte_window_count={count}, neutral 0; \
matte_mix_basis_points={mix}, neutral {MATTE_MIX_BASIS_POINTS_MAX}; \
matte_hue_center_centidegrees={hue_center}, neutral 0; \
matte_hue_width_centidegrees and matte_hue_softness_centidegrees={hue_width}, width neutral {MATTE_HUE_WIDTH_DISABLE_CENTIDEGREES} disables the hue leg; \
matte_saturation_ and matte_luma_ each with low/high/softness_basis_points={band}, neutral 0/{MATTE_MIX_BASIS_POINTS_MAX}/0; \
matte_window{{j}}_* for j=0..={last_window}: shape_token={shape}, 1 rect 2 ellipse, neutral 1; \
center_x/center_y_basis_points={centre}, neutral 5000; \
half_width/half_height_basis_points={half}, neutral 2500; \
rotation_centidegrees={rotation}, neutral 0; \
feather_basis_points={feather}, neutral 0; \
invert={token}, neutral 0. \
Prefer plan_secondary_correction, which accepts windows[] and qualifier{{}} and expands them; \
see inspect_grade_matte for measured coverage",
        token = control("matte_enabled"),
        count = control("matte_window_count"),
        mix = control("matte_mix_basis_points"),
        hue_center = control("matte_hue_center_centidegrees"),
        hue_width = control("matte_hue_width_centidegrees"),
        band = control("matte_saturation_low_basis_points"),
        shape = window("_shape_token"),
        centre = window("_center_x_basis_points"),
        half = window("_half_width_basis_points"),
        rotation = window("_rotation_centidegrees"),
        feather = window("_feather_basis_points"),
    )
}

/// The canonical AU2 parametric EQ effect name, kept local so the compact
/// description special case is greppable from the schema module.
const PARAMETRIC_EQ_EFFECT_NAME: &str = "audio_parametric_eq";

/// Whether a parameter name is one of `audio_parametric_eq`'s twelve peaking
/// band rows.
///
/// Deliberately exact — `band` followed by a band index in 1..=4 and an
/// underscore. A loose `starts_with("band")` would silently swallow a future
/// `bandwidth_hertz` or `band_solo`: the row would be dropped from the
/// enumeration *and* fall outside the pattern sentence, leaving a settable
/// parameter undocumented. A row this predicate rejects is simply enumerated.
fn is_parametric_eq_band_parameter(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("band") else {
        return false;
    };
    let mut characters = rest.chars();
    matches!(characters.next(), Some('1'..='4')) && characters.next() == Some('_')
}

/// A compact pattern description of the twelve `audio_parametric_eq` peaking
/// band parameters (AU2 §4.1, M36).
///
/// Four identical bands enumerated into each of the five effect tools is one
/// generating pattern written twenty times: the same M36 runtime-efficiency
/// argument that gave `color_curves` its pattern form and the CC5 matte its
/// shared legend. Every bound and neutral below is read from the Core
/// descriptor, so the sentence cannot drift from what Core validates.
fn audio_parametric_eq_pattern_documentation(parameters: &[EffectParameterDescriptor]) -> String {
    let band = |suffix: &str| {
        parameters
            .iter()
            .find(|parameter| {
                is_parametric_eq_band_parameter(parameter.name) && parameter.name.ends_with(suffix)
            })
            .map_or_else(
                || "?".to_owned(),
                |parameter| format!("{}..={}", parameter.min, parameter.max),
            )
    };
    let centres = parameters
        .iter()
        .filter(|parameter| {
            is_parametric_eq_band_parameter(parameter.name) && parameter.name.ends_with("_hertz")
        })
        .map(|parameter| parameter.neutral.to_string())
        .collect::<Vec<_>>();
    let band_count = centres.len();
    let neutral = |suffix: &str| {
        parameters
            .iter()
            .find(|parameter| {
                is_parametric_eq_band_parameter(parameter.name) && parameter.name.ends_with(suffix)
            })
            .map_or_else(|| "?".to_owned(), |parameter| parameter.neutral.to_string())
    };
    format!(
        "band{{i}}_hertz/band{{i}}_gain_tenth_db/band{{i}}_q_hundredths for i=1..={band_count}, \
one peaking band each: hertz={hertz}, neutral {centres} by band; gain_tenth_db={gain}, \
neutral {gain_neutral}; q_hundredths={q} hundredths of Q, neutral {q_neutral}",
        hertz = band("_hertz"),
        centres = centres.join("/"),
        gain = band("_gain_tenth_db"),
        gain_neutral = neutral("_gain_tenth_db"),
        q = band("_q_hundredths"),
        q_neutral = neutral("_q_hundredths"),
    )
}

/// A compact pattern description of the 133 `color_curves` parameters.
///
/// Deliberately never enumerates the generated names: see CC3 §2.4 and the
/// tool-schema-bloat risk in CC3 §13.
fn color_curves_pattern_documentation() -> String {
    let curves = ColorCurveChannel::ALL
        .map(ColorCurveChannel::name)
        .join("|");
    let last_index = COLOR_CURVE_MAX_POINTS.saturating_sub(1);
    format!(
        "{{curve}}_point_count={COLOR_CURVE_MIN_POINTS}..={COLOR_CURVE_MAX_POINTS}, neutral {COLOR_CURVE_MIN_POINTS} for curve in {curves}; \
{{curve}}_x{{j}}/{{curve}}_y{{j}} for j=0..={last_index} in {COLOR_CURVE_COORDINATE_MIN}..={COLOR_CURVE_COORDINATE_MAX} basis points of the grade709 range, \
neutral 0 at j=0 and {COLOR_CURVE_WHITE_BASIS_POINTS} otherwise; x must strictly increase over j<point_count and points at j>=point_count are ignored; \
bypass=0..=1, neutral 0. Prefer plan_color_curves, which accepts [[x, y], ...] lists and expands them"
    )
}

fn transition_documentation() -> String {
    let mut documentation = String::from("Supported transitions: ");
    for (index, transition) in TRANSITION_DESCRIPTORS.iter().enumerate() {
        if index != 0 {
            documentation.push_str("; ");
        }
        write!(
            documentation,
            "{}: {}",
            transition.name, transition.description
        )
        .expect("writing transition documentation to a String cannot fail");
    }
    documentation.push_str(
        ". Duration is a positive integer number of project frames no longer than the clip; one frame is fully visible. Every transition also ramps the clip's audio gain from silence to full across the same window.",
    );
    documentation
}

fn camel_to_snake(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 4);
    for (index, character) in value.chars().enumerate() {
        if character.is_uppercase() && index != 0 {
            output.push('_');
        }
        output.extend(character.to_lowercase());
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinewright_core::{AssetId, ClipId, TimeCode, TrackId};

    #[test]
    fn every_operation_variant_produces_a_valid_object_tool_schema() {
        let tools = operation_tools().unwrap();
        let names = tools
            .iter()
            .map(|definition| definition.tool.name.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "add_asset",
                "set_asset_color_description",
                "set_color_context",
                "upsert_bin",
                "remove_bin",
                "set_asset_bin",
                "upsert_string_out",
                "remove_string_out",
                "upsert_sync_group",
                "remove_sync_group",
                "upsert_audio_bus",
                "remove_audio_bus",
                // AU2 §5.4/§6.4: declared immediately after `RemoveAudioBus`
                // and before `AddTrack`, in `Operation` declaration order.
                "set_audio_master",
                "set_pan_law",
                "add_track",
                "remove_track",
                "set_track_sync_lock",
                "set_track_mix",
                "add_clip",
                "add_title",
                "split_clip",
                "trim_clip",
                "move_clip",
                "three_point_edit",
                "patched_three_point_edit",
                "slip_clip",
                "roll_edit",
                "slide_clip",
                "replace_clip",
                "fit_to_fill",
                "delete_clip",
                "ripple_delete_clip",
                "ripple_insert_gap",
                "link_clips",
                "unlink_clips",
                "add_marker",
                "remove_marker",
                "move_marker",
                "add_effect",
                "insert_effect",
                "remove_effect",
                "set_effect_param",
                "set_effect_keyframes",
                "clear_effect_keyframes",
                "remove_lut_asset",
                "set_title_param",
                "set_clip_audio",
                "add_transition",
                "remove_transition",
                "set_marker_param",
                "add_freeze_frame",
                "set_clip_speed",
            ]
        );
        for definition in tools {
            assert!(
                definition.tool.input_schema.contains_key("type")
                    || definition.tool.input_schema.contains_key("$ref")
                    || definition.tool.input_schema.contains_key("allOf"),
                "{} must have an object-compatible JSON schema",
                definition.tool.name
            );
            serde_json::to_string(&definition.tool.input_schema).unwrap();
        }
    }

    #[test]
    fn add_track_schema_exposes_optional_true_sync_lock_default() {
        let tools = operation_tools().unwrap();
        let add_track = tools
            .iter()
            .find(|definition| definition.tool.name == "add_track")
            .unwrap();
        let track_schema = &add_track.tool.input_schema["$defs"]["Track"];
        assert_eq!(
            track_schema["properties"]["sync_lock"]["default"],
            serde_json::json!(true)
        );
        assert!(
            track_schema["required"]
                .as_array()
                .unwrap()
                .iter()
                .all(|field| field != "sync_lock")
        );
    }

    #[test]
    fn asset_color_override_schema_is_typed_and_revision_gated() {
        let tools = operation_tools().unwrap();
        let override_tool = tools
            .iter()
            .find(|definition| definition.tool.name == "set_asset_color_description")
            .unwrap();
        let schema = &override_tool.tool.input_schema;

        assert_eq!(schema["required"], serde_json::json!(["expected_revision"]));
        assert_eq!(
            schema["allOf"][0]["properties"]["color_description"]["$ref"],
            "#/$defs/ColorDescription"
        );
        let description = &schema["$defs"]["ColorDescription"];
        for field in [
            "primaries",
            "transfer",
            "matrix",
            "range",
            "white_point",
            "bit_depth",
            "confidence_basis_points",
            "provenance",
        ] {
            assert!(
                description["properties"].get(field).is_some(),
                "color description schema omitted {field}"
            );
        }
        assert_eq!(
            description["properties"]["confidence_basis_points"]["maximum"],
            10_000
        );
        assert!(
            override_tool
                .tool
                .description
                .as_deref()
                .unwrap()
                .contains("user_override provenance")
        );
    }

    #[test]
    fn add_transition_schema_documents_every_registered_transition() {
        let tools = operation_tools().unwrap();
        let add_transition = tools
            .iter()
            .find(|definition| definition.tool.name == "add_transition")
            .unwrap();
        let description = add_transition.tool.description.as_deref().unwrap();
        for name in ["crossfade", "fade_from_black", "fade_from_white"] {
            assert!(
                description.contains(name),
                "add_transition description omitted {name}"
            );
        }
        assert!(description.contains("positive integer"));
        assert!(description.contains("audio gain"));
    }

    #[test]
    fn set_clip_audio_schema_documents_units_and_composition() {
        let tools = operation_tools().unwrap();
        let set_clip_audio = tools
            .iter()
            .find(|definition| definition.tool.name == "set_clip_audio")
            .unwrap();
        let description = set_clip_audio.tool.description.as_deref().unwrap();
        assert!(description.contains("tenths of a decibel"));
        assert!(description.contains("-600..=120"));
        assert!(description.contains("Fade-out anchors to the clip's project end"));
        assert!(description.contains("transition audio ramps"));
        let serialized = serde_json::to_value(&set_clip_audio.tool).unwrap();
        assert_eq!(
            serialized["annotations"]["idempotentHint"],
            serde_json::Value::Bool(true)
        );
    }

    /// AU1 §7 item 19: the generated `set_track_mix` tool documents both
    /// ranges and the pan law, and is annotated idempotent because the
    /// operation replaces all four values.
    ///
    /// AU2 §6.1/B14: the sentence no longer *asserts* a law. Under
    /// `PanLaw::ConstantPower` "0 is an exact identity" is false, so the
    /// description now points at the document-level `set_pan_law` and
    /// describes both laws; the negative assertion below pins that the old
    /// unconditional claim is gone.
    #[test]
    fn set_track_mix_schema_documents_ranges_and_the_balance_law() {
        let tools = operation_tools().unwrap();
        let set_track_mix = tools
            .iter()
            .find(|definition| definition.tool.name == "set_track_mix")
            .unwrap();
        let description = set_track_mix.tool.description.as_deref().unwrap();
        assert!(description.contains("-600..=120"));
        assert!(description.contains("-100..=100"));
        assert!(description.contains("-100 is hard left and 100 is hard right"));
        assert!(description.contains(
            "How a position becomes per-channel gains is a document-level setting, set_pan_law"
        ));
        assert!(description.contains("under the default balance law 0 is an exact identity"));
        assert!(description.contains("under constant_power 0 is -3.01 dB on both channels"));
        assert!(
            !description.contains("using a balance law"),
            "set_track_mix must not assert one law: {description}"
        );
        assert!(description.contains("any solo silences every non-solo track"));
        assert!(description.contains("sending neutral values removes the track's entry"));
        let serialized = serde_json::to_value(&set_track_mix.tool).unwrap();
        assert_eq!(
            serialized["annotations"]["idempotentHint"],
            serde_json::Value::Bool(true)
        );
        assert_eq!(
            serialized["annotations"]["destructiveHint"],
            serde_json::Value::Bool(false)
        );
    }

    #[test]
    fn operation_exhaustiveness_guard_requires_new_variants_to_be_acknowledged() {
        assert_eq!(
            operation_tool_name(&Operation::SetAssetColorDescription {
                asset: AssetId(1),
                color_description: kinewright_core::ColorDescription::default(),
            }),
            "set_asset_color_description"
        );
        assert_eq!(
            operation_tool_name(&Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false,
            }),
            "set_track_sync_lock"
        );
        assert_eq!(
            operation_tool_name(&Operation::SetTrackMix {
                track: TrackId(1),
                gain_tenth_db: -60,
                pan_percent: 25,
                mute: false,
                solo: true,
            }),
            "set_track_mix"
        );
        // AU2 §6.4: the two Part B variants.
        assert_eq!(
            operation_tool_name(&Operation::SetAudioMaster {
                master: kinewright_core::AudioMaster {
                    gain_tenth_db: -30,
                    effects: Vec::new(),
                },
            }),
            "set_audio_master"
        );
        assert_eq!(
            operation_tool_name(&Operation::SetPanLaw {
                law: kinewright_core::PanLaw::ConstantPower,
            }),
            "set_pan_law"
        );
        assert_eq!(
            operation_tool_name(&Operation::RippleInsertGap {
                track: TrackId(1),
                at: TimeCode(30),
                duration: TimeCode(15),
            }),
            "ripple_insert_gap"
        );
        let _ = (AssetId(1), ClipId(4));
    }

    #[test]
    fn relink_is_explicitly_omitted_from_generated_mutators() {
        let tools = operation_tools().unwrap();
        assert!(
            tools
                .iter()
                .all(|definition| definition.tool.name != "relink_asset")
        );
        assert!(INSPECTOR_TOOL_NAMES.contains(&"relink_media"));
        assert_eq!(
            operation_tool_name(&Operation::RelinkAsset {
                asset: AssetId(1),
                candidate: kinewright_core::RelinkCandidate {
                    path: std::path::PathBuf::from("replacement.mp4"),
                    fingerprint: kinewright_core::MediaSourceFingerprint::default(),
                    kind: kinewright_core::MediaKind::Video,
                    fps: kinewright_core::Rational::new(30, 1).unwrap(),
                    duration: TimeCode(30),
                    resolution: Some((320, 180)),
                },
                allow_unverified_source: true,
            }),
            "relink_asset"
        );
    }

    #[test]
    fn destructive_annotations_cover_ripple_delete_but_not_markers() {
        let tools = operation_tools().unwrap();
        for (name, destructive) in [
            ("ripple_delete_clip", true),
            ("set_track_sync_lock", false),
            ("set_track_mix", false),
            ("add_marker", false),
            ("move_marker", false),
            ("remove_marker", false),
            ("add_title", false),
            ("set_title_param", false),
            ("add_freeze_frame", false),
            ("set_clip_speed", false),
        ] {
            let tool = tools
                .iter()
                .find(|definition| definition.tool.name == name)
                .unwrap();
            let serialized = serde_json::to_value(&tool.tool).unwrap();
            assert_eq!(
                serialized["annotations"]["destructiveHint"],
                serde_json::Value::Bool(destructive),
                "wrong destructive annotation for {name}"
            );
        }
    }

    #[test]
    fn effect_documentation_includes_all_crop_parameters() {
        let documentation = effect_documentation()
            .strip_prefix("Supported effects: ")
            .expect("documentation must keep its stable prefix")
            .to_owned();
        assert!(documentation.contains(
            "crop(left_percent=0..=45, neutral 0, right_percent=0..=45, neutral 0, top_percent=0..=45, neutral 0, bottom_percent=0..=45, neutral 0)"
        ));
    }

    /// CC3 §2.4 and §13: `color_curves` owns 133 generated parameters. The
    /// tool description must summarise the three patterns instead of listing
    /// them, or every AddEffect/SetEffectParam listing grows by several
    /// kilobytes and violates M36's runtime-efficiency posture.
    #[test]
    fn color_curves_documentation_is_a_compact_pattern_not_133_entries() {
        let documentation = effect_documentation();
        let entry = |name: &str| {
            let start = documentation
                .find(&format!("{name}("))
                .unwrap_or_else(|| panic!("{name} must be documented"));
            let close = start
                + documentation[start..]
                    .find(')')
                    .expect("every entry closes its parameter list")
                + 1;
            documentation[start..close].to_owned()
        };

        let descriptor = EFFECT_DESCRIPTORS
            .iter()
            .find(|effect| effect.name == "color_curves")
            .expect("Core must register color_curves");
        // CC5 §2.2 appended the 47 matte parameters to every matte-capable
        // descriptor, so `color_curves` now owns 133 CC3 curve controls plus
        // 47: two generated families, both summarised rather than listed.
        assert_eq!(
            descriptor.parameters.len(),
            kinewright_core::COLOR_CURVES_DESCRIPTOR_PARAMETER_COUNT
        );
        assert_eq!(
            kinewright_core::COLOR_CURVES_DESCRIPTOR_PARAMETER_COUNT,
            133 + MATTE_PARAMETER_COUNT
        );
        let enumerated = descriptor
            .parameters
            .iter()
            .map(|parameter| {
                format!(
                    "{}={}..={}, neutral {}, ",
                    parameter.name, parameter.min, parameter.max, parameter.neutral
                )
                .len()
            })
            .sum::<usize>();
        assert!(
            enumerated > 7_168,
            "enumerating the descriptor would cost {enumerated} bytes"
        );

        let curves = entry("color_curves");
        // `color_curves` is the one kind carrying *two* generated families: the
        // 133 CC3 curve parameters and the 47 CC5 matte parameters. Each is
        // summarised by its own pattern, and the budget is stated per family so
        // a future family cannot be smuggled in under one loose total.
        let curve_pattern = color_curves_pattern_documentation();
        let matte_legend = matte_pattern_documentation();
        for (family, summary) in [("curves", &curve_pattern), ("matte", &matte_legend)] {
            assert!(
                summary.len() < 1_024,
                "the {family} summary must stay under 1 KB, was {} bytes: {summary}",
                summary.len()
            );
        }
        assert!(
            curves.len() < curve_pattern.len() + matte_legend.len() + 128,
            "the color_curves entry is its two pattern summaries and nothing else, was {} bytes: {curves}",
            curves.len()
        );
        // The compact form still states every bound an author needs, and
        // reads them from the Core descriptor rather than restating literals.
        assert!(curves.contains("{curve}_point_count=2..=16"));
        assert!(curves.contains("master|red|green|blue"));
        assert!(curves.contains("{curve}_x{j}/{curve}_y{j}"));
        assert!(curves.contains("j=0..=15"));
        assert!(curves.contains("-2000..=12000"));
        assert!(curves.contains("bypass=0..=1"));
        assert!(curves.contains("plan_color_curves"));
        assert!(
            !curves.contains("master_x0"),
            "the generated parameter names must not be enumerated"
        );

        // Thirteen controls is cheap, so color_wheels stays enumerated.
        let wheels = entry("color_wheels");
        assert!(wheels.contains("lift_master_basis_points=-2000..=2000, neutral 0"));
        assert!(wheels.contains("gain_blue_thousandths=0..=4000, neutral 1000"));
        assert!(wheels.contains("bypass=0..=1, neutral 0"));
    }

    /// The description of one generated operation tool, by tool name.
    fn generated_tool_description(tools: &[OperationToolDefinition], name: &str) -> String {
        tools
            .iter()
            .find(|definition| definition.tool.name == name)
            .unwrap_or_else(|| panic!("{name} must be a generated tool"))
            .tool
            .description
            .as_deref()
            .unwrap_or_else(|| panic!("{name} must carry a description"))
            .to_owned()
    }

    /// AU2 §7 item A18: the `upsert_audio_bus` prose names the four preferred
    /// AU2 nodes, tells the agent not to add the two legacy nodes to a new
    /// chain, states the 20 ms per-chain lookahead budget and the hold-only
    /// rule, and never claims the ITU detector (AU2 OPEN-4).
    #[test]
    fn au2_audio_bus_prose_names_the_new_nodes_and_the_chain_budget() {
        let tools = operation_tools().unwrap();
        let description_of = |name: &str| generated_tool_description(&tools, name);

        for tool in ["upsert_audio_bus", "remove_audio_bus"] {
            let description = description_of(tool);
            for phrase in [
                // The closed set Core enforces (`VisualEffectOnAudioBus`).
                // `upsert_audio_bus` carries no `effect_documentation()`, so
                // this arm is the only place an agent can read the legal names.
                "Bus effects must use one of audio_gain, audio_eq, audio_compressor, \
audio_ducking, audio_limiter, audio_parametric_eq, audio_gate, or \
audio_true_peak_limiter.",
                "audio_parametric_eq",
                "audio_compressor",
                "audio_gate",
                "audio_true_peak_limiter",
                "4x oversampled detector",
                "do not add them to a new chain",
                "bypass, detector and true_peak accept only hold keyframes",
                "at most 20 ms of lookahead_milliseconds",
                "cannot be keyframed at all",
                "Unrouted tracks feed the master directly",
            ] {
                assert!(
                    description.contains(phrase),
                    "{tool} description omitted {phrase}: {description}"
                );
            }
            // Every name Core accepts on a bus is named in the arm, so the
            // sentence cannot fall behind `is_audio_effect`.
            for name in [
                "audio_gain",
                "audio_eq",
                "audio_compressor",
                "audio_ducking",
                "audio_limiter",
                "audio_parametric_eq",
                "audio_gate",
                "audio_true_peak_limiter",
            ] {
                assert!(
                    kinewright_core::is_audio_effect(name),
                    "{name} must be a Core audio effect"
                );
                assert!(
                    description.contains(name),
                    "{tool} description omitted the legal bus effect {name}: {description}"
                );
            }
            // AU2 OPEN-4: the detector has the structure of BS.1770-4 Annex 2
            // with this contract's own coefficients, so the prose must never
            // advertise ITU conformance.
            assert!(
                !description.contains("ITU"),
                "{tool} description must not claim the ITU detector: {description}"
            );
        }
    }

    /// AU2 §7 item A18: the twelve `audio_parametric_eq` band rows are one
    /// generating pattern, not twelve entries repeated into each of the five
    /// effect tools; every other new AU2 audio row is enumerated normally.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn au2_parametric_eq_band_rows_are_one_pattern_not_twelve_entries() {
        let tools = operation_tools().unwrap();
        let description_of = |name: &str| generated_tool_description(&tools, name);
        let add_effect = description_of("add_effect");
        let entry = |name: &str| {
            let start = add_effect
                .find(&format!("{name}("))
                .unwrap_or_else(|| panic!("{name} must be documented"));
            let close = start
                + add_effect[start..]
                    .find(')')
                    .expect("every entry closes its parameter list")
                + 1;
            add_effect[start..close].to_owned()
        };

        // The twelve generated band rows are never enumerated, in any of the
        // five effect tools that carry `effect_documentation()`.
        for tool in [
            "add_effect",
            "insert_effect",
            "set_effect_param",
            "set_effect_keyframes",
            "clear_effect_keyframes",
        ] {
            let description = description_of(tool);
            for band in 1..=4 {
                for suffix in ["hertz", "gain_tenth_db", "q_hundredths"] {
                    let name = format!("band{band}_{suffix}");
                    assert!(
                        !description.contains(&name),
                        "{tool} must summarise {name} rather than enumerate it"
                    );
                }
            }
        }

        let parametric_eq = entry("audio_parametric_eq");
        let pattern = audio_parametric_eq_pattern_documentation(
            EFFECT_DESCRIPTORS
                .iter()
                .find(|effect| effect.name == "audio_parametric_eq")
                .expect("Core must register audio_parametric_eq")
                .parameters,
        );
        assert!(
            parametric_eq.contains(&pattern),
            "the audio_parametric_eq entry must carry the band pattern: {parametric_eq}"
        );
        // Asserted on the generated description, not on the helper's own
        // output, so these are not a self-test of the helper.
        assert!(
            parametric_eq
                .contains("band{i}_hertz/band{i}_gain_tenth_db/band{i}_q_hundredths for i=1..=4")
        );
        // Every bound and neutral is read from the Core descriptor.
        assert!(parametric_eq.contains("hertz=20..=20000, neutral 120/500/2000/8000 by band"));
        assert!(parametric_eq.contains("gain_tenth_db=-240..=240, neutral 0"));
        assert!(parametric_eq.contains("q_hundredths=10..=1800 hundredths of Q, neutral 71"));

        // The sentence states one set of bounds for all four bands, so the four
        // bands must actually agree. Unlike the CC5 matte windows, the twelve
        // rows are hand-written per band in Core, so a per-band edit could
        // otherwise make the summary a lie the agent cannot see through.
        let band_parameters = EFFECT_DESCRIPTORS
            .iter()
            .find(|effect| effect.name == "audio_parametric_eq")
            .expect("Core must register audio_parametric_eq")
            .parameters;
        let band_row = |index: u32, suffix: &str| {
            let name = format!("band{index}_{suffix}");
            *band_parameters
                .iter()
                .find(|parameter| parameter.name == name)
                .unwrap_or_else(|| panic!("Core must register {name}"))
        };
        for suffix in ["hertz", "gain_tenth_db", "q_hundredths"] {
            let first = band_row(1, suffix);
            assert!(
                is_parametric_eq_band_parameter(first.name),
                "band1_{suffix} must be summarised by the pattern"
            );
            for index in 2..=4 {
                let other = band_row(index, suffix);
                assert!(
                    is_parametric_eq_band_parameter(other.name),
                    "band{index}_{suffix} must be summarised by the pattern"
                );
                assert_eq!(
                    (other.min, other.max),
                    (first.min, first.max),
                    "band{index}_{suffix} must share band1_{suffix}'s bounds or the one-sentence summary is wrong"
                );
                if suffix != "hertz" {
                    assert_eq!(
                        other.neutral, first.neutral,
                        "band{index}_{suffix} must share band1_{suffix}'s neutral or the one-sentence summary is wrong"
                    );
                }
            }
        }
        // The four centre neutrals are the four bands' own, in band order.
        let centres = (1..=4)
            .map(|index| band_row(index, "hertz").neutral.to_string())
            .collect::<Vec<_>>()
            .join("/");
        assert!(
            parametric_eq.contains(&format!("neutral {centres} by band")),
            "the centre list must be bands 1..=4 in order: {parametric_eq}"
        );
        let enumerated = EFFECT_DESCRIPTORS
            .iter()
            .find(|effect| effect.name == "audio_parametric_eq")
            .expect("Core must register audio_parametric_eq")
            .parameters
            .iter()
            .filter(|parameter| is_parametric_eq_band_parameter(parameter.name))
            .map(|parameter| {
                format!(
                    "{}={}..={}, neutral {}, ",
                    parameter.name, parameter.min, parameter.max, parameter.neutral
                )
                .len()
            })
            .sum::<usize>();
        assert!(
            enumerated > pattern.len(),
            "enumerating the bands would cost {enumerated} bytes against the {} byte pattern",
            pattern.len()
        );

        // The other seven EQ rows, and every other AU2 row, are enumerated.
        for row in [
            "bypass=0..=1, neutral 0",
            "high_pass_hertz=0..=1000, neutral 0",
            "low_shelf_hertz=20..=1000, neutral 100",
            "low_shelf_gain_tenth_db=-240..=240, neutral 0",
            "high_shelf_hertz=1000..=20000, neutral 8000",
            "high_shelf_gain_tenth_db=-240..=240, neutral 0",
            "output_gain_tenth_db=-240..=240, neutral 0",
        ] {
            assert!(
                parametric_eq.contains(row),
                "the audio_parametric_eq entry omitted {row}: {parametric_eq}"
            );
        }
        let compressor = entry("audio_compressor");
        for row in [
            "knee_tenth_db=0..=240, neutral 0",
            "detector=0..=1, neutral 0",
            "rms_window_milliseconds=1..=100, neutral 10",
            "lookahead_milliseconds=0..=10, neutral 0",
        ] {
            assert!(
                compressor.contains(row),
                "the audio_compressor entry omitted {row}: {compressor}"
            );
        }
        let gate = entry("audio_gate");
        for row in [
            "threshold_tenth_db=-600..=0, neutral -600",
            "ratio_hundredths=100..=2000, neutral 100",
            "range_tenth_db=0..=800, neutral 0",
            "attack_milliseconds=1..=1000, neutral 1",
            "hold_milliseconds=0..=1000, neutral 10",
            "release_milliseconds=10..=5000, neutral 100",
        ] {
            assert!(
                gate.contains(row),
                "the audio_gate entry omitted {row}: {gate}"
            );
        }
        let limiter = entry("audio_true_peak_limiter");
        for row in [
            "ceiling_tenth_db=-120..=0, neutral 0",
            "lookahead_milliseconds=1..=10, neutral 5",
            "release_milliseconds=1..=1000, neutral 50",
            "true_peak=0..=1, neutral 1",
        ] {
            assert!(
                limiter.contains(row),
                "the audio_true_peak_limiter entry omitted {row}: {limiter}"
            );
        }
        // The shared bypass row reaches all eight audio kinds.
        for kind in [
            "audio_gain",
            "audio_eq",
            "audio_compressor",
            "audio_ducking",
            "audio_limiter",
            "audio_parametric_eq",
            "audio_gate",
            "audio_true_peak_limiter",
        ] {
            let documented = entry(kind);
            assert!(
                documented.contains("bypass=0..=1, neutral 0"),
                "the {kind} entry omitted the shared bypass row: {documented}"
            );
        }
    }

    /// AU2 §7 item B14: the two Part B mutators carry the §6.1 prose and the
    /// three full-set operations are annotated idempotent and non-destructive.
    ///
    /// `set_audio_master` and `set_pan_law` replace the whole master chain and
    /// the whole document law respectively, so re-sending one is a no-op; the
    /// `upsert_audio_bus` annotation was simply missing before AU2 even though
    /// it has always been a full upsert.
    #[test]
    fn au2_master_and_pan_law_tools_are_documented_and_idempotent() {
        let tools = operation_tools().unwrap();
        let description_of = |name: &str| generated_tool_description(&tools, name);

        let master = description_of("set_audio_master");
        for sentence in [
            "The master chain sits after the bus sum",
            "then the single hard clamp to -1.0..=1.0",
            "gain_tenth_db is an integer number of tenths of a decibel in -600..=120",
            "20 ms lookahead budget",
            "except audio_ducking, which has no sidechain at master and is rejected",
            "sending a zero gain and an empty effect list removes it",
        ] {
            assert!(
                master.contains(sentence),
                "set_audio_master omitted {sentence}: {master}"
            );
        }

        let law = description_of("set_pan_law");
        for sentence in [
            "balance is the default and leaves centre an exact identity",
            "constant_power",
            "-3.01 dB on both channels",
            "L^2 + R^2 equal to 1 at every position",
            "A one-channel output device applies no pan under either law, and channels beyond the first two take the unpanned gain.",
            "there is no per-track override",
        ] {
            assert!(
                law.contains(sentence),
                "set_pan_law omitted {sentence}: {law}"
            );
        }

        // §6.1: the bus arm gains the fader sentence, and both bus tools share
        // the arm.
        for name in ["upsert_audio_bus", "remove_audio_bus"] {
            let description = description_of(name);
            assert!(
                description.contains(
                    "Each bus also carries a post-effects fader, gain_tenth_db, in tenths of a decibel in -600..=120."
                ),
                "{name} omitted the bus fader sentence: {description}"
            );
        }

        for name in ["upsert_audio_bus", "set_audio_master", "set_pan_law"] {
            let tool = tools
                .iter()
                .find(|definition| definition.tool.name == name)
                .unwrap_or_else(|| panic!("{name} must be a generated tool"));
            let serialized = serde_json::to_value(&tool.tool).unwrap();
            assert_eq!(
                serialized["annotations"]["idempotentHint"],
                serde_json::Value::Bool(true),
                "{name} must be annotated idempotent"
            );
            assert_eq!(
                serialized["annotations"]["destructiveHint"],
                serde_json::Value::Bool(false),
                "{name} must not be annotated destructive"
            );
        }
        // `remove_audio_bus` is neither: it is not a full set, and the
        // destructive list is unchanged by AU2.
        let remove = tools
            .iter()
            .find(|definition| definition.tool.name == "remove_audio_bus")
            .unwrap();
        let serialized = serde_json::to_value(&remove.tool).unwrap();
        assert_eq!(
            serialized["annotations"]["idempotentHint"],
            serde_json::Value::Bool(false)
        );
        assert_eq!(
            serialized["annotations"]["destructiveHint"],
            serde_json::Value::Bool(false)
        );
    }

    /// CC5 §2.2, normative: `schema.rs` must not enumerate the 32
    /// `matte_window{j}_*` parameters per kind. It emits one shared legend,
    /// the same special case `color_curves` already receives.
    #[test]
    fn matte_documentation_is_one_compact_legend_not_47_entries_per_kind() {
        let legend = matte_pattern_documentation();
        // The per-kind cost of the legend, which four descriptors each pay.
        assert!(
            legend.len() < 1_024,
            "the matte legend must stay under 1 KB per kind, was {} bytes: {legend}",
            legend.len()
        );
        let enumerated = matte_parameters()
            .iter()
            .map(|parameter| {
                format!(
                    "{}={}..={}, neutral {}, ",
                    parameter.name, parameter.min, parameter.max, parameter.neutral
                )
                .len()
            })
            .sum::<usize>();
        assert!(
            enumerated > legend.len(),
            "enumerating the matte would cost {enumerated} bytes against the {} byte legend",
            legend.len()
        );

        // Every bound is read from the Core descriptors, so the legend cannot
        // drift from the values Core actually validates.
        assert!(legend.contains("matte_window{j}_*"));
        assert!(legend.contains("j=0..=3"));
        assert!(legend.contains("shape_token=1..=2"));
        assert!(legend.contains("center_x/center_y_basis_points=-10000..=20000"));
        assert!(legend.contains("half_width/half_height_basis_points=1..=10000"));
        assert!(legend.contains("rotation_centidegrees=-18000..=18000"));
        assert!(legend.contains("feather_basis_points=0..=10000"));
        assert!(legend.contains("plan_secondary_correction"));
        assert!(legend.contains("inspect_grade_matte"));
        // The 32 generated window names are never spelled out.
        for name in matte_parameters().iter().map(|parameter| parameter.name) {
            if name.starts_with("matte_window0")
                || name.starts_with("matte_window1")
                || name.starts_with("matte_window2")
                || name.starts_with("matte_window3")
            {
                assert!(
                    !legend.contains(name),
                    "the generated window parameter {name} must not be enumerated"
                );
            }
        }

        // Exactly the four matte-capable kinds carry it, and `technical_lut`
        // does not: a partially applied source normalization is not a
        // meaningful state (CC5 §2.1).
        let documentation = effect_documentation();
        let carriers = EFFECT_DESCRIPTORS
            .iter()
            .filter(|effect| is_matte_capable_color_node(effect.name))
            .map(|effect| effect.name)
            .collect::<Vec<_>>();
        assert_eq!(
            carriers,
            vec![
                "primary_correction",
                "color_wheels",
                "color_curves",
                "creative_look"
            ]
        );
        assert_eq!(
            documentation.matches(legend.as_str()).count(),
            carriers.len()
        );
        let technical = documentation
            .find("technical_lut(")
            .expect("technical_lut must be documented");
        let close = technical
            + documentation[technical..]
                .find(')')
                .expect("every entry closes its parameter list");
        assert!(!documentation[technical..close].contains("matte_"));
    }

    #[test]
    fn mutator_arguments_round_trip_through_the_generated_variant_tag() {
        let arguments = serde_json::from_value(serde_json::json!({
            "expected_revision": 7,
            "clip": 9,
            "at": 30
        }))
        .unwrap();
        assert_eq!(
            decode_operation("split_clip", arguments).unwrap(),
            RevisionedOperation {
                expected_revision: TimelineRevision(7),
                operation: Operation::SplitClip {
                    clip: ClipId(9),
                    at: TimeCode(30),
                },
            }
        );
    }

    #[test]
    fn every_mutator_requires_a_revision_precondition() {
        for definition in operation_tools().unwrap() {
            assert_eq!(
                definition.tool.input_schema["required"],
                serde_json::json!(["expected_revision"]),
                "{} omitted its revision contract",
                definition.tool.name
            );
        }
    }
    #[test]
    fn legacy_colour_effects_are_labelled_in_the_effect_documentation() {
        const LEGACY_LABEL: &str =
            " [legacy - outside CC1 managed conformance; prefer primary_correction]";
        let documentation = effect_documentation();
        // The label itself contains a semicolon, so locate each entry by its
        // name and closing parenthesis rather than by splitting on separators.
        let suffix_after = |name: &str| {
            let start = documentation
                .find(&format!("{name}("))
                .unwrap_or_else(|| panic!("{name} must be documented"));
            let close = start
                + documentation[start..]
                    .find(')')
                    .expect("every entry closes its parameter list")
                + 1;
            documentation[close..].to_owned()
        };
        for name in [
            "brightness",
            "contrast",
            "saturation",
            "look_lut",
            "cube_lut",
        ] {
            assert!(
                suffix_after(name).starts_with(LEGACY_LABEL),
                "{name} must be labelled as a compatibility stage"
            );
        }
        assert!(
            suffix_after("color_grade").starts_with(" [alias of primary_correction]"),
            "color_grade is a wire alias Core canonicalises to primary_correction"
        );
        let primary = suffix_after("primary_correction");
        assert!(!primary.starts_with(LEGACY_LABEL), "{primary}");
    }
}
