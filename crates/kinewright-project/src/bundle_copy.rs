//! Unsaved edits use exactly the named store's object, budget and pin owner.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::{BundleError, BundleSaveReport, BundleWriteAdmission, ProjectBundle},
    bundle_records::{HeadId, ObjectRef, ProjectId, SessionId},
    namespace::{PreparedDestination, ProjectNamespace},
    owned_inputs::{OwnedInputManifest, OwnedInputRef},
    storage_budget::StorageLimits,
    store::{ProjectStore, SourceCopyHold},
};
use kinewright_core::Core;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone)]
pub enum CopyStatus {
    Committed(BundleSaveReport),
    DurabilityUnknown,
    DefinitelyUnpublished,
    /// Save failed before root publication; its owner still has temporary cleanup to retry.
    CleanupPending {
        diagnostic: String,
    },
    /// The root is durable, but Core/accounting settlement still needs reconciliation.
    CommittedWithError {
        diagnostic: String,
    },
}
pub enum SaveAsOutcome {
    SameProject(BundleSaveReport),
    Copied(Box<BundleCopy>),
}
struct DestinationBefore {
    root: Option<Vec<u8>>,
    inventory: Option<Vec<u8>>,
    had_assets: bool,
    projects: Vec<ProjectId>,
    sessions: Vec<SessionId>,
}
/// Pending copies retain both namespace owners and the complete frozen source closure.
#[must_use]
pub struct BundleCopy {
    destination: Option<ProjectBundle>,
    source: Arc<Mutex<SourceCopyHold>>,
    _prepared: PreparedDestination,
    before: DestinationBefore,
    status: CopyStatus,
    input_rebindings: BTreeMap<ObjectRef, ObjectRef>,
    tag: Option<kinewright_core::HistorySaveTag>,
}
pub(crate) struct PendingCopySource {
    tag: kinewright_core::HistorySaveTag,
    source: Arc<Mutex<SourceCopyHold>>,
}
pub(crate) fn settle_copy_source(
    pending: &mut Option<PendingCopySource>,
) -> Result<(), BundleError> {
    if let Some(hold) = pending {
        let needs_reconciliation = hold
            .source
            .lock()
            .map_err(|_| BundleError::Capacity("copy source poisoned"))?
            .requires_reconciliation();
        if needs_reconciliation {
            crate::bundle::reconcile_save_tag(&hold.tag)?;
            hold.source
                .lock()
                .map_err(|_| BundleError::Capacity("copy source poisoned"))?
                .reconciled();
        }
    }
    *pending = None;
    Ok(())
}
impl BundleCopy {
    pub(crate) fn rebound_manifest(&self, source: &ObjectRef) -> Result<ObjectRef, BundleError> {
        self.input_rebindings
            .get(source)
            .cloned()
            .ok_or_else(|| BundleError::Identity("manifest outside frozen copy".into()))
    }
    pub(crate) fn source_manifest(&self, rebound: &ObjectRef) -> Result<ObjectRef, BundleError> {
        self.input_rebindings
            .iter()
            .find_map(|(source, target)| (target == rebound).then(|| source.clone()))
            .ok_or_else(|| BundleError::Identity("manifest outside retained source".into()))
    }
    pub(crate) fn destination_owner(&self) -> Result<&ProjectBundle, BundleError> {
        self.destination
            .as_ref()
            .ok_or_else(|| BundleError::Identity("missing retained writer".into()))
    }
    /// A retained owner never rolls back or consumes its namespace/source on settlement.
    pub(crate) fn reconcile_retained(
        &mut self,
        live: &Core,
    ) -> Result<Option<BundleSaveReport>, BundleError> {
        let destination = self
            .destination
            .as_mut()
            .ok_or_else(|| BundleError::Identity("missing retained writer".into()))?;
        destination.store.with_namespace(|n| {
            n.probe_retained_namespace()?;
            n.verify_read_authority()
        })?;
        let actual = destination.store.with_namespace(|n| n.read_root())?;
        if actual == self.before.root
            && !matches!(
                self.status,
                CopyStatus::Committed(_) | CopyStatus::CommittedWithError { .. }
            )
        {
            destination
                .store
                .with_namespace(|n| n.reconcile_copy_root(actual.as_deref()))?;
            destination.store.settle_object_writes()?;
            destination.store.settle_controls()?;
            destination.settle_destination_ticket(Some(live))?;
            destination.unsettled = false;
            destination.publication_state = crate::bundle::PublicationState::DefinitelyRefused;
            destination.publication_tag = None;
            self.source
                .lock()
                .map_err(|_| BundleError::Capacity("copy source poisoned"))?
                .reconciled();
            self.status = CopyStatus::DefinitelyUnpublished;
            return Ok(None);
        }
        self.reconcile_destination_policy(Some(live), true)
    }
    /// Serialized non-consuming borrow: the original copy, ancestor claim and source stay owned.
    pub(crate) fn save_retained(
        &mut self,
        core: &Core,
        inputs: &ObjectRef,
    ) -> Result<BundleSaveReport, BundleError> {
        self.reconcile_retained(core)?;
        let destination = self
            .destination
            .as_mut()
            .ok_or_else(|| BundleError::Identity("missing retained writer".into()))?;
        let _inputs = destination.store.pin(std::slice::from_ref(inputs))?;
        let result = destination.save(core, inputs);
        match &result {
            Ok(report) => self.status = CopyStatus::Committed(report.clone()),
            Err(_)
                if destination.publication_state() == crate::bundle::PublicationState::Unknown =>
            {
                self.tag.clone_from(&destination.publication_tag);
                self.source
                    .lock()
                    .map_err(|_| BundleError::Capacity("copy source poisoned"))?
                    .mark_uncertain()?;
                self.status = CopyStatus::DurabilityUnknown;
            }
            _ => {}
        }
        result
    }
    fn pending_source(&self) -> Option<PendingCopySource> {
        if !matches!(
            self.status,
            CopyStatus::DurabilityUnknown | CopyStatus::CommittedWithError { .. }
        ) {
            return None;
        }
        self.tag.clone().map(|tag| PendingCopySource {
            tag,
            source: Arc::clone(&self.source),
        })
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn destination_usage_probe(
        &self,
    ) -> Result<crate::storage_budget::StorageUsage, BundleError> {
        self.destination
            .as_ref()
            .ok_or_else(|| BundleError::Corrupt("copy owner missing".into()))?
            .store
            .usage()
    }
    #[must_use]
    pub const fn status(&self) -> &CopyStatus {
        &self.status
    }
    pub fn rebound_input(&self, source: &OwnedInputRef) -> Result<OwnedInputRef, BundleError> {
        let manifest_ref = self
            .input_rebindings
            .get(&source.manifest_ref)
            .ok_or_else(|| BundleError::Identity("input was not in the frozen copy".into()))?
            .clone();
        let destination = self
            .destination
            .as_ref()
            .ok_or_else(|| BundleError::Corrupt("copy owner missing".into()))?;
        let manifest = destination.store.read_input_manifest(&manifest_ref)?;
        if !manifest.entries.contains(&source.entry) {
            return Err(BundleError::Identity(
                "input entry differs from copied manifest".into(),
            ));
        }
        Ok(OwnedInputRef {
            manifest_ref,
            entry: source.entry.clone(),
        })
    }
    /// No destination writer can escape an unresolved publication.
    pub fn into_destination(mut self) -> Result<ProjectBundle, Box<Self>> {
        if matches!(self.status, CopyStatus::Committed(_))
            && let Some(destination) = self.destination.take()
        {
            return Ok(destination);
        }
        Err(Box::new(self))
    }
    #[must_use]
    pub fn pending_attempt(&self) -> Option<crate::AttemptBinding> {
        self.destination.as_ref()?.pending_attempt()
    }
    pub fn attach_core(&self, core: &Core) -> Result<crate::ProjectAttachment, BundleError> {
        self.destination
            .as_ref()
            .ok_or_else(|| BundleError::Corrupt("copy owner missing".into()))?
            .attach_core(core)
    }
    pub fn reconcile(&mut self) -> Result<Option<BundleSaveReport>, BundleError> {
        let original = self
            .destination
            .as_ref()
            .and_then(|d| d.pending_save.as_ref().map(|p| p.core.clone()));
        self.reconcile_destination(original.as_ref())
    }
    pub fn reconcile_destination(
        &mut self,
        live: Option<&Core>,
    ) -> Result<Option<BundleSaveReport>, BundleError> {
        self.reconcile_destination_policy(live, false)
    }
    fn reconcile_destination_policy(
        &mut self,
        live: Option<&Core>,
        retained: bool,
    ) -> Result<Option<BundleSaveReport>, BundleError> {
        if matches!(
            self.status,
            CopyStatus::Committed(_) | CopyStatus::DefinitelyUnpublished
        ) && let Some(destination) = &mut self.destination
        {
            destination.settle_destination_ticket(live)?;
        }
        match &self.status {
            CopyStatus::Committed(report) => return Ok(Some(report.clone())),
            CopyStatus::DefinitelyUnpublished => return Ok(None),
            CopyStatus::DurabilityUnknown
            | CopyStatus::CleanupPending { .. }
            | CopyStatus::CommittedWithError { .. } => {}
        }
        let destination = self
            .destination
            .as_mut()
            .ok_or_else(|| BundleError::Corrupt("copy owner missing".into()))?;
        if !retained && let Some(tag) = &self.tag {
            crate::bundle::reconcile_save_tag(tag)?;
        }
        let actual = destination.store.with_namespace(|n| n.read_root())?;
        if matches!(self.status, CopyStatus::CommittedWithError { .. })
            && actual == self.before.root
        {
            return Err(BundleError::Corrupt(
                "durably committed copy root disappeared or was replaced".into(),
            ));
        }
        if actual == self.before.root {
            destination.settle_destination_ticket(live)?;
            destination.store.settle_object_writes()?;
            destination.store.rollback_copy(
                self.before.root.as_deref(),
                self.before.inventory.as_deref(),
                self.before.had_assets,
            )?;
            destination.settle_destination_ticket(live)?;
            self.source
                .lock()
                .map_err(|_| BundleError::Capacity("copy source poisoned"))?
                .reconciled();
            self.status = CopyStatus::DefinitelyUnpublished;
            return Ok(None);
        }
        let report = match destination.reconcile_destination(live) {
            Ok(Some(report)) => report,
            Ok(None) => {
                return Err(BundleError::Corrupt(
                    "copy did not publish a Studio root".into(),
                ));
            }
            Err(error) => {
                if destination.publication_state() == crate::bundle::PublicationState::Committed {
                    self.status = CopyStatus::CommittedWithError {
                        diagnostic: error.to_string(),
                    };
                }
                return Err(error);
            }
        };
        self.source
            .lock()
            .map_err(|_| BundleError::Capacity("copy source poisoned"))?
            .reconciled();
        self.status = CopyStatus::Committed(report.clone());
        Ok(Some(report))
    }
}

fn destination(
    path: &Path,
    limits: StorageLimits,
) -> Result<(ProjectBundle, DestinationBefore, PreparedDestination), BundleError> {
    limits.validate()?;
    let prepared = ProjectNamespace::prepare_destination(path)?;
    let namespace = ProjectNamespace::inspect(&prepared.path)?;
    namespace.matches_destination(&prepared)?;
    let root = namespace.read_root()?;
    let admission = if let Some(bytes) = &root {
        BundleWriteAdmission::from_bytes(Some(prepared.path.clone()), bytes.clone())?
    } else {
        BundleWriteAdmission::new_document()
    };
    admission.require_supported()?;
    let had_assets = namespace.has_asset_store()?;
    if had_assets && !admission.is_studio() {
        return Err(BundleError::Identity(
            "unbound foreign destination store".into(),
        ));
    }
    namespace.probe_copy_contention()?;
    let inventory = namespace.read(
        &crate::bundle_recovery::inventory_path()?,
        limits.control_bytes,
    )?;
    let mut projects = vec![];
    let mut sessions = vec![];
    if let Some(root) = admission.data.file.as_ref().and_then(|f| f.studio.as_ref()) {
        projects.push(root.project_id.clone());
        projects.extend(root.origin_project_ids.clone());
        sessions.extend(root.origin_session_ids.clone());
    }
    #[cfg(any(test, feature = "test-util"))]
    crate::test_hook("copy_destination_pinned");
    let mut target = ProjectBundle::activate_copy(&prepared.path, admission, &prepared)?;
    if had_assets
        && target.store.with_namespace(|n| {
            n.read(
                &crate::bundle_recovery::inventory_path()?,
                limits.control_bytes,
            )
        })? != inventory
    {
        return Err(BundleError::Identity(
            "destination inventory changed during claim".into(),
        ));
    }
    target
        .store
        .with_namespace(|n| n.seal_destination_ancestors())?;
    target.store.begin_publication()?;
    drop(target.durable.take());
    target.store.restrict_copy_limits(limits)?;
    let project_id = ProjectId::fresh()?;
    target
        .store
        .rebind_unshared_copy_owner(project_id.clone())?;
    target.project_id = project_id;
    Ok((
        target,
        DestinationBefore {
            root,
            inventory,
            had_assets,
            projects,
            sessions,
        },
        prepared,
    ))
}
fn origins(
    target: &mut ProjectBundle,
    before: &DestinationBefore,
    projects: impl IntoIterator<Item = ProjectId>,
    sessions: impl IntoIterator<Item = SessionId>,
) -> Result<(), BundleError> {
    target.origin_project_ids = before
        .projects
        .iter()
        .cloned()
        .chain(projects)
        .filter(|id| *id != target.project_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    target.origin_session_ids = before
        .sessions
        .iter()
        .cloned()
        .chain(sessions)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if target
        .origin_project_ids
        .len()
        .checked_add(target.origin_session_ids.len())
        .ok_or(BundleError::Capacity("project origins overflow"))? as u64
        > target.store.limits().versions
    {
        return Err(BundleError::Capacity("project origins"));
    }
    Ok(())
}
fn rewrite_inputs(
    target: &mut ProjectBundle,
    reference: &ObjectRef,
    map: &mut BTreeMap<ObjectRef, ObjectRef>,
) -> Result<ObjectRef, BundleError> {
    if let Some(rebound) = map.get(reference) {
        return Ok(rebound.clone());
    }
    let original = target.store.read_input_manifest(reference)?;
    let rebound = OwnedInputManifest {
        project_id: Some(target.project_id.clone()),
        session_id: None,
        owner_epoch: target.store.owner_epoch(),
        entries: original.entries.clone(),
    };
    let sealed = target.store.admit_record(&rebound)?;
    let result = sealed.reference.clone();
    map.insert(reference.clone(), result.clone());
    drop(sealed);
    Ok(result)
}
fn result(
    mut target: ProjectBundle,
    before: DestinationBefore,
    prepared: PreparedDestination,
    mut source: SourceCopyHold,
    rebindings: BTreeMap<ObjectRef, ObjectRef>,
    publication: Result<BundleSaveReport, BundleError>,
) -> Result<BundleCopy, BundleError> {
    let status = match publication {
        Ok(report) => CopyStatus::Committed(report),
        Err(error) => {
            let uncertain = target.unsettled
                || target.store.with_namespace(|n| Ok(n.is_uncertain()))?
                || target.store.has_unsettled_objects()?;
            let root_unknown = target.publication_state == crate::bundle::PublicationState::Unknown;
            let committed = target.publication_state == crate::bundle::PublicationState::Committed;
            if root_unknown {
                if let Some(barrier) = target
                    .pending_save
                    .as_mut()
                    .and_then(|p| p.barrier.as_mut())
                {
                    barrier
                        .retain_until_reconcile()
                        .map_err(crate::bundle::history_error)?;
                }
            } else if !committed {
                target.settle_pending_save(false)?;
            }
            if !uncertain
                && target
                    .store
                    .rollback_copy(
                        before.root.as_deref(),
                        before.inventory.as_deref(),
                        before.had_assets,
                    )
                    .is_ok()
            {
                return Err(error);
            }
            if root_unknown || committed {
                source.mark_uncertain()?;
            }
            if committed {
                CopyStatus::CommittedWithError {
                    diagnostic: error.to_string(),
                }
            } else if root_unknown {
                CopyStatus::DurabilityUnknown
            } else {
                let diagnostic = match &error {
                    BundleError::CleanupPending(detail) => {
                        format!("save failed; cleanup pending: {detail}")
                    }
                    BundleError::DurabilityUnknown | BundleError::Unsettled => {
                        "save failed; cleanup pending: staged store writes need reconciliation"
                            .into()
                    }
                    other => format!("save failed; cleanup pending: {other}"),
                };
                CopyStatus::CleanupPending { diagnostic }
            }
        }
    };
    let tag = target.publication_tag.clone();
    Ok(BundleCopy {
        tag,
        destination: Some(target),
        source: Arc::new(Mutex::new(source)),
        _prepared: prepared,
        before,
        status,
        input_rebindings: rebindings,
    })
}
fn frozen_size(store: &ProjectStore, roots: &[ObjectRef], extra: u64) -> Result<u64, BundleError> {
    store.closure(roots)?.iter().try_fold(extra, |sum, r| {
        sum.checked_add(r.byte_length)
            .ok_or(BundleError::Capacity("copy size overflow"))
    })
}
impl ProjectBundle {
    pub fn save_as(&mut self, path: &Path) -> Result<SaveAsOutcome, BundleError> {
        self.save_as_with_limits(path, StorageLimits::default())
    }
    #[allow(
        clippy::too_many_lines,
        reason = "one frozen graph copied under the same two namespace owners"
    )]
    pub fn save_as_with_limits(
        &mut self,
        path: &Path,
        limits: StorageLimits,
    ) -> Result<SaveAsOutcome, BundleError> {
        // Only a definitely unpublished root with cleanup uncertainty may use retained reads.
        // Unknown roots and pending commands still require their ordinary reconciliation.
        let cleanup_only = self.cleanup_pending()
            && matches!(
                self.publication_state,
                crate::bundle::PublicationState::NeverAttempted
                    | crate::bundle::PublicationState::DefinitelyRefused
            )
            && self.pending_save.is_none()
            && self.pending_command.is_none()
            && self.root.is_some();
        if let Some(pending) = &self.pending_save {
            crate::bundle::reconcile_core_save(&pending.core)?;
        }
        settle_copy_source(&mut self.pending_copy_source)?;
        if self.unsettled && !cleanup_only {
            self.reconcile()?;
        }
        if self.pending_command.is_some() {
            return Err(BundleError::Unsettled);
        }
        // A frozen copy preserves every acknowledged checkpoint exactly. A lower
        // destination allowance cannot silently choose a different live undo floor.
        for id in self.branches.keys() {
            let history = self
                .store
                .read_copy_record::<kinewright_core::HistorySnapshot>(
                    &self.branches[id].history_ref,
                    cleanup_only,
                )?;
            let bounded = crate::bundle_commit::bound_history(&history, &limits)?;
            if bounded != *history {
                return Err(BundleError::Capacity(
                    "copy history exceeds destination allowance",
                ));
            }
        }
        if !cleanup_only {
            self.store.with_namespace(|n| n.verify())?;
            {
                let _source_root =
                    self.store
                        .reserve(0, self.admission.raw_bytes().len() as u64, 0, false)?;
                self.store
                    .with_namespace(|n| n.verify_admitted_root(self.admission.raw_bytes()))?;
            }
        }
        let resolved = crate::canonical_project_identity(path)
            .map_err(|e| BundleError::Identity(e.to_string()))?;
        if cleanup_only && resolved.try_exists()? {
            return Err(BundleError::Identity(
                "cleanup-only copy requires a fresh destination".into(),
            ));
        }
        if resolved == self.store.resolved_root()? {
            if cleanup_only {
                return Err(BundleError::Unsettled);
            }
            let root = self
                .root
                .as_ref()
                .ok_or_else(|| BundleError::Corrupt("no acknowledged copy source".into()))?;
            return Ok(SaveAsOutcome::SameProject(BundleSaveReport {
                collection: self.store.collection_status()?,
                path: resolved,
                project_id: self.project_id.clone(),
                generation: root.generation,
                head_id: self
                    .manual_head()
                    .cloned()
                    .ok_or_else(|| BundleError::Corrupt("no manual head".into()))?,
                root_sha256: kinewright_media::sha256_bytes(self.admission.raw_bytes()),
                byte_length: self.admission.raw_bytes().len() as u64,
            }));
        }
        let roots = self.root_references()?;
        let source = self.store.begin_copy_source(
            &roots,
            frozen_size(&self.store, &roots, self.admission.raw_bytes().len() as u64)?,
            cleanup_only,
        )?;
        let (mut target, before, prepared) = destination(path, limits)?;
        if cleanup_only && before.root.is_some() {
            return Err(BundleError::Identity(
                "cleanup-only destination appeared during admission".into(),
            ));
        }
        let mut map = BTreeMap::new();
        let publication = (|| {
            origins(
                &mut target,
                &before,
                std::iter::once(self.project_id.clone()).chain(self.origin_project_ids.clone()),
                self.origin_session_ids.clone(),
            )?;
            target
                .store
                .copy_graph_from_source(&self.store, &roots, cleanup_only)?;
            #[cfg(any(test, feature = "test-util"))]
            crate::io_observer::check_failure("copy_backup", &prepared.path)?;
            let snapshot = target.admit_root_checkpoint(self.admission.raw_bytes(), &roots)?;
            target.recovery.predecessors = std::iter::once(snapshot.reference.clone())
                .chain(self.recovery.predecessors.iter().cloned())
                .take(2)
                .collect();
            drop(snapshot);
            target.recovery.activation_project_id = Some(target.project_id.clone());
            target.recovery.manual_branch_id = Some(self.manual_branch_id.clone());
            target.recovery.namespace_root_file_name = prepared
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_owned);
            target.generation = target.generation.max(self.generation);
            let generation = target
                .generation
                .checked_add(1)
                .ok_or(BundleError::Capacity("copy generation"))?;
            target.manual_branch_id = self.manual_branch_id.clone();
            target.evidence.clone_from(&self.evidence);
            target.receipts = self.receipts.clone();
            target.issuers = self.issuers.clone();
            for state in target.issuers.values_mut() {
                state.retired = true;
            }
            let mut branches = BTreeMap::new();
            let mut index = BTreeMap::new();
            for (id, original) in &self.branches {
                let mut checkpoint = original.clone();
                checkpoint.input_manifest_ref =
                    rewrite_inputs(&mut target, &original.input_manifest_ref, &mut map)?;
                checkpoint.history_input_manifest_refs = original
                    .history_input_manifest_refs
                    .iter()
                    .map(|r| rewrite_inputs(&mut target, r, &mut map))
                    .collect::<Result<_, _>>()?;
                checkpoint.head_id = HeadId::fresh()?;
                checkpoint.predecessor = Some(original.head_id.clone());
                checkpoint.checkpoint_generation = generation;
                let sealed = target.store.admit_record(&checkpoint)?;
                index.insert(id.clone(), sealed.reference.clone());
                branches.insert(id.clone(), checkpoint);
                drop(sealed);
            }
            let index = target.store.admit_record(&index)?;
            let mut root = self
                .root
                .as_ref()
                .ok_or_else(|| BundleError::Corrupt("copy source root".into()))?
                .clone();
            root.branch_index_ref = index.reference.clone();
            drop(index);
            root.input_manifest_ref = branches[&self.manual_branch_id].input_manifest_ref.clone();
            target.root = Some(root);
            target.branches = branches;
            target.history_cache.clear();
            #[cfg(any(test, feature = "test-util"))]
            crate::test_hook("copy_before_publish");
            #[cfg(any(test, feature = "test-util"))]
            crate::io_observer::check_failure("copy_verify", &prepared.path)?;
            for reference in target.store.closure(&roots)? {
                target.store.verify(&reference)?;
            }
            let history = self
                .store
                .read_copy_record::<kinewright_core::HistorySnapshot>(
                    &self.branches[&self.manual_branch_id].history_ref,
                    cleanup_only,
                )?;
            let inputs = target.branches[&target.manual_branch_id]
                .input_manifest_ref
                .clone();
            crate::bundle_commit::BundleCommit::publish(&mut target, &history, &inputs)
        })();
        let copy = result(target, before, prepared, source, map, publication)?;
        self.pending_copy_source = copy.pending_source();
        Ok(SaveAsOutcome::Copied(Box::new(copy)))
    }
}
pub struct SessionStore {
    store: ProjectStore,
    session_id: SessionId,
    pending_copy_source: Option<PendingCopySource>,
}
impl SessionStore {
    /// Bind a newly created unsaved edit before distributing handles.
    pub fn attach_core(&self, core: &Core) -> Result<crate::ProjectAttachment, BundleError> {
        crate::history_save::ProjectAttachment::new(core).map_err(crate::bundle::history_error)
    }
    pub fn create(parent: &Path, limits: StorageLimits) -> Result<Self, BundleError> {
        limits.validate()?;
        let session_id = SessionId::fresh()?;
        let namespace = ProjectNamespace::create_session(parent, &session_id)?;
        let store = ProjectStore::new(namespace, None, Some(session_id.clone()), limits)?;
        Ok(Self {
            store,
            session_id,
            pending_copy_source: None,
        })
    }
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }
    #[must_use]
    pub fn store(&self) -> &ProjectStore {
        &self.store
    }
    pub fn store_mut(&mut self) -> &mut ProjectStore {
        &mut self.store
    }
    pub fn admit_inputs(
        &mut self,
        manifest: &OwnedInputManifest,
    ) -> Result<crate::store::SealedObject, BundleError> {
        if manifest.session_id.as_ref() != Some(&self.session_id)
            || manifest.project_id.is_some()
            || manifest.owner_epoch != self.store.owner_epoch()
        {
            return Err(BundleError::Identity("session input owner".into()));
        }
        self.store.admit_record(manifest)
    }
    pub fn first_save(
        &mut self,
        destination: &Path,
        core: &Core,
        inputs: &ObjectRef,
    ) -> Result<BundleCopy, BundleError> {
        self.first_save_with_limits(destination, core, inputs, StorageLimits::default())
    }
    pub fn first_save_with_limits(
        &mut self,
        path: &Path,
        core: &Core,
        inputs: &ObjectRef,
        limits: StorageLimits,
    ) -> Result<BundleCopy, BundleError> {
        crate::bundle::reconcile_core_save(core)?;
        settle_copy_source(&mut self.pending_copy_source)?;
        let frozen = core
            .export_history()
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let bounded = crate::bundle_commit::select_history(&frozen, &limits)?;
        let history = &bounded.history;
        let barrier = crate::bundle::pending_floor(core, bounded.floor)?;
        let roots = [inputs.clone()];
        let source = self.store.begin_copy(
            &roots,
            frozen_size(
                &self.store,
                &roots,
                history
                    .encoded_bytes()
                    .map_err(|e| BundleError::Corrupt(e.to_string()))?,
            )?,
        )?;
        let (mut target, before, prepared) = destination(path, limits)?;
        target.pending_save = Some(crate::bundle::PendingSave {
            core: core.clone(),
            barrier,
            root_sha256: None,
        });
        let mut map = BTreeMap::new();
        let publication = (|| {
            origins(
                &mut target,
                &before,
                std::iter::empty(),
                [self.session_id.clone()],
            )?;
            target.store.copy_graph_from(&self.store, &roots)?;
            let rebound = rewrite_inputs(&mut target, inputs, &mut map)?;
            // Preserve the exact capture owner as archival origin evidence, never as a new lease.
            target.evidence.push(inputs.clone());
            #[cfg(any(test, feature = "test-util"))]
            crate::test_hook("copy_before_publish");
            #[cfg(any(test, feature = "test-util"))]
            crate::io_observer::check_failure("copy_verify", &prepared.path)?;
            for reference in target.store.closure(&roots)? {
                target.store.verify(&reference)?;
            }
            let id = target.manual_branch_id.clone();
            let report = target.publish_pending(&frozen, history, &rebound, &id)?;
            Ok(report)
        })();
        let copy = result(target, before, prepared, source, map, publication)?;
        self.pending_copy_source = copy.pending_source();
        Ok(copy)
    }
}
