use std::{borrow::Cow, fmt::Write, sync::Arc};

use openreel_core::{
    EFFECT_DESCRIPTORS, Operation, TITLE_PARAMETER_DESCRIPTORS, TRANSITION_DESCRIPTORS,
    TimelineRevision,
};
use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use serde_json::{Map, Value};
use thiserror::Error;

pub const INSPECTOR_TOOL_NAMES: [&str; 48] = [
    "get_timeline_state",
    "search_capabilities",
    "get_capability",
    "invoke_capability",
    "prepare_edit_plan",
    "commit_edit_plan",
    "discard_edit_plan",
    "get_clip_info",
    "get_source_info",
    "search_media",
    "get_frame_at",
    "get_video_scopes",
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
    "plan_dialogue_assembly",
    "plan_beat_pacing",
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
    #[error("unknown OpenReel operation tool: {0}")]
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
        Operation::UpsertBin { .. } => "upsert_bin",
        Operation::RemoveBin { .. } => "remove_bin",
        Operation::SetAssetBin { .. } => "set_asset_bin",
        Operation::UpsertStringOut { .. } => "upsert_string_out",
        Operation::RemoveStringOut { .. } => "remove_string_out",
        Operation::UpsertSyncGroup { .. } => "upsert_sync_group",
        Operation::RemoveSyncGroup { .. } => "remove_sync_group",
        Operation::UpsertAudioBus { .. } => "upsert_audio_bus",
        Operation::RemoveAudioBus { .. } => "remove_audio_bus",
        Operation::AddTrack { .. } => "add_track",
        Operation::RemoveTrack { .. } => "remove_track",
        Operation::SetTrackSyncLock { .. } => "set_track_sync_lock",
        Operation::AddClip { .. } => "add_clip",
        Operation::AddTitle { .. } => "add_title",
        Operation::SplitClip { .. } => "split_clip",
        Operation::TrimClip { .. } => "trim_clip",
        Operation::MoveClip { .. } => "move_clip",
        Operation::ThreePointEdit { .. } => "three_point_edit",
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
        Operation::RemoveEffect { .. } => "remove_effect",
        Operation::SetEffectParam { .. } => "set_effect_param",
        Operation::SetEffectKeyframes { .. } => "set_effect_keyframes",
        Operation::ClearEffectKeyframes { .. } => "clear_effect_keyframes",
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
        .idempotent(name == "set_clip_audio")
        .open_world(false);
    let mut description = format!(
        "Apply Operation::{variant} to the live timeline only at expected_revision from get_timeline_state. All frame values are exact integers."
    );
    if matches!(
        variant.as_str(),
        "AddEffect" | "SetEffectParam" | "SetEffectKeyframes" | "ClearEffectKeyframes"
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
        "RippleDeleteClip" | "RippleInsertGap" => description.push_str(
            " Ripple shifts the edited track and every other sync-locked track. Unlocked tracks remain fixed. Project markers at or after the ripple point always shift regardless of track sync locks. The delete ripple point is the removed clip's pre-edit end; insert uses its explicit at frame. Only clips starting at or after that point shift, and a straddling clip remains unchanged.",
        ),
        "SetTrackSyncLock" => description.push_str(
            " Sync lock is enabled by default. Disable it only when a track should run free during ripple edits on other tracks.",
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
            " Audio buses route each track to at most one bus. Bus effects must use audio_gain, audio_eq, audio_compressor, audio_ducking, or audio_limiter; their numeric controls support the same fixed-point keyframe curves as clip effects. Ducking reads the listed sidechain tracks before bus processing. Unrouted tracks feed the master directly.",
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
        for (parameter_index, parameter) in effect.parameters.iter().enumerate() {
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
        documentation.push(')');
    }
    documentation.push_str(
        ". cube_lut additionally requires parameters.path as a non-empty text path to a 3D .cube file; intensity_percent remains integer-automatable.",
    );
    documentation
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
    use openreel_core::{AssetId, ClipId, TimeCode, TrackId};

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
                "upsert_bin",
                "remove_bin",
                "set_asset_bin",
                "upsert_string_out",
                "remove_string_out",
                "upsert_sync_group",
                "remove_sync_group",
                "upsert_audio_bus",
                "remove_audio_bus",
                "add_track",
                "remove_track",
                "set_track_sync_lock",
                "add_clip",
                "add_title",
                "split_clip",
                "trim_clip",
                "move_clip",
                "three_point_edit",
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
                "remove_effect",
                "set_effect_param",
                "set_effect_keyframes",
                "clear_effect_keyframes",
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

    #[test]
    fn operation_exhaustiveness_guard_requires_new_variants_to_be_acknowledged() {
        assert_eq!(
            operation_tool_name(&Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false,
            }),
            "set_track_sync_lock"
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
    fn destructive_annotations_cover_ripple_delete_but_not_markers() {
        let tools = operation_tools().unwrap();
        for (name, destructive) in [
            ("ripple_delete_clip", true),
            ("set_track_sync_lock", false),
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
        let documentation = effect_documentation();
        assert!(documentation.contains(
            "crop(left_percent=0..=45, neutral 0, right_percent=0..=45, neutral 0, top_percent=0..=45, neutral 0, bottom_percent=0..=45, neutral 0)"
        ));
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
}
