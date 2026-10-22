//! Raw support admission and the single durable bundle reader/writer.
#![allow(clippy::missing_errors_doc)]
use std::{fmt, io};

#[derive(Debug)]
pub enum BundleError {
    Io(io::Error),
    Identity(String),
    Contention(String),
    DurabilityUnsupported,
    DurabilityUnknown,
    DurabilityUnknownAt(PathBuf),
    HistoryChanged,
    SaveInProgress,
    HistoryBarrierInvariant,
    HistoryCommitInvariant,
    Capacity(&'static str),
    Unsupported(String),
    Corrupt(String),
    Unsettled,
}
impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => e.fmt(f),
            Self::Identity(e) => write!(f, "identity_refused: {e}"),
            Self::Contention(e) => write!(f, "contention: {e}"),
            Self::DurabilityUnsupported => f.write_str("durability_unsupported"),
            Self::DurabilityUnknown => f.write_str("durability_unknown"),
            Self::DurabilityUnknownAt(path) => write!(
                f,
                "durability_unknown: {}; retry when the destination is readable",
                path.display()
            ),
            Self::HistoryChanged => f.write_str("history_changed: retry the save"),
            Self::SaveInProgress => f.write_str("save_in_progress: retry after the current save"),
            Self::HistoryBarrierInvariant => f.write_str("internal_history_barrier_invariant"),
            Self::HistoryCommitInvariant => {
                f.write_str("internal_history_commit_invariant: ticket released")
            }
            Self::Capacity(e) => write!(f, "storage_backpressure: {e}"),
            Self::Unsupported(e) => write!(f, "unsupported_bundle: {e}"),
            Self::Corrupt(e) => write!(f, "damaged_bundle: {e}"),
            Self::Unsettled => f.write_str("publication requires reconciliation"),
        }
    }
}
impl std::error::Error for BundleError {}
impl From<io::Error> for BundleError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
#[cfg(target_os = "linux")]
impl From<rustix::io::Errno> for BundleError {
    fn from(e: rustix::io::Errno) -> Self {
        Self::Io(e.into())
    }
}

