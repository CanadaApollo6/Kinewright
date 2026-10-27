//! Frozen S0 record identities and S1 payloads. Later producers stay unsupported.
#![allow(clippy::missing_errors_doc)]
use crate::{bundle::BundleError, receipts::CommandId};
use kinewright_core::{Clip, ClipId, Operation};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt::Write as _};

macro_rules! identity {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let value = String::deserialize(d)?;
                if value.len() != 32
                    || !value
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(serde::de::Error::custom("invalid opaque identity"));
                }
                Ok(Self(value))
            }
        }
        impl $name {
            pub fn fresh() -> Result<Self, BundleError> {
                let mut bytes = [0_u8; 16];
                getrandom::fill(&mut bytes).map_err(|e| BundleError::Identity(e.to_string()))?;
                let mut encoded = String::with_capacity(32);
                for byte in bytes {
                    let _ = write!(encoded, "{byte:02x}");
                }
                Ok(Self(encoded))
            }
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}
identity!(ProjectId);
identity!(BranchId);
identity!(HeadId);
identity!(BatchId);
identity!(SessionId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    Document,
    History,
    BranchCheckpoint,
    BranchIndex,
    RootCheckpoint,
    SpecialistBase,
    HistoryFloor,
    RecoveryClose,
    Lineage,
    StoreManifest,
    InputManifest,
    InputChunkTable,
    OwnedMedia,
    OwnedFont,
    OwnedNotice,
    OwnedLut,
    OwnedScript,
    OwnedData,
    OwnedStyle,
    OwnedReference,
    OwnedEvidence,
    ReceiptIndex,
    IssuerState,
    CreationBrief,
    VersionIndex,
    FeedbackIndex,
    FeedbackRevision,
    FeedbackBatch,
    RunIndex,
    RunSnapshot,
    RunEventSegment,
    RunEffect,
    RunRequest,
    RunDelivery,
    RunResult,
    RunUsage,
    ClaimIndex,
    TakeMap,
    TakeEvidence,
    Code,
    External,
    PersonMatte,
}
impl RecordKind {
    #[must_use]
    pub const fn supported(self) -> bool {
        matches!(
            self,
            Self::Document
                | Self::History
                | Self::BranchCheckpoint
                | Self::BranchIndex
                | Self::RootCheckpoint
                | Self::Lineage
                | Self::SpecialistBase
                | Self::HistoryFloor
                | Self::RecoveryClose
                | Self::StoreManifest
                | Self::InputManifest
                | Self::InputChunkTable
                | Self::ReceiptIndex
                | Self::IssuerState
                | Self::VersionIndex
                | Self::FeedbackIndex
                | Self::FeedbackRevision
                | Self::FeedbackBatch
                | Self::RunIndex
                | Self::RunSnapshot
                | Self::RunEventSegment
                | Self::RunEffect
                | Self::RunRequest
                | Self::RunDelivery
                | Self::RunResult
                | Self::RunUsage
                | Self::OwnedMedia
                | Self::OwnedFont
                | Self::OwnedNotice
                | Self::OwnedLut
                | Self::OwnedScript
                | Self::OwnedData
                | Self::OwnedStyle
                | Self::OwnedReference
                | Self::OwnedEvidence
        )
    }
    #[must_use]
    pub const fn is_owned_bytes(self) -> bool {
        self.is_blob() && !matches!(self, Self::RootCheckpoint)
    }
    #[must_use]
    pub const fn is_blob(self) -> bool {
        matches!(
            self,
            Self::RootCheckpoint
                | Self::OwnedMedia
                | Self::OwnedFont
                | Self::OwnedNotice
                | Self::OwnedLut
                | Self::OwnedScript
                | Self::OwnedData
                | Self::OwnedStyle
                | Self::OwnedReference
                | Self::OwnedEvidence
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectRef {
    pub sha256: String,
    pub byte_length: u64,
    pub record_kind: RecordKind,
}
impl ObjectRef {
    pub fn validate(&self) -> Result<(), BundleError> {
        if self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(BundleError::Corrupt("invalid SHA-256 identity".into()));
        }
        if !self.record_kind.supported() {
            return Err(BundleError::Unsupported(format!(
                "record kind {:?}",
                self.record_kind
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchCheckpoint {
    pub branch_id: BranchId,
    pub head_id: HeadId,
    pub document_ref: ObjectRef,
    pub document_hash: String,
    pub baseline_ref: ObjectRef,
    pub history_ref: ObjectRef,
    pub lineage_ref: ObjectRef,
    pub input_manifest_ref: ObjectRef,
    /// Input context at each accepted command, including undone/recoverable heads.
    pub history_input_manifest_refs: Vec<ObjectRef>,
    pub batch_ids: Vec<BatchId>,
    pub applied_operations_digest: String,
    pub checkpoint_generation: u64,
    pub floor_generation: u64,
    pub command_cut: Option<CommandId>,
    pub predecessor: Option<HeadId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take_map_revision_ref: Option<ObjectRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecialistBaseRecord {
    pub target_branch: BranchId,
    pub base_head: HeadId,
    pub base_document_hash: String,
    pub original_baseline_ref: ObjectRef,
    pub frozen_checkpoint_ref: ObjectRef,
    pub full_applied_delta: Vec<Operation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryFloorCheckpoint {
    pub preceding_checkpoint_generation: u64,
    pub preceding_floor_generation: u64,
    pub floor_generation: u64,
    pub command_cut: Option<CommandId>,
    pub old_baseline_ref: ObjectRef,
    pub new_baseline_ref: ObjectRef,
    pub checkpoint_ref: ObjectRef,
    pub equivalence_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryCloseCheckpoint {
    pub preceding_checkpoint_ref: ObjectRef,
    pub reconciled_cut: Option<CommandId>,
    pub suffix_digest: String,
    pub closed_checkpoint_ref: ObjectRef,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OccurrenceId {
    pub branch_origin: BranchId,
    pub creation_batch_id: BatchId,
    pub result_ordinal: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineageRecord {
    pub branch_origin: BranchId,
    pub baseline: BTreeMap<ClipId, OccurrenceId>,
    pub current: BTreeMap<ClipId, OccurrenceId>,
    pub undo: Vec<BTreeMap<ClipId, OccurrenceId>>,
    pub redo: Vec<BTreeMap<ClipId, OccurrenceId>>,
    /// Child occurrence, parent occurrence, exact child range from Core application.
    pub split_ancestry: Vec<(OccurrenceId, OccurrenceId, Clip)>,
}
