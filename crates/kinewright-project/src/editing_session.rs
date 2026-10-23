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
type RescueSource<'a> = (&'a ProjectStore, Vec<(ObjectRef, MissingOwnedUse)>);

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
    pub(crate) capture: StorageReservation<FrozenRescueCapture>,
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
    association: Option<ActiveAssociation>,
    rescue: RescueSessionState,
}
impl Ja1EditingSession {
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
            association: None,
            rescue: RescueSessionState::default(),
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
        Self::from_bundle(bundle, core, inputs)
    }
    pub fn open(path: &Path) -> Result<Self, BundleError> {
        let loaded = LoadedBundle::read(path)?;
        let core = Core::spawn(kinewright_core::Document::default())
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        loaded.restore_manual(&core)?;
        let inputs = loaded.root()?.input_manifest_ref.clone();
        let mut session = Self::from_bundle(loaded.into_writer()?, &core, inputs)?;
        session
            .association
            .as_mut()
            .ok_or(BundleError::HistoryBarrierInvariant)?
            .saved = Some(session.history_digest()?);
        Ok(session)
    }
    pub fn from_bundle(
        bundle: ProjectBundle,
        core: &Core,
        inputs: ObjectRef,
    ) -> Result<Self, BundleError> {
        let attachment = bundle.attach_core(core)?;
        let path = bundle.store().resolved_root()?;
        Ok(Self {
            core: core.clone(),
            attachment,
            source: None,
            destination: Some(RetainedDestination::Named(Box::new(bundle))),
            inputs,
            association: Some(ActiveAssociation { path, saved: None }),
            rescue: RescueSessionState::default(),
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
    pub const fn source(&self) -> Option<&SessionStore> {
        self.source.as_ref()
    }
    pub fn source_mut(&mut self) -> Result<&mut SessionStore, BundleError> {
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
        let history = self
            .core
            .export_history()
            .map_err(crate::bundle::history_error)?;
        let bytes =
            serde_json::to_vec(&history).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        Ok(kinewright_media::sha256_bytes(&bytes))
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
        let mut roots = bundle
            .root_references()?
            .into_iter()
            .map(|r| (r, MissingOwnedUse::Manifest))
            .collect::<Vec<_>>();
        roots.push((self.inputs.clone(), MissingOwnedUse::Current));
        Ok((&bundle.store, roots))
    }
    pub fn first_save(
        &mut self,
        path: &Path,
        limits: StorageLimits,
    ) -> Result<CopyStatus, BundleError> {
        if self.destination.is_some() {
            return Err(BundleError::Identity("session already associated".into()));
        }
        let source = self
            .source
            .as_mut()
            .ok_or(BundleError::HistoryBarrierInvariant)?;
        let copy = source.first_save_with_limits(path, &self.core, &self.inputs, limits)?;
        self.inputs = copy.rebound_manifest(&self.inputs)?;
        let status = copy.status().clone();
        let saved = if matches!(status, CopyStatus::Committed(_)) {
            Some(self.history_digest()?)
        } else {
            None
        };
        self.association = Some(ActiveAssociation {
            path: copy.destination_owner()?.store().resolved_root()?,
            saved,
        });
        self.destination = Some(RetainedDestination::FirstSave(Box::new(copy)));
        self.retire_ordinary_copy()?;
        Ok(status)
    }
    /// Ordinary saves never inspect the rescue slot ledger.
    pub fn save_active(&mut self) -> Result<BundleSaveReport, BundleError> {
        let report = self
            .destination
            .as_mut()
            .ok_or_else(|| BundleError::Identity("choose a first-save destination".into()))?
            .save(&self.core, &self.inputs)?;
        let saved = self.history_digest()?;
        self.association
            .as_mut()
            .ok_or(BundleError::HistoryBarrierInvariant)?
            .saved = Some(saved);
        self.retire_ordinary_copy()?;
        Ok(report)
    }
    fn retire_ordinary_copy(&mut self) -> Result<(), BundleError> {
        if !self.rescue.attempts.is_empty()
            || !matches!(&self.destination, Some(RetainedDestination::FirstSave(copy)) if matches!(copy.status(), CopyStatus::Committed(_)))
        {
            return Ok(());
        }
        if let Some(RetainedDestination::FirstSave(copy)) = self.destination.take() {
            match copy.into_destination() {
                Ok(bundle) => {
                    self.destination = Some(RetainedDestination::Named(Box::new(bundle)));
                    self.source = None;
                }
                Err(copy) => {
                    self.destination = Some(RetainedDestination::FirstSave(copy));
                    return Err(BundleError::Unsettled);
                }
            }
        }
        Ok(())
    }
    pub fn reconcile_active(&mut self) -> Result<Option<BundleSaveReport>, BundleError> {
        let destination = self
            .destination
            .as_mut()
            .ok_or(BundleError::HistoryBarrierInvariant)?;
        let tag = destination.bundle()?.publication_tag.clone();
        let result = destination.reconcile(&self.core);
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
                capture,
                ticket,
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
