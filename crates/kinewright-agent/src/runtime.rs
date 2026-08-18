//! Harness-neutral agent runtime contracts.
//!
//! The MCP server exposes this compact contract. The complete editor capability
//! registry remains internal for on-demand discovery and dispatch.

use std::collections::{BTreeSet, HashMap, VecDeque};

use kinewright_core::{BatchError, Document, Operation, TimelineRevision, apply_batch};
use rmcp::model::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::schema::{INSPECTOR_TOOL_NAMES, operation_tools};

pub const COMPACT_TOOL_NAMES: [&str; 7] = [
    "get_timeline_state",
    "search_capabilities",
    "get_capability",
    "invoke_capability",
    "prepare_edit_plan",
    "commit_edit_plan",
    "discard_edit_plan",
];

const META_CAPABILITY_NAMES: [&str; 7] = [
    "apply_edit_plan",
    "search_capabilities",
    "get_capability",
    "invoke_capability",
    "prepare_edit_plan",
    "commit_edit_plan",
    "discard_edit_plan",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
    Inspector,
    Planner,
    Action,
    EditOperation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapabilityDescriptor {
    pub name: String,
    pub kind: CapabilityKind,
    pub summary: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ToolSurfaceMetrics {
    pub tool_count: usize,
    pub serialized_bytes: u64,
    pub input_schema_bytes: u64,
    pub description_bytes: u64,
}

impl ToolSurfaceMetrics {
    #[must_use]
    pub fn measure(tools: &[Tool]) -> Self {
        Self {
            tool_count: tools.len(),
            serialized_bytes: tools
                .iter()
                .map(serialized_len)
                .fold(0, u64::saturating_add),
            input_schema_bytes: tools
                .iter()
                .map(|tool| serialized_len(tool.input_schema.as_ref()))
                .fold(0, u64::saturating_add),
            description_bytes: tools
                .iter()
                .filter_map(|tool| tool.description.as_deref())
                .map(|description| u64::try_from(description.len()).unwrap_or(u64::MAX))
                .fold(0, u64::saturating_add),
        }
    }
}

fn serialized_len<T: Serialize>(value: &T) -> u64 {
    serde_json::to_vec(value).map_or(0, |bytes| u64::try_from(bytes.len()).unwrap_or(u64::MAX))
}

#[must_use]
pub fn compact_tool_names() -> Vec<String> {
    COMPACT_TOOL_NAMES.into_iter().map(str::to_owned).collect()
}

#[must_use]
pub fn capabilities(tools: &[Tool]) -> Vec<CapabilityDescriptor> {
    let operation_names = operation_tools()
        .map(|definitions| {
            definitions
                .into_iter()
                .map(|definition| definition.tool.name.into_owned())
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    tools
        .iter()
        .filter(|tool| !META_CAPABILITY_NAMES.contains(&tool.name.as_ref()))
        .map(|tool| CapabilityDescriptor {
            name: tool.name.to_string(),
            kind: capability_kind(tool.name.as_ref(), &operation_names),
            summary: tool
                .description
                .as_deref()
                .map(first_sentence)
                .unwrap_or_default(),
        })
        .collect()
}

#[must_use]
pub fn search_capabilities(
    tools: &[Tool],
    query: Option<&str>,
    kinds: &[CapabilityKind],
    limit: usize,
) -> Vec<CapabilityDescriptor> {
    let query = query.map(str::trim).filter(|query| !query.is_empty());
    capabilities(tools)
        .into_iter()
        .filter(|capability| kinds.is_empty() || kinds.contains(&capability.kind))
        .filter(|capability| {
            query.is_none_or(|query| {
                let query = query.to_ascii_lowercase();
                let haystack = format!(
                    "{} {}",
                    capability
                        .name
                        .to_ascii_lowercase()
                        .replace(['_', '-'], " "),
                    capability.summary.to_ascii_lowercase()
                );
                haystack.contains(&query)
                    || query.split_whitespace().all(|term| haystack.contains(term))
            })
        })
        .take(limit.clamp(1, 100))
        .collect()
}

fn capability_kind(name: &str, operation_names: &BTreeSet<String>) -> CapabilityKind {
    if operation_names.contains(name) {
        return CapabilityKind::EditOperation;
    }
    if name.starts_with("plan_") {
        return CapabilityKind::Planner;
    }
    if name.starts_with("get_") || name == "search_media" || name.starts_with("track_") {
        return CapabilityKind::Inspector;
    }
    CapabilityKind::Action
}

fn first_sentence(description: &str) -> String {
    let end = description
        .find('.')
        .map_or(description.len(), |index| index + 1);
    description[..end].to_owned()
}

#[must_use]
pub fn is_invocable_capability(name: &str) -> bool {
    INSPECTOR_TOOL_NAMES.contains(&name)
        && !matches!(
            name,
            "apply_edit_plan"
                | "search_capabilities"
                | "get_capability"
                | "invoke_capability"
                | "prepare_edit_plan"
                | "commit_edit_plan"
                | "discard_edit_plan"
        )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct PreparedPlanId(pub u64);

impl std::fmt::Display for PreparedPlanId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EditPlanPreview {
    pub expected_revision: TimelineRevision,
    pub operation_count: usize,
    pub before_tracks: usize,
    pub after_tracks: usize,
    pub before_clips: usize,
    pub after_clips: usize,
    pub before_duration_frames: i64,
    pub after_duration_frames: i64,
    pub destructive_operations: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PreparedEditPlan {
    pub id: PreparedPlanId,
    pub expected_revision: TimelineRevision,
    pub operations: Vec<Operation>,
    pub preview: EditPlanPreview,
}

#[derive(Debug)]
pub(crate) struct PreparedPlanStore {
    capacity: usize,
    next_id: u64,
    order: VecDeque<PreparedPlanId>,
    plans: HashMap<PreparedPlanId, PreparedEditPlan>,
}

impl Default for PreparedPlanStore {
    fn default() -> Self {
        Self::with_capacity(64)
    }
}

impl PreparedPlanStore {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            next_id: 1,
            order: VecDeque::new(),
            plans: HashMap::new(),
        }
    }

    pub(crate) fn prepare(
        &mut self,
        expected_revision: TimelineRevision,
        actual_revision: TimelineRevision,
        document: &Document,
        values: Vec<Value>,
    ) -> Result<PreparedEditPlan, PreparePlanError> {
        if expected_revision != actual_revision {
            return Err(PreparePlanError::RevisionConflict {
                expected: expected_revision,
                actual: actual_revision,
            });
        }
        let operations = values
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                decode_plan_operation_value(value).map_err(|error| {
                    PreparePlanError::InvalidOperation {
                        op_number: index + 1,
                        error,
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.prepare_operations(expected_revision, actual_revision, document, operations)
    }

    pub(crate) fn prepare_operations(
        &mut self,
        expected_revision: TimelineRevision,
        actual_revision: TimelineRevision,
        document: &Document,
        operations: Vec<Operation>,
    ) -> Result<PreparedEditPlan, PreparePlanError> {
        if expected_revision != actual_revision {
            return Err(PreparePlanError::RevisionConflict {
                expected: expected_revision,
                actual: actual_revision,
            });
        }
        let mut candidate = document.clone();
        apply_batch(&mut candidate, &operations).map_err(PreparePlanError::InvalidPlan)?;
        let id = PreparedPlanId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        let plan = PreparedEditPlan {
            id,
            expected_revision,
            preview: plan_preview(expected_revision, document, &candidate, &operations),
            operations,
        };
        self.plans.insert(id, plan.clone());
        self.order.push_back(id);
        while self.plans.len() > self.capacity {
            if let Some(expired) = self.order.pop_front() {
                self.plans.remove(&expired);
            }
        }
        Ok(plan)
    }

    pub(crate) fn get(&self, id: PreparedPlanId) -> Option<PreparedEditPlan> {
        self.plans.get(&id).cloned()
    }

    pub(crate) fn take(&mut self, id: PreparedPlanId) -> Option<PreparedEditPlan> {
        let plan = self.plans.remove(&id);
        if plan.is_some() {
            self.order.retain(|candidate| *candidate != id);
        }
        plan
    }

    pub(crate) fn discard(&mut self, id: PreparedPlanId) -> bool {
        self.take(id).is_some()
    }
}

fn plan_preview(
    expected_revision: TimelineRevision,
    before: &Document,
    after: &Document,
    operations: &[Operation],
) -> EditPlanPreview {
    EditPlanPreview {
        expected_revision,
        operation_count: operations.len(),
        before_tracks: before.tracks.len(),
        after_tracks: after.tracks.len(),
        before_clips: clip_count(before),
        after_clips: clip_count(after),
        before_duration_frames: before.duration.0,
        after_duration_frames: after.duration.0,
        destructive_operations: operations
            .iter()
            .filter_map(|operation| match operation {
                Operation::DeleteClip { .. }
                | Operation::RippleDeleteClip { .. }
                | Operation::RemoveTrack { .. }
                | Operation::RemoveBin { .. }
                | Operation::RemoveStringOut { .. }
                | Operation::RemoveSyncGroup { .. }
                | Operation::RemoveAudioBus { .. } => {
                    Some(crate::schema::operation_tool_name(operation).to_owned())
                }
                _ => None,
            })
            .collect(),
    }
}

fn clip_count(document: &Document) -> usize {
    document.tracks.iter().map(|track| track.clips.len()).sum()
}

pub(crate) fn decode_plan_operation_value(value: Value) -> Result<Operation, String> {
    if let Value::String(serialized) = &value {
        let parsed = serde_json::from_str::<Value>(serialized)
            .map_err(|error| format!("stringified operation is not valid JSON: {error}"))?;
        return decode_plan_operation_value(parsed);
    }
    if let Ok(operation) = serde_json::from_value::<Operation>(value.clone()) {
        return Ok(operation);
    }
    let Value::Object(mut object) = value else {
        return Err("operation must be an object".to_owned());
    };
    if let Some(op) = object.remove("op") {
        let op = op
            .as_str()
            .ok_or_else(|| "operation op must be a snake_case string".to_owned())?;
        let variant = snake_to_pascal(op);
        let tagged = Value::Object(serde_json::Map::from_iter([(
            variant,
            Value::Object(object),
        )]));
        return serde_json::from_value(tagged).map_err(|error| error.to_string());
    }
    if object.len() == 1 {
        let (name, payload) = object.into_iter().next().expect("length checked");
        let variant = snake_to_pascal(&name);
        let tagged = Value::Object(serde_json::Map::from_iter([(variant, payload)]));
        return serde_json::from_value(tagged).map_err(|error| error.to_string());
    }
    Err("operation must use the generated enum envelope or include an op field".to_owned())
}

fn snake_to_pascal(value: &str) -> String {
    value
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(characters).collect()
            })
        })
        .collect()
}

#[derive(Debug, Error)]
pub(crate) enum PreparePlanError {
    #[error("timeline revision conflict: expected {expected}, actual {actual}")]
    RevisionConflict {
        expected: TimelineRevision,
        actual: TimelineRevision,
    },
    #[error("operation {op_number} could not be decoded: {error}")]
    InvalidOperation { op_number: usize, error: String },
    #[error("edit plan is invalid: {0}")]
    InvalidPlan(BatchError),
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use kinewright_core::{ClipId, Marker, MarkerId, TimeCode};
    use rmcp::model::JsonObject;
    use serde_json::json;

    use super::*;

    #[test]
    fn compact_operation_decoding_is_runtime_owned() {
        assert_eq!(
            decode_plan_operation_value(json!({
                "op": "split_clip",
                "clip": 7,
                "at": 30
            }))
            .unwrap(),
            Operation::SplitClip {
                clip: ClipId(7),
                at: TimeCode(30),
            }
        );
    }

    #[test]
    fn stringified_operation_objects_are_tolerated_at_the_agent_boundary() {
        assert_eq!(
            decode_plan_operation_value(Value::String(
                r#"{"op":"split_clip","clip":7,"at":30}"#.to_owned(),
            ))
            .unwrap(),
            Operation::SplitClip {
                clip: ClipId(7),
                at: TimeCode(30),
            }
        );
        assert!(
            decode_plan_operation_value(Value::String("not json".to_owned()))
                .unwrap_err()
                .starts_with("stringified operation is not valid JSON")
        );
    }

    #[test]
    fn capability_search_is_bounded_and_kind_filtered() {
        let tools = vec![
            Tool::new(
                "get_frame_at",
                "Render one frame.",
                Arc::new(JsonObject::new()),
            ),
            Tool::new(
                "queue_export",
                "Queue a delivery export.",
                Arc::new(JsonObject::new()),
            ),
        ];
        let result = search_capabilities(&tools, Some("frame"), &[CapabilityKind::Inspector], 10);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "get_frame_at");
    }

    #[test]
    fn prepared_plan_store_rejects_stale_and_invalid_plans() {
        let document = Document::default();
        let mut store = PreparedPlanStore::default();
        assert!(matches!(
            store.prepare(
                TimelineRevision(1),
                TimelineRevision(2),
                &document,
                vec![json!({"op": "delete_clip", "clip": 1})],
            ),
            Err(PreparePlanError::RevisionConflict { .. })
        ));
        assert!(matches!(
            store.prepare(
                TimelineRevision(2),
                TimelineRevision(2),
                &document,
                vec![json!({"op": "delete_clip", "clip": 1})],
            ),
            Err(PreparePlanError::InvalidPlan(_))
        ));
    }

    #[test]
    fn prepared_plan_store_consumes_a_plan_once() {
        let mut document = Document::default();
        document.tracks.push(kinewright_core::Track {
            id: kinewright_core::TrackId(1),
            kind: kinewright_core::TrackKind::Video,
            sync_lock: true,
            clips: Vec::new(),
        });
        let mut store = PreparedPlanStore::default();
        let plan = store
            .prepare(
                TimelineRevision(0),
                TimelineRevision(0),
                &document,
                vec![json!({
                    "op": "add_marker",
                    "marker": {"id": 1, "position": 0, "label": "Review", "color_token": 0}
                })],
            )
            .unwrap();
        assert_eq!(store.take(plan.id).unwrap().id, plan.id);
        assert!(store.take(plan.id).is_none());
    }

    #[test]
    fn prepared_plan_store_accepts_server_built_operations_without_json_round_trip() {
        let document = Document::default();
        let operation = Operation::AddMarker {
            marker: Marker {
                id: MarkerId(1),
                position: TimeCode::ZERO,
                label: "Review".to_owned(),
                color_token: 0,
            },
        };
        let mut store = PreparedPlanStore::default();

        let plan = store
            .prepare_operations(
                TimelineRevision::default(),
                TimelineRevision::default(),
                &document,
                vec![operation.clone()],
            )
            .unwrap();

        assert_eq!(plan.operations, vec![operation]);
        assert_eq!(plan.preview.operation_count, 1);
        assert_eq!(store.get(plan.id).unwrap().id, plan.id);
    }
}
