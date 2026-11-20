//! New/Open to Close ownership. Focus/view handles never own persistence resources.
#![allow(clippy::missing_errors_doc)]
#[cfg(any(test, feature = "test-util"))]
use crate::bundle_rescue::RescueConsent;
use crate::history_save::PublicationTag;
use crate::operation_actor::Core;
use crate::{
    ProjectAttachment,
    bundle::{
        BundleError, BundleSaveReport, BundleWriteAdmission, LoadedBundle, ProjectBundle,
        PublicationState,
    },
    bundle_copy::{BundleCopy, CopyStatus, SessionStore},
    bundle_records::ObjectRef,
    bundle_rescue::{
        FrozenRescueCapture, MissingOwnedUse, ReconcilePhase, RescueEligibility, RescueProbe,
    },
    namespace::DestinationInspection,
    owned_inputs::OwnedInputManifest,
    storage_budget::{StorageLimits, StorageReservation},
    store::{ProjectStore, RetainedReadAdmission},
};
use std::path::{Path, PathBuf};
mod durable;
mod manual;
mod owned_media;
mod release;
pub use manual::{ManualAuthority, ManualHistoryCommit, ManualRecoveryEndpoint};
mod recovery;
mod retained_work;
mod review;
pub use retained_work::{
    RetainedTaskDiscovery, RetainedTaskWork, RetainedWorkFailure, RetainedWorkKind,
};
mod switch;
pub use switch::PreparedGuiSwitch;
mod rescue;
mod rescue_selection;
pub use durable::{
    BranchBinding, DurableBranchRole, DurableBranchSessionGuard, FrozenTaskResult,
    PersistedTaskCheckpoint,
};
type RescueSource<'a> = (&'a ProjectStore, Vec<(ObjectRef, MissingOwnedUse)>);
type HistoryDigestCache = Option<crate::operation_actor::ChangeId>;

pub struct ActiveAssociation {
    path: PathBuf,
    saved: Option<String>,
    saved_position: Option<kinewright_core::undo::CommandKey>,
}
impl ActiveAssociation {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
    #[must_use]
    pub fn saved_digest(&self) -> Option<&str> {
        self.saved.as_deref()
    }
    #[must_use]
    pub const fn saved_position(&self) -> Option<kinewright_core::undo::CommandKey> {
        self.saved_position
    }
}
pub enum RetainedDestination {
    Named(Box<ProjectBundle>),
    FirstSave(Box<BundleCopy>),
}
impl RetainedDestination {
    fn bundle_mut(&mut self) -> Result<&mut ProjectBundle, BundleError> {
        match self {
            Self::Named(bundle) => Ok(bundle),
            Self::FirstSave(copy) => copy.destination_owner_mut(),
        }
    }
    fn bundle(&self) -> Result<&ProjectBundle, BundleError> {
        match self {
            Self::Named(bundle) => Ok(bundle),
            Self::FirstSave(copy) => copy.destination_owner(),
        }
    }
    fn reconcile(&mut self, core: &Core) -> Result<Option<BundleSaveReport>, BundleError> {
        match self {
            Self::Named(bundle) => {
                if bundle.pending_command.is_some() {
                    bundle.reconcile_command(core)?;
                    Ok(None)
                } else {
                    bundle.reconcile_destination(Some(core))
                }
            }
            Self::FirstSave(copy) => copy.reconcile_retained(core),
        }
    }
    fn save(&mut self, core: &Core, inputs: &ObjectRef) -> Result<BundleSaveReport, BundleError> {
        match self {
            Self::Named(bundle) => bundle.save(core, inputs),
            Self::FirstSave(copy) => copy.save_retained(core, inputs),
        }
    }
}

/// An adopted attempt owns actual actor/source objects independently of its namespace owner.
pub struct RetainedAttempt {
    state: PublicationState,
    pub(crate) tag: PublicationTag,
    pub(crate) core: Option<Core>,
    pub(crate) capture: std::sync::Arc<StorageReservation<FrozenRescueCapture>>,
    pub(crate) project_id: crate::bundle_records::ProjectId,
    pub(crate) pending_command:
        Option<(crate::receipts::CommandId, crate::receipts::CommandDigest)>,
    pub(crate) ticket: Option<crate::AttemptBinding>,
}
#[derive(Default)]
pub struct RescueSessionState {
    attempts: Vec<RetainedAttempt>,
    slots: RescueSlots,
}
#[derive(Default)]
pub struct RescueSlots {
    consumed: Vec<PublicationTag>,
    reserved: bool,
}
pub struct RescueReservation {
    source: PublicationTag,
}
impl RetainedAttempt {
    #[must_use]
    pub const fn publication_state(&self) -> PublicationState {
        self.state
    }
    #[must_use]
    pub fn actor(&self) -> Option<kinewright_core::ActorIdentity> {
        self.core.as_ref().map(Core::actor_identity)
    }
    #[must_use]
    pub const fn tag(&self) -> &PublicationTag {
        &self.tag
    }
    #[must_use]
    pub fn reads(&self) -> &RetainedReadAdmission {
        self.capture.reads()
    }
    #[must_use]
    pub const fn ticket(&self) -> Option<&crate::AttemptBinding> {
        self.ticket.as_ref()
    }
}
impl RescueSessionState {
    fn settle(&mut self, tag: &PublicationTag, state: PublicationState) {
        if state != PublicationState::Unknown {
            for attempt in &mut self.attempts {
                if &attempt.tag == tag && attempt.state == PublicationState::Unknown {
                    attempt.state = state;
                }
            }
        }
    }
}
impl RescueSlots {
    #[must_use]
    pub fn consumed(&self) -> usize {
        self.consumed.len()
    }
    pub fn check(&self, source: &PublicationTag) -> Result<(), BundleError> {
        let needed = usize::from(!self.consumed.contains(source)) + 1;
        if self.reserved || self.consumed.len() + needed > 4 {
            return Err(BundleError::Capacity("four rescue attempts"));
        }
        Ok(())
    }
    /// Called only when the owning lifecycle adopts actual source holds at first mutation.
    pub fn begin(&mut self, source: PublicationTag) -> Result<RescueReservation, BundleError> {
        self.check(&source)?;
        if !self.consumed.contains(&source) {
            self.consumed.push(source.clone());
        }
        self.reserved = true;
        crate::namespace::note("rescue_slot_begin", &source.destination);
        Ok(RescueReservation { source })
    }
    pub fn finish(
        &mut self,
        reservation: RescueReservation,
        unknown: Option<PublicationTag>,
    ) -> Result<(), BundleError> {
        if !self.reserved || !self.consumed.contains(&reservation.source) {
            return Err(BundleError::HistoryBarrierInvariant);
        }
        if let Some(tag) = unknown {
            if self.consumed.contains(&tag) {
                return Err(BundleError::HistoryBarrierInvariant);
            }
            self.consumed.push(tag);
        }
        self.reserved = false;
        drop(reservation);
        Ok(())
    }
}

pub struct Ja1EditingSession {
    core: Core,
    attachment: ProjectAttachment,
    source: Option<SessionStore>,
    destination: Option<RetainedDestination>,
    inputs: ObjectRef,
    input_leases: Vec<crate::owned_inputs::OwnedInputLease>,
    association: Option<ActiveAssociation>,
    rescue: RescueSessionState,
    prepared: Option<crate::bundle_switch::PreparedProjectSelection>,
    recovery_save_pending: bool,
    selection_result: Option<(crate::bundle_recovery::RecoverySelection, BundleSaveReport)>,
    durable_sessions: Vec<durable::RegisteredSession>,
    pending_candidate: Option<StorageReservation<durable::PendingCandidateCommit>>,
    retained_destinations: Vec<(RetainedDestination, Option<Core>)>,
    originating_session: crate::bundle_records::SessionId,
    dirty_cache: std::cell::RefCell<HistoryDigestCache>,
    input_usage: std::sync::Arc<std::sync::Mutex<crate::storage_budget::StorageUsage>>,
    retained_task_work_cache: std::cell::RefCell<Option<(String, RetainedTaskDiscovery)>>,
    host_attached: bool,
    host_retention_lost: bool,
    host_cut: Option<crate::operation_actor::AcceptedCut>,
    host_published_digest: Option<String>,
    host_rescue_actors: Vec<Core>,
    host_source_admission: Option<crate::bundle_records::SessionId>,
    preliminary_save: Option<PreliminarySave>,
    pending_copy_cut: Option<crate::operation_actor::AcceptedCut>,
    pending_import_source: Option<PreparedHostAttachment>,
}

/// One transient publisher receipt, backed by the surviving ordinary L branch.
/// It owns no actor, history snapshot, pin, namespace or additional ledger.
struct PreliminarySave {
    report: BundleSaveReport,
    branch: crate::bundle_records::BranchId,
    pending: bool,
}