use crate::{
    bundle_records::{BranchId, HeadId, ObjectRef, RecordKind},
    project::{ProjectFile, ProjectSaveError},
    storage_budget::{StorageLimits, StorageReservation, StorageUsage},
};
use kinewright_core::{Document, PROJECT_FORMAT_VERSION};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupportInventory {
    pub required_fields: BTreeSet<String>,
    pub required_kinds: BTreeSet<RecordKind>,
    pub required_apis: BTreeMap<String, u32>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StudioRoot {
    pub project_id: crate::bundle_records::ProjectId,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub origin_project_ids: Vec<crate::bundle_records::ProjectId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub origin_session_ids: Vec<crate::bundle_records::SessionId>,
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_brief_ref: Option<ObjectRef>,
    pub manual_branch_id: BranchId,
    pub manual_switch_generation: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidate_branch_ids: Vec<BranchId>,
    pub branch_index_ref: ObjectRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_index_ref: Option<ObjectRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback_index_ref: Option<ObjectRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_index_ref: Option<ObjectRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_index_ref: Option<ObjectRef>,
    pub input_manifest_ref: ObjectRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_index_ref: Option<ObjectRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer_state_ref: Option<ObjectRef>,
    pub store_manifest_ref: ObjectRef,
}
impl StudioRoot {
    pub(crate) fn admit(&self) -> Result<(), BundleError> {
        if self
            .origin_project_ids
            .len()
            .checked_add(self.origin_session_ids.len())
            .ok_or(BundleError::Capacity("project origins overflow"))? as u64
            > StorageLimits::default().versions
            || self
                .origin_session_ids
                .iter()
                .collect::<BTreeSet<_>>()
                .len()
                != self.origin_session_ids.len()
            || self.origin_project_ids.contains(&self.project_id)
            || self
                .origin_project_ids
                .iter()
                .collect::<BTreeSet<_>>()
                .len()
                != self.origin_project_ids.len()
        {
            return Err(BundleError::Capacity("project origins"));
        }
        if self.generation == 0 || self.manual_switch_generation != 0 {
            return Err(BundleError::Unsupported(
                "unsupported Studio generation/switch state".into(),
            ));
        }
        if let Some(reference) = [
            self.creation_brief_ref.as_ref(),
            self.version_index_ref.as_ref(),
            self.feedback_index_ref.as_ref(),
            self.run_index_ref.as_ref(),
            self.claim_index_ref.as_ref(),
        ]
        .into_iter()
        .flatten()
        .next()
        {
            reference.validate()?;
            return Err(BundleError::Unsupported(
                "inactive Studio producer/index".into(),
            ));
        }
        for (reference, kind) in [
            (&self.branch_index_ref, RecordKind::BranchIndex),
            (&self.input_manifest_ref, RecordKind::InputManifest),
            (&self.store_manifest_ref, RecordKind::StoreManifest),
        ] {
            reference.validate()?;
            if reference.record_kind != kind {
                return Err(BundleError::Corrupt(
                    "Studio reference kind mismatch".into(),
                ));
            }
        }
        if self.receipt_index_ref.is_some() && self.issuer_state_ref.is_none() {
            return Err(BundleError::Corrupt(
                "receipt index without issuer state".into(),
            ));
        }
        for (reference, kind) in [
            (&self.receipt_index_ref, RecordKind::ReceiptIndex),
            (&self.issuer_state_ref, RecordKind::IssuerState),
        ] {
            if let Some(reference) = reference {
                reference.validate()?;
                if reference.record_kind != kind {
                    return Err(BundleError::Corrupt("receipt/issuer kind".into()));
                }
            }
        }
        Ok(())
    }
    pub(crate) fn references(&self) -> Vec<ObjectRef> {
        let mut refs = vec![
            self.branch_index_ref.clone(),
            self.input_manifest_ref.clone(),
            self.store_manifest_ref.clone(),
        ];
        refs.extend(self.receipt_index_ref.clone());
        refs.extend(self.issuer_state_ref.clone());
        refs
    }
}
pub(crate) struct AdmissionData {
    source: Option<PathBuf>,
    bytes: Vec<u8>,
    version: u32,
    studio_key: bool,
    pub(crate) file: Option<ProjectFile>,
    pub(crate) inventory: SupportInventory,
    refusal: Option<String>,
    pub(crate) usage: Arc<Mutex<StorageUsage>>,
    _holds: Vec<StorageReservation>,
}
/// Retained raw provenance. It cannot be reconstructed from a Document projection.
#[derive(Clone)]
pub struct BundleWriteAdmission {
    pub(crate) data: Arc<AdmissionData>,
}
impl std::fmt::Debug for BundleWriteAdmission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BundleWriteAdmission")
            .field("source", &self.data.source)
            .field("version", &self.data.version)
            .field("refusal", &self.data.refusal)
            .finish_non_exhaustive()
    }
}
impl PartialEq for BundleWriteAdmission {
    fn eq(&self, other: &Self) -> bool {
        self.data.source == other.data.source
            && self.data.bytes == other.data.bytes
            && self.data.refusal == other.data.refusal
    }
}
impl Eq for BundleWriteAdmission {}
impl Default for BundleWriteAdmission {
    fn default() -> Self {
        Self::new_document()
    }
}
impl BundleWriteAdmission {
    #[must_use]
    pub fn new_document() -> Self {
        Self {
            data: Arc::new(AdmissionData {
                source: None,
                bytes: vec![],
                version: 1,
                studio_key: false,
                file: None,
                inventory: SupportInventory::default(),
                refusal: None,
                usage: Arc::new(Mutex::new(StorageUsage::default())),
                _holds: vec![],
            }),
        }
    }
    pub fn from_written_legacy(path: &Path, bytes: &[u8]) -> Result<Self, ProjectSaveError> {
        let source = crate::canonical_project_identity(path)
            .map_err(|e| ProjectSaveError::Write(e.to_string()))?;
        Self::from_bytes(Some(source), bytes.to_vec()).map_err(|e| {
            ProjectSaveError::UnsupportedBundle {
                notice: e.to_string(),
            }
        })
    }
    pub fn read(path: &Path) -> Result<Self, BundleError> {
        let source = crate::canonical_project_identity(path)
            .map_err(|e| BundleError::Identity(e.to_string()))?;
        let mut file = File::open(&source)?;
        let limit = StorageLimits::default().control_bytes;
        let length = file.metadata()?.len();
        let read_limit = if length > limit {
            let mut probe =
                serde_json::Deserializer::from_reader(std::io::BufReader::new(&mut file));
            if !legacy_envelope(&mut probe)? {
                return Err(BundleError::Capacity("root envelope bytes"));
            }
            std::io::Seek::rewind(&mut file)?;
            length
        } else {
            limit
        };
        let bytes = crate::bundle_recovery::bounded_bytes(file, read_limit)?;
        Self::from_bytes(Some(source), bytes)
    }
    pub(crate) fn from_bytes(source: Option<PathBuf>, bytes: Vec<u8>) -> Result<Self, BundleError> {
        Self::from_bytes_using(source, bytes, Arc::new(Mutex::new(StorageUsage::default())))
    }
    #[allow(
        clippy::too_many_lines,
        reason = "one raw admission boundary preserves legacy behavior and applies Studio quotas"
    )]
    pub(crate) fn from_bytes_using(
        source: Option<PathBuf>,
        bytes: Vec<u8>,
        usage: Arc<Mutex<StorageUsage>>,
    ) -> Result<Self, BundleError> {
        let mut probe = serde_json::Deserializer::from_slice(&bytes);
        let legacy = legacy_envelope(&mut probe)?;
        let limits = StorageLimits::default();
        let holds = if legacy {
            // v1/v2 keep their pre-S1 size contract. Studio quotas apply only to Studio roots.
            let mut scan_limits = limits.clone();
            scan_limits.decoded_record_bytes = u64::MAX;
            crate::store::preflight_record(&bytes, &scan_limits)?;
            vec![]
        } else {
            let encoded = StorageReservation::reserve(
                &usage,
                &limits,
                0,
                bytes.len() as u64,
                0,
                false,
                u64::MAX,
            )?;
            let preflight = StorageReservation::reserve(
                &usage,
                &limits,
                0,
                limits.decoded_record_bytes,
                0,
                false,
                u64::MAX,
            )?;
            let decoded_charge = crate::store::preflight_record(&bytes, &limits)?;
            drop(preflight);
            let value_charge = StorageReservation::reserve(
                &usage,
                &limits,
                0,
                decoded_charge,
                0,
                false,
                u64::MAX,
            )?;
            vec![encoded, value_charge]
        };
        let mut raw: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let version = kinewright_core::project_format_version(&bytes);
        let object = raw
            .as_object_mut()
            .ok_or_else(|| BundleError::Corrupt("root is not an object".into()))?;
        let studio_key = object.contains_key("studio");
        object.remove("format_version");
        let studio_raw = object.remove("studio");
        let mut inventory = SupportInventory::default();
        let mut refusal = if version > PROJECT_FORMAT_VERSION {
            Some(format!(
                "project format {version} exceeds supported {PROJECT_FORMAT_VERSION}"
            ))
        } else {
            None
        };
        match kinewright_core::document_support_fields(&raw) {
            Ok(fields) => inventory.required_fields = fields,
            Err(error) => {
                refusal.get_or_insert(error);
            }
        }
        if let Err(error) = crate::bundle_recovery::support_semantics(&raw) {
            refusal.get_or_insert(error.to_string());
        }
        let document: Option<Document> = serde_json::from_value(raw).ok();
        let studio = match studio_raw {
            None | Some(serde_json::Value::Null) => None,
            Some(raw) => match serde_json::from_value::<StudioRoot>(raw) {
                Ok(studio) => {
                    if let Err(error) = studio.admit() {
                        refusal.get_or_insert(error.to_string());
                    }
                    Some(studio)
                }
                Err(error) => {
                    refusal.get_or_insert(error.to_string());
                    None
                }
            },
        };
        if studio_key && (studio.is_none() || version != 3) || version == 3 && studio.is_none() {
            refusal.get_or_insert("Studio requires a complete format-3 envelope".into());
        }
        if let Some(document) = &document
            && let Err(error) = document.validate()
        {
            refusal.get_or_insert(error.to_string());
        }
        let file = document.map(|document| ProjectFile {
            format_version: version,
            document,
            studio,
        });
        if file.is_none() {
            refusal.get_or_insert("unreadable Document projection".into());
        }
        Ok(Self {
            data: Arc::new(AdmissionData {
                source,
                bytes,
                version,
                studio_key,
                file,
                inventory,
                refusal,
                usage,
                _holds: holds,
            }),
        })
    }
    #[must_use]
    pub fn source(&self) -> Option<&Path> {
        self.data.source.as_deref()
    }
    #[must_use]
    pub fn read_version(&self) -> u32 {
        self.data.version
    }
    #[must_use]
    pub fn is_studio(&self) -> bool {
        self.data.studio_key
    }
    #[must_use]
    pub fn raw_bytes(&self) -> &[u8] {
        &self.data.bytes
    }
    #[must_use]
    pub fn refusal(&self) -> Option<&str> {
        self.data.refusal.as_deref()
    }
    pub fn require_supported(&self) -> Result<(), BundleError> {
        if let Some(reason) = self.refusal() {
            return Err(BundleError::Unsupported(reason.into()));
        }
        Ok(())
    }
    pub fn legacy_document(&self) -> Result<Document, BundleError> {
        self.require_supported()?;
        if self.is_studio() {
            return Err(BundleError::Unsupported(
                "Document-only IO cannot preserve Studio state".into(),
            ));
        }
        self.inspection_document()
            .cloned()
            .ok_or_else(|| BundleError::Corrupt("no source Document".into()))
    }
    #[must_use]
    pub fn inspection_document(&self) -> Option<&Document> {
        self.data.file.as_ref().map(|file| &file.document)
    }
    pub fn authorize_legacy_write(&self, destination: &Path) -> Result<(), ProjectSaveError> {
        self.require_supported()
            .map_err(|e| ProjectSaveError::UnsupportedBundle {
                notice: e.to_string(),
            })?;
        if self.is_studio() {
            #[cfg(windows)]
            let notice = ProjectNamespace::acquire(destination)
                .err()
                .unwrap_or(BundleError::DurabilityUnsupported)
                .to_string();
            #[cfg(not(windows))]
            let notice = "Document-only Save/Save As cannot preserve Studio state".into();
            return Err(ProjectSaveError::UnsupportedBundle { notice });
        }
        if let Some(source) = self.source() {
            refuse_activation_journal(source)?;
            let current = Self::read(source).map_err(|e| ProjectSaveError::Write(e.to_string()))?;
            if current.raw_bytes() != self.raw_bytes() {
                return Err(ProjectSaveError::UnsupportedBundle {
                    notice: "source changed since admission".into(),
                });
            }
        }
        refuse_activation_journal(destination)?;
        match std::fs::symlink_metadata(destination) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(ProjectSaveError::Write(error.to_string())),
            Ok(metadata) if !metadata.is_file() && !metadata.file_type().is_symlink() => {
                return Err(ProjectSaveError::Write(
                    "destination is not a regular project file".into(),
                ));
            }
            Ok(_) => {}
        }
        {
            let current =
                Self::read(destination).map_err(|e| ProjectSaveError::Write(e.to_string()))?;
            current
                .require_supported()
                .map_err(|e| ProjectSaveError::UnsupportedBundle {
                    notice: e.to_string(),
                })?;
            if current.is_studio() {
                return Err(ProjectSaveError::UnsupportedBundle {
                    notice: "Document-only IO cannot replace a Studio destination".into(),
                });
            }
        }
        Ok(())
    }
}
fn legacy_envelope<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, BundleError> {
    use serde::de::{IgnoredAny, MapAccess, Visitor};
    struct Probe;
    impl<'de> Visitor<'de> for Probe {
        type Value = bool;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("project envelope")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<bool, A::Error> {
            let mut version = 1_u32;
            let mut studio = false;
            while let Some(key) = map.next_key::<String>()? {
                if key == "format_version" {
                    version = map.next_value()?;
                } else {
                    studio |= key == "studio";
                    map.next_value::<IgnoredAny>()?;
                }
            }
            Ok(version <= 2 && !studio)
        }
    }
    deserializer
        .deserialize_map(Probe)
        .map_err(|e| BundleError::Corrupt(e.to_string()))
}

