//! Exact object-identity effects emitted by the operation kernel.
use crate::{Clip, ClipContent, ClipId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationIdentityEffect {
    pub created: Vec<ClipId>,
    pub deleted: Vec<ClipId>,
    /// Content of new clips or clips whose content kind changed in this operation.
    pub clip_contents: Vec<(ClipId, ClipContent)>,
    /// Exact source range and identity before a split, including intermediate splits.
    pub split_parent: Option<Clip>,
    /// The actual children produced by that operation, before later batch edits.
    pub split_results: Vec<Clip>,
}
