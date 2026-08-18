use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How a three-point source selection is committed to the target track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThreePointMode {
    /// Open time at the record point, honoring cross-track sync locks.
    Insert,
    /// Replace only the selected range on the target track.
    Overwrite,
}