fn refuse_activation_journal(path: &Path) -> Result<(), ProjectSaveError> {
    let path = crate::canonical_project_identity(path)
        .map_err(|e| ProjectSaveError::Write(e.to_string()))?;
    if path
        .with_extension("kinewright-assets")
        .join("studio/recovery/inventory")
        .exists()
    {
        return Err(ProjectSaveError::UnsupportedBundle {
            notice: "Studio activation is pending; reconcile it before a legacy save".into(),
        });
    }
    Ok(())
}
#[derive(Debug, Clone)]
pub struct BundleSaveReport {
    pub collection: crate::store::CollectionStatus,
    pub path: PathBuf,
    pub project_id: crate::bundle_records::ProjectId,
    pub generation: u64,
    pub head_id: HeadId,
    pub root_sha256: String,
    pub byte_length: u64,
}

use crate::{
    bundle_records::{BranchCheckpoint, LineageRecord, ProjectId},
    namespace::ProjectNamespace,
    store::{ProjectStore, RecoveryInventory, SealedObject, StoreManifest, StorePin},
};
use kinewright_core::{Core, HistorySnapshot};

/// Fully verified, read-only graph. Claiming is a separate consuming operation.
pub(crate) type LoadedBranch = (
    StorageReservation<BranchCheckpoint>,
    Arc<StorageReservation<HistorySnapshot>>,
    Arc<StorageReservation<LineageRecord>>,
);

