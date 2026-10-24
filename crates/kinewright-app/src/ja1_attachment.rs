//! Local S2-R controls. The originating owner moves into each finite background operation.
use eframe::egui;
use kinewright_core::{
    AttachmentState, Command, Core, Document, Event, Query, QueryResult, TimelineRevision,
};
use kinewright_project::{
    bundle::{BundleError, PublicationState},
    bundle_copy::CopyStatus,
    bundle_records::{BranchId, ObjectRef},
    bundle_recovery::RecoverySelection,
    bundle_rescue::{RescueConsent, RescueEligibility, RescueProbe, RescueSelection},
    bundle_switch::ProjectSelectionResult,
    editing_session::Ja1EditingSession,
    storage_budget::StorageLimits,
};
use std::fmt::Write as _;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub(crate) struct PreparedView {
    pub core: Core,
    pub events: crossbeam_channel::Receiver<Event>,
    pub document: Arc<Document>,
    pub revision: TimelineRevision,
}
impl PreparedView {
    fn prepare(core: &Core) -> Result<Self, BundleError> {
        let events = core
            .subscribe()
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let Event::QueryResult(QueryResult::Snapshot { document, revision }) = core
            .request(Command::Query(Query::Snapshot))
            .map_err(|e| BundleError::Corrupt(e.to_string()))?
        else {
            return Err(BundleError::HistoryBarrierInvariant);
        };
        Ok(Self {
            core: core.clone(),
            events,
            document,
            revision,
        })
    }
}

pub(crate) enum Action {
    Save,
    ConfirmSuccessorSave,
    SaveAs(PathBuf),
    FirstSave(PathBuf),
    Retry,
    PreviewRecovery(RecoverySelection),
    OpenRecovery,
    Abandon,
    PreviewRescue,
    ConsentTrim,
    ConsentOmission,
    RescueTo(PathBuf),
    Cancel,
    PreviewRetirement(BranchId),
    RemoveBranch,
    PreviewForget,
    Forget,
    Reconcile(PathBuf),
    PreviewRelease(PathBuf),
    Release,
    SaveSession(BranchId),
    CloseSession(BranchId),
}
enum Choice {
    SuccessorSave(Option<PathBuf>),
    Recovery,
    Rescue {
        selection: Box<RescueSelection>,
        consent: RescueConsent,
        trim: bool,
        omission: bool,
    },
    Remove {
        branch: BranchId,
        root: String,
    },
    Forget {
        point: ObjectRef,
        root: String,
    },
    Release(PathBuf),
}

