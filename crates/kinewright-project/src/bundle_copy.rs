//! Unsaved edits use exactly the named store's object, budget and pin owner.
#![allow(clippy::missing_errors_doc)]
use crate::operation_actor::Core;
use crate::{
    bundle::{BundleError, BundleSaveReport, BundleWriteAdmission, ProjectBundle},
    bundle_records::{HeadId, ObjectRef, ProjectId, SessionId},
    namespace::{PreparedDestination, ProjectNamespace},
    owned_inputs::{OwnedInputManifest, OwnedInputRef},
    storage_budget::StorageLimits,
    store::{ProjectStore, SourceCopyHold},
};
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
    source: CopySource,
    _prepared: PreparedDestination,
    before: DestinationBefore,
    status: CopyStatus,
    input_rebindings: BTreeMap<ObjectRef, ObjectRef>,
    tag: Option<kinewright_core::HistorySaveTag>,
}
enum CopySource {
    Local(Arc<Mutex<SourceCopyHold>>),
    Borrowed(crate::source_boundary::BorrowedSourceDescriptor),
}
impl From<SourceCopyHold> for CopySource {
    fn from(hold: SourceCopyHold) -> Self {
        Self::Local(Arc::new(Mutex::new(hold)))
    }
}
impl CopySource {
    fn mark_uncertain(&mut self) -> Result<(), BundleError> {
        if let Self::Local(source) = self {
            source
                .lock()
                .map_err(|_| BundleError::Capacity("copy source poisoned"))?
                .mark_uncertain()?;
        }
        Ok(())
    }
    fn reconciled(&mut self) -> Result<(), BundleError> {
        if let Self::Local(source) = self {
            source
                .lock()
                .map_err(|_| BundleError::Capacity("copy source poisoned"))?
                .reconciled();
        }
        Ok(())
    }
    fn close_originating_context(&mut self) {
        if let Self::Local(source) = self
            && let Ok(mut source) = source.lock()
        {
            source.close_originating_context();
        }
    }
}
pub(crate) struct PendingCopySource {
    pool: crate::operation_history::HistoryPool,
    tag: kinewright_core::HistorySaveTag,
    source: Arc<Mutex<SourceCopyHold>>,
}
pub(crate) fn close_copy_source(pending: &mut Option<PendingCopySource>) {
    if let Some(pending) = pending
        && let Ok(mut source) = pending.source.lock()
    {
        source.close_originating_context();
    }
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
            crate::bundle::reconcile_save_tag(&hold.tag, &hold.pool)?;
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
    /// Execute a GUI-origin first save in the destination process. The source owner
    /// has already admitted and retained the exact graph and history overlap.
    pub fn first_save_borrowed(
        path: &Path,
        source: crate::source_boundary::BorrowedSourceDescriptor,
        reader: &mut dyn crate::source_boundary::BorrowedSourceReader,
        history: &crate::operation_actor::PortableHistory,
        inputs: &ObjectRef,
        limits: StorageLimits,
        pool: &crate::operation_history::HistoryPool,
    ) -> Result<Self, BundleError> {
        source.validate(&limits)?;
        if source.session_id.is_none()
            || source.project_id.is_some()
            || !source.roots.contains(inputs)
        {
            return Err(BundleError::Identity(
                "borrowed first-save source owner or inputs".into(),
            ));
        }
        let snapshot = crate::source_boundary::BorrowedSnapshotBinding::from_history(history)?;
        reader.prove_snapshot(&source, &snapshot)?;
        let inspection = ProjectStore::inspect_rescue_destination(path, &[])?;
        let identity = inspection.identity()?;
        if reader.inspect_destination(&source, inspection.path())? != identity {
            return Err(BundleError::Identity(
                "borrowed destination identity differs".into(),
            ));
        }
        let (mut target, before, prepared) = destination_admitted(
            path,
            limits,
            Some(&inspection),
            &mut || {
                reader.prove_snapshot(&source, &snapshot)?;
                let current = ProjectStore::inspect_borrowed_destination(inspection.path(), &[])?;
                if reader.inspect_destination(&source, inspection.path())? != current.identity()? {
                    return Err(BundleError::Identity(
                        "borrowed destination changed before mutation".into(),
                    ));
                }
                Ok(())
            },
            pool,
        )?;
        let selected = history
            .snapshot
            .validate_shared(pool, crate::operation_history::Role::Ordinary)?
            .into_reader()?
            .into_capture()?
            .into_install()?;
        let revision = kinewright_core::TimelineRevision(
            history
                .revision
                .0
                .checked_add(1)
                .ok_or(BundleError::Capacity("install revision"))?,
        );
        let mut map = BTreeMap::new();
        let publication = (|| {
            origins(
                &mut target,
                &before,
                std::iter::empty(),
                source.session_id.clone(),
            )?;
            target.store.copy_graph_from_borrowed(&source, reader)?;
            let original = target.store.read_input_manifest(inputs)?;
            if original.project_id != source.project_id
                || original.session_id != source.session_id
                || original.owner_epoch != source.owner_epoch
            {
                return Err(BundleError::Identity(
                    "borrowed input owner differs from admission".into(),
                ));
            }
            let rebound = rebind_inputs(&mut target, &original)?;
            map.insert(inputs.clone(), rebound.clone());
            target.evidence.push(inputs.clone());
            reader.prove_snapshot(&source, &snapshot)?;
            crate::bundle_commit::BundleCommit::publish_snapshot(
                &mut target,
                &selected,
                revision,
                &rebound,
            )
        })();
        result(
            target,
            before,
            prepared,
            CopySource::Borrowed(source),
            map,
            publication,
        )
    }

    pub(crate) fn close_originating_context(&mut self) {
        self.source.close_originating_context();
    }

    #[must_use]
    pub fn borrowed_source(&self) -> Option<&crate::source_boundary::BorrowedSourceDescriptor> {
        match &self.source {
            CopySource::Borrowed(source) => Some(source),
            CopySource::Local(_) => None,
        }
    }

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
    pub(crate) fn destination_owner_mut(&mut self) -> Result<&mut ProjectBundle, BundleError> {
        self.destination
            .as_mut()
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
        if destination.unsettled
            && destination.publication_state() == crate::bundle::PublicationState::Unknown
        {
            self.tag.clone_from(&destination.publication_tag);
            self.status = CopyStatus::DurabilityUnknown;
            self.source.mark_uncertain()?;
        }
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
            self.source.reconciled()?;
            self.status = CopyStatus::DefinitelyUnpublished;
            return Ok(None);
        }
        self.reconcile_destination_policy(Some(live), true)
    }
    /// Serialized non-consuming borrow: the original copy, ancestor claim and source stay owned.
    /// Retry an unactivated first-save selection. The original source remains
    /// frozen until the exact selected epoch is known durable and installed.
    pub(crate) fn retry_install(
        &mut self,
        core: &Core,
        inputs: &ObjectRef,
    ) -> Result<BundleSaveReport, BundleError> {
        self.reconcile_retained(core)?;
        if let CopyStatus::Committed(report) = &self.status {
            return Ok(report.clone());
        }
        let capture = core
            .capture_state()
            .map_err(crate::bundle::operation_error)?;
        let revision = kinewright_core::TimelineRevision(
            capture
                .change
                .revision
                .0
                .checked_add(1)
                .ok_or(BundleError::Capacity("install revision"))?,
        );
        let selected = capture.snapshot.into_install()?;
        let destination = self.destination.as_mut().ok_or(BundleError::Unsettled)?;
        let result = destination.with_person_publication(|bundle| {
            crate::bundle_commit::BundleCommit::publish_snapshot(
                bundle, &selected, revision, inputs,
            )
        });
        match &result {
            Ok(report) => self.status = CopyStatus::Committed(report.clone()),
            Err(_)
                if destination.publication_state() == crate::bundle::PublicationState::Unknown =>
            {
                self.tag.clone_from(&destination.publication_tag);
                self.source.mark_uncertain()?;
                self.status = CopyStatus::DurabilityUnknown;
            }
            _ => {}
        }
        result
    }
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
                self.source.mark_uncertain()?;
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
        let CopySource::Local(source) = &self.source else {
            return None;
        };
        let pool = self.destination.as_ref()?.history_pool.clone();
        self.tag.clone().map(|tag| PendingCopySource {
            pool,
            tag,
            source: Arc::clone(source),
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
            .and_then(|d| d.pending_publication.as_ref().map(|p| p.core.clone()));
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
            crate::bundle::reconcile_save_tag(tag, &destination.history_pool)?;
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
            destination.copy_publication = None;
            destination.store.settle_object_writes()?;
            destination.store.rollback_copy(
                self.before.root.as_deref(),
                self.before.inventory.as_deref(),
                self.before.had_assets,
            )?;
            destination.settle_destination_ticket(live)?;
            self.source.reconciled()?;
            self.status = CopyStatus::DefinitelyUnpublished;
            return Ok(None);
        }
        let report = match destination
            .with_person_publication(|destination| destination.reconcile_destination(live))
        {
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
        self.source.reconciled()?;
        self.status = CopyStatus::Committed(report.clone());
        Ok(Some(report))
    }
}

