//! Bounded rescue provenance. Historical values never confer write authority.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::{BundleError, ProjectBundle},
    bundle_records::{ObjectRef, ProjectId, RecordKind, SessionId},
    receipts::{CommandDigest, CommandId, ReceiptLookup},
    storage_budget::StorageLimits,
    store::{BundleRecord, StoreManifest},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const RESCUE_ROLE: &str = "rescue_copy_v1";
pub const PROVENANCE_BYTES: usize = 65_536;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RescueAttemptProvenance {
    pub session_id: SessionId,
    pub project_id: ProjectId,
    pub destination: std::path::PathBuf,
    pub parent_device: u64,
    pub parent_inode: u64,
    pub attempted_root_sha256: String,
    pub pending_command: Option<(CommandId, CommandDigest)>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryOmission {
    pub role: String,
    pub root: ObjectRef,
    pub operation: String,
    pub cause: String,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvenanceSummary {
    pub count: u64,
    pub count_overflow: bool,
    pub digest: String,
    pub unresolved: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RescueRecoveryPoint {
    pub source_root: ObjectRef,
    pub rebound: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RescueRecoveryRetirement {
    pub source_root: ObjectRef,
    pub reason: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RescueCopyV1 {
    pub role: String,
    pub schema: u32,
    pub project_id: ProjectId,
    pub originating_session: SessionId,
    pub full_capture_sha256: String,
    pub selected_capture_sha256: String,
    pub old_floor: u64,
    pub new_floor: u64,
    pub removed_undo: u64,
    pub redo_preserved: bool,
    pub gesture_preserved: bool,
    pub acknowledged_root: Option<ObjectRef>,
    pub acknowledged_absence: Option<String>,
    pub recovery_origin: Option<RescueRecoveryPoint>,
    pub omissions: Vec<RecoveryOmission>,
    pub attempts: Vec<RescueAttemptProvenance>,
    pub summary: ProvenanceSummary,
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl RescueRecoveryPoint {
    #[must_use]
    pub const fn label(&self) -> &'static str {
        if self.rebound {
            "rebound recovery point"
        } else {
            "source's last acknowledged recovery point"
        }
    }
}
impl RescueCopyV1 {
    fn reserved_provenance_bytes(&self) -> Result<usize, BundleError> {
        let encode = |value: &Self| {
            serde_json::to_vec(value).map_err(|e| BundleError::Corrupt(e.to_string()))
        };
        let mut bounded = self.clone();
        bounded.summary.count = u64::MAX;
        bounded.summary.digest = "f".repeat(64);
        bounded.summary.count_overflow = false;
        bounded.summary.unresolved = false;
        Ok(encode(&bounded)?.len().max(encode(self)?.len()))
    }
    /// Current entries and current omissions are exact. Only inherited attempts compact.
    pub fn compact(
        &mut self,
        inherited: impl IntoIterator<Item = RescueAttemptProvenance>,
    ) -> Result<(), BundleError> {
        self.validate(&StorageLimits::default())?;
        for attempt in inherited {
            let encoded =
                serde_json::to_vec(&attempt).map_err(|e| BundleError::Corrupt(e.to_string()))?;
            self.attempts.push(attempt);
            if self.attempts.len() <= 16 && self.reserved_provenance_bytes()? <= PROVENANCE_BYTES {
                continue;
            }
            self.attempts.pop();
            let (count, overflow) = self.summary.count.overflowing_add(1);
            self.summary.count = if overflow { u64::MAX } else { count };
            self.summary.count_overflow |= overflow;
            self.summary.unresolved = true;
            self.summary.digest = kinewright_media::sha256_bytes(
                &[self.summary.digest.as_bytes(), &encoded].concat(),
            );
        }
        self.validate(&StorageLimits::default())
    }
    #[must_use]
    pub fn lookup(&self, id: &CommandId, digest: &CommandDigest) -> Option<ReceiptLookup> {
        self.attempts
            .iter()
            .filter_map(|a| a.pending_command.as_ref())
            .find(|(command, _)| command == id)
            .map(|(_, expected)| {
                if expected == digest {
                    ReceiptLookup::UnresolvedOrigin
                } else {
                    ReceiptLookup::IdPayloadConflict
                }
            })
    }
}
impl crate::store::record_sealed::Sealed for RescueCopyV1 {}
impl BundleRecord for RescueCopyV1 {
    const KIND: RecordKind = RecordKind::OwnedEvidence;
    fn dependencies(&self) -> Vec<ObjectRef> {
        self.acknowledged_root.iter().cloned().collect()
    }
    fn validate(&self, _: &StorageLimits) -> Result<(), BundleError> {
        if self.role != RESCUE_ROLE || self.schema != 1 {
            return Err(BundleError::Unsupported("rescue role/schema".into()));
        }
        if !hash(&self.full_capture_sha256)
            || !hash(&self.selected_capture_sha256)
            || self.new_floor < self.old_floor
            || !self.redo_preserved
            || !self.gesture_preserved
            || self.acknowledged_root.is_some() == self.acknowledged_absence.is_some()
            || self.attempts.len() > 16
            || self
                .attempts
                .iter()
                .filter(|a| a.session_id == self.originating_session)
                .count()
                > 4
            || (!self.summary.digest.is_empty() && !hash(&self.summary.digest))
            || ((self.summary.count > 0 || self.summary.count_overflow)
                && (!self.summary.unresolved || !hash(&self.summary.digest)))
            || (self.summary.count_overflow && self.summary.count != u64::MAX)
        {
            return Err(BundleError::Corrupt("rescue capture/provenance".into()));
        }
        for root in self
            .acknowledged_root
            .iter()
            .chain(self.omissions.iter().map(|o| &o.root))
            .chain(self.recovery_origin.iter().map(|o| &o.source_root))
        {
            root.validate()?;
            if root.record_kind != RecordKind::RootCheckpoint {
                return Err(BundleError::Corrupt("rescue recovery kind".into()));
            }
        }
        for attempt in &self.attempts {
            if !hash(&attempt.attempted_root_sha256) {
                return Err(BundleError::Corrupt("rescue attempt root".into()));
            }
        }
        if self.reserved_provenance_bytes()? > PROVENANCE_BYTES {
            return Err(BundleError::Capacity("rescue provenance"));
        }
        Ok(())
    }
}

pub(crate) fn required_apis(switch_generation: u64, rescue: bool) -> BTreeMap<String, u32> {
    let mut apis = crate::bundle_recovery::base_apis();
    if switch_generation != 0 {
        apis.insert("manual-switch".into(), 1);
    }
    if rescue {
        apis.insert("rescue-copy".into(), 1);
    }
    apis
}
pub(crate) fn check_apis(
    declared: &BTreeMap<String, u32>,
    required: &BTreeMap<String, u32>,
) -> Result<(), BundleError> {
    let supported = crate::bundle_recovery::supported_apis();
    if declared.iter().any(|(k, v)| supported.get(k) != Some(v))
        || required.iter().any(|(k, v)| declared.get(k) != Some(v))
    {
        return Err(BundleError::Unsupported("required API inventory".into()));
    }
    Ok(())
}
impl ProjectBundle {
    pub fn retain_rescue_record(
        &mut self,
        record: &RescueCopyV1,
    ) -> Result<ObjectRef, BundleError> {
        if record.project_id != self.project_id {
            return Err(BundleError::Identity("rescue owner".into()));
        }
        let sealed = self.store_mut()?.admit_record(record)?;
        let reference = sealed.reference.clone();
        self.evidence.push(reference.clone());
        self.pending_evidence.push(sealed.into_pin());
        Ok(reference)
    }
    pub fn rescue_records(&self) -> Result<Vec<RescueCopyV1>, BundleError> {
        self.evidence
            .iter()
            .filter(|r| {
                r.record_kind == RecordKind::OwnedEvidence
                    && r.byte_length <= PROVENANCE_BYTES as u64
            })
            .filter_map(|r| {
                let result = self.store.read_evidence(r).and_then(|bytes| parse(&bytes));
                match result {
                    Ok(None) => None,
                    Ok(Some(record)) => Some(Ok(record)),
                    Err(e) => Some(Err(e)),
                }
            })
            .collect()
    }
}
pub(crate) fn parse(bytes: &[u8]) -> Result<Option<RescueCopyV1>, BundleError> {
    let Ok(raw) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return Ok(None);
    };
    if raw.get("role").is_none() {
        return Ok(None);
    }
    let record: RescueCopyV1 =
        serde_json::from_value(raw).map_err(|e| BundleError::Unsupported(e.to_string()))?;
    record.validate(&StorageLimits::default())?;
    Ok(Some(record))
}
pub(crate) fn validate_attachment(
    reference: &ObjectRef,
    record: Option<&RescueCopyV1>,
    manifest: &StoreManifest,
) -> Result<(), BundleError> {
    let role = manifest.evidence_roles.get(&reference.sha256);
    if role.map(String::as_str) != record.map(|r| r.role.as_str()) {
        return Err(BundleError::Unsupported("rescue attachment role".into()));
    }
    if let Some(record) = record {
        let mut expected = record.dependencies();
        expected.sort();
        expected.dedup();
        let mut actual = manifest
            .dependencies
            .get(&reference.sha256)
            .cloned()
            .unwrap_or_default();
        actual.sort();
        actual.dedup();
        if expected != actual {
            return Err(BundleError::Corrupt("rescue record edges".into()));
        }
    }
    Ok(())
}