/// GUI-owned source/actor half survives this handover. Only wire values and a
/// weak, read-only GUI callback service are returned; no source ledger moves.
pub struct PreparedHostAttachment {
    pub named_path: Option<PathBuf>,
    pub expected_root_sha256: Option<String>,
    pub project_id: Option<crate::bundle_records::ProjectId>,
    pub history: crate::operation_actor::PortableHistory,
    pub inputs: ObjectRef,
    pub source: Option<crate::source_boundary::BorrowedSourceDescriptor>,
    pub source_service: Option<crate::source_boundary::BorrowedSourceService>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostActivationKind {
    Rescue,
    Recovery,
    Switch,
}
fn portable_checkpoint(
    bundle: &ProjectBundle,
    store: &ProjectStore,
    checkpoint: &crate::bundle_records::BranchCheckpoint,
    retained: bool,
) -> Result<crate::operation_actor::PortableHistory, BundleError> {
    let selected = if retained {
        // The exact preceding publisher report has already been matched to
        // its surviving branch. A later unresolved selection does not revoke
        // read-only access to that committed immutable cut.
        store.with_namespace(|namespace| {
            namespace.verify_read_authority()?;
            crate::operation_history::StoredOwnerRecord::read_namespace(
                namespace,
                checkpoint,
                bundle.history_pool(),
                &store.usage_arc(),
                store.limits(),
            )
            .map_err(BundleError::from)
        })?
    } else {
        checkpoint.open(store, bundle.history_pool())?
    };
    let snapshot = selected.into_reader()?.into_capture()?.into_transport()?;
    Ok(crate::operation_actor::PortableHistory {
        snapshot: std::sync::Arc::new(snapshot),
        branch: checkpoint.branch_id.clone(),
        revision: checkpoint.revision,
    })
}
impl Ja1EditingSession {
    /// Read the checkpoint identified by the publisher's exact report. Current
    /// actor history is deliberately irrelevant, including edits accepted after IO.
    pub fn published_history(
        &self,
        report: &BundleSaveReport,
    ) -> Result<crate::operation_actor::PortableHistory, BundleError> {
        let bundle = self.active_bundle()?;
        let store = bundle.store.person_publication_store();
        if let Some(saved) = &self.preliminary_save
            && report.project_id == saved.report.project_id
            && report.generation == saved.report.generation
            && report.head_id == saved.report.head_id
            && report.history_sha256 == saved.report.history_sha256
            && report.root_sha256 == saved.report.root_sha256
            && report.byte_length == saved.report.byte_length
        {
            let checkpoint = bundle.checkpoint(&saved.branch)?;
            if report.project_id != *bundle.project_id()
                || checkpoint.head_id != report.head_id
                || checkpoint.identity.to_string() != report.history_sha256
                || checkpoint.generation != report.generation
                || !crate::same_project_file(&report.path, &store.resolved_root()?)
            {
                return Err(BundleError::Identity(
                    "preliminary publisher report differs from surviving L checkpoint".into(),
                ));
            }
            store.with_namespace(|namespace| namespace.verify_read_authority())?;
            return portable_checkpoint(bundle, &store, checkpoint, true);
        }
        let root = bundle.root().ok_or(BundleError::HistoryBarrierInvariant)?;
        let checkpoint = bundle.checkpoint(bundle.manual_branch_id())?;
        let raw = bundle.admission.raw_bytes();
        if report.project_id != *bundle.project_id()
            || report.generation != root.generation
            || report.head_id != checkpoint.head_id
            || report.history_sha256 != checkpoint.identity.to_string()
            || report.root_sha256 != kinewright_media::sha256_bytes(raw)
            || report.byte_length != raw.len() as u64
            || !crate::same_project_file(&report.path, &store.resolved_root()?)
        {
            return Err(BundleError::Identity(
                "publisher report differs from the retained checkpoint".into(),
            ));
        }
        store.with_namespace(|namespace| {
            namespace.verify_read_authority()?;
            if namespace.read_root()?.as_deref() != Some(raw) {
                return Err(BundleError::Identity(
                    "published checkpoint root was replaced".into(),
                ));
            }
            Ok(())
        })?;
        portable_checkpoint(bundle, &store, checkpoint, false)
    }

    /// Explicit GUI context Close. The caller drains its source callback service
    /// and readers, then retries until true before dropping the source owner.
    pub fn close_originating_source(&mut self) -> Result<bool, BundleError> {
        self.host_lost()?;
        self.input_leases.clear();
        self.source
            .as_mut()
            .map_or(Ok(true), SessionStore::close_originating_source)
    }

    /// After explicit Close has drained source readers, retire the foreground
    /// history synchronously. Slow namespace/worker destruction may follow on a
    /// reaper; stale view handles must not keep this full pool lease alive.
    pub fn retire_closed_history(&self) -> Result<(), BundleError> {
        if !self.host_retention_lost {
            return Err(BundleError::Unsettled);
        }
        if matches!(
            self.core.change_id(),
            Err(crate::operation_actor::Error::Retired)
        ) {
            return Ok(());
        }
        // request_close already froze this exact cut. Reuse it even if a
        // failed handoff sealed the view; a sealed actor cannot be refrozen.
        let cut = if let Some(cut) = self.host_cut {
            cut
        } else {
            self.attachment
                .freeze()
                .map_err(crate::bundle::operation_error)?
        };
        self.attachment
            .retire(&cut)
            .map_err(crate::bundle::operation_error)
    }

    fn require_local_writer(&self) -> Result<(), BundleError> {
        if self.host_attached || self.host_retention_lost {
            return Err(BundleError::Identity(
                "source retention was not re-established for local writes".into(),
            ));
        }
        Ok(())
    }

    /// Update GUI association only from the authenticated publisher's reply.
    /// `complete_host_cut` must already have validated its returned history digest.
    pub fn set_host_association(
        &mut self,
        path: &Path,
        publisher_digest: &str,
    ) -> Result<(), BundleError> {
        if !self.host_attached
            || self.host_retention_lost
            || self.host_cut.is_some()
            || self.host_published_digest.as_deref() != Some(publisher_digest)
        {
            return Err(BundleError::Identity(
                "host publication has not completed its exact cut".into(),
            ));
        }
        self.association = Some(ActiveAssociation {
            path: path.to_path_buf(),
            saved: Some(publisher_digest.to_owned()),
            saved_position: Some(
                self.core
                    .change_id()
                    .map_err(crate::bundle::operation_error)?
                    .position,
            ),
        });
        Ok(())
    }

    fn prepare_handover_cut(
        &self,
    ) -> Result<
        (
            crate::operation_actor::AcceptedCut,
            crate::operation_history::SnapshotIdentity,
        ),
        BundleError,
    > {
        if self.host_attached
            || self.host_retention_lost
            || self.prepared.is_some()
            || self.recovery_save_pending
            || !self.rescue.attempts.is_empty()
            || !self.retained_destinations.is_empty()
            || !self.durable_sessions.is_empty()
        {
            return Err(BundleError::Identity(
                "host handover requires settled originating ownership".into(),
            ));
        }
        let was_live = self
            .core
            .attachment_state()
            .map_err(crate::bundle::operation_error)?
            == kinewright_core::AttachmentState::Live;
        let before = self
            .attachment
            .freeze()
            .map_err(crate::bundle::operation_error)?;
        let prepared = (|| {
            let selected = self
                .attachment
                .prepare_reinstall(
                    &before,
                    crate::operation_actor::CommandContext {
                        creation: crate::operation_history::CreationOrigin {
                            branch: self
                                .core
                                .creation_branch()
                                .map_err(crate::bundle::operation_error)?,
                            batch: crate::bundle_records::BatchId::fresh()?,
                        },
                        inputs: vec![],
                        approval: None,
                        transition: None,
                    },
                )
                .map_err(crate::bundle::operation_error)?;
            let identity = selected
                .identity()
                .map_err(crate::bundle::operation_error)?;
            self.attachment
                .commit_prepared(&before, selected, identity)
                .map_err(crate::bundle::operation_error)
        })();
        if prepared.is_err() && was_live {
            self.attachment
                .resume(&before)
                .map_err(crate::bundle::operation_error)?;
        }
        prepared.map(|cut| (cut, before.identity()))
    }