fn destination(
    path: &Path,
    limits: StorageLimits,
    pool: &crate::operation_history::HistoryPool,
) -> Result<(ProjectBundle, DestinationBefore, PreparedDestination), BundleError> {
    destination_admitted(path, limits, None, &mut || Ok(()), pool)
}
fn destination_admitted(
    path: &Path,
    limits: StorageLimits,
    inspection: Option<&crate::namespace::DestinationInspection>,
    on_mutation: &mut dyn FnMut() -> Result<(), BundleError>,
    pool: &crate::operation_history::HistoryPool,
) -> Result<(ProjectBundle, DestinationBefore, PreparedDestination), BundleError> {
    limits.validate()?;
    let prepared = if let Some(inspection) = inspection {
        ProjectNamespace::prepare_rescue_destination(inspection, on_mutation)?
    } else {
        ProjectNamespace::prepare_destination(path)?
    };
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
    if inspection.is_some() && (root.is_some() || had_assets) {
        return Err(BundleError::Identity(
            "rescue destination is occupied".into(),
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
    if inspection.is_some() {
        on_mutation()?;
    }
    let mut target = ProjectBundle::activate_copy(&prepared.path, admission, &prepared, pool)?;
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
    target.copy_publication = Some(crate::copy_publication::CopyPublication::Pending);
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
    let result = rebind_inputs(target, &original)?;
    map.insert(reference.clone(), result.clone());
    Ok(result)
}
fn rebind_inputs(
    target: &mut ProjectBundle,
    original: &OwnedInputManifest,
) -> Result<ObjectRef, BundleError> {
    let rebound = OwnedInputManifest {
        project_id: Some(target.project_id.clone()),
        session_id: None,
        owner_epoch: target.store.owner_epoch(),
        entries: original.entries.clone(),
    };
    Ok(target.store.admit_record(&rebound)?.reference.clone())
}
fn result(
    mut target: ProjectBundle,
    before: DestinationBefore,
    prepared: PreparedDestination,
    source: impl Into<CopySource>,
    rebindings: BTreeMap<ObjectRef, ObjectRef>,
    publication: Result<BundleSaveReport, BundleError>,
) -> Result<BundleCopy, BundleError> {
    let mut source = source.into();
    let status = match publication {
        Ok(report) => CopyStatus::Committed(report),
        Err(error) => {
            if !matches!(
                target.publication_state,
                crate::bundle::PublicationState::Unknown
                    | crate::bundle::PublicationState::Committed
            ) {
                target.copy_publication = None;
            }
            let uncertain = target.unsettled
                || target.store.with_namespace(|n| Ok(n.is_uncertain()))?
                || target.store.has_unsettled_objects()?;
            let root_unknown = target.publication_state == crate::bundle::PublicationState::Unknown;
            let committed = target.publication_state == crate::bundle::PublicationState::Committed;
            if !root_unknown && !committed {
                target.settle_pending_publication(false)?;
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
        source,
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
    pub fn save_as_with_limits(
        &mut self,
        path: &Path,
        limits: StorageLimits,
    ) -> Result<SaveAsOutcome, BundleError> {
        self.save_as_with_admission(path, limits, None)
    }
    pub fn save_as_admitted(
        &mut self,
        inspection: &crate::namespace::DestinationInspection,
        limits: StorageLimits,
    ) -> Result<SaveAsOutcome, BundleError> {
        self.save_as_with_admission(inspection.path(), limits, Some(inspection))
    }
    fn save_as_with_admission(
        &mut self,
        path: &Path,
        limits: StorageLimits,
        inspection: Option<&crate::namespace::DestinationInspection>,
    ) -> Result<SaveAsOutcome, BundleError> {
        self.with_person_publication(|bundle| bundle.copy_person(path, limits, inspection))
    }
    #[allow(clippy::too_many_lines, reason = "one frozen Save As transaction")]
    fn copy_person(
        &mut self,
        path: &Path,
        limits: StorageLimits,
        inspection: Option<&crate::namespace::DestinationInspection>,
    ) -> Result<SaveAsOutcome, BundleError> {
        // Only a definitely unpublished root with cleanup uncertainty may use retained reads.
        // Unknown roots and pending commands still require their ordinary reconciliation.
        let cleanup_only = self.cleanup_pending()
            && matches!(
                self.publication_state,
                crate::bundle::PublicationState::NeverAttempted
                    | crate::bundle::PublicationState::DefinitelyRefused
            )
            && self.pending_publication.is_none()
            && self.pending_command.is_none()
            && self.root.is_some();
        if let Some(pending) = &self.pending_publication {
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
        for checkpoint in self.branches.values() {
            let record = self
                .store
                .read_copy_record::<crate::operation_history::StoredOwnerRecord>(
                    &checkpoint.owner_ref,
                    cleanup_only,
                )?;
            record.sizing.limits()?;
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
        if crate::same_project_file(&resolved, &self.store.resolved_root()?) {
            if let Some(inspection) = inspection {
                let current = ProjectStore::inspect_borrowed_destination(path, &[])?;
                if current.identity()? != inspection.identity()? {
                    return Err(BundleError::Identity(
                        "Save As admitted parent changed".into(),
                    ));
                }
            }
            if cleanup_only {
                return Err(BundleError::Unsettled);
            }
            let root = self
                .root
                .as_ref()
                .ok_or_else(|| BundleError::Corrupt("no acknowledged copy source".into()))?;
            return Ok(SaveAsOutcome::SameProject(BundleSaveReport {
                position: self.checkpoint(&self.manual_branch_id)?.position,
                history_sha256: self
                    .checkpoint(&self.manual_branch_id)?
                    .identity
                    .to_string(),
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
        let (mut target, before, prepared) = if let Some(inspection) = inspection {
            ProjectStore::revalidate_rescue_destination(inspection, &[&self.store])?;
            destination_admitted(
                path,
                limits,
                Some(inspection),
                &mut || {
                    ProjectStore::inspect_borrowed_destination(inspection.path(), &[&self.store])?;
                    Ok(())
                },
                &self.history_pool,
            )?
        } else {
            destination(path, limits, &self.history_pool)?
        };
        if cleanup_only && before.root.is_some() {
            return Err(BundleError::Identity(
                "cleanup-only destination appeared during admission".into(),
            ));
        }
        let mut map = BTreeMap::new();
        let publication = target.with_person_publication(|target| {
            origins(
                target,
                &before,
                std::iter::once(self.project_id.clone()).chain(self.origin_project_ids.clone()),
                self.origin_session_ids.clone(),
            )?;
            target
                .store
                .copy_graph_from_source(&self.store, &roots, cleanup_only)?;
            #[cfg(any(test, feature = "test-util"))]
            crate::io_observer::check_failure("copy_backup", &prepared.path)?;
            let snapshot = target
                .store
                .admit_root_checkpoint(self.admission.raw_bytes(), &roots)?;
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
            target.capture_copy_tasks(self)?;
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
                // A new project cannot replay its source's command authority.
                // Retained documents/commands stay exact; selection starts the
                // destination stream under its own project and install epoch.
                checkpoint.stream = None;
                checkpoint.install = None;
                checkpoint.input_manifest_ref =
                    rewrite_inputs(target, &original.input_manifest_ref, &mut map)?;
                checkpoint.head_id = HeadId::fresh()?;
                checkpoint.predecessor = Some(original.head_id.clone());
                checkpoint.generation = generation;
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
            #[cfg(any(test, feature = "test-util"))]
            crate::test_hook("copy_before_publish");
            #[cfg(any(test, feature = "test-util"))]
            crate::io_observer::check_failure("copy_verify", &prepared.path)?;
            for reference in target.store.closure(&roots)? {
                target.store.verify(&reference)?;
            }
            let checkpoint = self.checkpoint(&self.manual_branch_id)?;
            // The verified acknowledged graph has already been copied. Read
            // its destination bytes, including when A retains cleanup-only IO.
            let snapshot = checkpoint
                .open(&target.store, &self.history_pool)?
                .into_reader()?
                .into_capture()?
                .into_install()?;
            let revision = kinewright_core::TimelineRevision(
                checkpoint
                    .revision
                    .0
                    .checked_add(1)
                    .ok_or(BundleError::Capacity("install revision"))?,
            );
            let inputs = target
                .checkpoint(&target.manual_branch_id)?
                .input_manifest_ref
                .clone();
            crate::bundle_commit::BundleCommit::publish_snapshot(
                target, &snapshot, revision, &inputs,
            )
        });
        let copy = result(target, before, prepared, source, map, publication)?;
        self.pending_copy_source = copy.pending_source();
        Ok(SaveAsOutcome::Copied(Box::new(copy)))
    }
}
pub struct SessionStore {
    store: ProjectStore,
    session_id: SessionId,
    pending_copy_source: Option<PendingCopySource>,
    borrowed_sources: Vec<Arc<crate::source_boundary::RetainedBorrowedSource>>,
    borrowed_sources_closed: bool,
}
impl SessionStore {
    pub(crate) fn admitted_borrowed_snapshot(
        &self,
        admission: &SessionId,
        identity: crate::operation_history::SnapshotIdentity,
    ) -> Result<Option<crate::source_boundary::BorrowedSnapshotBinding>, BundleError> {
        if self.borrowed_sources_closed {
            return Err(BundleError::Identity("originating source is closed".into()));
        }
        let retained = self
            .borrowed_sources
            .iter()
            .find(|source| source.descriptor.admission_id() == admission)
            .ok_or_else(|| {
                BundleError::Identity("source retention was not re-established".into())
            })?;
        crate::source_boundary::BorrowedSourceService::new(retained)
            .admitted_snapshot(&retained.descriptor, identity)
    }
    pub(crate) fn ensure_borrowed_snapshot(
        &self,
        admission: &SessionId,
        history: &crate::operation_actor::PortableHistory,
    ) -> Result<crate::source_boundary::BorrowedSnapshotBinding, BundleError> {
        if self.borrowed_sources_closed {
            return Err(BundleError::Identity("originating source is closed".into()));
        }
        let retained = self
            .borrowed_sources
            .iter()
            .find(|r| r.descriptor.admission_id() == admission)
            .ok_or_else(|| {
                BundleError::Identity("source retention was not re-established".into())
            })?;
        crate::source_boundary::BorrowedSourceService::new(retained)
            .ensure_snapshot(&retained.descriptor, history)
    }

    /// Idempotent explicit Close; false means a reader/ordinary hold must retire.
    /// On error retain this `SessionStore` and retry: failed bytes stay charged.
    pub fn close_originating_source(&mut self) -> Result<bool, BundleError> {
        self.close_originating_context();
        self.pending_copy_source = None;
        self.store.close_temporary_source()
    }

    /// The originating process retains the actual pin/hold/reservations until Close.
    /// Returning, cloning or dropping the descriptor cannot release them.
    pub fn retain_borrowed_source(
        &mut self,
        roots: &[ObjectRef],
        extra_overlap_bytes: u64,
    ) -> Result<crate::source_boundary::BorrowedSourceDescriptor, BundleError> {
        self.check_borrowed_source_room()?;
        let retained = self
            .store
            .retain_borrowed_source(roots, extra_overlap_bytes)?;
        let descriptor = retained.descriptor.clone();
        self.borrowed_sources.push(Arc::new(retained));
        Ok(descriptor)
    }

    pub(crate) fn check_borrowed_source_room(&self) -> Result<(), BundleError> {
        if self.borrowed_sources_closed {
            return Err(BundleError::Identity("originating source is closed".into()));
        }
        if self.borrowed_sources.len() as u64 >= self.store.limits().pending_commands {
            return Err(BundleError::Capacity("retained borrowed source attempts"));
        }
        Ok(())
    }

    pub(crate) fn release_unpublished_borrowed_source(&mut self, admission: &SessionId) {
        self.borrowed_sources
            .retain(|source| source.descriptor.admission_id() != admission);
    }

    /// GUI-side verified reader lease. Only bytes from this reader cross the protocol.
    /// An outstanding lease keeps all actual source charges alive after Close.
    pub fn open_borrowed_source(
        &self,
        source: &crate::source_boundary::BorrowedSourceDescriptor,
        object: &ObjectRef,
    ) -> Result<impl std::io::Read + std::io::Seek + use<>, BundleError> {
        if self.borrowed_sources_closed {
            return Err(BundleError::Identity("originating source is closed".into()));
        }
        let retained = self
            .borrowed_sources
            .iter()
            .find(|r| r.descriptor == *source)
            .ok_or_else(|| {
                BundleError::Identity("source retention was not re-established".into())
            })?;
        crate::source_boundary::BorrowedSourceService::new(retained)
            .open_borrowed_source(source, object)
    }

    pub fn borrowed_source_service(
        &self,
        source: &crate::source_boundary::BorrowedSourceDescriptor,
    ) -> Result<crate::source_boundary::BorrowedSourceService, BundleError> {
        if self.borrowed_sources_closed {
            return Err(BundleError::Identity("originating source is closed".into()));
        }
        let retained = self
            .borrowed_sources
            .iter()
            .find(|r| r.descriptor == *source)
            .ok_or_else(|| {
                BundleError::Identity("source retention was not re-established".into())
            })?;
        Ok(crate::source_boundary::BorrowedSourceService::new(retained))
    }

    /// Bounded callback for an authenticated attachment request. No locator is accepted.
    pub fn read_borrowed_source(
        &self,
        source: &crate::source_boundary::BorrowedSourceDescriptor,
        object: &ObjectRef,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, BundleError> {
        self.borrowed_source_service(source)?
            .read_borrowed_source(source, object, offset, buffer)
    }

    /// Explicit originating Close, never host detach/disconnect. Repeats are inert;
    /// active GUI reader leases retain their admission until reader retirement.
    pub fn close_borrowed_sources(&mut self) {
        self.borrowed_sources_closed = true;
        for retained in &self.borrowed_sources {
            retained
                .accepting_reads
                .store(false, std::sync::atomic::Ordering::SeqCst);
        }
        self.borrowed_sources.clear();
    }

    #[must_use]
    pub fn has_borrowed_sources(&self) -> bool {
        !self.borrowed_sources.is_empty()
    }

    pub(crate) fn close_originating_context(&mut self) {
        close_copy_source(&mut self.pending_copy_source);
        self.close_borrowed_sources();
    }

    /// Bind a newly created unsaved edit before distributing handles.
    pub fn attach_core(&self, core: &Core) -> Result<crate::ProjectAttachment, BundleError> {
        crate::history_save::ProjectAttachment::new(core).map_err(crate::bundle::operation_error)
    }
    pub fn create(parent: &Path, limits: StorageLimits) -> Result<Self, BundleError> {
        limits.validate()?;
        let session_id = SessionId::fresh()?;
        let namespace = ProjectNamespace::create_session(parent, &session_id)?;
        crate::namespace::note("session_store_create", &namespace.identity().resolved_root);
        let store = ProjectStore::new(namespace, None, Some(session_id.clone()), limits)?;
        Ok(Self {
            store,
            session_id,
            pending_copy_source: None,
            borrowed_sources: vec![],
            borrowed_sources_closed: false,
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
        crate::history_save::bind(core).map_err(crate::bundle::operation_error)?;
        if core
            .attachment_state()
            .map_err(crate::bundle::operation_error)?
            == kinewright_core::AttachmentState::Sealed
        {
            return Err(BundleError::HistoryBarrierInvariant);
        }
        settle_copy_source(&mut self.pending_copy_source)?;
        let capture = core
            .capture_state()
            .map_err(crate::bundle::operation_error)?;
        let count = capture.snapshot.record_references().count() as u64 + 1;
        let root_charge = self.store.reserve(
            0,
            count
                .checked_mul(512)
                .ok_or(BundleError::Capacity("capture record roots"))?,
            0,
            false,
        )?;
        let roots = root_charge.hold(
            std::iter::once(inputs.clone())
                .chain(capture.snapshot.record_references().cloned())
                .collect::<Vec<_>>(),
        );
        let history_bytes = capture.snapshot.encoded_bytes()?;
        let source = self
            .store
            .begin_copy(&roots, frozen_size(&self.store, &roots, history_bytes)?)?;
        let selected = capture.snapshot.into_install()?;
        let revision = kinewright_core::TimelineRevision(
            capture
                .change
                .revision
                .0
                .checked_add(1)
                .ok_or(BundleError::Capacity("install revision"))?,
        );
        let (mut target, before, prepared) = destination(path, limits, core.history_pool())?;
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
            target.evidence.push(inputs.clone());
            #[cfg(any(test, feature = "test-util"))]
            crate::test_hook("copy_before_publish");
            #[cfg(any(test, feature = "test-util"))]
            crate::io_observer::check_failure("copy_verify", &prepared.path)?;
            for reference in target.store.closure(&roots)? {
                target.store.verify(&reference)?;
            }
            crate::bundle_commit::BundleCommit::publish_snapshot(
                &mut target,
                &selected,
                revision,
                &rebound,
            )
        })();
        let copy = result(target, before, prepared, source, map, publication)?;
        self.pending_copy_source = copy.pending_source();
        Ok(copy)
    }
}

/// A1 copies the selected accepted cut without reconciling A or acknowledging its tentative command.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn rescue_copy(
    source: &ProjectStore,
    named: Option<&ProjectBundle>,
    inputs: &ObjectRef,
    roots: &[ObjectRef],
    inspection: &crate::namespace::DestinationInspection,
    on_mutation: &mut dyn FnMut() -> Result<(), BundleError>,
    limits: StorageLimits,
    core: &Core,
    selected: &crate::bundle_rescue::FrozenRescueCapture,
    captures: &[std::sync::Arc<
        crate::storage_budget::StorageReservation<crate::bundle_rescue::FrozenRescueCapture>,
    >],
    record: &crate::rescue_record::RescueCopyV1,
) -> Result<BundleCopy, BundleError> {
    let copied_bytes = source
        .closure_with_inventory(roots, false)?
        .iter()
        .try_fold(
            captures.iter().try_fold(0_u64, |n, capture| {
                let snapshot = capture
                    .snapshot
                    .lock()
                    .map_err(|_| BundleError::Unsettled)?;
                let bytes = snapshot.as_ref().map_or(
                    Ok(0),
                    crate::operation_history::OwnerSnapshot::encoded_bytes,
                )?;
                n.checked_add(bytes)
                    .ok_or(BundleError::Capacity("rescue captures"))
            })?,
            |n, r| {
                n.checked_add(r.byte_length)
                    .ok_or(BundleError::Capacity("rescue copy overlap"))
            },
        )?;
    let hold = source.begin_copy_source(roots, copied_bytes, true)?;
    let (mut target, before, prepared) = destination_admitted(
        inspection.path(),
        limits,
        Some(inspection),
        on_mutation,
        core.history_pool(),
    )?;
    if before.root.is_some() || before.had_assets {
        return Err(BundleError::Identity(
            "rescue destination must be fresh and unoccupied".into(),
        ));
    }
    let mut map = BTreeMap::new();
    let publication = (|| {
        let mut record = record.clone();
        record.project_id = target.project_id.clone();
        let mut archive_projects = Vec::new();
        let mut archive_sessions = Vec::new();
        for capture in captures {
            let owner = capture
                .archive_store
                .lock()
                .map_err(|_| BundleError::Unsettled)?;
            if let Some(owner) = owner.as_ref() {
                let (project, session) = owner.input_owner();
                archive_projects.extend(project);
                archive_sessions.extend(session);
            }
        }
        if let Some(original) = named {
            origins(
                &mut target,
                &before,
                std::iter::once(original.project_id.clone())
                    .chain(original.origin_project_ids.clone())
                    .chain(archive_projects),
                original
                    .origin_session_ids
                    .iter()
                    .cloned()
                    .chain(archive_sessions),
            )?;
        } else {
            origins(
                &mut target,
                &before,
                archive_projects,
                source.input_owner().1.into_iter().chain(archive_sessions),
            )?;
        }
        target.store.copy_rescue_graph(source, roots)?;
        let mut selected_inputs =
            (*source.read_copy_record::<OwnedInputManifest>(inputs, true)?).clone();
        let kinewright_core::Event::QueryResult(kinewright_core::QueryResult::Document(current)) =
            core.request(kinewright_core::Command::Query(
                kinewright_core::Query::Document,
            ))
            .map_err(|refusal| crate::bundle::operation_error(refusal.error))?
        else {
            return Err(BundleError::HistoryBarrierInvariant);
        };
        crate::bundle_commit::retain_document_inputs(&current, &mut selected_inputs);
        let rebound = rebind_inputs(&mut target, &selected_inputs)?;
        map.insert(inputs.clone(), rebound.clone());
        let mut sources = BTreeSet::from([rebound.clone()]);
        if let Some(original) = named {
            target.manual_branch_id = original.manual_branch_id.clone();
            target.capture_copy_tasks(original)?;
            target.generation = original.generation;
            target.root.clone_from(&original.root);
            target.recovery = original.recovery.clone();
            target.recovery.predecessors = original
                .recovery
                .predecessors
                .iter()
                .enumerate()
                .filter(|(i, _)| {
                    !record
                        .omissions
                        .iter()
                        .any(|o| o.role == format!("predecessor:{i}"))
                })
                .map(|(_, r)| r.clone())
                .collect();
            if record.omissions.iter().any(|o| o.role == "pre_migration") {
                target.recovery.pre_migration = None;
            }
            target.recovery.staging = None;
            target.recovery.activation_project_id = Some(target.project_id.clone());
            target.recovery.manual_branch_id = Some(target.manual_branch_id.clone());
            target.recovery.namespace_root_file_name = prepared
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_owned);
            target.receipts = original.receipts.clone();
            target.issuers = original.issuers.clone();
            for issuer in target.issuers.values_mut() {
                issuer.retired = true;
            }
            for reference in &original.acknowledged_evidence()? {
                let inherited =
                    if reference.record_kind == crate::bundle_records::RecordKind::OwnedEvidence {
                        crate::rescue_record::parse(&source.read_copy_evidence(reference, true)?)?
                    } else {
                        None
                    };
                if let Some(inherited) = inherited {
                    let attempts = inherited
                        .attempts
                        .into_iter()
                        .filter(|a| !record.attempts.contains(a))
                        .collect::<Vec<_>>();
                    record.compact(attempts)?;
                    if inherited.summary.unresolved {
                        let (count, overflow) = record
                            .summary
                            .count
                            .overflowing_add(inherited.summary.count);
                        record.summary.count = if overflow { u64::MAX } else { count };
                        record.summary.count_overflow |=
                            overflow || inherited.summary.count_overflow;
                        record.summary.unresolved = true;
                        record.summary.digest = kinewright_media::sha256_bytes(
                            &[
                                record.summary.digest.as_bytes(),
                                inherited.summary.digest.as_bytes(),
                            ]
                            .concat(),
                        );
                    }
                } else {
                    target.evidence.push(reference.clone());
                }
            }
            let mut branches = BTreeMap::new();
            let mut index = BTreeMap::new();
            let mut seals = Vec::new();
            for (id, prior) in &original.branches {
                if id == &original.manual_branch_id {
                    branches.insert(id.clone(), prior.clone());
                    continue;
                }
                let mut checkpoint = prior.clone();
                checkpoint.input_manifest_ref =
                    rewrite_inputs(&mut target, &checkpoint.input_manifest_ref, &mut map)?;
                let sealed = target.store.admit_record(&checkpoint)?;
                index.insert(id.clone(), sealed.reference.clone());
                seals.push(sealed);
                branches.insert(id.clone(), checkpoint);
            }
            let index = target.store.admit_record(&index)?;
            if let Some(root) = &mut target.root {
                root.branch_index_ref = index.reference.clone();
            }
            target.branches = branches;
            if record
                .omissions
                .iter()
                .any(|o| o.role == "acknowledged_root")
            {
                record.acknowledged_absence =
                    Some("unavailable acknowledged root omitted by explicit consent".into());
            } else {
                let ack = target.store.admit_root_checkpoint(
                    original.admission.raw_bytes(),
                    &original.root_references()?,
                )?;
                let (projected, origin) = target.project_recovery_root(&ack.reference)?;
                record.acknowledged_root = Some(projected);
                record.acknowledged_absence = None;
                record.recovery_origin = Some(origin);
            }
        }
        // Keep each unresolved source cut cold and exact before releasing its
        // full RAM holder. This bounded list is the originating session's four
        // attempts, not an indefinitely retained journal of folded commands.
        let mut capture_seals = Vec::new();
        for capture in captures {
            let snapshot = capture
                .snapshot
                .lock()
                .map_err(|_| BundleError::Unsettled)?;
            let reference = if let Some(snapshot) = snapshot.as_ref() {
                sources.extend(
                    snapshot
                        .origins()
                        .iter()
                        .flat_map(|origin| origin.inputs.iter().cloned()),
                );
                let bindings =
                    crate::operation_publication::bindings(&target.store, &sources, &rebound)?;
                let sealed = snapshot.persist_checkpoint(&mut target.store, |resource| {
                    Ok(bindings.get(resource).cloned())
                })?;
                let checkpoint = crate::operation_checkpoint::Checkpoint::from_sealed(
                    crate::operation_checkpoint::Header {
                        branch: target.manual_branch_id.clone(),
                        head: HeadId::fresh()?,
                        revision: capture.cut.change().revision,
                        inputs: rebound.clone(),
                        generation: target
                            .generation
                            .checked_add(1)
                            .ok_or(BundleError::Capacity("rescue archive generation"))?,
                        predecessor: None,
                        command_cut: None,
                        take_map: None,
                        stream: None,
                    },
                    &sealed,
                )?;
                let checkpoint = target.store.admit_record(&checkpoint)?;
                let reference = checkpoint.reference.clone();
                capture_seals.push(sealed.into_object());
                capture_seals.push(checkpoint);
                *capture.archive.lock().map_err(|_| BundleError::Unsettled)? =
                    Some(reference.clone());
                reference
            } else {
                let reference = capture
                    .archive
                    .lock()
                    .map_err(|_| BundleError::Unsettled)?
                    .as_ref()
                    .cloned()
                    .ok_or(BundleError::Unsettled)?;
                let store = capture
                    .archive_store
                    .lock()
                    .map_err(|_| BundleError::Unsettled)?;
                target.store.copy_rescue_graph(
                    store.as_ref().ok_or(BundleError::Unsettled)?,
                    std::slice::from_ref(&reference),
                )?;
                reference
            };
            if !record.capture_checkpoints.contains(&reference) {
                record.capture_checkpoints.push(reference);
            }
            drop(snapshot);
            if !std::ptr::eq(&raw const ***capture, selected) {
                // This older capture now has a synced, independently pinned
                // archive and an actual namespace owner. It needs no full
                // in-memory history while the next rescue is published, even
                // if that publication later refuses or needs cleanup.
                capture.release_snapshot(&target.store)?;
            }
        }
        target.retain_rescue_record(&record)?;
        #[cfg(any(test, feature = "test-util"))]
        crate::io_observer::check_failure("rescue_record_before_publish", &prepared.path)?;
        let mut selected_state = selected
            .snapshot
            .lock()
            .map_err(|_| BundleError::Unsettled)?;
        let selected_state = selected_state.as_mut().ok_or(BundleError::HistoryChanged)?;
        let epoch = selected
            .cut
            .change()
            .position
            .epoch
            .checked_add(1)
            .ok_or(BundleError::Capacity("install epoch"))?;
        let revision = kinewright_core::TimelineRevision(
            selected
                .cut
                .change()
                .revision
                .0
                .checked_add(1)
                .ok_or(BundleError::Capacity("install revision"))?,
        );
        selected_state.with_install_view(epoch, |view| {
            crate::bundle_commit::BundleCommit::publish_snapshot(
                &mut target,
                view,
                revision,
                &rebound,
            )
        })?
    })();
    result(target, before, prepared, hold, map, publication)
}
