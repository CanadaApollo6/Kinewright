use std::{borrow::Cow, sync::Arc};

use openreel_core::Operation;
use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use serde_json::{Map, Value};
use thiserror::Error;

pub const INSPECTOR_TOOL_NAMES: [&str; 11] = [
    "get_timeline_state",
    "get_clip_info",
    "get_frame_at",
    "get_transcript",
    "get_timeline_transcript",
    "get_silences",
    "get_timeline_silences",
    "get_scene_changes",
    "get_timeline_scene_changes",
    "apply_edit_plan",
    "import_media",
];

#[derive(Debug, Clone)]
pub struct OperationToolDefinition {
    pub variant: String,
    pub tool: Tool,
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

pub fn decode_operation(tool_name: &str, arguments: JsonObject) -> Result<Operation, SchemaError> {
    let definition = operation_tools()?
        .into_iter()
        .find(|definition| definition.tool.name == tool_name)
        .ok_or_else(|| SchemaError::UnknownTool(tool_name.to_owned()))?;
    let tagged = Value::Object(Map::from_iter([(
        definition.variant,
        Value::Object(arguments),
    )]));
    serde_json::from_value(tagged).map_err(|error| SchemaError::InvalidArguments {
        tool: tool_name.to_owned(),
        error: error.to_string(),
    })
}

pub fn all_tool_names() -> Result<Vec<String>, SchemaError> {
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
        Operation::AddTrack { .. } => "add_track",
        Operation::RemoveTrack { .. } => "remove_track",
        Operation::AddClip { .. } => "add_clip",
        Operation::SplitClip { .. } => "split_clip",
        Operation::TrimClip { .. } => "trim_clip",
        Operation::MoveClip { .. } => "move_clip",
        Operation::DeleteClip { .. } => "delete_clip",
        Operation::AddEffect { .. } => "add_effect",
        Operation::RemoveEffect { .. } => "remove_effect",
        Operation::SetEffectParam { .. } => "set_effect_param",
        Operation::AddTransition { .. } => "add_transition",
        Operation::RemoveTransition { .. } => "remove_transition",
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
    let mut input = input
        .as_object()
        .cloned()
        .ok_or_else(|| SchemaError::InvalidInput(variant.clone()))?;
    if let Some(definitions) = definitions {
        input.insert("$defs".to_owned(), definitions.clone());
    }
    let name = camel_to_snake(variant);
    let annotations = ToolAnnotations::new()
        .read_only(false)
        .destructive(matches!(name.as_str(), "delete_clip" | "remove_track"))
        .idempotent(false)
        .open_world(false);
    let tool = Tool::new(
        name,
        Cow::Owned(format!(
            "Apply Operation::{variant} to the live timeline. All frame values are exact integers."
        )),
        Arc::new(input),
    )
    .with_annotations(annotations);
    Ok(OperationToolDefinition {
        variant: variant.clone(),
        tool,
    })
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
                "add_track",
                "remove_track",
                "add_clip",
                "split_clip",
                "trim_clip",
                "move_clip",
                "delete_clip",
                "add_effect",
                "remove_effect",
                "set_effect_param",
                "add_transition",
                "remove_transition",
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
    fn operation_exhaustiveness_guard_requires_new_variants_to_be_acknowledged() {
        assert_eq!(
            operation_tool_name(&Operation::SplitClip {
                clip: ClipId(4),
                at: TimeCode(30),
            }),
            "split_clip"
        );
        let _ = (AssetId(1), TrackId(1));
    }

    #[test]
    fn mutator_arguments_round_trip_through_the_generated_variant_tag() {
        let arguments = serde_json::from_value(serde_json::json!({
            "clip": 9,
            "at": 30
        }))
        .unwrap();
        assert_eq!(
            decode_operation("split_clip", arguments).unwrap(),
            Operation::SplitClip {
                clip: ClipId(9),
                at: TimeCode(30),
            }
        );
    }
}