    /// Freeze producers before calling. A named writer is released only after its
    /// exact checkpoint and absence of unresolved owners/readers have been verified.
    #[allow(
        clippy::too_many_lines,
        reason = "one handover keeps clean-cut publication, namespace release and failure resumption in order"
    )]
    pub fn prepare_host_attachment(&mut self) -> Result<PreparedHostAttachment, BundleError> {
        let saved_handover = self.association.is_some() && !self.dirty()?;
        let (cut, source_identity) = self.prepare_handover_cut()?;
        // Reinstalling a clean cut starts its attachment epoch but accepts no
        // edit. Publish that exact continuity state before releasing the writer,
        // so the host and GUI can acquire a stream from the same saved identity.
        // A dirty bootstrap still requires the person's explicit Save.
        if saved_handover {
            let result = match self.destination.as_mut() {
                Some(RetainedDestination::Named(bundle)) => {
                    bundle.with_person_publication(|bundle| {
                        bundle.save_handover(&self.core, &self.inputs, &cut, source_identity)
                    })
                }
                _ => Err(BundleError::HistoryBarrierInvariant),
            };
            match result {
                Ok(report) => {
                    if let Some(association) = &mut self.association {
                        association.saved = Some(report.history_sha256);
                        association.saved_position = Some(report.position);
                    }
                }
                Err(error) => {
                    if !self.has_unresolved_publication() {
                        self.attachment
                            .resume(&cut)
                            .map_err(crate::bundle::operation_error)?;
                    }
                    return Err(error);
                }
            }
        }
        let capture = self
            .core
            .capture_state()
            .map_err(crate::bundle::operation_error)?;
        if capture.change != cut.change() {
            return Err(BundleError::HistoryChanged);
        }
        let root_count = capture.snapshot.record_references().count() as u64 + 1;
        let _roots_charge = self.store()?.reserve(
            0,
            root_count
                .checked_mul(512)
                .ok_or(BundleError::Capacity("source roots"))?,
            false,
        )?;
        let mut roots = capture
            .snapshot
            .record_references()
            .cloned()
            .collect::<Vec<_>>();
        roots.push(self.inputs.clone());
        roots.sort();
        roots.dedup();
        let portable = capture.into_portable()?;
        let result = (|| {
            let mut source = None;
            let mut source_service = None;
            let mut named_path = None;
            let mut expected_root_sha256 = None;
            let mut project_id = None;
            if let Some(destination) = &mut self.destination {
                let RetainedDestination::Named(bundle) = destination else {
                    return Err(BundleError::Unsettled);
                };
                if bundle.root.is_none() {
                    return Err(BundleError::Identity(
                        "Save once to convert this legacy project to Studio format before attaching its host. Your local edits remain available and saveable.".into(),
                    ));
                }
                if bundle.unsettled
                    || bundle.pending_command.is_some()
                    || bundle.pending_publication.is_some()
                    || bundle.pending_copy_source.is_some()
                    || !bundle.pending_evidence.is_empty()
                    || !self.input_leases.is_empty()
                {
                    return Err(BundleError::Unsettled);
                }
                named_path = Some(bundle.store.resolved_root()?);
                expected_root_sha256 =
                    Some(kinewright_media::sha256_bytes(bundle.admission.raw_bytes()));
                project_id = Some(bundle.project_id.clone());
                if let Err(error) = bundle.store.release_named_writer_for_host(
                    bundle.admission.raw_bytes(),
                    usize::from(bundle.durable.is_some()),
                ) {
                    // Release failures may have relinquished the OS lock. Never
                    // resume edits under a possibly lost named writer authority.
                    self.host_retention_lost = true;
                    return Err(error);
                }
                self.destination = None;
            } else {
                let owner = self
                    .source
                    .as_mut()
                    .ok_or(BundleError::HistoryBarrierInvariant)?;
                let descriptor = owner.retain_borrowed_source(&roots, portable.encoded_bytes()?)?;
                let service = owner.borrowed_source_service(&descriptor)?;
                service.ensure_snapshot(&descriptor, &portable)?;
                source_service = Some(service);
                source = Some(descriptor);
            }
            Ok(PreparedHostAttachment {
                named_path,
                expected_root_sha256,
                project_id,
                history: portable,
                inputs: self.inputs.clone(),
                source,
                source_service,
            })
        })();
        if result.is_ok() {
            self.host_attached = true;
            self.host_cut = Some(cut);
            self.host_source_admission = result
                .as_ref()
                .ok()
                .and_then(|prepared| prepared.source.as_ref())
                .map(|source| source.admission_id().clone());
        } else if !self.host_retention_lost {
            self.attachment
                .resume(&cut)
                .map_err(crate::bundle::operation_error)?;
        }
        result
    }

    /// A failed bootstrap has reaped its child before returning here. Reacquire
    /// the original named destination and verify its exact root before resuming
    /// the retained GUI cut; the dirty GUI history is never restored from disk.
    pub fn restore_failed_host_launch(
        &mut self,
        path: Option<&Path>,
        expected_root: Option<&str>,
    ) -> Result<(), BundleError> {
        let cut = *self.host_cut.as_ref().ok_or(BundleError::HistoryChanged)?;
        if let Some(path) = path {
            let loaded = LoadedBundle::read(path, self.core.history_pool())?;
            if Some(kinewright_media::sha256_bytes(loaded.admission().raw_bytes()).as_str())
                != expected_root
            {
                return Err(BundleError::HistoryChanged);
            }
            let bundle = loaded.into_writer()?;
            // Verify again under the reacquired, identity-pinned writer.
            bundle.store.with_namespace(|namespace| {
                namespace.verify()?;
                let bytes = namespace.read_root()?.ok_or(BundleError::HistoryChanged)?;
                if Some(kinewright_media::sha256_bytes(&bytes).as_str()) != expected_root {
                    return Err(BundleError::HistoryChanged);
                }
                Ok(())
            })?;
            self.destination = Some(RetainedDestination::Named(Box::new(bundle)));
        }
        self.attachment
            .resume(&cut)
            .map_err(crate::bundle::operation_error)?;
        self.host_cut = None;
        self.host_attached = false;
        self.host_source_admission = None;
        Ok(())
    }

    #[must_use]
    pub const fn host_retention_lost(&self) -> bool {
        self.host_retention_lost
    }

    /// Capture a foreground operation. No continuous/manual stream is introduced here.
    pub fn freeze_host_cut(&mut self) -> Result<crate::operation_actor::AcceptedCut, BundleError> {
        if !self.host_attached || self.host_retention_lost {
            return Err(BundleError::Identity(
                "source retention was not re-established".into(),
            ));
        }
        if let Some(cut) = &self.host_cut {
            self.ensure_host_snapshot(cut)?;
            return Ok(*cut);
        }
        let cut = self
            .attachment
            .freeze()
            .map_err(crate::bundle::operation_error)?;
        self.host_cut = Some(cut);
        self.ensure_host_snapshot(&cut)?;
        Ok(cut)
    }

    /// Admit the exact foreground snapshot on the one existing GUI source ledger.
    /// A named-only origin has no GUI temporary-source overlap to grow.
    pub fn ensure_host_snapshot(
        &self,
        cut: &crate::operation_actor::AcceptedCut,
    ) -> Result<Option<crate::source_boundary::BorrowedSnapshotBinding>, BundleError> {
        if !self.host_attached || self.host_retention_lost || self.host_cut.as_ref() != Some(cut) {
            return Err(BundleError::Identity(
                "snapshot admission is outside its exact foreground cut".into(),
            ));
        }
        self.host_source_admission
            .as_ref()
            .map(|admission| {
                let source = self
                    .source
                    .as_ref()
                    .ok_or(BundleError::HistoryBarrierInvariant)?;
                if let Some(binding) =
                    source.admitted_borrowed_snapshot(admission, cut.identity())?
                {
                    return Ok(binding);
                }
                source.ensure_borrowed_snapshot(
                    admission,
                    &self
                        .core
                        .portable_history()
                        .map_err(crate::bundle::operation_error)?,
                )
            })
            .transpose()
    }

    /// Install a publisher-returned history under the existing attachment proof.
    /// Callers keep every GUI producer fenced throughout this foreground operation.
    pub fn complete_host_cut(
        &mut self,
        cut: &crate::operation_actor::AcceptedCut,
        returned_history: Option<crate::operation_actor::PortableHistory>,
        publisher_digest: Option<&str>,
    ) -> Result<(), BundleError> {
        if !self.host_attached || self.host_retention_lost || self.host_cut.as_ref() != Some(cut) {
            return Err(BundleError::Identity(
                "host completion is outside its exact foreground cut".into(),
            ));
        }
        let mut installed = *cut;
        if let Some(history) = returned_history {
            if publisher_digest.is_none_or(|digest| history.identity().to_string() != digest) {
                return Err(BundleError::HistoryChanged);
            }
            if history.identity() != cut.identity() {
                let crate::operation_actor::PortableHistory {
                    snapshot,
                    branch,
                    revision,
                } = history;
                let role = snapshot.role();
                let snapshot = snapshot.receive(self.core.history_pool(), role)?;
                self.attachment
                    .install_peer_snapshot(cut, snapshot, branch, revision)
                    .map_err(crate::bundle::operation_error)?;
                installed = self
                    .attachment
                    .freeze()
                    .map_err(crate::bundle::operation_error)?;
            }
        } else if publisher_digest.is_some_and(|digest| cut.identity().to_string() != digest) {
            return Err(BundleError::HistoryChanged);
        }
        self.attachment
            .resume(&installed)
            .map_err(crate::bundle::operation_error)?;
        self.host_cut = None;
        self.host_published_digest = publisher_digest.map(str::to_owned);
        if let Some(digest) = publisher_digest
            && let Some(association) = &mut self.association
        {
            association.saved = Some(digest.to_owned());
            association.saved_position = Some(installed.change().position);
        }
        Ok(())
    }

    /// Prepare App resources against a frozen fresh actor, then seal the old cut.
    /// A post-seal resume failure fails closed; it cannot restore the sealed source.
    pub fn activate_host_cut(
        &mut self,
        cut: &crate::operation_actor::AcceptedCut,
        history: crate::operation_actor::PortableHistory,
        publisher_digest: &str,
        kind: HostActivationKind,
        prepare: impl FnOnce(&Core) -> Result<(), BundleError>,
    ) -> Result<Core, BundleError> {
        self.activate_host_cut_prepared(cut, history, publisher_digest, kind, prepare, None)
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "the original exact-cut handoff also consumes optional prepublication GUI resources"
    )]
    /// `history` is borrowed when `prepared` already restored it (only its
    /// digest is checked here), and is restored by value otherwise.
    fn activate_host_cut_prepared(
        &mut self,
        cut: &crate::operation_actor::AcceptedCut,
        history: crate::operation_actor::PortableHistory,
        publisher_digest: &str,
        kind: HostActivationKind,
        prepare: impl FnOnce(&Core) -> Result<(), BundleError>,
        prepared: Option<PreparedGuiSwitch>,
    ) -> Result<Core, BundleError> {
        if !self.host_attached || self.host_retention_lost || self.host_cut.as_ref() != Some(cut) {
            return Err(BundleError::Identity(
                "host activation is outside its exact foreground cut".into(),
            ));
        }
        if kind == HostActivationKind::Rescue && self.host_rescue_actors.len() >= 4 {
            return Err(BundleError::Capacity("retained GUI rescue actors"));
        }
        if history.identity().to_string() != publisher_digest {
            self.host_lost()?;
            return Err(BundleError::HistoryChanged);
        }
        let (core, attachment, fresh_cut) = if let Some(prepared) = prepared {
            if prepared.identity != history.identity()
                || prepared.cut.change().revision != history.revision
            {
                return Err(BundleError::HistoryChanged);
            }
            (prepared.core, prepared.attachment, prepared.cut)
        } else {
            let branch = history.branch.clone();
            let revision = history.revision;
            let snapshot = history.validate(self.core.history_pool())?;
            let core = self
                .attachment
                .open_validated_selection(cut, snapshot, branch, revision)
                .map_err(crate::bundle::operation_error)?;
            let attachment =
                ProjectAttachment::new(&core).map_err(crate::bundle::operation_error)?;
            let fresh_cut = attachment
                .freeze()
                .map_err(crate::bundle::operation_error)?;
            (core, attachment, fresh_cut)
        };
        if let Err(error) = prepare(&core) {
            Self::seal_prepared_host_actor(&core, &attachment)?;
            if self
                .core
                .attachment_state()
                .map_err(crate::bundle::operation_error)?
                != kinewright_core::AttachmentState::Frozen
                || self
                    .attachment
                    .capture()
                    .map_err(crate::bundle::operation_error)?
                    != *cut
            {
                self.host_lost()?;
            }
            return Err(error);
        }
        if core
            .attachment_state()
            .map_err(crate::bundle::operation_error)?
            != kinewright_core::AttachmentState::Frozen
            || attachment
                .capture()
                .map_err(crate::bundle::operation_error)?
                != fresh_cut
        {
            Self::seal_prepared_host_actor(&core, &attachment)?;
            return Err(BundleError::HistoryChanged);
        }
        if let Err(error) = self.attachment.retire(cut) {
            Self::seal_prepared_host_actor(&core, &attachment)?;
            self.host_lost()?;
            return Err(crate::bundle::operation_error(error));
        }
        if kind != HostActivationKind::Switch
            && let Err(error) = attachment.resume(&fresh_cut)
        {
            // A is already sealed. Keep its history/source and retire all escaped
            // prepared B clones rather than leaving a second usable actor.
            Self::seal_prepared_host_actor(&core, &attachment)?;
            self.host_lost()?;
            return Err(crate::bundle::operation_error(error));
        }
        if kind == HostActivationKind::Rescue {
            self.host_rescue_actors.push(self.core.clone());
        }
        self.core = core.clone();
        self.attachment = attachment;
        self.host_cut = if kind == HostActivationKind::Switch {
            Some(fresh_cut)
        } else {
            None
        };
        self.host_published_digest = Some(publisher_digest.to_owned());
        *self.dirty_cache.borrow_mut() = None;
        if let Some(association) = &mut self.association {
            association.saved = Some(publisher_digest.to_owned());
            association.saved_position = Some(fresh_cut.change().position);
        }
        Ok(core)
    }

    fn seal_prepared_host_actor(
        core: &Core,
        attachment: &ProjectAttachment,
    ) -> Result<(), BundleError> {
        if core
            .attachment_state()
            .map_err(crate::bundle::operation_error)?
            == kinewright_core::AttachmentState::Sealed
        {
            return Ok(());
        }
        let current = attachment
            .freeze()
            .map_err(crate::bundle::operation_error)?;
        attachment
            .retire(&current)
            .map_err(crate::bundle::operation_error)
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn retained_host_actor_count(&self) -> usize {
        self.host_rescue_actors.len()
    }

    /// Transport failure is permanent for this originating context (final L2).
    /// Keep all actual temporary sources, holds and actor histories until explicit Close.
    pub fn host_lost(&mut self) -> Result<(), BundleError> {
        self.host_retention_lost = true;
        if self
            .core
            .attachment_state()
            .map_err(crate::bundle::operation_error)?
            == kinewright_core::AttachmentState::Sealed
        {
            return Ok(());
        }
        let cut = self
            .attachment
            .freeze()
            .map_err(crate::bundle::operation_error)?;
        self.host_cut = Some(cut);
        Ok(())
    }

    /// Host-only validation shadow installation for an explicit foreground snapshot.
    pub fn install_host_snapshot(
        &mut self,
        history: crate::operation_actor::PortableHistory,
    ) -> Result<(), BundleError> {
        if self.host_attached || self.prepared.is_some() || self.has_unresolved_publication() {
            return Err(BundleError::Unsettled);
        }
        let crate::operation_actor::PortableHistory {
            snapshot,
            branch,
            revision,
        } = history;
        let role = snapshot.role();
        let snapshot = snapshot.receive(self.core.history_pool(), role)?;
        self.install_validated_host_snapshot(snapshot, branch, revision)
    }
    /// Install the semantically validated incoming owner without another capture.
    pub fn install_validated_host_snapshot(
        &mut self,
        snapshot: crate::operation_history::ValidatedOwnerSnapshot,
        branch: crate::bundle_records::BranchId,
        revision: kinewright_core::TimelineRevision,
    ) -> Result<(), BundleError> {
        if self.host_attached || self.prepared.is_some() || self.has_unresolved_publication() {
            return Err(BundleError::Unsettled);
        }
        let was_live = self
            .core
            .attachment_state()
            .map_err(crate::bundle::operation_error)?
            == kinewright_core::AttachmentState::Live;
        let cut = self
            .attachment
            .freeze()
            .map_err(crate::bundle::operation_error)?;
        if let Err(error) = self
            .attachment
            .install_peer_snapshot(&cut, snapshot, branch, revision)
        {
            if was_live {
                self.attachment
                    .resume(&cut)
                    .map_err(crate::bundle::operation_error)?;
            }
            return Err(crate::bundle::operation_error(error));
        }
        let installed = self
            .attachment
            .freeze()
            .map_err(crate::bundle::operation_error)?;
        self.attachment
            .resume(&installed)
            .map_err(crate::bundle::operation_error)
    }
    /// Adopt the host destination while its GUI source remains a non-owning descriptor.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "adoption consumes the original source input identity along with the copy and rebinds it to the destination"
    )]
    pub fn from_borrowed_copy(
        copy: BundleCopy,
        core: &Core,
        inputs: ObjectRef,
    ) -> Result<Self, BundleError> {
        if copy.borrowed_source().is_none() {
            return Err(BundleError::Identity(
                "expected borrowed GUI source copy".into(),
            ));
        }
        let rebound = copy.rebound_manifest(&inputs)?;
        let attachment = copy.attach_core(core)?;
        let (saved, saved_position) = if let CopyStatus::Committed(report) = copy.status() {
            let destination = copy.destination_owner()?;
            let checkpoint = destination.checkpoint(destination.manual_branch_id())?;
            let snapshot = checkpoint.open(&destination.store, core.history_pool())?;
            let epoch = snapshot.position().epoch;
            let cut = attachment
                .freeze()
                .map_err(crate::bundle::operation_error)?;
            attachment
                .install_snapshot(&cut, snapshot, Some(epoch))
                .map_err(crate::bundle::operation_error)?;
            let installed = attachment
                .freeze()
                .map_err(crate::bundle::operation_error)?;
            let installed = attachment
                .retarget_selection(&installed, destination.manual_branch_id().clone())
                .map_err(crate::bundle::operation_error)?;
            attachment
                .resume(&installed)
                .map_err(crate::bundle::operation_error)?;
            (Some(report.history_sha256.clone()), Some(report.position))
        } else {
            (None, None)
        };
        let path = copy.destination_owner()?.store.resolved_root()?;
        Ok(Self {
            core: core.clone(),
            attachment,
            source: None,
            destination: Some(RetainedDestination::FirstSave(Box::new(copy))),
            inputs: rebound,
            input_leases: vec![],
            association: Some(ActiveAssociation {
                path,
                saved,
                saved_position,
            }),
            rescue: RescueSessionState::default(),
            prepared: None,
            recovery_save_pending: false,
            selection_result: None,
            durable_sessions: vec![],
            pending_candidate: None,
            retained_destinations: vec![],
            originating_session: crate::bundle_records::SessionId::fresh()?,
            dirty_cache: std::cell::RefCell::new(None),
            input_usage: std::sync::Arc::new(std::sync::Mutex::new(
                crate::storage_budget::StorageUsage::default(),
            )),
            retained_task_work_cache: std::cell::RefCell::new(None),
            host_attached: false,
            host_retention_lost: false,
            host_cut: None,
            host_published_digest: None,
            host_rescue_actors: vec![],
            host_source_admission: None,
            preliminary_save: None,
            pending_copy_cut: None,
            pending_import_source: None,
        })
    }

    pub fn new(parent: &Path, core: &Core, limits: StorageLimits) -> Result<Self, BundleError> {
        let mut source = SessionStore::create(parent, limits)?;
        let attachment = source.attach_core(core)?;
        let inputs = source
            .admit_inputs(&OwnedInputManifest {
                project_id: None,
                session_id: Some(source.session_id().clone()),
                owner_epoch: source.store().owner_epoch(),
                entries: vec![],
            })?
            .reference
            .clone();
        Ok(Self {
            core: core.clone(),
            attachment,
            source: Some(source),
            destination: None,
            inputs,
            input_leases: vec![],
            association: None,
            rescue: RescueSessionState::default(),
            prepared: None,
            recovery_save_pending: false,
            selection_result: None,
            durable_sessions: vec![],
            pending_candidate: None,
            retained_destinations: vec![],
            dirty_cache: std::cell::RefCell::new(None),
            input_usage: std::sync::Arc::new(std::sync::Mutex::new(
                crate::storage_budget::StorageUsage::default(),
            )),
            retained_task_work_cache: std::cell::RefCell::new(None),
            originating_session: crate::bundle_records::SessionId::fresh()?,
            host_attached: false,
            host_retention_lost: false,
            host_cut: None,
            host_published_digest: None,
            host_rescue_actors: vec![],
            host_source_admission: None,
            preliminary_save: None,
            pending_copy_cut: None,
            pending_import_source: None,
        })
    }
    pub fn activate(
        path: &Path,
        admission: BundleWriteAdmission,
        core: &Core,
        inputs: Option<&ObjectRef>,
    ) -> Result<Self, BundleError> {
        let mut bundle = ProjectBundle::activate(path, admission, core.history_pool())?;
        let inputs = if let Some(inputs) = inputs {
            inputs.clone()
        } else {
            bundle.empty_inputs()?.reference.clone()
        };
        let mut session = Self::from_bundle(bundle, core, inputs)?;
        session
            .association
            .as_mut()
            .ok_or(BundleError::HistoryBarrierInvariant)?
            .path = path.to_path_buf();
        Ok(session)
    }
    pub fn open(
        path: &Path,
        pool: &crate::operation_history::HistoryPool,
    ) -> Result<Self, BundleError> {
        Self::open_loaded(path, LoadedBundle::read(path, pool)?)
    }
    /// Replace a fresh read-only document inspection after the complete native
    /// record validates. This view has accepted no transition; its cached
    /// document remains with the caller if writer acquisition needs a retry.
    pub fn open_document_view(
        path: &Path,
        view: &Core,
        inspection: &kinewright_core::Document,
    ) -> Result<Self, BundleError> {
        let loaded = LoadedBundle::read(path, view.history_pool())?;
        if loaded.admission().inspection_document() != Some(inspection) {
            return Err(BundleError::HistoryChanged);
        }
        Self::retire_document_view(view)?;
        Self::open_loaded(path, loaded)
    }
    /// Only a newly constructed inspection with no accepted transition may be
    /// retired. Callers keep its cached document until the replacement opens.
    pub fn retire_document_view(view: &Core) -> Result<(), BundleError> {
        let change = match view.change_id() {
            Ok(change) => change,
            Err(crate::operation_actor::Error::Retired) => return Ok(()),
            Err(error) => return Err(crate::bundle::operation_error(error)),
        };
        if change.generation != 0
            || change.revision.0 != 0
            || change.position.epoch != 0
            || change.position.position != 0
            || view
                .attachment_state()
                .map_err(crate::bundle::operation_error)?
                != kinewright_core::AttachmentState::Live
        {
            return Err(BundleError::HistoryChanged);
        }
        let attachment =
            crate::ProjectAttachment::new(view).map_err(crate::bundle::operation_error)?;
        let cut = attachment
            .freeze()
            .map_err(crate::bundle::operation_error)?;
        if cut.change() != change {
            attachment
                .resume(&cut)
                .map_err(crate::bundle::operation_error)?;
            return Err(BundleError::HistoryChanged);
        }
        attachment
            .retire(&cut)
            .map_err(crate::bundle::operation_error)?;
        Ok(())
    }
    fn open_loaded(path: &Path, mut loaded: LoadedBundle) -> Result<Self, BundleError> {
        let inputs = loaded.root()?.input_manifest_ref.clone();
        let branch = loaded.root()?.manual_branch_id.clone();
        let checkpoint = loaded.manual_checkpoint()?.clone();
        let snapshot = loaded
            .branches
            .get_mut(&branch)
            .and_then(|entry| entry.1.take())
            .ok_or(BundleError::Unsettled)?;
        let core = Core::open(snapshot, branch, checkpoint.revision)
            .map_err(crate::bundle::operation_error)?;
        let bundle = loaded.into_writer()?;
        let mut session = Self::from_bundle(bundle, &core, inputs)?;
        let association = session
            .association
            .as_mut()
            .ok_or(BundleError::HistoryBarrierInvariant)?;
        association.path = path.to_path_buf();
        association.saved = Some(checkpoint.identity.to_string());
        association.saved_position = Some(checkpoint.position);
        Ok(session)
    }
    pub fn from_bundle(
        bundle: ProjectBundle,
        core: &Core,
        inputs: ObjectRef,
    ) -> Result<Self, BundleError> {
        crate::namespace::note("ja1_session_attach", &bundle.store().resolved_root()?);
        let attachment = bundle.attach_core(core)?;
        if core
            .creation_branch()
            .map_err(crate::bundle::operation_error)?
            != *bundle.manual_branch_id()
        {
            let cut = attachment
                .freeze()
                .map_err(crate::bundle::operation_error)?;
            let selected = attachment
                .retarget_selection(&cut, bundle.manual_branch_id().clone())
                .map_err(crate::bundle::operation_error)?;
            attachment
                .resume(&selected)
                .map_err(crate::bundle::operation_error)?;
        }

        let path = bundle.store().resolved_root()?;
        let cut = attachment
            .capture()
            .map_err(crate::bundle::operation_error)?;
        let saved = bundle
            .branches
            .get(bundle.manual_branch_id())
            .filter(|checkpoint| {
                bundle.root.is_some()
                    && !bundle.unsettled
                    && bundle.pending_command.is_none()
                    && bundle.pending_publication.is_none()
                    && checkpoint.identity == cut.identity()
                    && checkpoint.input_manifest_ref == inputs
            })
            .map(|checkpoint| (checkpoint.identity.to_string(), checkpoint.position));
        let saved_position = saved.as_ref().map(|(_, position)| *position);
        let saved = saved.map(|(identity, _)| identity);
        Ok(Self {
            core: core.clone(),
            attachment,
            source: None,
            destination: Some(RetainedDestination::Named(Box::new(bundle))),
            inputs,
            input_leases: vec![],
            association: Some(ActiveAssociation {
                path,
                saved,
                saved_position,
            }),
            rescue: RescueSessionState::default(),
            prepared: None,
            recovery_save_pending: false,
            selection_result: None,
            durable_sessions: vec![],
            pending_candidate: None,
            retained_destinations: vec![],
            dirty_cache: std::cell::RefCell::new(None),
            input_usage: std::sync::Arc::new(std::sync::Mutex::new(
                crate::storage_budget::StorageUsage::default(),
            )),
            retained_task_work_cache: std::cell::RefCell::new(None),
            originating_session: crate::bundle_records::SessionId::fresh()?,
            host_attached: false,
            host_retention_lost: false,
            host_cut: None,
            host_published_digest: None,
            host_rescue_actors: vec![],
            host_source_admission: None,
            preliminary_save: None,
            pending_copy_cut: None,
            pending_import_source: None,
        })
    }
    #[must_use]
    pub const fn core(&self) -> &Core {
        &self.core
    }
    #[must_use]
    pub const fn association(&self) -> Option<&ActiveAssociation> {
        self.association.as_ref()
    }
    #[must_use]
    pub fn rescue_slots(&self) -> usize {
        self.rescue.slots.consumed()
    }
    #[must_use]
    pub fn retained_attempts(&self) -> &[RetainedAttempt] {
        &self.rescue.attempts
    }
    #[must_use]
    pub fn has_prepared_selection(&self) -> bool {
        self.prepared.is_some()
    }
    #[must_use]
    pub fn preliminary_save_pending(&self) -> bool {
        self.recovery_save_pending
    }
    #[must_use]
    pub fn has_unresolved_publication(&self) -> bool {
        self.destination
            .as_ref()
            .is_some_and(|d| d.bundle().is_ok_and(|b| b.unsettled))
    }
    pub fn pending_publication_cut(&self) -> Result<Option<crate::AttemptBinding>, BundleError> {
        Ok(self.active_bundle()?.pending_attempt())
    }
    pub fn active_publication_state(&self) -> Result<PublicationState, BundleError> {
        Ok(self.active_bundle()?.publication_state())
    }
    pub fn acknowledged_recovery_point(&self) -> Result<Option<ObjectRef>, BundleError> {
        Ok(self
            .active_bundle()?
            .rescue_records()?
            .into_iter()
            .find_map(|r| r.acknowledged_root))
    }
    pub fn retained_destinations(&self) -> Result<Vec<(PathBuf, PublicationState)>, BundleError> {
        self.retained_destinations
            .iter()
            .map(|(owner, _)| {
                let bundle = owner.bundle()?;
                Ok((bundle.store.resolved_root()?, bundle.publication_state()))
            })
            .collect()
    }
    #[must_use]
    pub fn pending_agent_command(&self) -> bool {
        self.active_bundle()
            .is_ok_and(|bundle| bundle.pending_command.is_some())
    }
    #[must_use]
    pub fn pending_agent_command_at(&self, path: &Path) -> bool {
        self.destination
            .iter()
            .chain(self.retained_destinations.iter().map(|(d, _)| d))
            .any(|d| {
                d.bundle().is_ok_and(|b| {
                    b.store.resolved_root().is_ok_and(|p| p == path) && b.pending_command.is_some()
                })
            })
    }
    /// Whether a retained owner is a never-active capture copy, rather than its source.
    #[must_use]
    pub fn is_capture_destination(&self, path: &Path) -> bool {
        self.retained_destinations.iter().any(|(owner, actor)| {
            owner.bundle().is_ok_and(|bundle| {
                bundle.store.resolved_root().is_ok_and(|p| p == path)
                    && actor.as_ref().is_none_or(|actor| {
                        actor.actor_identity() != self.core.actor_identity()
                            && actor.attachment_state().is_ok_and(|state| {
                                matches!(
                                    state,
                                    kinewright_core::AttachmentState::Frozen
                                        | kinewright_core::AttachmentState::Sealed
                                )
                            })
                            && !self.rescue.attempts.iter().any(|attempt| {
                                attempt.actor() == Some(actor.actor_identity())
                                    && attempt.capture.cut().change().actor
                                        == actor.actor_identity()
                            })
                            && actor.snapshot_identity().map_or_else(
                                |_| {
                                    actor.attachment_state().is_ok_and(|state| {
                                        state == kinewright_core::AttachmentState::Sealed
                                    })
                                },
                                |identity| {
                                    bundle
                                        .checkpoint(bundle.manual_branch_id())
                                        .is_ok_and(|cut| cut.identity == identity)
                                },
                            )
                    })
            })
        })
    }
    /// Release only a proven, never-active capture destination. Source owners and slots stay.
    pub fn completed_capture_backs_recovery(&self, path: &Path) -> Result<bool, BundleError> {
        for attempt in &self.rescue.attempts {
            if attempt.capture.tag.destination == path {
                return Ok(true);
            }
            let archive = attempt
                .capture
                .archive_store
                .lock()
                .map_err(|_| BundleError::Unsettled)?;
            if archive
                .as_ref()
                .is_some_and(|store| store.resolved_root().is_ok_and(|root| root == path))
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub fn release_completed_capture(&mut self, path: &Path) -> Result<(), BundleError> {
        if self.completed_capture_backs_recovery(path)? {
            return Err(BundleError::Identity("this copy retains archived recovery states; close this editing context before opening the copy for editing".into()));
        }
        let index = self
            .retained_destinations
            .iter()
            .position(|(owner, _)| {
                owner
                    .bundle()
                    .is_ok_and(|b| b.store.resolved_root().is_ok_and(|p| p == path))
            })
            .ok_or_else(|| BundleError::Identity("unknown retained destination".into()))?;
        let bundle = self.retained_destinations[index].0.bundle()?;
        if bundle.publication_state() != PublicationState::Committed
            || bundle.unsettled
            || !self.is_capture_destination(path)
        {
            return Err(BundleError::Identity("only a proven completed capture copy may be released; rescue sources remain retained".into()));
        }
        self.retained_destinations.remove(index);
        Ok(())
    }
    #[must_use]
    pub const fn source(&self) -> Option<&SessionStore> {
        self.source.as_ref()
    }
    pub fn source_mut(&mut self) -> Result<&mut SessionStore, BundleError> {
        self.require_local_writer()?;
        if self.destination.is_some() {
            return Err(BundleError::Identity(
                "associated source is retained read-only".into(),
            ));
        }
        self.source
            .as_mut()
            .ok_or(BundleError::HistoryBarrierInvariant)
    }
    pub fn set_unsaved_inputs(&mut self, inputs: ObjectRef) -> Result<(), BundleError> {
        let source = self.source_mut()?;
        source.store().read_input_manifest(&inputs)?;
        self.inputs = inputs;
        Ok(())
    }
    pub fn dirty(&self) -> Result<bool, BundleError> {
        let position = self
            .core
            .change_id()
            .map_err(crate::bundle::operation_error)?
            .position;
        Ok(self.association.as_ref().and_then(|a| a.saved_position) != Some(position))
    }
    pub fn store(&self) -> Result<&ProjectStore, BundleError> {
        if let Some(destination) = &self.destination {
            Ok(destination.bundle()?.store())
        } else {
            self.source
                .as_ref()
                .map(SessionStore::store)
                .ok_or(BundleError::HistoryBarrierInvariant)
        }
    }
    fn capture_source(&self) -> Result<RescueSource<'_>, BundleError> {
        let destination = self
            .destination
            .as_ref()
            .ok_or(BundleError::HistoryBarrierInvariant)?;
        let bundle = destination.bundle()?;
        let unsaved = self.association.as_ref().is_some_and(|a| a.saved.is_none());
        if unsaved
            && let (Some(source), RetainedDestination::FirstSave(copy)) =
                (&self.source, destination)
        {
            return Ok((
                source.store(),
                vec![(
                    copy.source_manifest(&self.inputs)?,
                    MissingOwnedUse::Current,
                )],
            ));
        }
        let roots = Self::rescue_required_roots(bundle)?
            .into_iter()
            .map(|r| (r, MissingOwnedUse::Manifest))
            .collect();
        Ok((&bundle.store, roots))
    }
    pub fn first_save(
        &mut self,
        path: &Path,
        limits: StorageLimits,
    ) -> Result<CopyStatus, BundleError> {
        self.require_local_writer()?;
        if self.destination.is_some() {
            return Err(BundleError::Identity("session already associated".into()));
        }
        let source = self
            .source
            .as_mut()
            .ok_or(BundleError::HistoryBarrierInvariant)?;
        let cut = self
            .attachment
            .freeze()
            .map_err(crate::bundle::operation_error)?;
        let copy = match source.first_save_with_limits(path, &self.core, &self.inputs, limits) {
            Ok(copy) => copy,
            Err(error) => {
                self.attachment
                    .resume(&cut)
                    .map_err(crate::bundle::operation_error)?;
                return Err(error);
            }
        };
        self.pending_copy_cut = Some(cut);
        self.inputs = copy.rebound_manifest(&self.inputs)?;
        let mut status = copy.status().clone();
        if let CopyStatus::Committed(report) = &mut status {
            report.path = path.to_path_buf();
        }
        let saved = if let CopyStatus::Committed(report) = &status {
            Some(report.history_sha256.clone())
        } else {
            None
        };
        self.association = Some(ActiveAssociation {
            path: path.to_path_buf(),
            saved,
            saved_position: if let CopyStatus::Committed(report) = &status {
                Some(report.position)
            } else {
                None
            },
        });
        self.destination = Some(RetainedDestination::FirstSave(Box::new(copy)));
        if matches!(status, CopyStatus::Committed(_)) {
            self.adopt_copy_snapshot()?;
        } else {
            // The retained copy owns the uncertain saved prefix. Local edits
            // may continue; reconciliation must preserve their later suffix.
            self.attachment
                .resume(&cut)
                .map_err(crate::bundle::operation_error)?;
        }
        self.retire_ordinary_copy()?;
        Ok(status)
    }
    fn adopt_copy_snapshot(&mut self) -> Result<(), BundleError> {
        let Some(original) = self.pending_copy_cut else {
            return Ok(());
        };
        let bundle = self.active_bundle()?;
        if bundle.publication_state() != PublicationState::Committed || bundle.unsettled {
            return Err(BundleError::Unsettled);
        }
        let checkpoint = bundle.checkpoint(bundle.manual_branch_id())?.clone();
        let current = self
            .attachment
            .freeze()
            .map_err(crate::bundle::operation_error)?;
        let installed = if current.change().position.epoch == checkpoint.position.epoch {
            // A prior attempt installed the epoch and failed later. Never install
            // that epoch twice or replace commands accepted since that attempt.
            current
        } else {
            if current.change().position.epoch != original.change().position.epoch {
                return Err(BundleError::HistoryChanged);
            }
            let snapshot = if current == original {
                checkpoint.open(&self.active_bundle()?.store, self.core.history_pool())?
            } else {
                // Reconciliation names the saved prefix, not later local work.
                // Preserve the whole current owner while adopting its saved
                // namespace's epoch. The saved prefix remains the dirty baseline.
                self.core
                    .capture_state()
                    .map_err(crate::bundle::operation_error)?
                    .snapshot
                    .into_install_at(checkpoint.position.epoch)?
                    .validate()?
            };
            self.attachment
                .install_snapshot(&current, snapshot, Some(checkpoint.position.epoch))
                .map_err(crate::bundle::operation_error)?;
            self.attachment
                .capture()
                .map_err(crate::bundle::operation_error)?
        };
        self.pending_copy_cut = Some(installed);
        let selected = self
            .attachment
            .retarget_selection(&installed, checkpoint.branch_id)
            .map_err(crate::bundle::operation_error)?;
        self.attachment
            .resume(&selected)
            .map_err(crate::bundle::operation_error)?;
        self.pending_copy_cut = None;
        if let Some(association) = &mut self.association {
            association.saved = Some(checkpoint.identity.to_string());
            association.saved_position = Some(checkpoint.position);
        }
        Ok(())
    }
    /// Ordinary saves never inspect the rescue slot ledger.
    pub fn save_active(&mut self) -> Result<BundleSaveReport, BundleError> {
        self.require_local_writer()?;
        if self.pending_copy_cut.is_some() {
            let Some(RetainedDestination::FirstSave(copy)) = &mut self.destination else {
                return Err(BundleError::Unsettled);
            };
            let report = copy.retry_install(&self.core, &self.inputs)?;
            self.adopt_copy_snapshot()?;
            self.retire_ordinary_copy()?;
            if !self.dirty()? {
                return Ok(report);
            }
        }

        let mut report = self
            .destination
            .as_mut()
            .ok_or_else(|| BundleError::Identity("choose a first-save destination".into()))?
            .save(&self.core, &self.inputs)?;
        let saved = report.history_sha256.clone();
        let association = self
            .association
            .as_mut()
            .ok_or(BundleError::HistoryBarrierInvariant)?;
        association.saved = Some(saved);
        association.saved_position = Some(report.position);
        self.retire_ordinary_copy()?;
        if let Some(association) = &self.association {
            report.path.clone_from(&association.path);
        }
        Ok(report)
    }
    /// Keep a caller's display spelling after a verified handoff. IO stays with its owner.
    pub fn set_path_spelling(&mut self, path: &Path) {
        if let Some(association) = &mut self.association
            && crate::same_project_file(&association.path, path)
        {
            association.path = path.to_path_buf();
        }
    }
    fn retire_ordinary_copy(&mut self) -> Result<(), BundleError> {
        if !self.rescue.attempts.is_empty()
            || matches!(&self.destination, Some(RetainedDestination::FirstSave(copy)) if copy.borrowed_source().is_some())
            || !matches!(&self.destination, Some(RetainedDestination::FirstSave(copy)) if matches!(copy.status(), CopyStatus::Committed(_)))
        {
            return Ok(());
        }
        if let Some(RetainedDestination::FirstSave(copy)) = self.destination.take() {
            match copy.into_destination() {
                Ok(bundle) => {
                    self.destination = Some(RetainedDestination::Named(Box::new(bundle)));
                    if !self
                        .source
                        .as_ref()
                        .is_some_and(SessionStore::has_borrowed_sources)
                    {
                        self.source = None;
                    }
                }
                Err(copy) => {
                    self.destination = Some(RetainedDestination::FirstSave(copy));
                    return Err(BundleError::Unsettled);
                }
            }
        }
        Ok(())
    }
    fn core_for_destination(&self, bundle: &ProjectBundle, default: Option<&Core>) -> Option<Core> {
        let default = self
            .pending_candidate
            .as_ref()
            .filter(|pending| bundle.pending_command.as_deref() == Some(&pending.envelope))
            .map_or(default, |pending| Some(&pending.core));
        let eligible = |core: &&Core| {
            core.attachment_state()
                .is_ok_and(|state| state != kinewright_core::AttachmentState::Sealed)
        };
        let Some(attempt) = bundle.pending_attempt() else {
            return default.filter(eligible).cloned();
        };
        if let Some(core) = default
            .filter(eligible)
            .filter(|core| core.actor_identity() == attempt.actor())
        {
            return Some(core.clone());
        }
        std::iter::once(&self.core)
            .chain(
                self.durable_sessions
                    .iter()
                    .filter(|registered| registered.guard.epoch == bundle.store.owner_epoch())
                    .map(|registered| registered.guard.core()),
            )
            .find(|core| eligible(core) && core.actor_identity() == attempt.actor())
            .cloned()
    }
    pub fn reconcile_active(&mut self) -> Result<Option<BundleSaveReport>, BundleError> {
        let live = self.core_for_destination(self.active_bundle()?, Some(&self.core));
        let destination = self
            .destination
            .as_mut()
            .ok_or(BundleError::HistoryBarrierInvariant)?;
        let tag = destination.bundle()?.publication_tag.clone();
        let result = if let Some(live) = live {
            destination.reconcile(&live)
        } else {
            match destination {
                RetainedDestination::Named(bundle) => bundle.reconcile_destination(None),
                RetainedDestination::FirstSave(copy) => copy.reconcile_destination(None),
            }
        };
        if let Some(tag) = tag {
            self.rescue
                .settle(&tag, destination.bundle()?.publication_state());
        }
        let report = result?;
        if self.pending_copy_cut.is_some()
            && self.active_bundle()?.publication_state() == PublicationState::Committed
        {
            self.adopt_copy_snapshot()?;
        }
        self.settle_pending_candidate()?;
        Ok(report)
    }
    /// Foreground probe. Only a failed unknown attempt can supply an offer.
    pub fn probe_active(&mut self) -> Result<Option<RescueProbe>, BundleError> {
        let Some(destination) = &mut self.destination else {
            return Ok(None);
        };
        let bundle = destination.bundle()?;
        let Some(tag) = bundle.publication_tag.clone() else {
            return Ok(None);
        };
        let ticket = bundle.pending_attempt();
        let state = bundle.publication_state();
        if state != PublicationState::Unknown {
            return Ok(None);
        }
        let mut phase = ReconcilePhase::Namespace;
        let outcome = (|| {
            let bundle = destination.bundle()?;
            bundle.store.with_namespace(|n| {
                n.clear_probe_failure();
                n.probe_retained_namespace()
            })?;
            phase = ReconcilePhase::Authority;
            bundle.store.with_namespace(|n| n.verify_read_authority())?;
            phase = ReconcilePhase::RootRead;
            let bytes = bundle.store.with_namespace(|n| n.read_root())?;
            phase = ReconcilePhase::Graph;
            if let Some(bytes) = bytes {
                let admission =
                    BundleWriteAdmission::from_bytes(Some(tag.destination.clone()), bytes)?;
                admission.require_supported()?;
                if admission.is_studio() {
                    LoadedBundle::read(&tag.destination, self.core.history_pool())?;
                }
            }
            phase = ReconcilePhase::Barrier;
            destination.reconcile(&self.core)
        })();
        self.rescue
            .settle(&tag, destination.bundle()?.publication_state());
        match outcome {
            Ok(_) => Ok(None),
            Err(error) => {
                let (observed, error) = destination
                    .bundle()?
                    .store
                    .with_namespace(|n| Ok(n.take_probe_failure(error)))?;
                let mut probe = RescueProbe::failed(
                    tag,
                    ticket,
                    observed.map_or(phase, |(phase, _)| phase),
                    destination.bundle()?.publication_state(),
                    error,
                );
                if let Some((_, operation)) = observed {
                    probe.operation = operation;
                }
                Ok(Some(probe))
            }
        }
    }
    /// Second serialized probe and frozen source admission. No attempt is adopted here.
    pub fn prepare_rescue(
        &mut self,
        offer: &RescueProbe,
    ) -> Result<Option<std::sync::Arc<StorageReservation<FrozenRescueCapture>>>, BundleError> {
        if offer.eligibility != RescueEligibility::Eligible {
            return Err(BundleError::Identity("ineligible rescue offer".into()));
        }
        let Some(second) = self.probe_active()? else {
            return Ok(None);
        };
        if second.eligibility != RescueEligibility::Eligible
            || second.tag != offer.tag
            || second.ticket != offer.ticket
        {
            return Err(BundleError::HistoryChanged);
        }
        let was_frozen = self
            .core
            .attachment_state()
            .map_err(crate::bundle::operation_error)?
            == kinewright_core::AttachmentState::Frozen;
        let (store, roots) = self.capture_source()?;
        let _working = store.reserve_capture_scratch()?;
        let cut = self
            .attachment
            .freeze()
            .map_err(crate::bundle::operation_error)?;
        // Multiple destination attempts of the identical frozen cut are one
        // immutable capture, already independently retained by that attempt.
        for attempt in &self.rescue.attempts {
            if attempt.capture.cut == cut
                && attempt.capture.tag == second.tag
                && attempt.capture.was_frozen == was_frozen
                && attempt
                    .capture
                    .snapshot
                    .lock()
                    .map_err(|_| BundleError::Unsettled)?
                    .is_some()
            {
                return Ok(Some(std::sync::Arc::clone(&attempt.capture)));
            }
        }
        let result = (|| {
            if self.source.is_none() || self.association.as_ref().is_some_and(|a| a.saved.is_some())
            {
                store.prime_retained_owner_proofs(
                    &self.active_bundle()?.root_references()?,
                    self.core.history_pool(),
                )?;
            }
            for attempt in &self.rescue.attempts {
                let archive = attempt
                    .capture
                    .archive
                    .lock()
                    .map_err(|_| BundleError::Unsettled)?;
                let source = attempt
                    .capture
                    .archive_store
                    .lock()
                    .map_err(|_| BundleError::Unsettled)?;
                if let (Some(reference), Some(source)) = (archive.as_ref(), source.as_ref()) {
                    source.prime_retained_owner_proofs(
                        std::slice::from_ref(reference),
                        self.core.history_pool(),
                    )?;
                }
            }
            let reads = store.admit_retained_reads(&roots)?;
            let captured = self
                .core
                .capture_state()
                .map_err(crate::bundle::operation_error)?;
            if captured.change != cut.change() {
                return Err(BundleError::HistoryChanged);
            }
            // Retained in-memory capture metadata remains admissible while the
            // original publication is unresolved; this grants no write authority.
            let charge = store.reserve_copy_read(0, 4096, false, true)?;
            let binding =
                serde_json::to_vec(&(cut.identity(), cut.change().position, &roots, &self.inputs))
                    .map_err(|e| BundleError::Corrupt(e.to_string()))?;
            let digest = kinewright_media::sha256_bytes(&binding);
            Ok(charge.hold(FrozenRescueCapture {
                cut,
                snapshot: std::sync::Mutex::new(Some(captured.snapshot)),
                archive: std::sync::Mutex::new(None),
                archive_store: std::sync::Mutex::new(None),
                archive_pin: std::sync::Mutex::new(None),
                tag: second.tag,
                digest,
                reads,
                was_frozen,
            }))
        })();
        if result.is_err() && !was_frozen {
            self.attachment
                .resume(&cut)
                .map_err(crate::bundle::operation_error)?;
        }
        result.map(|capture| Some(std::sync::Arc::new(capture)))
    }
    pub fn cancel_rescue(
        &self,
        capture: std::sync::Arc<StorageReservation<FrozenRescueCapture>>,
    ) -> Result<(), BundleError> {
        if !capture.was_frozen {
            self.attachment
                .resume(capture.cut())
                .map_err(crate::bundle::operation_error)?;
        }
        drop(capture);
        Ok(())
    }
    pub fn inspect_rescue_destination(
        &self,
        path: &Path,
    ) -> Result<DestinationInspection, BundleError> {
        let mut stores = vec![self.store()?];
        if let Some(source) = &self.source {
            stores.push(source.store());
        }
        for (owner, _) in &self.retained_destinations {
            stores.push(owner.bundle()?.store());
        }
        ProjectStore::inspect_rescue_destination(path, &stores)
    }
    /// C3's accounting adapter. It does no B publication and is never a shipping rescue action.
    #[cfg(any(test, feature = "test-util"))]
    pub fn adopt_attempt_probe(
        &mut self,
        capture: std::sync::Arc<StorageReservation<FrozenRescueCapture>>,
        consent: &RescueConsent,
        destination: &DestinationInspection,
        unknown: Option<PublicationTag>,
    ) -> Result<(), BundleError> {
        capture.check_consent(consent)?;
        let current = self
            .destination
            .as_ref()
            .ok_or(BundleError::HistoryBarrierInvariant)?
            .bundle()?;
        if current.publication_tag.as_ref() != Some(&capture.tag)
            || current.publication_state() != PublicationState::Unknown
        {
            return Err(BundleError::HistoryChanged);
        }
        if self
            .attachment
            .capture()
            .map_err(crate::bundle::operation_error)?
            != capture.cut
        {
            return Err(BundleError::HistoryChanged);
        }
        let mut stores = vec![self.store()?];
        if let Some(source) = &self.source {
            stores.push(source.store());
        }
        ProjectStore::revalidate_rescue_destination(destination, &stores)?;
        let tag = capture.tag.clone();
        let ticket = current.pending_attempt();
        let project_id = current.project_id().clone();
        let pending_command = current
            .pending_command
            .as_ref()
            .map(|e| e.digest().map(|d| (e.id.clone(), d)))
            .transpose()?;
        let retained = self
            .rescue
            .attempts
            .iter()
            .any(|attempt| attempt.tag == tag);
        if !retained {
            self.rescue
                .attempts
                .try_reserve(1)
                .map_err(|_| BundleError::Capacity("retained attempts"))?;
        }
        let reservation = self.rescue.slots.begin(tag.clone())?;
        if !retained {
            self.rescue.attempts.push(RetainedAttempt {
                state: PublicationState::Unknown,
                tag,
                core: Some(self.core.clone()),
                capture,
                ticket,
                project_id,
                pending_command,
            });
        }
        self.rescue.slots.finish(reservation, unknown)?;
        Ok(())
    }
    #[cfg(any(test, feature = "test-util"))]
    pub fn resume_probe(&self) -> Result<(), BundleError> {
        self.attachment
            .resume(
                &self
                    .attachment
                    .capture()
                    .map_err(crate::bundle::operation_error)?,
            )
            .map_err(crate::bundle::operation_error)
    }
    #[cfg(any(test, feature = "test-util"))]
    pub fn freeze_probe(&self) -> Result<crate::operation_actor::AcceptedCut, BundleError> {
        self.attachment
            .freeze()
            .map_err(crate::bundle::operation_error)
    }
    #[cfg(all(target_os = "linux", any(test, feature = "test-util")))]
    pub fn defer_parent_barrier_probe(&self) -> Result<(), BundleError> {
        self.store()?
            .with_namespace(|n| n.defer_parent_barrier_probe())
    }
    #[cfg(any(test, feature = "test-util"))]
    pub fn set_slots_probe(&mut self, slots: RescueSlots) {
        self.rescue.slots = slots;
    }
}

impl Drop for Ja1EditingSession {
    fn drop(&mut self) {
        let fence = |core: &Core| {
            if let Ok(owner) = ProjectAttachment::new(core)
                && let Ok(cut) = owner.freeze()
            {
                let _ = owner.retire(&cut);
            }
        };
        fence(&self.core);
        for registration in &self.durable_sessions {
            fence(registration.guard.core());
        }
        for (_, core) in &self.retained_destinations {
            if let Some(core) = core {
                fence(core);
            }
        }
        // Only source reuse ends here. Destination tags, outcomes and uncertain staging
        // keep their ordinary namespace policy; outstanding reader Arcs still own pins.
        let close = |owner: &mut RetainedDestination| match owner {
            RetainedDestination::Named(bundle) => {
                crate::bundle_copy::close_copy_source(&mut bundle.pending_copy_source);
            }
            RetainedDestination::FirstSave(copy) => copy.close_originating_context(),
        };
        if let Some(owner) = &mut self.destination {
            close(owner);
        }
        for (owner, _) in &mut self.retained_destinations {
            close(owner);
        }
        if let Some(source) = &mut self.source {
            source.close_originating_context();
        }
    }
}
