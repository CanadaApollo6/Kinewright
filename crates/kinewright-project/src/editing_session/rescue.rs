//! A1 publication and immediate-only handoff under the same originating owner.
use super::{
    ActiveAssociation, BundleError, CopyStatus, Core, DestinationInspection, FrozenRescueCapture,
    Ja1EditingSession, Path, ProjectAttachment, ProjectStore, PublicationState, RescueEligibility,
    RetainedAttempt, RetainedDestination, StorageLimits, StorageReservation,
};
use crate::{
    bundle_rescue::{RescueConsent, RescueOutcome, RescueSelection},
    rescue_record::{ProvenanceSummary, RESCUE_ROLE, RescueAttemptProvenance, RescueCopyV1},
};

impl Ja1EditingSession {
    pub fn preview_rescue(
        &self,
        capture: StorageReservation<FrozenRescueCapture>,
        limits: StorageLimits,
    ) -> Result<RescueSelection, BundleError> {
        let was_frozen = capture.was_frozen;
        let cut = capture.cut.clone();
        let result = self.preview_rescue_inner(capture, limits);
        if result.is_err() && !was_frozen {
            self.attachment
                .resume(&cut)
                .map_err(crate::bundle::history_error)?;
        }
        result
    }
    pub fn cancel_rescue_selection(&self, selection: RescueSelection) -> Result<(), BundleError> {
        self.cancel_rescue(selection.capture)
    }
    pub fn rescue(
        &mut self,
        selection: RescueSelection,
        consent: &RescueConsent,
        inspection: &DestinationInspection,
    ) -> Result<RescueOutcome, BundleError> {
        self.rescue_with_activation(selection, consent, inspection, |_| Ok(()))
    }
    /// The caller prepares its attachment before any destination mutation or actor seal.
    pub fn rescue_with_activation(
        &mut self,
        selection: RescueSelection,
        consent: &RescueConsent,
        inspection: &DestinationInspection,
        prepare: impl FnOnce(&Core) -> Result<(), BundleError>,
    ) -> Result<RescueOutcome, BundleError> {
        self.require_local_writer()?;
        let resume = !selection.capture.was_frozen;
        let result = self.rescue_inner(selection, consent, inspection, prepare);
        if result.is_err()
            && resume
            && self
                .core
                .attachment_state()
                .map_err(crate::bundle::history_error)?
                == kinewright_core::AttachmentState::Frozen
        {
            let current = self
                .attachment
                .capture()
                .map_err(crate::bundle::history_error)?;
            self.attachment
                .resume(&current)
                .map_err(crate::bundle::history_error)?;
        }
        result
    }
    #[allow(clippy::too_many_lines)]
    fn rescue_inner(
        &mut self,
        selection: RescueSelection,
        consent: &RescueConsent,
        inspection: &DestinationInspection,
        prepare: impl FnOnce(&Core) -> Result<(), BundleError>,
    ) -> Result<RescueOutcome, BundleError> {
        selection.check_consent(consent)?;
        let Some(current_probe) = self.probe_active()? else {
            return Err(BundleError::HistoryChanged);
        };
        if current_probe.tag != selection.capture.tag
            || current_probe.eligibility != RescueEligibility::Eligible
        {
            return Err(BundleError::HistoryChanged);
        }
        if self
            .attachment
            .capture()
            .map_err(crate::bundle::history_error)?
            != selection.capture.cut
            || self.active_bundle()?.publication_tag.as_ref() != Some(&selection.capture.tag)
            || self.active_bundle()?.publication_state() != PublicationState::Unknown
        {
            return Err(BundleError::HistoryChanged);
        }
        self.rescue.slots.check(&selection.capture.tag)?;
        let mut stores = vec![self.store()?];
        if let Some(source) = &self.source {
            stores.push(source.store());
        }
        for (owner, _) in &self.retained_destinations {
            stores.push(owner.bundle()?.store());
        }
        ProjectStore::revalidate_rescue_destination(inspection, &stores)?;
        if inspection.path().try_exists()? {
            return Err(BundleError::Identity(
                "rescue destination must be fresh".into(),
            ));
        }
        let b_core = Core::spawn((*selection.history.baseline).clone())
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let b_attachment = ProjectAttachment::new(&b_core).map_err(crate::bundle::history_error)?;
        b_attachment
            .restore_history(selection.history.clone())
            .map_err(crate::bundle::history_error)?;
        let b_cut = b_attachment
            .freeze()
            .map_err(crate::bundle::history_error)?;
        prepare(&b_core)?;
        let old_core = self.core.clone();
        let old_cut = selection.capture.cut.clone();
        let was_frozen = selection.capture.was_frozen;
        let disclosure = self.rescue_handoff_disclosure(&selection)?;
        let tag = selection.capture.tag.clone();
        let existing = self.rescue.attempts.iter().position(|a| a.tag == tag);
        self.rescue
            .attempts
            .try_reserve(2)
            .map_err(|_| BundleError::Capacity("retained attempts"))?;
        self.retained_destinations
            .try_reserve(2)
            .map_err(|_| BundleError::Capacity("retained destinations"))?;
        // Reserve the record before the first destination mutation, while capture ownership is provisional.
        let bundle = self.active_bundle()?;
        let mut attempts = Vec::new();
        for attempt in &self.rescue.attempts {
            attempts.push(RescueAttemptProvenance {
                session_id: self.originating_session.clone(),
                project_id: attempt.project_id.clone(),
                destination: attempt.tag.destination.clone(),
                parent_device: attempt.tag.parent_device,
                parent_inode: attempt.tag.parent_inode,
                attempted_root_sha256: attempt.tag.root_sha256.clone(),
                pending_command: attempt.pending_command.clone(),
            });
        }
        if existing.is_none() {
            attempts.push(RescueAttemptProvenance {
                session_id: self.originating_session.clone(),
                project_id: bundle.project_id().clone(),
                destination: tag.destination.clone(),
                parent_device: tag.parent_device,
                parent_inode: tag.parent_inode,
                attempted_root_sha256: tag.root_sha256.clone(),
                pending_command: bundle
                    .pending_command
                    .as_ref()
                    .map(|e| e.digest().map(|digest| (e.id.clone(), digest)))
                    .transpose()?,
            });
        }
        let record = RescueCopyV1 {
            role: RESCUE_ROLE.into(),
            schema: 1,
            project_id: bundle.project_id().clone(),
            originating_session: self.originating_session.clone(),
            full_capture_sha256: selection.capture.digest.clone(),
            selected_capture_sha256: selection.selected_digest,
            old_floor: old_cut.history().floor_offset,
            new_floor: selection.history.floor_offset,
            removed_undo: selection.removed_undo,
            redo_preserved: true,
            gesture_preserved: true,
            acknowledged_root: None,
            acknowledged_absence: Some(
                "first save of an unsaved session has no acknowledged root".into(),
            ),
            recovery_origin: None,
            omissions: selection.omissions,
            attempts,
            summary: ProvenanceSummary::default(),
        };
        let encoded =
            serde_json::to_vec(&record).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let _record_charge = self.store()?.reserve_retained_capture(&encoded)?;
        let original_attempt_core = self
            .core_for_destination(self.active_bundle()?, Some(&self.core))
            .unwrap_or_else(|| old_core.clone());
        let ticket = self.active_bundle()?.pending_attempt();
        let project_id = self.active_bundle()?.project_id().clone();
        let pending_command = self
            .active_bundle()?
            .pending_command
            .as_ref()
            .map(|e| e.digest().map(|d| (e.id.clone(), d)))
            .transpose()?;
        let mut reservation = None;
        let mut capture = Some(std::sync::Arc::new(selection.capture));
        let unsaved =
            self.association.as_ref().is_some_and(|a| a.saved.is_none()) && self.source.is_some();
        let owner = self.destination.as_ref().ok_or(BundleError::Unsettled)?;
        let (source, named, inputs) = if unsaved && let RetainedDestination::FirstSave(copy) = owner
        {
            (
                self.source.as_ref().ok_or(BundleError::Unsettled)?.store(),
                None,
                copy.source_manifest(&self.inputs)?,
            )
        } else {
            let bundle = owner.bundle()?;
            (&bundle.store, Some(bundle), self.inputs.clone())
        };
        let rescue = &mut self.rescue;
        let mut on_mutation = || {
            if reservation.is_none() {
                reservation = Some(rescue.slots.begin(tag.clone())?);
                if existing.is_none() {
                    rescue.attempts.push(RetainedAttempt {
                        state: PublicationState::Unknown,
                        tag: tag.clone(),
                        core: original_attempt_core.clone(),
                        capture: capture.take().ok_or(BundleError::HistoryBarrierInvariant)?,
                        ticket: ticket.clone(),
                        project_id: project_id.clone(),
                        pending_command: pending_command.clone(),
                    });
                }
            }
            Ok(())
        };
        let publication = crate::bundle_copy::rescue_copy(
            source,
            named,
            &inputs,
            &selection.roots,
            inspection,
            &mut on_mutation,
            selection.limits,
            &selection.history,
            &record,
        );
        let mut copy = match publication {
            Ok(copy) => copy,
            Err(error) => {
                if let Some(reservation) = reservation {
                    self.rescue.slots.finish(reservation, None)?;
                }
                if !was_frozen {
                    self.attachment
                        .resume(&old_cut)
                        .map_err(crate::bundle::history_error)?;
                }
                return Err(error);
            }
        };
        let unknown = if matches!(copy.status(), CopyStatus::DurabilityUnknown) {
            copy.destination_owner()?.publication_tag.clone()
        } else {
            None
        };
        self.rescue.slots.finish(
            reservation.ok_or(BundleError::HistoryBarrierInvariant)?,
            unknown.clone(),
        )?;
        if let Some(destination_tag) = unknown {
            let source_capture = std::sync::Arc::clone(
                &self
                    .rescue
                    .attempts
                    .iter()
                    .find(|a| a.tag == tag)
                    .ok_or(BundleError::HistoryBarrierInvariant)?
                    .capture,
            );
            self.rescue.attempts.push(RetainedAttempt {
                state: PublicationState::Unknown,
                tag: destination_tag,
                core: b_core.clone(),
                capture: source_capture,
                ticket: None,
                project_id: copy.destination_owner()?.project_id().clone(),
                pending_command: None,
            });
        }
        let status = copy.status().clone();
        let immediate = matches!(status, CopyStatus::Committed(_));
        if immediate {
            let inputs = copy.rebound_manifest(&inputs)?;
            let saved = kinewright_media::sha256_bytes(
                &serde_json::to_vec(&selection.history)
                    .map_err(|e| BundleError::Corrupt(e.to_string()))?,
            );
            let path = copy.destination_owner()?.store().resolved_root()?;
            // Every fallible preparation precedes the seal. A changed cut leaves a capture copy.
            if self.attachment.seal(&old_cut).is_ok() {
                if let Err(error) = b_attachment.resume(&b_cut) {
                    self.retained_destinations
                        .push((RetainedDestination::FirstSave(Box::new(copy)), b_core));
                    return Ok(RescueOutcome {
                        status,
                        activated: false,
                        disclosure: format!(
                            "A is sealed and editing could not move to B: {error}. The committed rescue copy is at {}. Open B separately read-only, or explicitly release its completed copy owner before opening it for editing. {disclosure}",
                            inspection.path().display()
                        ),
                    });
                }
                let original = self
                    .destination
                    .take()
                    .ok_or(BundleError::HistoryBarrierInvariant)?;
                self.retained_destinations.push((original, old_core));
                self.destination = Some(RetainedDestination::FirstSave(Box::new(copy)));
                self.core = b_core;
                self.attachment = b_attachment;
                self.inputs = inputs;
                self.input_leases.clear();
                self.association = Some(ActiveAssociation {
                    path,
                    saved: Some(saved),
                });
                self.prepared = None;
                self.recovery_save_pending = false;
                self.selection_result = None;
                return Ok(RescueOutcome {
                    status,
                    activated: true,
                    disclosure,
                });
            }
        }
        // Unknown/late copies never have activation permission, even if the histories later match.
        let _ = copy.destination_owner_mut()?;
        self.retained_destinations
            .push((RetainedDestination::FirstSave(Box::new(copy)), b_core));
        if !was_frozen {
            self.attachment
                .resume(&old_cut)
                .map_err(crate::bundle::history_error)?;
        }
        Ok(RescueOutcome {
            status,
            activated: false,
            disclosure,
        })
    }
    pub fn reconcile_retained_destination(
        &mut self,
        path: &Path,
    ) -> Result<PublicationState, BundleError> {
        let index = self
            .retained_destinations
            .iter()
            .position(|(owner, _)| {
                owner
                    .bundle()
                    .is_ok_and(|bundle| bundle.store.resolved_root().is_ok_and(|p| p == path))
            })
            .ok_or_else(|| BundleError::Identity("unknown retained destination".into()))?;
        // Only an original registered actor may settle its own ticket at this namespace.
        // Captured B and the sealed manual A receive no Core call.
        let live = self.core_for_destination(self.retained_destinations[index].0.bundle()?, None);
        let owner = &mut self.retained_destinations[index].0;
        let tag = owner.bundle()?.publication_tag.clone();
        match owner {
            RetainedDestination::Named(bundle) => {
                bundle.reconcile_destination(live.as_ref())?;
            }
            RetainedDestination::FirstSave(copy) => {
                copy.reconcile_destination(live.as_ref())?;
            }
        }
        let state = owner.bundle()?.publication_state();
        if let Some(tag) = &tag {
            self.rescue.settle(tag, state);
        }
        Ok(state)
    }
}