pub struct LoadedBundle {
    pub(crate) admission: BundleWriteAdmission,
    pub(crate) namespace: ProjectNamespace,
    pub(crate) manifest: StorageReservation<StoreManifest>,
    pub(crate) branches: BTreeMap<BranchId, LoadedBranch>,
    pub(crate) verified: BTreeSet<String>,
    pub(crate) inventory: SupportInventory,
    pub(crate) receipts: Option<StorageReservation<crate::receipts::ReceiptLedger>>,
    pub(crate) issuers: Option<
        StorageReservation<BTreeMap<crate::receipts::IssuerId, crate::receipts::IssuerState>>,
    >,
}
impl LoadedBundle {
    pub fn read(path: &Path) -> Result<Self, BundleError> {
        crate::bundle_recovery::read(path)
    }
    #[must_use]
    pub fn admission(&self) -> &BundleWriteAdmission {
        &self.admission
    }
    pub fn root(&self) -> Result<&StudioRoot, BundleError> {
        self.admission
            .data
            .file
            .as_ref()
            .and_then(|f| f.studio.as_ref())
            .ok_or_else(|| BundleError::Corrupt("missing Studio root".into()))
    }
    pub fn manual_checkpoint(&self) -> Result<&BranchCheckpoint, BundleError> {
        self.branches
            .get(&self.root()?.manual_branch_id)
            .map(|b| &*b.0)
            .ok_or_else(|| BundleError::Corrupt("missing manual branch".into()))
    }
    /// Replays through Core only after the complete graph and support set were checked.
    pub fn restore_manual(&self, core: &Core) -> Result<(), BundleError> {
        reconcile_core_save(core)?;
        let branch = self
            .branches
            .get(&self.root()?.manual_branch_id)
            .ok_or_else(|| BundleError::Corrupt("missing manual branch".into()))?;
        core.restore_history((**branch.1).clone())
            .map_err(|e| BundleError::Corrupt(e.to_string()))
    }
    pub fn into_writer(self) -> Result<ProjectBundle, BundleError> {
        self.claim_writer(StorageLimits::default())
    }
    #[cfg(any(test, feature = "test-util"))]
    pub fn into_writer_with_limits_probe(
        self,
        limits: StorageLimits,
    ) -> Result<ProjectBundle, BundleError> {
        self.claim_writer(limits)
    }
    fn claim_writer(mut self, limits: StorageLimits) -> Result<ProjectBundle, BundleError> {
        let root = self.root()?.clone();
        self.namespace.claim()?;
        if self.namespace.read_root()?.as_deref() != Some(self.admission.raw_bytes()) {
            return Err(BundleError::Identity(
                "root changed between validation and claim".into(),
            ));
        }
        self.namespace.reconcile(self.admission.raw_bytes())?;
        let store = ProjectStore::from_verified(
            self.namespace,
            root.project_id.clone(),
            &self.manifest,
            &root.store_manifest_ref,
            &self.verified,
            Arc::clone(&self.admission.data.usage),
            limits,
        )?;
        let history_cache = self
            .branches
            .iter()
            .map(|(id, branch)| {
                (
                    id.clone(),
                    CachedHistory {
                        reference: branch.0.history_ref.clone(),
                        history: Arc::clone(&branch.1),
                    },
                )
            })
            .collect();
        let branches = self
            .branches
            .into_iter()
            .map(|(id, b)| (id, (*b.0).clone()))
            .collect();
        let mut bundle = ProjectBundle {
            store,
            project_id: root.project_id.clone(),
            origin_project_ids: root.origin_project_ids.clone(),
            origin_session_ids: root.origin_session_ids.clone(),
            manual_branch_id: root.manual_branch_id.clone(),
            generation: root.generation,
            branches,
            history_cache,
            root: Some(root),
            admission: self.admission,
            inventory: self.inventory,
            recovery: self.manifest.recovery.clone(),
            evidence: self.manifest.durable_pins.clone(),
            pending_evidence: vec![],
            durable: None,
            unsettled: false,
            receipts: self
                .receipts
                .as_ref()
                .map_or_else(crate::receipts::ReceiptLedger::default, |v| (**v).clone()),
            issuers: self
                .issuers
                .as_ref()
                .map_or_else(BTreeMap::new, |v| (**v).clone()),
            pending_command: None,
            pending_result: None,
            last_collection_error: None,
            pending_save: None,
            pending_copy_source: None,
            publication_tag: None,
        };
        bundle.durable = Some(bundle.store.mark_committed(&bundle.root_references()?)?);
        Ok(bundle)
    }
}

/// A verified history bound to the immutable checkpoint object it came from.
pub(crate) struct CachedHistory {
    pub(crate) reference: ObjectRef,
    pub(crate) history: Arc<StorageReservation<HistorySnapshot>>,
}