pub(crate) struct Ja1Attachment {
    pub owner: Ja1EditingSession,
    pub message: String,
    choice: Option<Choice>,
    offer: Option<RescueProbe>,
    pub view: Option<PreparedView>,
    points: Vec<RecoverySelection>,
    branches: Vec<(BranchId, Vec<String>)>,
    candidates: Vec<BranchId>,
    destinations: Vec<(PathBuf, PublicationState)>,
    frozen: bool,
    pub panel_open: bool,
    known_agent_effects: std::collections::BTreeSet<PathBuf>,
    save_disclosure: String,
    #[cfg(test)]
    last_selection: Option<ProjectSelectionResult>,
    #[cfg(test)]
    last_save: Option<kinewright_project::bundle::BundleSaveReport>,
    #[cfg(test)]
    last_rescue: Option<kinewright_project::bundle_rescue::RescueOutcome>,
    #[cfg(test)]
    last_retirement: Option<kinewright_project::bundle::BundleSaveReport>,
}
impl Ja1Attachment {
    pub fn open(path: &Path) -> Result<Self, BundleError> {
        Self::new(Ja1EditingSession::open(path)?)
    }
    pub fn new(owner: Ja1EditingSession) -> Result<Self, BundleError> {
        let view = Some(PreparedView::prepare(owner.core())?);
        let mut this = Self {
            owner,
            message: "Studio editing. Saved means a verified durable save.".into(),
            choice: None,
            offer: None,
            view,
            points: vec![],
            branches: vec![],
            candidates: vec![],
            destinations: vec![],
            frozen: false,
            panel_open: true,
            known_agent_effects: std::collections::BTreeSet::new(),
            save_disclosure: String::new(),
            #[cfg(test)]
            last_selection: None,
            #[cfg(test)]
            last_save: None,
            #[cfg(test)]
            last_rescue: None,
            #[cfg(test)]
            last_retirement: None,
        };
        this.refresh();
        Ok(this)
    }
    fn refresh(&mut self) {
        self.frozen = self.owner.core().attachment_state() != Ok(AttachmentState::Live);
        self.points = self.owner.list_recovery_points().unwrap_or_default();
        self.branches = self
            .owner
            .list_working_branches()
            .unwrap_or_default()
            .into_iter()
            .map(|(id, _, blockers)| (id, blockers))
            .collect();
        self.candidates = self.owner.retirement_candidates().unwrap_or_default();
        self.destinations = self.owner.retained_destinations().unwrap_or_default();
    }
    pub fn destination_change(&self) -> Result<(), BundleError> {
        self.owner.check_destination_change()
    }
    pub fn close_text(&self) -> String {
        let mut text = self.owner.close_disclosure().unwrap_or_else(|e| format!("Could not verify this project's state: {e}. Closing can discard work still held only in this session."));
        if !self.destinations.is_empty() {
            text.push_str(" Closing ends retention of the original session, including unknown destinations; it does not prove that they never saved.");
        }
        text
    }
    pub fn read_only(&self) -> bool {
        self.frozen || self.choice.is_some()
    }
    pub fn apply(&mut self, action: Action) -> Result<(), BundleError> {
        let result = self.apply_inner(action);
        if let Err(error) = &result {
            self.message = match error {
                BundleError::RescueMissingOwnedBytes(missing) => format!(
                    "Rescue copy was not saved. Required project data is unavailable from A: {:?}. {} {} items omitted. Your edits remain in this session. Reconnect A or restore these exact objects, then retry. A's save outcome is still unknown. Source media availability is separate.",
                    missing.entries,
                    if missing.dependencies_complete {
                        ""
                    } else {
                        "The dependency list is incomplete."
                    },
                    missing.omitted
                ),
                BundleError::Capacity(_) => format!(
                    "There is not enough room to reopen the proposed project. Your current edit is {}. Remove an eligible working branch, then retry this same selection. {error}",
                    if self.owner.dirty()? {
                        "unchanged"
                    } else {
                        "saved in its working branch"
                    }
                ),
                _ => error.to_string(),
            };
            if self
                .owner
                .core()
                .attachment_state()
                .is_ok_and(|s| s == AttachmentState::Frozen)
            {
                self.message.push_str(" Rescue copy was not saved. Editing remains read-only while A's save or selection is unresolved.");
            }
        }
        self.refresh();
        result
    }
    #[allow(clippy::too_many_lines)]
    fn apply_inner(&mut self, action: Action) -> Result<(), BundleError> {
        match action {
            Action::Save => {
                if self
                    .owner
                    .association()
                    .is_some_and(|a| self.known_agent_effects.contains(a.path()))
                {
                    self.message = "A contains the agent command's effect as of its proven save; your current edits are unchanged. Saving this current edit is a separate successor save and omits that effect. Confirm this choice before saving.".into();
                    self.choice = Some(Choice::SuccessorSave(None));
                    return Ok(());
                }
                self.apply_inner(Action::ConfirmSuccessorSave)?;
            }
            Action::ConfirmSuccessorSave => {
                let destination = match self.choice.take() {
                    Some(Choice::SuccessorSave(path)) => path,
                    choice => {
                        self.choice = choice;
                        None
                    }
                };
                let report = self.owner.save_active()?;
                self.message = format!("Saved {}.", report.path.display());
                self.known_agent_effects.remove(&report.path);
                self.choice = None;
                #[cfg(test)]
                {
                    self.last_save = Some(report);
                }
                if let Some(path) = destination {
                    self.apply_inner(Action::SaveAs(path))?;
                }
            }
            Action::FirstSave(path) => {
                self.message = copy_text(&self.owner.first_save(&path, StorageLimits::default())?);
            }
            Action::SaveAs(path) => {
                self.owner.check_destination_change()?;
                if self
                    .owner
                    .association()
                    .is_some_and(|a| self.known_agent_effects.contains(a.path()))
                {
                    self.message = "A contains the agent command's effect as of its proven save; your current edits are unchanged. Save As first saves this current edit at A and omits that effect. Confirm this separate successor save.".into();
                    self.choice = Some(Choice::SuccessorSave(Some(path)));
                } else {
                    self.message = copy_text(&self.owner.save_as(&path, StorageLimits::default())?);
                }
            }
            Action::PreviewRecovery(target) => {
                self.owner.prepare_recovery(target)?;
                self.save_disclosure = self.owner.recovery_save_disclosure()?;
                self.message = format!(
                    "Editing is read-only while your current work is saved, then the selected rebound recovery point is opened. Your prior edit remains an ordinary working branch. {} {}",
                    self.save_disclosure,
                    self.owner.selection_availability()?
                );
                self.choice = Some(Choice::Recovery);
            }
            Action::OpenRecovery => {
                self.choice = None;
                if self.owner.dirty()? && !self.owner.has_unresolved_publication() {
                    self.save_disclosure = self.owner.recovery_save_disclosure()?;
                }
                let mut view = None;
                let result = self.owner.select_local_with_activation(|core| {
                    view = Some(PreparedView::prepare(core)?);
                    Ok(())
                });
                self.message = match &result {
                    ProjectSelectionResult::Activated(report) => {
                        self.view = view;
                        format!(
                            "Opened rebound recovery point in {}. Your prior work remains saved in its working branch.",
                            report.path.display()
                        )
                    }
                    ProjectSelectionResult::Unknown => self.unknown_text(),
                    ProjectSelectionResult::CommittedWithError(error) => format!(
                        "The selection committed; editing remains read-only while attachment is reconciled: {error}"
                    ),
                    ProjectSelectionResult::Refused(error) => {
                        self.message = if self.owner.dirty()? {
                            "Your current edit is unchanged. The save did not complete, so the recovery point was not opened. ".into()
                        } else {
                            "Your current work was saved in its working branch. The recovery point was not opened. Editing continues there. ".into()
                        };
                        let prefix = self.message.clone();
                        self.message =
                            format!("{prefix}{error}. {}", self.owner.selection_availability()?);
                        if matches!(error, BundleError::Capacity(_)) {
                            self.message.push_str(" There is not enough room to reopen the proposed project. Remove an eligible working branch, then retry this same selected point.");
                        }
                        self.message.clone()
                    }
                };
                self.message.push(' ');
                self.message.push_str(&self.save_disclosure);
                #[cfg(test)]
                {
                    self.last_selection = Some(result);
                }
            }
            Action::Retry => {
                let destination = self.owner.association().map(|a| a.path().to_path_buf());
                let pending = destination
                    .as_ref()
                    .is_some_and(|path| self.owner.pending_agent_command_at(path));
                let reconciliation = if self.owner.has_prepared_selection() {
                    self.apply_inner(Action::OpenRecovery)?;
                    Ok(())
                } else {
                    let result = self.owner.reconcile_active().map(|_| ());
                    self.message = "A's destination was checked; current edits and their Saved baseline are unchanged.".into();
                    result
                };
                // A failed ordinary/first-save Retry must still reach the phase-aware probe.
                self.offer = self.owner.probe_active()?;
                if pending && !self.owner.has_unresolved_publication() {
                    if self.owner.active_publication_state()? == PublicationState::Committed {
                        self.message.push_str(" A contains the agent command's effect as of its proven save; your current edits are unchanged. A later Save is a separate successor and will disclose that omission.");
                        if let Some(path) = destination {
                            self.known_agent_effects.insert(path);
                        }
                    } else {
                        self.message.push_str(" A shows the older or absent root; the pending command was not proved committed. Your current edits are unchanged.");
                    }
                }
                if self
                    .offer
                    .as_ref()
                    .is_some_and(|p| p.eligibility == RescueEligibility::Eligible)
                {
                    self.message.push_str(" A's save outcome is unknown. Your edits are still in this session. Save rescue copy is available.");
                }
                reconciliation?;
            }
            Action::Abandon => {
                self.message = self.owner.abandon_recovery()?;
                self.message
                    .push_str(" Selection explicitly abandoned. You may now change destination.");
                self.choice = None;
            }
            Action::PreviewRescue => {
                self.offer = self.owner.probe_active()?;
                let offer = self.offer.as_ref().ok_or(BundleError::HistoryChanged)?;
                let capture = self
                    .owner
                    .prepare_rescue(offer)?
                    .ok_or(BundleError::HistoryChanged)?;
                let selection = self
                    .owner
                    .preview_rescue(capture, self.owner.store()?.limits().clone())?;
                let consent = selection.consent();
                self.message = rescue_preview(&selection);
                self.message
                    .push_str(&self.owner.rescue_handoff_disclosure(&selection)?);
                self.choice = Some(Choice::Rescue {
                    selection: Box::new(selection),
                    consent,
                    trim: false,
                    omission: false,
                });
            }
            Action::ConsentTrim => {
                let Some(Choice::Rescue {
                    selection,
                    consent,
                    trim,
                    ..
                }) = &mut self.choice
                else {
                    return Err(BundleError::HistoryChanged);
                };
                selection.consent_trim(consent)?;
                *trim = true;
            }
            Action::ConsentOmission => {
                let Some(Choice::Rescue {
                    selection,
                    consent,
                    omission,
                    ..
                }) = &mut self.choice
                else {
                    return Err(BundleError::HistoryChanged);
                };
                selection.consent_omission(consent)?;
                *omission = true;
            }
            Action::RescueTo(path) => {
                let inspection = self.owner.inspect_rescue_destination(&path)?;
                self.rescue_with_inspection(&inspection)?;
            }
            Action::Cancel => {
                if let Some(Choice::Rescue { selection, .. }) = self.choice.take() {
                    self.owner.cancel_rescue_selection(*selection)?;
                }
                self.message = if self.owner.has_unresolved_publication() {
                    "Rescue copy was not saved. Editing remains read-only while A's save or selection is unresolved.".into()
                } else if self.owner.has_prepared_selection() {
                    "Cancelled. Your current edit is unchanged; the selected point remains prepared for retry or explicit abandonment.".into()
                } else {
                    "Cancelled. Your current edit is unchanged.".into()
                };
                self.choice = None;
            }
            Action::PreviewRetirement(branch) => {
                self.message = self.owner.retirement_disclosure(&branch)?;
                self.choice = Some(Choice::Remove {
                    branch,
                    root: self.owner.root_sha256()?,
                });
            }
            Action::RemoveBranch => {
                let Some(Choice::Remove { branch, root }) = self.choice.take() else {
                    return Err(BundleError::HistoryChanged);
                };
                let report = self.owner.retire_working_branch(&branch, &root, true)?;
                #[cfg(test)]
                {
                    self.last_retirement = Some(report);
                }
                #[cfg(not(test))]
                {
                    let _ = report;
                }
                self.message = "Working branch removed. Independent recovery, evidence, readers and source holds may retain its bytes. Retry uses the same selected point.".into();
            }
            Action::PreviewForget => {
                let point = self
                    .owner
                    .acknowledged_recovery_point()?
                    .ok_or(BundleError::HistoryChanged)?;
                self.message = format!(
                    "Forget the older recovery point from A: {}. Its history will no longer be offered by this rescue record. Independent evidence, readers and recovery roots can still retain these bytes. This does not settle A.",
                    point.sha256
                );
                self.choice = Some(Choice::Forget {
                    point,
                    root: self.owner.root_sha256()?,
                });
            }
            Action::Forget => {
                let Some(Choice::Forget { point, root }) = self.choice.take() else {
                    return Err(BundleError::HistoryChanged);
                };
                let report = self
                    .owner
                    .retire_rescue_recovery_point(&point, &root, true)?;
                #[cfg(test)]
                {
                    self.last_retirement = Some(report);
                }
                #[cfg(not(test))]
                {
                    let _ = report;
                }
                self.message = "The older recovery point from A was explicitly forgotten. Independent references remain.".into();
            }
            Action::Reconcile(path) => {
                let pending = self.owner.pending_agent_command_at(&path);
                let state = self.owner.reconcile_retained_destination(&path)?;
                self.message = format!(
                    "{}: {state:?}. Current edits and their Saved baseline are unchanged.",
                    path.display()
                );
                if pending && state == PublicationState::Committed {
                    self.message.push_str(" A contains the agent command's effect as of its proven save; your current edits are unchanged.");
                    self.known_agent_effects.insert(path.clone());
                }
                if state == PublicationState::Committed && self.owner.is_capture_destination(&path)
                {
                    self.message.push_str(" This destination holds a copy as of capture. Editing continues in the current session; you can open it separately read-only. A separate editable open requires explicit release of its completed destination owner.");
                }
            }
            Action::PreviewRelease(path) => {
                self.message = format!(
                    "Release {} for a separate editable open, only after verified commit. This will not switch editing, release A's source data or refund rescue slots.",
                    path.display()
                );
                self.choice = Some(Choice::Release(path));
            }
            Action::Release => {
                let Some(Choice::Release(path)) = self.choice.take() else {
                    return Err(BundleError::HistoryChanged);
                };
                self.owner.release_completed_capture(&path)?;
                self.message = "Completed capture owner explicitly released for a separate editable open. Current editing and original source retention are unchanged.".into();
            }
            Action::SaveSession(ref branch) | Action::CloseSession(ref branch) => {
                let close = matches!(&action, Action::CloseSession(_));
                let (_, mut guard) = self
                    .owner
                    .durable_sessions()
                    .into_iter()
                    .find(|(_, guard)| guard.branch_id() == branch)
                    .ok_or(BundleError::HistoryChanged)?;
                self.owner.persist_durable_branch(&mut guard)?;
                if close {
                    self.owner.close_durable_branch(&guard)?;
                }
                self.message = "Session saved. Retirement eligibility has been rechecked; a live session or required base remains protected.".into();
            }
        }
        Ok(())
    }
    fn rescue_with_inspection(
        &mut self,
        inspection: &kinewright_project::namespace::DestinationInspection,
    ) -> Result<(), BundleError> {
        let path = inspection.path();
        let Some(Choice::Rescue {
            selection, consent, ..
        }) = self.choice.take()
        else {
            return Err(BundleError::HistoryChanged);
        };
        let loss = rescue_preview(&selection);
        let mut view = None;
        let outcome =
            self.owner
                .rescue_with_activation(*selection, &consent, inspection, |core| {
                    view = Some(PreparedView::prepare(core)?);
                    Ok(())
                })?;
        self.message = if outcome.activated {
            self.view = view;
            format!(
                "Rescue copy saved to {}. Editing continues in B. A may also contain a copy from this session; its save outcome is unknown. {} {loss}",
                path.display(),
                outcome.disclosure
            )
        } else if self.owner.has_prepared_selection() {
            "B's save outcome is unknown. Editing remains read-only while A's save or selection is unresolved.".into()
        } else {
            "B's save outcome is unknown. Editing continues in the original session.".into()
        };
        #[cfg(test)]
        {
            self.last_rescue = Some(outcome);
        }
        Ok(())
    }
    fn unknown_text(&self) -> String {
        if self.owner.preliminary_save_pending() {
            "Your current save outcome is unknown. Editing is read-only while it is checked; the recovery point has not been opened.".into()
        } else {
            "Your current work is durable in the project at its current location. Editing is read-only while the recovery selection is checked. If this storage never comes back, Save rescue copy is the way to keep your work. Closing can discard the only accessible copy in this session.".into()
        }
    }
    #[allow(clippy::too_many_lines)]
    pub fn show(&self, ui: &mut egui::Ui) -> Option<Action> {
        let mut action = None;
        ui.label(&self.message);
        if self.owner.has_prepared_selection() {
            ui.label(self.owner.selection_availability().unwrap_or_default());
        }
        match &self.choice {
            Some(Choice::SuccessorSave(_)) => {
                if ui
                    .button("Save this current edit without that agent effect")
                    .clicked()
                {
                    action = Some(Action::ConfirmSuccessorSave);
                }
            }
            Some(Choice::Recovery) => {
                if ui
                    .button("Save current work, then open recovery point")
                    .clicked()
                {
                    action = Some(Action::OpenRecovery);
                }
            }
            Some(Choice::Rescue {
                selection,
                trim,
                omission,
                ..
            }) => {
                if selection.removed_undo() > 0
                    && !trim
                    && ui.button("Remove this older Undo history in B").clicked()
                {
                    action = Some(Action::ConsentTrim);
                }
                if !selection.omissions().is_empty()
                    && (selection.removed_undo() == 0 || *trim)
                    && !omission
                    && ui
                        .button("Omit these unavailable recovery points")
                        .clicked()
                {
                    action = Some(Action::ConsentOmission);
                }
                if (selection.removed_undo() == 0 || *trim)
                    && (selection.omissions().is_empty() || *omission)
                    && ui.button("Save rescue copy to…").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("Kinewright project", &["kinewright"])
                        .save_file()
                {
                    action = Some(Action::RescueTo(path));
                }
            }
            Some(Choice::Remove { .. }) => {
                if ui.button("Confirm working branch loss").clicked() {
                    action = Some(Action::RemoveBranch);
                }
            }
            Some(Choice::Forget { .. }) => {
                if ui
                    .button("Forget the older recovery point from A")
                    .clicked()
                {
                    action = Some(Action::Forget);
                }
            }
            Some(Choice::Release(_)) => {
                if ui.button("Release completed destination owner").clicked() {
                    action = Some(Action::Release);
                }
            }
            None => {
                if ui.button("Retry A / selection").clicked() {
                    action = Some(Action::Retry);
                }
                if self
                    .offer
                    .as_ref()
                    .is_some_and(|p| p.eligibility == RescueEligibility::Eligible)
                    && ui.button("Save rescue copy").clicked()
                {
                    action = Some(Action::PreviewRescue);
                }
                if !self.frozen {
                    for (index, point) in self.points.iter().enumerate() {
                        if ui
                            .button(format!("Open rebound recovery point {}", index + 1))
                            .clicked()
                        {
                            action = Some(Action::PreviewRecovery(point.clone()));
                        }
                    }
                    for (branch, blockers) in &self.branches {
                        ui.label(format!(
                            "Working branch {}: {}",
                            branch.as_str(),
                            blockers.join(" ")
                        ));
                        if blockers.is_empty()
                            && ui
                                .button(format!("Save current edit, then open {}", branch.as_str()))
                                .clicked()
                        {
                            action = Some(Action::PreviewRecovery(
                                RecoverySelection::WorkingBranch(branch.clone()),
                            ));
                        }
                        if self.candidates.contains(branch)
                            && ui
                                .button(format!("Remove this working branch {}", branch.as_str()))
                                .clicked()
                        {
                            action = Some(Action::PreviewRetirement(branch.clone()));
                        }
                        for (name, guard) in self.owner.durable_sessions() {
                            if guard.branch_id() == branch || &guard.base().target_branch == branch
                            {
                                ui.label(format!("This branch is in use by {name} / specialist base. Save or close that session first."));
                                if ui.button(format!("Save this session: {name}")).clicked() {
                                    action = Some(Action::SaveSession(guard.branch_id().clone()));
                                }
                                if ui.button(format!("Close this session: {name}")).clicked() {
                                    action = Some(Action::CloseSession(guard.branch_id().clone()));
                                }
                            }
                        }
                    }
                    if self
                        .owner
                        .acknowledged_recovery_point()
                        .ok()
                        .flatten()
                        .is_some()
                        && ui
                            .button("Forget the older recovery point from A…")
                            .clicked()
                    {
                        action = Some(Action::PreviewForget);
                    }
                }
                for (path, state) in &self.destinations {
                    if ui
                        .button(format!("Check {} ({state:?})", path.display()))
                        .clicked()
                    {
                        action = Some(Action::Reconcile(path.clone()));
                    }
                    if *state == PublicationState::Committed
                        && self.owner.is_capture_destination(path)
                        && ui
                            .button(format!("Release completed copy {}…", path.display()))
                            .clicked()
                    {
                        action = Some(Action::PreviewRelease(path.clone()));
                    }
                }
            }
        }
        if self.choice.is_some() && ui.button("Cancel").clicked() {
            action = Some(Action::Cancel);
        }
        if self.owner.has_prepared_selection() && !self.owner.has_unresolved_publication() {
            ui.label("Abandoning the selection lets a session-only point go. A crash can lose it.");
            if ui.button("Abandon this selection explicitly").clicked() {
                action = Some(Action::Abandon);
            }
        }
        action
    }
}
fn copy_text(status: &CopyStatus) -> String {
    match status {
        CopyStatus::Committed(report) => format!("Saved {}.", report.path.display()),
        CopyStatus::CleanupPending { diagnostic } => format!("Save failed; cleanup pending. {diagnostic}"),
        CopyStatus::DurabilityUnknown => "This destination's save outcome is unknown. Your edits are still in this session. Retry A to check it.".into(),
        CopyStatus::DefinitelyUnpublished => "Save did not complete. Your current edit is unchanged.".into(),
        CopyStatus::CommittedWithError { diagnostic } => format!("The root was saved, but its attachment still needs reconciliation. {diagnostic}"),
    }
}
fn rescue_preview(selection: &RescueSelection) -> String {
    let mut text = if selection.removed_undo() > 0 {
        format!(
            "Save a rescue copy with older Undo history removed in B. Undo before step {} will be unavailable there; {} oldest Undo entries are removed. Current edits, Redo, and the open gesture are kept. A is unchanged. ",
            selection.undo_floor(),
            selection.removed_undo()
        )
    } else {
        "Save a rescue copy with all Undo and Redo kept. ".into()
    };
    if !selection.omissions().is_empty() {
        text.push_str("Older recovery points from A are not in the rescue copy. ");
        for omission in selection.omissions() {
            let _ = write!(
                text,
                "{} {}: {} ({}) ",
                omission.role, omission.root.sha256, omission.cause, omission.operation
            );
        }
        if selection
            .omissions()
            .iter()
            .any(|o| o.role == "acknowledged_root")
        {
            text.push_str(
                "The recovery point for A's last acknowledged save is not in the rescue copy. ",
            );
        }
    }
    text
}

#[cfg(test)]
#[path = "ja1_attachment_tests.rs"]
pub(crate) mod tests;
