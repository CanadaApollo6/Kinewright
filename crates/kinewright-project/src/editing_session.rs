//! New/Open to Close ownership. Focus/view handles never own persistence resources.
#![allow(clippy::missing_errors_doc)]
#[cfg(any(test, feature = "test-util"))]
use crate::bundle_rescue::RescueConsent;
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
use kinewright_core::{Core, HistorySaveTag};
use std::path::{Path, PathBuf};
mod durable;
mod manual;
pub use manual::ManualRecoveryEndpoint;
mod recovery;
mod switch;
pub use switch::PreparedGuiSwitch;
mod rescue;
mod rescue_selection;
pub use durable::{
    BranchBinding, DurableBranchSessionGuard, FrozenTaskResult,
    specialist_floor_delta_from_baseline,
};
type RescueSource<'a> = (&'a ProjectStore, Vec<(ObjectRef, MissingOwnedUse)>);
type HistoryDigestCache = Option<(
    kinewright_core::ActorIdentity,
    kinewright_core::HistoryChangeId,
    String,
)>;

pub struct ActiveAssociation {
    path: PathBuf,
    saved: Option<String>,
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
    pub(crate) tag: HistorySaveTag,
    pub(crate) core: Core,
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
    consumed: Vec<HistorySaveTag>,
    reserved: bool,
}
pub struct RescueReservation {
    source: HistorySaveTag,
}
impl RetainedAttempt {
    #[must_use]
    pub const fn publication_state(&self) -> PublicationState {
        self.state
    }
    #[must_use]
    pub fn actor(&self) -> kinewright_core::ActorIdentity {
        self.core.actor_identity()
    }
    #[must_use]
    pub const fn tag(&self) -> &HistorySaveTag {
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
    fn settle(&mut self, tag: &HistorySaveTag, state: PublicationState) {
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
    pub fn check(&self, source: &HistorySaveTag) -> Result<(), BundleError> {
        let needed = usize::from(!self.consumed.contains(source)) + 1;
        if self.reserved || self.consumed.len() + needed > 4 {
            return Err(BundleError::Capacity("four rescue attempts"));
        }
        Ok(())
    }
    /// Called only when the owning lifecycle adopts actual source holds at first mutation.
    pub fn begin(&mut self, source: HistorySaveTag) -> Result<RescueReservation, BundleError> {
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
        unknown: Option<HistorySaveTag>,
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
    retained_destinations: Vec<(RetainedDestination, Core)>,
    originating_session: crate::bundle_records::SessionId,
    dirty_cache: std::cell::RefCell<HistoryDigestCache>,
    save_disclosure_cache: std::cell::RefCell<
        Option<(
            kinewright_core::ActorIdentity,
            kinewright_core::HistoryChangeId,
            StorageLimits,
            String,
        )>,
    >,
    host_attached: bool,
    host_retention_lost: bool,
    host_cut: Option<kinewright_core::AcceptedCut>,
    host_published_digest: Option<String>,
    host_rescue_actors: Vec<Core>,
    host_source_admission: Option<crate::bundle_records::SessionId>,
    preliminary_save: Option<PreliminarySave>,
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
    pub history: kinewright_core::HistorySnapshot,
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
impl Ja1EditingSession {
    /// Read the checkpoint identified by the publisher's exact report. Current
    /// actor history is deliberately irrelevant, including edits accepted after IO.
    pub fn published_history(
        &self,
        report: &BundleSaveReport,
    ) -> Result<kinewright_core::HistorySnapshot, BundleError> {
        let bundle = self.active_bundle()?;
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
                || checkpoint.history_ref.sha256 != report.history_sha256
                || checkpoint.checkpoint_generation != report.generation
                || !crate::same_project_file(&report.path, &bundle.store.resolved_root()?)
            {
                return Err(BundleError::Identity(
                    "preliminary publisher report differs from surviving L checkpoint".into(),
                ));
            }
            bundle
                .store
                .with_namespace(|namespace| namespace.verify_read_authority())?;
            let history = bundle
                .store
                .read_copy_record::<kinewright_core::HistorySnapshot>(
                    &checkpoint.history_ref,
                    true,
                )?;
            return Ok((*history).clone());
        }
        let root = bundle.root().ok_or(BundleError::HistoryBarrierInvariant)?;
        let checkpoint = bundle.checkpoint(bundle.manual_branch_id())?;
        let raw = bundle.admission.raw_bytes();
        if report.project_id != *bundle.project_id()
            || report.generation != root.generation
            || report.head_id != checkpoint.head_id
            || report.history_sha256 != checkpoint.history_ref.sha256
            || report.root_sha256 != kinewright_media::sha256_bytes(raw)
            || report.byte_length != raw.len() as u64
            || !crate::same_project_file(&report.path, &bundle.store.resolved_root()?)
        {
            return Err(BundleError::Identity(
                "publisher report differs from the retained checkpoint".into(),
            ));
        }
        bundle.store.with_namespace(|namespace| {
            namespace.verify_read_authority()?;
            if namespace.read_root()?.as_deref() != Some(raw) {
                return Err(BundleError::Identity(
                    "published checkpoint root was replaced".into(),
                ));
            }
            Ok(())
        })?;
        let history = bundle
            .store
            .read_copy_record::<kinewright_core::HistorySnapshot>(&checkpoint.history_ref, true)?;
        Ok((*history).clone())
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
        });
        Ok(())
    }

    /// Freeze producers before calling. A named writer is released only after its
    /// exact checkpoint and absence of unresolved owners/readers have been verified.
    pub fn prepare_host_attachment(&mut self) -> Result<PreparedHostAttachment, BundleError> {
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
        if self
            .core
            .pending_history_save()
            .map_err(crate::bundle::history_error)?
            .is_some()
        {
            return Err(BundleError::Unsettled);
        }
        let cut = self
            .attachment
            .freeze()
            .map_err(crate::bundle::history_error)?;
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
                    || bundle.pending_save.is_some()
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
                let descriptor = owner.retain_borrowed_source(
                    std::slice::from_ref(&self.inputs),
                    cut.history()
                        .encoded_bytes()
                        .map_err(crate::bundle::history_error)?,
                )?;
                let service = owner.borrowed_source_service(&descriptor)?;
                service.ensure_snapshot(&descriptor, cut.history())?;
                source_service = Some(service);
                source = Some(descriptor);
            }
            Ok(PreparedHostAttachment {
                named_path,
                expected_root_sha256,
                project_id,
                history: cut.history().clone(),
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
                .map_err(crate::bundle::history_error)?;
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
        let cut = self
            .host_cut
            .as_ref()
            .ok_or(BundleError::HistoryChanged)?
            .clone();
        if let Some(path) = path {
            let loaded = LoadedBundle::read(path)?;
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
            .map_err(crate::bundle::history_error)?;
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
    pub fn freeze_host_cut(&mut self) -> Result<kinewright_core::AcceptedCut, BundleError> {
        if !self.host_attached || self.host_retention_lost {
            return Err(BundleError::Identity(
                "source retention was not re-established".into(),
            ));
        }
        if let Some(cut) = &self.host_cut {
            self.ensure_host_snapshot(cut)?;
            return Ok(cut.clone());
        }
        let cut = self
            .attachment
            .freeze()
            .map_err(crate::bundle::history_error)?;
        self.host_cut = Some(cut.clone());
        self.ensure_host_snapshot(&cut)?;
        Ok(cut)
    }

    /// Admit the exact foreground snapshot on the one existing GUI source ledger.
    /// A named-only origin has no GUI temporary-source overlap to grow.
    pub fn ensure_host_snapshot(
        &self,
        cut: &kinewright_core::AcceptedCut,
    ) -> Result<Option<crate::source_boundary::BorrowedSnapshotBinding>, BundleError> {
        if !self.host_attached || self.host_retention_lost || self.host_cut.as_ref() != Some(cut) {
            return Err(BundleError::Identity(
                "snapshot admission is outside its exact foreground cut".into(),
            ));
        }
        self.host_source_admission
            .as_ref()
            .map(|admission| {
                self.source
                    .as_ref()
                    .ok_or(BundleError::HistoryBarrierInvariant)?
                    .ensure_borrowed_snapshot(admission, cut.history())
            })
            .transpose()
    }

    /// Install a publisher-returned history under the existing attachment proof.
    /// Callers keep every GUI producer fenced throughout this foreground operation.
    pub fn complete_host_cut(
        &mut self,
        cut: &kinewright_core::AcceptedCut,
        returned_history: Option<kinewright_core::HistorySnapshot>,
        publisher_digest: Option<&str>,
    ) -> Result<(), BundleError> {
        if !self.host_attached || self.host_retention_lost || self.host_cut.as_ref() != Some(cut) {
            return Err(BundleError::Identity(
                "host completion is outside its exact foreground cut".into(),
            ));
        }
        if let Some(digest) = publisher_digest {
            let history = returned_history.as_ref().unwrap_or_else(|| cut.history());
            let bytes =
                serde_json::to_vec(history).map_err(|e| BundleError::Corrupt(e.to_string()))?;
            if kinewright_media::sha256_bytes(&bytes) != digest {
                self.host_lost()?;
                return Err(BundleError::Identity(
                    "host publisher history digest differs".into(),
                ));
            }
        }
        if let Some(history) = &returned_history
            && history != cut.history()
        {
            let discard = history
                .floor_offset
                .checked_sub(cut.history().floor_offset)
                .and_then(|n| usize::try_from(n).ok())
                .ok_or(BundleError::HistoryChanged)?;
            let floor = kinewright_core::HistoryTransition::floor(cut.history().clone(), discard)
                .map_err(crate::bundle::history_error)?;
            if floor.after() != history {
                self.host_lost()?;
                return Err(BundleError::Identity("ordinary host completion must be the exact captured history or its legal floor".into()));
            }
        }
        self.attachment
            .resume(cut)
            .map_err(crate::bundle::history_error)?;
        if let Some(history) = returned_history
            && history != *cut.history()
            && let Err(error) = self
                .attachment
                .install_checkpoint(cut.history().clone(), history)
        {
            self.host_lost()?;
            return Err(crate::bundle::history_error(error));
        }
        self.host_cut = None;
        self.host_published_digest = publisher_digest.map(str::to_owned);
        if let Some(digest) = publisher_digest
            && let Some(association) = &mut self.association
        {
            association.saved = Some(digest.to_owned());
        }
        Ok(())
    }

    /// Prepare App resources against a frozen fresh actor, then seal the old cut.
    /// A post-seal resume failure fails closed; it cannot restore the sealed source.
    pub fn activate_host_cut(
        &mut self,
        cut: &kinewright_core::AcceptedCut,
        history: kinewright_core::HistorySnapshot,
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
    fn activate_host_cut_prepared(
        &mut self,
        cut: &kinewright_core::AcceptedCut,
        history: kinewright_core::HistorySnapshot,
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
        let bytes =
            serde_json::to_vec(&history).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        if kinewright_media::sha256_bytes(&bytes) != publisher_digest {
            self.host_lost()?;
            return Err(BundleError::Identity(
                "host activation publisher history digest differs".into(),
            ));
        }
        let (core, attachment, fresh_cut) = if let Some(prepared) = prepared {
            (prepared.core, prepared.attachment, prepared.cut)
        } else {
            let core = Core::spawn(kinewright_core::Document::default())
                .map_err(|e| BundleError::Corrupt(e.to_string()))?;
            let attachment = ProjectAttachment::new(&core).map_err(crate::bundle::history_error)?;
            attachment
                .restore_history(history)
                .map_err(crate::bundle::history_error)?;
            let fresh_cut = attachment.freeze().map_err(crate::bundle::history_error)?;
            (core, attachment, fresh_cut)
        };
        if let Err(error) = prepare(&core) {
            Self::seal_prepared_host_actor(&core, &attachment)?;
            if self
                .core
                .attachment_state()
                .map_err(crate::bundle::history_error)?
                != kinewright_core::AttachmentState::Frozen
                || self
                    .attachment
                    .capture()
                    .map_err(crate::bundle::history_error)?
                    != *cut
            {
                self.host_lost()?;
            }
            return Err(error);
        }
        if core
            .attachment_state()
            .map_err(crate::bundle::history_error)?
            != kinewright_core::AttachmentState::Frozen
            || attachment.capture().map_err(crate::bundle::history_error)? != fresh_cut
        {
            Self::seal_prepared_host_actor(&core, &attachment)?;
            return Err(BundleError::HistoryChanged);
        }
        if let Err(error) = self.attachment.seal(cut) {
            Self::seal_prepared_host_actor(&core, &attachment)?;
            self.host_lost()?;
            return Err(crate::bundle::history_error(error));
        }
        if kind != HostActivationKind::Switch
            && let Err(error) = attachment.resume(&fresh_cut)
        {
            // A is already sealed. Keep its history/source and retire all escaped
            // prepared B clones rather than leaving a second usable actor.
            Self::seal_prepared_host_actor(&core, &attachment)?;
            self.host_lost()?;
            return Err(crate::bundle::history_error(error));
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
        }
        Ok(core)
    }

    fn seal_prepared_host_actor(
        core: &Core,
        attachment: &ProjectAttachment,
    ) -> Result<(), BundleError> {
        if core
            .attachment_state()
            .map_err(crate::bundle::history_error)?
            == kinewright_core::AttachmentState::Sealed
        {
            return Ok(());
        }
        let current = attachment.freeze().map_err(crate::bundle::history_error)?;
        attachment
            .seal(&current)
            .map_err(crate::bundle::history_error)
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
            .map_err(crate::bundle::history_error)?
            == kinewright_core::AttachmentState::Sealed
        {
            return Ok(());
        }
        let cut = self
            .attachment
            .freeze()
            .map_err(crate::bundle::history_error)?;
        self.host_cut = Some(cut);
        Ok(())
    }

    /// Host-only validation shadow installation for an explicit foreground snapshot.
    pub fn install_host_snapshot(
        &mut self,
        history: kinewright_core::HistorySnapshot,
    ) -> Result<(), BundleError> {
        if self.host_attached || self.prepared.is_some() || self.has_unresolved_publication() {
            return Err(BundleError::Unsettled);
        }
        self.attachment
            .restore_history(history)
            .map_err(crate::bundle::history_error)
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
        let saved = if let CopyStatus::Committed(report) = copy.status() {
            let destination = copy.destination_owner()?;
            let checkpoint = destination.checkpoint(destination.manual_branch_id())?;
            let history = destination
                .store
                .read_record::<kinewright_core::HistorySnapshot>(&checkpoint.history_ref)?;
            attachment
                .restore_history((*history).clone())
                .map_err(crate::bundle::history_error)?;
            Some(report.history_sha256.clone())
        } else {
            None
        };
        let path = copy.destination_owner()?.store.resolved_root()?;
        Ok(Self {
            core: core.clone(),
            attachment,
            source: None,
            destination: Some(RetainedDestination::FirstSave(Box::new(copy))),
            inputs: rebound,
            input_leases: vec![],
            association: Some(ActiveAssociation { path, saved }),
            rescue: RescueSessionState::default(),
            prepared: None,
            recovery_save_pending: false,
            selection_result: None,
            durable_sessions: vec![],
            pending_candidate: None,
            retained_destinations: vec![],
            originating_session: crate::bundle_records::SessionId::fresh()?,
            dirty_cache: std::cell::RefCell::new(None),
            save_disclosure_cache: std::cell::RefCell::new(None),
            host_attached: false,
            host_retention_lost: false,
            host_cut: None,
            host_published_digest: None,
            host_rescue_actors: vec![],
            host_source_admission: None,
            preliminary_save: None,
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
            save_disclosure_cache: std::cell::RefCell::new(None),
            originating_session: crate::bundle_records::SessionId::fresh()?,
            host_attached: false,
            host_retention_lost: false,
            host_cut: None,
            host_published_digest: None,
            host_rescue_actors: vec![],
            host_source_admission: None,
            preliminary_save: None,
        })
    }
    pub fn activate(
        path: &Path,
        admission: BundleWriteAdmission,
        core: &Core,
        inputs: Option<&ObjectRef>,
    ) -> Result<Self, BundleError> {
        let mut bundle = ProjectBundle::activate(path, admission)?;
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
    pub fn open(path: &Path) -> Result<Self, BundleError> {
        let loaded = LoadedBundle::read(path)?;
        let inputs = loaded.root()?.input_manifest_ref.clone();
        let branch = loaded.root()?.manual_branch_id.clone();
        let history = (**loaded
            .branches
            .get(&branch)
            .ok_or(BundleError::Unsettled)?
            .1)
            .clone();
        let saved = loaded.manual_checkpoint()?.history_ref.sha256.clone();
        let bundle = loaded.into_writer()?;
        crate::namespace::note("ja1_session_create", path);
        let core = Core::spawn(kinewright_core::Document::default())
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        bundle
            .attach_core(&core)?
            .restore_history(history)
            .map_err(crate::bundle::history_error)?;
        let mut session = Self::from_bundle(bundle, &core, inputs)?;
        let association = session
            .association
            .as_mut()
            .ok_or(BundleError::HistoryBarrierInvariant)?;
        association.path = path.to_path_buf();
        association.saved = Some(saved);
        Ok(session)
    }
    pub fn from_bundle(
        bundle: ProjectBundle,
        core: &Core,
        inputs: ObjectRef,
    ) -> Result<Self, BundleError> {
        crate::namespace::note("ja1_session_attach", &bundle.store().resolved_root()?);
        let attachment = bundle.attach_core(core)?;
        let path = bundle.store().resolved_root()?;
        Ok(Self {
            core: core.clone(),
            attachment,
            source: None,
            destination: Some(RetainedDestination::Named(Box::new(bundle))),
            inputs,
            input_leases: vec![],
            association: Some(ActiveAssociation { path, saved: None }),
            rescue: RescueSessionState::default(),
            prepared: None,
            recovery_save_pending: false,
            selection_result: None,
            durable_sessions: vec![],
            pending_candidate: None,
            retained_destinations: vec![],
            dirty_cache: std::cell::RefCell::new(None),
            save_disclosure_cache: std::cell::RefCell::new(None),
            originating_session: crate::bundle_records::SessionId::fresh()?,
            host_attached: false,
            host_retention_lost: false,
            host_cut: None,
            host_published_digest: None,
            host_rescue_actors: vec![],
            host_source_admission: None,
            preliminary_save: None,
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
            owner.bundle().is_ok_and(|b| {
                b.store.resolved_root().is_ok_and(|p| p == path)
                    && actor.actor_identity() != self.core.actor_identity()
                    && actor
                        .attachment_state()
                        .is_ok_and(|s| s == kinewright_core::AttachmentState::Frozen)
                    && !self.rescue.attempts.iter().any(|a| {
                        a.core.actor_identity() == actor.actor_identity()
                            && a.capture.cut().actor() == actor.actor_identity()
                    })
            })
        })
    }
    /// Release only a proven, never-active capture destination. Source owners and slots stay.
    pub fn release_completed_capture(&mut self, path: &Path) -> Result<(), BundleError> {
        let index = self
            .retained_destinations
            .iter()
            .position(|(owner, _)| {
                owner
                    .bundle()
                    .is_ok_and(|b| b.store.resolved_root().is_ok_and(|p| p == path))
            })
            .ok_or_else(|| BundleError::Identity("unknown retained destination".into()))?;
        let (owner, actor) = &self.retained_destinations[index];
        let bundle = owner.bundle()?;
        if bundle.publication_state() != PublicationState::Committed
            || bundle.unsettled
            || actor.actor_identity() == self.core.actor_identity()
            || actor
                .attachment_state()
                .map_err(crate::bundle::history_error)?
                != kinewright_core::AttachmentState::Frozen
            || actor
                .pending_history_save()
                .map_err(crate::bundle::history_error)?
                .is_some()
            || self.rescue.attempts.iter().any(|a| {
                a.core.actor_identity() == actor.actor_identity()
                    && a.capture.cut().actor() == actor.actor_identity()
            })
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
        Ok(self.association.as_ref().and_then(|a| a.saved.as_deref())
            != Some(self.history_digest()?.as_str()))
    }
    fn history_digest(&self) -> Result<String, BundleError> {
        Self::cached_history_digest(&self.core, &self.dirty_cache)
    }
    fn cached_history_digest(
        core: &Core,
        cache: &std::cell::RefCell<HistoryDigestCache>,
    ) -> Result<String, BundleError> {
        let actor = core.actor_identity();
        let revision = core
            .history_change_id()
            .map_err(crate::bundle::history_error)?;
        if let Some((cached_actor, cached_revision, digest)) = &*cache.borrow()
            && *cached_actor == actor
            && *cached_revision == revision
        {
            return Ok(digest.clone());
        }
        let (revision, history) = core
            .export_history_identified()
            .map_err(crate::bundle::history_error)?;
        #[cfg(any(test, feature = "test-util"))]
        crate::namespace::note("studio_dirty_history_hash", Path::new("dirty-poll"));
        let bytes =
            serde_json::to_vec(&history).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let digest = kinewright_media::sha256_bytes(&bytes);
        *cache.borrow_mut() = Some((actor, revision, digest.clone()));
        Ok(digest)
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
        let copy = source.first_save_with_limits(path, &self.core, &self.inputs, limits)?;
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
        });
        self.destination = Some(RetainedDestination::FirstSave(Box::new(copy)));
        self.retire_ordinary_copy()?;
        Ok(status)
    }
    /// Ordinary saves never inspect the rescue slot ledger.
    pub fn save_active(&mut self) -> Result<BundleSaveReport, BundleError> {
        self.require_local_writer()?;
        let mut report = self
            .destination
            .as_mut()
            .ok_or_else(|| BundleError::Identity("choose a first-save destination".into()))?
            .save(&self.core, &self.inputs)?;
        let saved = report.history_sha256.clone();
        self.association
            .as_mut()
            .ok_or(BundleError::HistoryBarrierInvariant)?
            .saved = Some(saved);
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
        self.durable_sessions
            .iter()
            .filter(|registered| registered.guard.epoch == bundle.store.owner_epoch())
            .map(|registered| registered.guard.core())
            .find(|core| eligible(core) && core.actor_identity() == attempt.actor())
            .cloned()
    }
    pub fn reconcile_active(&mut self) -> Result<Option<BundleSaveReport>, BundleError> {
        if self.pending_candidate.is_some() {
            self.settle_pending_candidate(true)?;
            return Ok(None);
        }
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
        result
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
                    LoadedBundle::read(&tag.destination)?;
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
    ) -> Result<Option<StorageReservation<FrozenRescueCapture>>, BundleError> {
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
            .map_err(crate::bundle::history_error)?
            == kinewright_core::AttachmentState::Frozen;
        let (store, roots) = self.capture_source()?;
        let _working = store.reserve_capture_scratch()?;
        let cut = self
            .attachment
            .freeze()
            .map_err(crate::bundle::history_error)?;
        let result = (|| {
            let reads = store.admit_retained_reads(&roots)?;
            let bytes = serde_json::to_vec(cut.history())
                .map_err(|e| BundleError::Corrupt(e.to_string()))?;
            let charge = store.reserve_retained_capture(&bytes)?;
            let history_digest = kinewright_media::sha256_bytes(&bytes);
            let binding =
                serde_json::to_vec(&(cut.revision(), &roots, &self.inputs, history_digest))
                    .map_err(|e| BundleError::Corrupt(e.to_string()))?;
            let digest = kinewright_media::sha256_bytes(&binding);
            Ok(charge.hold(FrozenRescueCapture {
                cut: cut.clone(),
                tag: second.tag,
                digest,
                reads,
                was_frozen,
            }))
        })();
        if result.is_err() && !was_frozen {
            self.attachment
                .resume(&cut)
                .map_err(crate::bundle::history_error)?;
        }
        result.map(Some)
    }
    pub fn cancel_rescue(
        &self,
        capture: StorageReservation<FrozenRescueCapture>,
    ) -> Result<(), BundleError> {
        if !capture.was_frozen {
            self.attachment
                .resume(capture.cut())
                .map_err(crate::bundle::history_error)?;
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
        capture: StorageReservation<FrozenRescueCapture>,
        consent: &RescueConsent,
        destination: &DestinationInspection,
        unknown: Option<HistorySaveTag>,
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
            .map_err(crate::bundle::history_error)?
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
                core: self.core.clone(),
                capture: std::sync::Arc::new(capture),
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
                    .map_err(crate::bundle::history_error)?,
            )
            .map_err(crate::bundle::history_error)
    }
    #[cfg(any(test, feature = "test-util"))]
    pub fn freeze_probe(&self) -> Result<kinewright_core::AcceptedCut, BundleError> {
        self.attachment
            .freeze()
            .map_err(crate::bundle::history_error)
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
                let _ = owner.seal(&cut);
            }
        };
        fence(&self.core);
        for registration in &self.durable_sessions {
            fence(registration.guard.core());
        }
        for (_, core) in &self.retained_destinations {
            fence(core);
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