/// One synchronous named-bundle owner. It accepts frozen Core exports, never live snapshots assembled piecemeal.
pub struct ProjectBundle {
    pub(crate) publication_tag: Option<kinewright_core::HistorySaveTag>,
    pub(crate) pending_save: Option<PendingSave>,
    pub(crate) pending_copy_source: Option<crate::bundle_copy::PendingCopySource>,
    pub(crate) last_collection_error: Option<String>,
    pub(crate) store: ProjectStore,
    pub(crate) project_id: ProjectId,
    pub(crate) origin_project_ids: Vec<ProjectId>,
    pub(crate) origin_session_ids: Vec<crate::bundle_records::SessionId>,
    pub(crate) manual_branch_id: BranchId,
    pub(crate) generation: u64,
    pub(crate) branches: BTreeMap<BranchId, BranchCheckpoint>,
    pub(crate) history_cache: BTreeMap<BranchId, CachedHistory>,
    pub(crate) root: Option<StudioRoot>,
    pub(crate) admission: BundleWriteAdmission,
    pub(crate) inventory: SupportInventory,
    pub(crate) recovery: RecoveryInventory,
    pub(crate) durable: Option<StorePin>,
    pub(crate) evidence: Vec<ObjectRef>,
    pub(crate) pending_evidence: Vec<StorePin>,
    pub(crate) unsettled: bool,
    pub(crate) receipts: crate::receipts::ReceiptLedger,
    pub(crate) issuers: BTreeMap<crate::receipts::IssuerId, crate::receipts::IssuerState>,
    pub(crate) pending_command: Option<StorageReservation<crate::receipts::CommandEnvelope>>,
    pub(crate) pending_result: Option<crate::receipts::ReceiptResult>,
}
pub(crate) struct PendingSave {
    pub(crate) core: Core,
    pub(crate) barrier: Option<crate::history_save::FloorBarrier>,
    pub(crate) root_sha256: Option<String>,
}
impl ProjectBundle {
    /// Explicit activation. Existing Studio state is resumed without minting another manual branch.
    #[allow(
        clippy::too_many_lines,
        reason = "activation validates and protects the original under one authority"
    )]
    pub fn activate(path: &Path, admission: BundleWriteAdmission) -> Result<Self, BundleError> {
        Self::activate_checked(path, admission, None)
    }
    pub(crate) fn activate_copy(
        path: &Path,
        admission: BundleWriteAdmission,
        prepared: &crate::namespace::PreparedDestination,
    ) -> Result<Self, BundleError> {
        Self::activate_checked(path, admission, Some(prepared))
    }
    #[allow(
        clippy::too_many_lines,
        reason = "activation binds support, namespace identity and exact migration backup before publication"
    )]
    fn activate_checked(
        path: &Path,
        admission: BundleWriteAdmission,
        prepared: Option<&crate::namespace::PreparedDestination>,
    ) -> Result<Self, BundleError> {
        admission.require_supported()?;
        if admission.is_studio() {
            let mut loaded = LoadedBundle::read(path)?;
            if loaded.admission.raw_bytes() != admission.raw_bytes() {
                return Err(BundleError::Identity("activation source changed".into()));
            }
            if let Some(prepared) = prepared {
                loaded.namespace.matches_destination(prepared)?;
                loaded.namespace.claim()?;
                loaded.namespace.matches_destination(prepared)?;
            }
            return loaded.into_writer();
        }
        #[cfg(not(target_os = "linux"))]
        {
            drop(admission);
            let _ = ProjectNamespace::acquire(path)?;
            Err(BundleError::DurabilityUnsupported)
        }
        #[cfg(target_os = "linux")]
        Self::activate_linux(path, admission, prepared)
    }

    #[cfg(target_os = "linux")]
    #[allow(clippy::too_many_lines)]
    fn activate_linux(
        path: &Path,
        admission: BundleWriteAdmission,
        prepared: Option<&crate::namespace::PreparedDestination>,
    ) -> Result<Self, BundleError> {
        {
            let mut namespace = ProjectNamespace::inspect(path)?;
            if let Some(prepared) = prepared {
                namespace.matches_destination(prepared)?;
                if namespace.has_asset_store()? {
                    return Err(BundleError::Identity(
                        "unbound foreign destination store".into(),
                    ));
                }
            }
            let original = namespace.read_root()?;
            if original.as_deref() != admission.source().map(|_| admission.raw_bytes()) {
                return Err(BundleError::Identity(
                    "activation source differs from admitted bytes".into(),
                ));
            }
            namespace.claim()?;
            if let Some(prepared) = prepared {
                namespace.matches_destination(prepared)?;
                if namespace.has_asset_store()? {
                    return Err(BundleError::Identity(
                        "destination store appeared during claim".into(),
                    ));
                }
            }
            if namespace.read_root()? != original {
                return Err(BundleError::Identity(
                    "activation changed during claim".into(),
                ));
            }
            let limits = StorageLimits::default();
            let inventory_path = crate::bundle_recovery::inventory_path()?;
            let prior = namespace.read(&inventory_path, limits.control_bytes)?;
            let mut recovery: RecoveryInventory = match prior {
                Some(bytes) => {
                    crate::store::preflight_record(&bytes, &limits)?;
                    serde_json::from_slice(&bytes)
                        .map_err(|e| BundleError::Corrupt(e.to_string()))?
                }
                None => RecoveryInventory::default(),
            };
            recovery.validate()?;
            let project_id = recovery
                .activation_project_id
                .clone()
                .map_or_else(ProjectId::fresh, Ok)?;
            let manual_branch_id = recovery
                .manual_branch_id
                .clone()
                .map_or_else(BranchId::fresh, Ok)?;
            let root_name = namespace
                .identity()
                .resolved_root
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| BundleError::Identity("root filename".into()))?
                .to_owned();
            if recovery
                .namespace_root_file_name
                .as_ref()
                .is_some_and(|n| n != &root_name)
            {
                return Err(BundleError::Identity(
                    "activation journal belongs to another root".into(),
                ));
            }
            recovery.activation_project_id = Some(project_id.clone());
            recovery.manual_branch_id = Some(manual_branch_id.clone());
            recovery.namespace_root_file_name = Some(root_name);
            let mut store = ProjectStore::new(namespace, Some(project_id.clone()), None, limits)?;
            store.adopt_usage(Arc::clone(&admission.data.usage))?;
            store.account_residuals(false)?;
            let mut durable = None;
            if let Some(original) = original {
                let reference = ObjectRef {
                    sha256: kinewright_media::sha256_bytes(&original),
                    byte_length: original.len() as u64,
                    record_kind: RecordKind::RootCheckpoint,
                };
                if recovery
                    .pre_migration
                    .as_ref()
                    .is_some_and(|r| r != &reference)
                {
                    return Err(BundleError::Corrupt(
                        "activation backup differs from exact original".into(),
                    ));
                }
                let sealed = store.admit_recovered(&mut original.as_slice(), &reference, &[])?;
                store.verify(&reference)?;
                recovery.pre_migration = Some(reference.clone());
                // The immutable backup has an independent logical pin even when B3 shares its bytes.
                durable = Some(sealed.into_pin());
            }
            store.publish_inventory(&recovery)?;
            Ok(Self {
                store,
                project_id,
                origin_project_ids: vec![],
                origin_session_ids: vec![],
                manual_branch_id,
                generation: 0,
                branches: BTreeMap::new(),
                history_cache: BTreeMap::new(),
                root: None,
                admission,
                inventory: SupportInventory::default(),
                recovery,
                durable,
                evidence: vec![],
                pending_evidence: vec![],
                unsettled: false,
                receipts: crate::receipts::ReceiptLedger::default(),
                issuers: BTreeMap::new(),
                pending_command: None,
                pending_result: None,
                last_collection_error: None,
                pending_save: None,
                pending_copy_source: None,
                publication_tag: None,
            })
        }
    }
    pub(crate) fn cached_history(
        &self,
        branch: &BranchId,
    ) -> Option<&Arc<StorageReservation<HistorySnapshot>>> {
        let current = &self.branches.get(branch)?.history_ref;
        self.history_cache
            .get(branch)
            .filter(|cached| &cached.reference == current)
            .map(|cached| &cached.history)
    }
    #[cfg(any(test, feature = "test-util"))]
    pub fn invalidate_history_cache_binding_probe(&mut self) {
        if let Some(cached) = self.history_cache.get_mut(&self.manual_branch_id) {
            cached.reference.sha256 = "0".repeat(64);
        }
    }
    #[must_use]
    pub fn project_id(&self) -> &ProjectId {
        &self.project_id
    }
    #[cfg(any(test, feature = "test-util"))]
    pub fn restrict_limits_probe(&mut self, limits: StorageLimits) -> Result<(), BundleError> {
        self.store_mut()?;
        drop(self.durable.take());
        let result = self.store.restrict_copy_limits(limits);
        if self.root.is_some() {
            self.durable = Some(self.store.mark_committed(&self.root_references()?)?);
        }
        result
    }
    #[must_use]
    pub fn manual_branch_id(&self) -> &BranchId {
        &self.manual_branch_id
    }
    #[must_use]
    pub fn store(&self) -> &ProjectStore {
        &self.store
    }
    pub fn store_mut(&mut self) -> Result<&mut ProjectStore, BundleError> {
        if self.unsettled {
            return Err(BundleError::Unsettled);
        }
        self.store.with_namespace(|n| n.verify())?;
        Ok(&mut self.store)
    }
    pub(crate) fn root_references(&self) -> Result<Vec<ObjectRef>, BundleError> {
        let root = self
            .root
            .as_ref()
            .ok_or_else(|| BundleError::Corrupt("no root".into()))?;
        let mut refs = vec![
            root.branch_index_ref.clone(),
            root.input_manifest_ref.clone(),
            root.store_manifest_ref.clone(),
        ];
        refs.extend(root.receipt_index_ref.clone());
        refs.extend(root.issuer_state_ref.clone());
        Ok(refs)
    }
    #[must_use]
    pub fn root(&self) -> Option<&StudioRoot> {
        self.root.as_ref()
    }
    #[must_use]
    pub fn manual_head(&self) -> Option<&HeadId> {
        self.branches
            .get(&self.manual_branch_id)
            .map(|b| &b.head_id)
    }
    #[must_use]
    pub fn evidence_refs(&self) -> &[ObjectRef] {
        &self.evidence
    }
    /// Retain opaque evidence in the complete next graph, without inventing producer/receipt semantics.
    pub fn retain_evidence(&mut self, reference: &ObjectRef) -> Result<(), BundleError> {
        self.store_mut()?;
        if reference.record_kind != RecordKind::OwnedEvidence {
            return Err(BundleError::Unsupported("opaque evidence kind".into()));
        }
        if self.evidence.contains(reference) {
            return Ok(());
        }
        if self.evidence.len() as u64 >= self.store.limits().objects {
            return Err(BundleError::Capacity("durable evidence pins"));
        }
        self.pending_evidence
            .push(self.store.pin(std::slice::from_ref(reference))?);
        self.evidence.push(reference.clone());
        Ok(())
    }
    pub fn admit_inputs(
        &mut self,
        manifest: &crate::owned_inputs::OwnedInputManifest,
    ) -> Result<SealedObject, BundleError> {
        self.store_mut()?.admit_record(manifest)
    }
    pub fn empty_inputs(&mut self) -> Result<SealedObject, BundleError> {
        self.admit_inputs(&crate::owned_inputs::OwnedInputManifest {
            project_id: Some(self.project_id.clone()),
            session_id: None,
            owner_epoch: self.store.owner_epoch(),
            entries: vec![],
        })
    }
    /// Validate the exact published graph, complete its parent barrier, then clear unknown publication.
    #[allow(
        clippy::too_many_lines,
        reason = "one reconciliation refreshes destination authority, graph and runtime state together"
    )]
    pub fn reconcile(&mut self) -> Result<Option<BundleSaveReport>, BundleError> {
        if let Some(pending) = &self.pending_save {
            reconcile_core_save(&pending.core)?;
        }
        self.unsettled = true;
        let collection = self.store.collection_status()?;
        self.store.settle_object_writes()?;
        // An object-directory sync with an unknown outcome is retried before settling.
        self.store
            .with_namespace(ProjectNamespace::finish_object_batch)?;
        if self.store.has_unsettled_objects()? {
            return Err(BundleError::Unsettled);
        }
        let path = self.store.resolved_root()?;
        let Some(bytes) = self.store.with_namespace(|n| n.read_root())? else {
            return self.settle_absent_root();
        };
        let admission = BundleWriteAdmission::from_bytes(Some(path.clone()), bytes)?;
        admission.require_supported()?;
        if !admission.is_studio() {
            if admission.raw_bytes() != self.admission.raw_bytes() {
                self.store
                    .with_namespace(|n| n.reconcile(admission.raw_bytes()))?;
                self.store.settle_controls()?;
                self.settle_pending_save(false)?;
                return Err(BundleError::Corrupt(
                    "unexpected legacy reconciliation root".into(),
                ));
            }
            self.store
                .with_namespace(|n| n.reconcile(admission.raw_bytes()))?;
            self.store.settle_controls()?;
            self.settle_pending_save(false)?;
            self.unsettled = false;
            return Ok(None);
        }
        let loaded = LoadedBundle::read(&path)?;
        let root = loaded.root()?.clone();
        if root.project_id != self.project_id {
            self.store
                .with_namespace(|n| n.reconcile(loaded.admission.raw_bytes()))?;
            self.store.settle_controls()?;
            self.settle_pending_save(false)?;
            return Err(BundleError::Identity(
                "reconciliation project identity".into(),
            ));
        }
        let head = loaded.manual_checkpoint()?.head_id.clone();
        self.store.closure(&[
            root.branch_index_ref.clone(),
            root.input_manifest_ref.clone(),
            root.store_manifest_ref.clone(),
        ])?;
        self.store
            .with_namespace(|n| n.reconcile(loaded.admission.raw_bytes()))?;
        self.store.settle_controls()?;
        self.receipts = loaded
            .receipts
            .as_ref()
            .map_or_else(crate::receipts::ReceiptLedger::default, |v| (**v).clone());
        self.issuers = loaded
            .issuers
            .as_ref()
            .map_or_else(BTreeMap::new, |v| (**v).clone());
        self.evidence = loaded.manifest.durable_pins.clone();
        self.history_cache = loaded
            .branches
            .iter()
            .map(|(id, branch)| {
                (
                    id.clone(),
                    CachedHistory {
                        reference: branch.0.history_ref.clone(),
                        history: Arc::clone(&branch.1),
                    },
                )
            })
            .collect();
        self.branches = loaded
            .branches
            .iter()
            .map(|(id, b)| (id.clone(), (*b.0).clone()))
            .collect();
        self.origin_project_ids.clone_from(&root.origin_project_ids);
        self.origin_session_ids.clone_from(&root.origin_session_ids);
        self.generation = root.generation;
        self.recovery = loaded.manifest.recovery.clone();
        self.inventory = loaded.inventory;
        self.root = Some(root);
        self.admission = loaded.admission;
        self.durable = Some(self.store.mark_committed(&self.root_references()?)?);
        self.store.publish_inventory(&self.recovery)?;
        self.store.allow_collection()?;
        let own_root = self.pending_save.as_ref().is_some_and(|pending| {
            pending.root_sha256.as_deref()
                == Some(kinewright_media::sha256_bytes(self.admission.raw_bytes()).as_str())
        });
        self.settle_pending_save(own_root)?;
        self.unsettled = false;
        Ok(Some(BundleSaveReport {
            collection,
            path,
            project_id: self.project_id.clone(),
            generation: self.generation,
            head_id: head,
            root_sha256: kinewright_media::sha256_bytes(self.admission.raw_bytes()),
            byte_length: self.admission.raw_bytes().len() as u64,
        }))
    }
    fn settle_absent_root(&mut self) -> Result<Option<BundleSaveReport>, BundleError> {
        if self.root.is_some() {
            // Only a never-published owner can confirm absence as its original root.
            return Err(BundleError::Corrupt("missing reconciliation root".into()));
        }
        self.store.with_namespace(|n| n.reconcile_copy_root(None))?;
        self.store.settle_controls()?;
        self.settle_pending_save(false)?;
        self.unsettled = false;
        Ok(None)
    }
    pub fn serialized_root(&self) -> Result<StorageReservation<Vec<u8>>, BundleError> {
        let root = self
            .root
            .as_ref()
            .ok_or_else(|| BundleError::Corrupt("unpublished bundle".into()))?;
        let branch = self
            .branches
            .get(&root.manual_branch_id)
            .ok_or_else(|| BundleError::Corrupt("missing manual branch".into()))?;
        let document = self.store.read_record::<Document>(&branch.document_ref)?;
        let (bytes, charge) = crate::bundle_commit::serialize_root(&self.store, &document, root)?;
        Ok(charge.hold(bytes))
    }
    /// Retire the preceding save's unreachable objects after its acknowledgment.
    /// This also runs on the next save/open, so interrupted maintenance is retried.
    pub fn collect_after_ack(&mut self) -> Result<u64, BundleError> {
        let result = self.collect_after_ack_inner();
        self.last_collection_error = result.as_ref().err().map(ToString::to_string);
        result
    }
    /// The last maintenance failure is retained until collection succeeds.
    #[must_use]
    pub fn collection_error(&self) -> Option<&str> {
        self.last_collection_error.as_deref()
    }
    fn collect_after_ack_inner(&mut self) -> Result<u64, BundleError> {
        if self.unsettled {
            return Err(BundleError::Unsettled);
        }
        self.store
            .with_namespace(ProjectNamespace::finish_object_batch)?;
        if self.root.is_none() {
            return Ok(0);
        }
        self.store.publish_inventory(&self.recovery)?;
        self.store.allow_collection()?;
        self.store.collect_unpublished()
    }
    pub fn save(
        &mut self,
        core: &Core,
        inputs: &ObjectRef,
    ) -> Result<BundleSaveReport, BundleError> {
        reconcile_core_save(core)?;
        crate::bundle_copy::settle_copy_source(&mut self.pending_copy_source)?;
        if self.unsettled {
            self.reconcile()?;
        }
        if self.pending_command.is_some() {
            return Err(BundleError::Unsettled);
        }
        let before = core
            .export_history()
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let id = self.manual_branch_id.clone();
        self.publish_core(core, &before, inputs, &id)
    }

    pub(crate) fn publish_core(
        &mut self,
        core: &Core,
        selected: &HistorySnapshot,
        inputs: &ObjectRef,
        branch: &BranchId,
    ) -> Result<BundleSaveReport, BundleError> {
        reconcile_core_save(core)?;
        if self.unsettled {
            return Err(BundleError::Unsettled);
        }
        let mut aligned = selected.clone();
        if let Some(prior) = self.branches.get(branch) {
            let durable = if let Some(cached) = self.cached_history(branch) {
                Arc::clone(cached)
            } else {
                Arc::new(
                    self.store
                        .read_record::<HistorySnapshot>(&prior.history_ref)?,
                )
            };
            if aligned.floor_offset < durable.floor_offset {
                aligned = kinewright_core::HistoryTransition::floor(
                    selected.clone(),
                    usize::try_from(durable.floor_offset - selected.floor_offset)
                        .map_err(|_| BundleError::Capacity("floor offset"))?,
                )
                .map_err(history_error)?
                .after()
                .clone();
            }
        }
        let bounded = crate::bundle_commit::select_history(&aligned, self.store.limits())?;
        let transition = if aligned.floor_offset == selected.floor_offset {
            bounded.floor
        } else if bounded.history.floor_offset > selected.floor_offset {
            Some(
                kinewright_core::HistoryTransition::floor(
                    selected.clone(),
                    usize::try_from(bounded.history.floor_offset - selected.floor_offset)
                        .map_err(|_| BundleError::Capacity("floor offset"))?,
                )
                .map_err(history_error)?,
            )
        } else {
            None
        };
        #[cfg(any(test, feature = "test-util"))]
        crate::test_hook("history_selected");
        let barrier = pending_floor(core, transition)?;
        self.publish_barrier(core, barrier, selected, &bounded.history, inputs, branch)
    }

    pub(crate) fn publish_barrier(
        &mut self,
        core: &Core,
        barrier: Option<crate::history_save::FloorBarrier>,
        history: &HistorySnapshot,
        selected: &HistorySnapshot,
        inputs: &ObjectRef,
        branch: &BranchId,
    ) -> Result<BundleSaveReport, BundleError> {
        self.pending_save = Some(PendingSave {
            core: core.clone(),
            barrier,
            root_sha256: None,
        });
        self.publish_pending(history, selected, inputs, branch)
    }

    pub(crate) fn publish_pending(
        &mut self,
        history: &HistorySnapshot,
        selected: &HistorySnapshot,
        inputs: &ObjectRef,
        branch: &BranchId,
    ) -> Result<BundleSaveReport, BundleError> {
        let result = crate::bundle_commit::BundleCommit::publish_branch_selected(
            self,
            history,
            inputs,
            branch,
            None,
            Some(selected),
        );
        match result {
            Ok(report) => {
                self.settle_pending_save(true)?;
                Ok(report)
            }
            Err(error) => {
                if self
                    .pending_save
                    .as_ref()
                    .is_some_and(|p| p.root_sha256.is_some())
                    && self.unsettled
                {
                    self.unsettled = true;
                    if let Some(barrier) =
                        self.pending_save.as_mut().and_then(|p| p.barrier.as_mut())
                    {
                        barrier.retain_until_reconcile().map_err(history_error)?;
                    }
                } else {
                    self.settle_pending_save(false)?;
                }
                if self.store.with_namespace(|n| Ok(n.is_uncertain()))?
                    || self.store.has_unsettled_objects()?
                {
                    self.unsettled = true;
                }
                Err(error)
            }
        }
    }

    pub(crate) fn settle_pending_save(&mut self, commit: bool) -> Result<(), BundleError> {
        if let Some(pending) = &mut self.pending_save
            && let Some(barrier) = &mut pending.barrier
        {
            let result = if commit {
                barrier.commit()
            } else {
                barrier.release()
            };
            if let Err(error) = result {
                self.unsettled = true;
                self.pending_save = None;
                return Err(history_error(error));
            }
        }
        self.pending_save = None;
        Ok(())
    }

    pub(crate) fn admit_root_checkpoint(
        &mut self,
        bytes: &[u8],
        deps: &[ObjectRef],
    ) -> Result<SealedObject, BundleError> {
        let reference = ObjectRef {
            sha256: kinewright_media::sha256_bytes(bytes),
            byte_length: bytes.len() as u64,
            record_kind: RecordKind::RootCheckpoint,
        };
        self.store.admit_blob(&mut &*bytes, &reference, deps)
    }
}

