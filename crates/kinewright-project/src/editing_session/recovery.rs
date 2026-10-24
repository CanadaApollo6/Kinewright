//! Local Save-then-select and explicit, owner-serialized retirement.
use super::{
    ActiveAssociation, BundleError, BundleSaveReport, BundleWriteAdmission, CopyStatus,
    Ja1EditingSession, ObjectRef, Path, ProjectBundle, PublicationState, RetainedDestination,
    StorageLimits,
};
use crate::{
    bundle_commit::BundleCommit,
    bundle_records::{BranchId, HeadId},
    bundle_recovery::RecoverySelection,
    bundle_switch::{PreparedProjectSelection, ProjectSelectionResult},
};
use kinewright_core::AttachmentState;

impl Ja1EditingSession {
    pub(super) fn active_bundle(&self) -> Result<&ProjectBundle, BundleError> {
        self.destination
            .as_ref()
            .ok_or(BundleError::Unsettled)?
            .bundle()
    }
    pub(super) fn active_bundle_mut(&mut self) -> Result<&mut ProjectBundle, BundleError> {
        self.destination
            .as_mut()
            .ok_or(BundleError::Unsettled)?
            .bundle_mut()
    }
    pub fn prepare_recovery(&mut self, target: RecoverySelection) -> Result<(), BundleError> {
        if let Some(prepared) = &self.prepared {
            return if prepared.target() == &target {
                Ok(())
            } else {
                Err(BundleError::HistoryChanged)
            };
        }
        if let Some((previous, report)) = &self.selection_result
            && previous == &target
            && !self.dirty()?
            && self.root_sha256()? == report.root_sha256
        {
            return Ok(());
        }
        self.admit_selection_target(&target)?;
        self.prepared = Some(
            self.active_bundle()?
                .prepare_project_selection(&self.core, target)?,
        );
        self.selection_result = None;
        Ok(())
    }
    /// Duplicate/lost replies keep the same preparation, R identity and target pin.
    pub fn select_local(&mut self) -> ProjectSelectionResult {
        self.select_local_with_activation(|_| Ok(()))
    }
    /// Prepare a caller's fallible subscriptions/resources before publication or sealing.
    pub fn select_local_with_activation(
        &mut self,
        mut prepare: impl FnMut(&kinewright_core::Core) -> Result<(), BundleError>,
    ) -> ProjectSelectionResult {
        if let Some((_, report)) = &self.selection_result {
            if let Err(error) = prepare(&self.core) {
                return ProjectSelectionResult::Refused(error);
            }
            return ProjectSelectionResult::Activated(report.clone());
        }
        let Some(mut prepared) = self.prepared.take() else {
            return ProjectSelectionResult::Refused(BundleError::Identity(
                "prepare a recovery selection first".into(),
            ));
        };
        if let Err(error) = self.admit_selection_target(prepared.target()) {
            self.prepared = Some(prepared);
            return ProjectSelectionResult::Refused(error);
        }
        if let Err(error) = prepare(&prepared.restore.core) {
            self.prepared = Some(prepared);
            return ProjectSelectionResult::Refused(error);
        }
        let result = self.select_local_prepared(&mut prepared);
        match &result {
            ProjectSelectionResult::Activated(report) => {
                let target = prepared.target().clone();
                self.core = prepared.restore.core;
                self.inputs = prepared.restore.checkpoint.input_manifest_ref;
                self.attachment = prepared.restore.attachment;
                self.input_leases = prepared.restore.input_leases;
                self.association = Some(ActiveAssociation {
                    path: report.path.clone(),
                    saved: Some(prepared.restore.history_digest),
                });
                self.selection_result = Some((target, report.clone()));
            }
            _ => self.prepared = Some(prepared),
        }
        result
    }
    fn select_local_prepared(
        &mut self,
        prepared: &mut PreparedProjectSelection,
    ) -> ProjectSelectionResult {
        if prepared.publication_started() {
            let core = self.core.clone();
            return match self.active_bundle_mut() {
                Ok(bundle) => bundle.publish_project_selection(prepared, &core),
                Err(error) => ProjectSelectionResult::Refused(error),
            };
        }
        let result = (|| {
            if self.recovery_save_pending {
                self.reconcile_active()?;
                self.recovery_save_pending = false;
                if self.active_bundle()?.publication_state() != PublicationState::Committed {
                    let cut = self
                        .attachment
                        .capture()
                        .map_err(crate::bundle::history_error)?;
                    self.attachment
                        .resume(&cut)
                        .map_err(crate::bundle::history_error)?;
                    return Err(BundleError::HistoryChanged);
                }
                let saved = self
                    .active_bundle()?
                    .checkpoint(self.active_bundle()?.manual_branch_id())?
                    .history_ref
                    .sha256
                    .clone();
                self.association
                    .as_mut()
                    .ok_or(BundleError::Unsettled)?
                    .saved = Some(saved);
            }
            let cut = self
                .attachment
                .freeze()
                .map_err(crate::bundle::history_error)?;
            let save = if self.dirty()? {
                let core = self.core.clone();
                let inputs = self.inputs.clone();
                self.active_bundle_mut()?
                    .save_frozen(&core, &inputs, &cut)
                    .map(|_| ())
            } else {
                Ok(())
            };
            if let Err(error) = save {
                if self.active_bundle()?.publication_state() == PublicationState::Unknown
                    || self.active_bundle()?.unsettled
                {
                    self.recovery_save_pending = true;
                } else {
                    let current = self
                        .attachment
                        .capture()
                        .map_err(crate::bundle::history_error)?;
                    self.attachment
                        .resume(&current)
                        .map_err(crate::bundle::history_error)?;
                }
                return Err(error);
            }
            let saved = self
                .active_bundle()?
                .checkpoint(self.active_bundle()?.manual_branch_id())?
                .history_ref
                .sha256
                .clone();
            self.association
                .as_mut()
                .ok_or(BundleError::Unsettled)?
                .saved = Some(saved);
            // This validates the exact ordinary floor result against durable L; a bypassed suffix refuses.
            if let Err(error) = self
                .active_bundle()?
                .refresh_selection_source(prepared, &self.core)
            {
                let current = self
                    .attachment
                    .capture()
                    .map_err(crate::bundle::history_error)?;
                self.attachment
                    .resume(&current)
                    .map_err(crate::bundle::history_error)?;
                return Err(error);
            }
            Ok(())
        })();
        if let Err(error) = result {
            return if self.recovery_save_pending {
                ProjectSelectionResult::Unknown
            } else {
                ProjectSelectionResult::Refused(error)
            };
        }
        let core = self.core.clone();
        match self.active_bundle_mut() {
            Ok(bundle) => bundle.publish_project_selection(prepared, &core),
            Err(error) => ProjectSelectionResult::Refused(error),
        }
    }
    pub fn recovery_save_disclosure(&self) -> Result<String, BundleError> {
        let history = self
            .core
            .export_history()
            .map_err(crate::bundle::history_error)?;
        let selected = crate::bundle_commit::select_history(&history, self.store()?.limits())?;
        let lost = history
            .cursor
            .undo
            .len()
            .saturating_sub(selected.history.cursor.undo.len());
        Ok(format!(
            "Saving your current work removes {lost} oldest Undo entries. Current content, complete Redo and the open gesture are preserved."
        ))
    }
    pub fn selection_availability(&self) -> Result<String, BundleError> {
        let Some(prepared) = &self.prepared else {
            return Ok(String::new());
        };
        let bundle = self.active_bundle()?;
        let mut message = match prepared.target() {
            RecoverySelection::Checkpoint { root, .. } => {
                let reachable = bundle
                    .store
                    .closure(&bundle.root_references()?)?
                    .contains(root);
                if !reachable {
                    "This recovery point is no longer in the project's automatic list. It is kept only for this selection in this session. Abandoning the selection lets it go.".to_owned()
                } else if self.dirty()?
                    && bundle.recovery.predecessors.last() == Some(root)
                    && bundle.recovery.predecessors.len() == 2
                {
                    "Saving your current work will remove this recovery point from the project's automatic list. It will be kept only for this selection in this session until opened. Abandoning the selection lets it go; a crash can lose it.".to_owned()
                } else {
                    "This recovery point is still durably listed in the project.".to_owned()
                }
            }
            RecoverySelection::WorkingBranch(_) => {
                "This working branch is protected while this selection is prepared.".to_owned()
            }
        };
        if prepared.publication_started() && bundle.unsettled {
            message.push_str(" A keeps it only if the selection completed.");
        }
        Ok(message)
    }
    pub fn close_disclosure(&self) -> Result<String, BundleError> {
        let mut disclosure = self.selection_availability()?;
        if self.active_bundle()?.unsettled || self.dirty()? {
            disclosure.push_str(" Your save or selection may be unresolved. If this storage never comes back, Save rescue copy is the way to keep your work. Closing can discard the only accessible copy in this session.");
        }
        if self.prepared.is_some() {
            disclosure
                .push_str(" Closing abandons the selection and lets a session-only point go.");
        }
        for registered in &self.durable_sessions {
            let bundle = self.registered_bundle(&registered.guard)?;
            let history = registered
                .guard
                .core()
                .export_history()
                .map_err(crate::bundle::history_error)?;
            let digest = kinewright_media::sha256_bytes(
                &serde_json::to_vec(&history).map_err(|e| BundleError::Corrupt(e.to_string()))?,
            );
            if bundle.unsettled
                || bundle
                    .checkpoint(&registered.guard.branch)?
                    .history_ref
                    .sha256
                    != digest
            {
                disclosure.push(' ');
                disclosure.push_str(&registered.name);
                disclosure.push_str(" has unsaved work or an unresolved save in its original project. Save or close that session first; closing this context can discard its accepted work.");
            }
        }
        Ok(disclosure)
    }
    pub fn rescue_handoff_disclosure(
        &self,
        selection: &crate::bundle_rescue::RescueSelection,
    ) -> Result<String, BundleError> {
        let mut text = self.selection_availability()?;
        if let Some(prepared) = &self.prepared {
            text.push_str(" Rescue handoff abandons the recovery selection at A irrevocably.");
            if let RecoverySelection::Checkpoint { root, .. } = prepared.target()
                && !selection.reads.objects().contains(root)
            {
                text.push_str(" The selected recovery point is not in B.");
            }
        }
        Ok(text)
    }
    pub fn open_project(&self, path: &Path) -> Result<Self, BundleError> {
        self.check_destination_change()?;
        Self::open(path)
    }
    pub fn save_as(
        &mut self,
        path: &Path,
        limits: StorageLimits,
    ) -> Result<CopyStatus, BundleError> {
        self.check_destination_change()?;
        if self.dirty()? {
            self.save_active()?;
        }
        let outcome = self
            .active_bundle_mut()?
            .save_as_with_limits(path, limits)?;
        match outcome {
            crate::bundle_copy::SaveAsOutcome::SameProject(report) => {
                Ok(CopyStatus::Committed(report))
            }
            crate::bundle_copy::SaveAsOutcome::Copied(copy) => {
                let status = copy.status().clone();
                if let CopyStatus::Committed(report) = &status {
                    let inputs = copy.rebound_manifest(&self.inputs)?;
                    let path = copy.destination_owner()?.store().resolved_root()?;
                    let previous = self
                        .destination
                        .replace(RetainedDestination::FirstSave(copy));
                    if let Some(previous) = previous {
                        self.retained_destinations
                            .push((previous, self.core.clone()));
                    }
                    self.inputs = inputs;
                    self.association = Some(ActiveAssociation {
                        path,
                        saved: Some(report.history_sha256.clone()),
                    });
                } else {
                    self.retained_destinations
                        .push((RetainedDestination::FirstSave(copy), self.core.clone()));
                }
                Ok(status)
            }
        }
    }
    pub fn check_destination_change(&self) -> Result<(), BundleError> {
        if self.prepared.is_some() {
            return Err(BundleError::Identity(format!(
                "{} Abandon this selection explicitly before Save As or opening another project.",
                self.selection_availability()?
            )));
        }
        Ok(())
    }
    pub fn abandon_recovery(&mut self) -> Result<String, BundleError> {
        let disclosure = self.selection_availability()?;
        if self.recovery_save_pending
            || self.active_bundle()?.unsettled
            || self
                .core
                .attachment_state()
                .map_err(crate::bundle::history_error)?
                == AttachmentState::Frozen
        {
            return Err(BundleError::Unsettled);
        }
        self.prepared = None;
        self.selection_result = None;
        Ok(disclosure)
    }
    pub fn list_recovery_points(&self) -> Result<Vec<RecoverySelection>, BundleError> {
        let bundle = self.active_bundle()?;
        let roots = bundle
            .rescue_records()?
            .into_iter()
            .filter_map(|r| r.acknowledged_root)
            .chain(bundle.recovery.predecessors.iter().cloned());
        roots
            .map(|reference| {
                let bytes = bundle.store.read_checkpoint_bytes(&reference)?;
                let admission = BundleWriteAdmission::from_bytes(None, bytes)?;
                admission.require_supported()?;
                let branch = admission
                    .data
                    .file
                    .as_ref()
                    .and_then(|f| f.studio.as_ref())
                    .ok_or(BundleError::Unsettled)?
                    .manual_branch_id
                    .clone();
                Ok(RecoverySelection::Checkpoint {
                    root: reference,
                    branch,
                })
            })
            .collect()
    }
    pub fn list_working_branches(
        &self,
    ) -> Result<Vec<(BranchId, HeadId, Vec<String>)>, BundleError> {
        let bundle = self.active_bundle()?;
        Ok(bundle
            .branches
            .iter()
            .map(|(id, checkpoint)| {
                (
                    id.clone(),
                    checkpoint.head_id.clone(),
                    self.branch_blockers(id),
                )
            })
            .collect())
    }
    pub fn retirement_disclosure(&self, branch: &BranchId) -> Result<String, BundleError> {
        let bundle = self.active_bundle()?;
        let checkpoint = bundle.checkpoint(branch)?;
        let history = bundle.branch_history(branch)?;
        let bytes =
            serde_json::to_vec(&*history).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let shared = bundle
            .branches
            .iter()
            .any(|(id, c)| id != branch && c.history_ref == checkpoint.history_ref);
        let relief = if shared {
            0
        } else {
            crate::store::preflight_record(&bytes, bundle.store.limits())?
        };
        let retained = bundle
            .store
            .closure(&[bundle.checkpoint_reference(branch)?])?
            .iter()
            .map(|r| r.byte_length)
            .sum::<u64>();
        Ok(format!(
            "Remove working branch {} at head {}: its content {} and {} Undo / {} Redo entries leave the current branch index. Current-history residency relief: {relief} bytes. Its closure contains {retained} bytes; immediate reclaimable bytes: 0, because this publication retains the previous root. Automatic recovery, independent evidence and readers may keep these bytes after later saves. {}",
            branch.as_str(),
            checkpoint.head_id.as_str(),
            checkpoint.document_hash,
            history.cursor.undo.len(),
            history.cursor.redo.len(),
            self.branch_blockers(branch).join(" ")
        ))
    }
    #[cfg(any(test, feature = "test-util"))]
    pub fn restrict_limits_probe(&mut self, limits: StorageLimits) -> Result<(), BundleError> {
        self.active_bundle_mut()?.restrict_limits_probe(limits)
    }
    #[cfg(any(test, feature = "test-util"))]
    pub fn change_prepared_target_probe(&mut self, change_head: bool) -> Result<(), BundleError> {
        let branch = self
            .prepared
            .as_ref()
            .ok_or(BundleError::Unsettled)?
            .target_branch()
            .clone();
        let bundle = self.active_bundle_mut()?;
        if change_head {
            let mut checkpoint = bundle.checkpoint(&branch)?.clone();
            checkpoint.head_id = HeadId::fresh()?;
            let sealed = bundle.store.admit_record(&checkpoint)?;
            let root = bundle.root.as_mut().ok_or(BundleError::Unsettled)?;
            let mut index = (*bundle
                .store
                .read_record::<std::collections::BTreeMap<BranchId, ObjectRef>>(
                    &root.branch_index_ref,
                )?)
            .clone();
            index.insert(branch.clone(), sealed.reference.clone());
            let index = bundle.store.admit_record(&index)?;
            root.branch_index_ref = index.reference.clone();
            bundle.branches.insert(branch, checkpoint);
        } else {
            bundle.branches.remove(&branch);
        }
        BundleCommit::publish_maintenance(bundle).map(|_| ())
    }
    pub fn retirement_candidates(&self) -> Result<Vec<BranchId>, BundleError> {
        if self
            .core
            .attachment_state()
            .map_err(crate::bundle::history_error)?
            != AttachmentState::Live
            || self.active_bundle()?.unsettled
        {
            return Ok(vec![]);
        }
        Ok(self
            .list_working_branches()?
            .into_iter()
            .filter(|(_, _, blockers)| blockers.is_empty())
            .map(|(id, _, _)| id)
            .collect())
    }
    fn admit_selection_target(&self, target: &RecoverySelection) -> Result<(), BundleError> {
        if let RecoverySelection::WorkingBranch(branch) = target {
            let blockers = self.session_branch_blockers(branch);
            if !blockers.is_empty() {
                return Err(BundleError::Contention(blockers.join(" ")));
            }
        }
        Ok(())
    }
    fn session_branch_blockers(&self, id: &BranchId) -> Vec<String> {
        self.durable_sessions.iter().filter(|session| {
            self.active_bundle().is_ok_and(|bundle| bundle.store.owner_epoch() == session.guard.epoch)
                && (&session.guard.branch == id || &session.guard.base.target_branch == id)
        }).map(|session| format!("This branch is in use by {} / specialist base. Save or close that session first.", session.name)).collect()
    }
    fn branch_blockers(&self, id: &BranchId) -> Vec<String> {
        let mut blockers = vec![];
        if self
            .active_bundle()
            .is_ok_and(|b| b.manual_branch_id() == id)
        {
            blockers.push("active working branch".into());
        }
        if self
            .prepared
            .as_ref()
            .is_some_and(|p| p.protects_branch(id))
        {
            blockers.push("prepared recovery selection target".into());
        }
        blockers.extend(self.session_branch_blockers(id));
        blockers
    }
    pub fn root_sha256(&self) -> Result<String, BundleError> {
        Ok(kinewright_media::sha256_bytes(
            self.active_bundle()?.admission.raw_bytes(),
        ))
    }
    fn admit_retirement(&self, expected_root: &str, confirmed: bool) -> Result<(), BundleError> {
        if !confirmed {
            return Err(BundleError::Identity(
                "confirm exact content/history loss before removal".into(),
            ));
        }
        if self
            .core
            .attachment_state()
            .map_err(crate::bundle::history_error)?
            != AttachmentState::Live
            || self.active_bundle()?.unsettled
            || self.active_bundle()?.pending_command.is_some()
        {
            return Err(BundleError::Unsettled);
        }
        if self.root_sha256()? != expected_root {
            return Err(BundleError::HistoryChanged);
        }
        Ok(())
    }
    pub fn retire_working_branch(
        &mut self,
        branch: &BranchId,
        expected_root: &str,
        confirmed: bool,
    ) -> Result<BundleSaveReport, BundleError> {
        self.admit_retirement(expected_root, confirmed)?;
        let blockers = self.branch_blockers(branch);
        if !blockers.is_empty() {
            return Err(BundleError::Identity(blockers.join(" ")));
        }
        let bundle = self.active_bundle_mut()?;
        let removed = bundle
            .branches
            .remove(branch)
            .ok_or(BundleError::HistoryChanged)?;
        let result = BundleCommit::publish_maintenance(bundle);
        if result.is_err() && !bundle.unsettled {
            bundle.branches.insert(branch.clone(), removed);
        } else if result.is_ok() {
            bundle.history_cache.remove(branch);
        }
        result
    }
    pub fn retire_rescue_recovery_point(
        &mut self,
        point: &ObjectRef,
        expected_root: &str,
        confirmed: bool,
    ) -> Result<BundleSaveReport, BundleError> {
        self.admit_retirement(expected_root, confirmed)?;
        let bundle = self.active_bundle_mut()?;
        let mut replacements = vec![];
        for reference in &bundle.evidence {
            if reference.record_kind == crate::bundle_records::RecordKind::OwnedEvidence
                && let Some(mut record) =
                    crate::rescue_record::parse(&bundle.store.read_evidence(reference)?)?
                && record.acknowledged_root.as_ref() == Some(point)
            {
                record.acknowledged_root = None;
                record.acknowledged_absence =
                    Some("Forget the older recovery point from A: explicitly retired".into());
                replacements.push((reference.clone(), record));
            }
        }
        if replacements.is_empty() {
            return Err(BundleError::HistoryChanged);
        }
        let original = bundle.evidence.clone();
        let pending_before = bundle.pending_evidence.len();
        let staged = replacements
            .into_iter()
            .map(|(old, record)| {
                bundle
                    .store_mut()?
                    .admit_record(&record)
                    .map(|sealed| (old, sealed))
            })
            .collect::<Result<Vec<_>, BundleError>>()?;
        for (old, sealed) in staged {
            bundle.evidence.retain(|r| r != &old);
            bundle.evidence.push(sealed.reference.clone());
            bundle.pending_evidence.push(sealed.into_pin());
        }
        let result = BundleCommit::publish_maintenance(bundle);
        if result.is_err() && !bundle.unsettled {
            bundle.evidence = original;
            bundle.pending_evidence.truncate(pending_before);
        }
        result
    }
}