/// Install the revocable barrier before destination IO starts.
pub(crate) fn pending_floor(
    core: &Core,
    transition: Option<kinewright_core::HistoryTransition>,
) -> Result<Option<crate::history_save::FloorBarrier>, BundleError> {
    transition
        .map(|floor| crate::history_save::begin(core, floor))
        .transpose()
        .map_err(history_error)
}

pub(crate) fn history_error(error: kinewright_core::HistoryError) -> BundleError {
    match error {
        kinewright_core::HistoryError::CaptureChanged => BundleError::HistoryChanged,
        kinewright_core::HistoryError::BarrierInvariant => BundleError::HistoryBarrierInvariant,
        kinewright_core::HistoryError::SaveInProgress => BundleError::SaveInProgress,
        kinewright_core::HistoryError::CommitInvariant => BundleError::HistoryCommitInvariant,
        other => BundleError::Corrupt(other.to_string()),
    }
}

/// Reconcile an orphan independently of the old store's object reservation ledger.
pub(crate) fn reconcile_core_save(core: &Core) -> Result<(), BundleError> {
    crate::history_save::bind(core).map_err(history_error)?;
    let Some(pending) = core.pending_history_save().map_err(history_error)? else {
        return Ok(());
    };
    if !pending.ready {
        return Err(BundleError::SaveInProgress);
    }
    let tag = pending
        .tag
        .as_ref()
        .ok_or(BundleError::HistoryBarrierInvariant)?;
    let commit = reconcile_save_tag(tag)?;
    crate::history_save::settle(core, &pending, commit).map_err(history_error)
}

pub(crate) fn reconcile_save_tag(
    tag: &kinewright_core::HistorySaveTag,
) -> Result<bool, BundleError> {
    let read = || -> Result<bool, BundleError> {
        let namespace = ProjectNamespace::inspect(&tag.destination)?;
        let identity = namespace.identity();
        let bytes = namespace.read_root()?;
        // A readable destination whose parent is no longer the tagged directory cannot
        // hold this attempt's root: a definite release, never a commit.
        let tagged_parent = identity.resolved_root == tag.destination
            && identity.parent_device == tag.parent_device
            && identity.parent_inode == tag.parent_inode;
        let own = tagged_parent
            && bytes
                .as_ref()
                .is_some_and(|bytes| kinewright_media::sha256_bytes(bytes) == tag.root_sha256);
        if own {
            let loaded = LoadedBundle::read(&tag.destination)?;
            if loaded.namespace.identity() != identity
                || Some(loaded.admission.raw_bytes()) != bytes.as_deref()
            {
                return Err(BundleError::Identity("reconciliation root changed".into()));
            }
        }
        namespace.seal_observed_root(bytes.as_deref())?;
        Ok(own)
    };
    read().map_err(|_| BundleError::DurabilityUnknownAt(tag.destination.clone()))
}
