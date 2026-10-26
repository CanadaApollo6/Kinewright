//! Receipt protocol around the same pinned local Save/select operation.
use super::{BundleError, BundleSaveReport, Ja1EditingSession};
use crate::{
    bundle_switch::{
        ProjectSelectionResult, SwitchAttachment, SwitchBranch, SwitchRequest, SwitchTarget,
    },
    manual_history::ManualLease,
    receipts::{CommandEnvelope, CommandId, ReceiptLookup},
};

pub struct PreparedGuiSwitch {
    pub(super) core: kinewright_core::Core,
    pub(super) attachment: crate::ProjectAttachment,
    pub(super) cut: kinewright_core::AcceptedCut,
    payload: crate::bundle_switch::SwitchPayload,
}
impl PreparedGuiSwitch {
    #[must_use]
    pub const fn core(&self) -> &kinewright_core::Core {
        &self.core
    }
    #[must_use]
    pub fn matches(&self, committed: &SwitchAttachment) -> bool {
        self.payload.history == committed.history
            && self.payload.inputs == committed.inputs
            && self.payload.lineage == committed.lineage
    }
}

impl Ja1EditingSession {
    #[cfg(any(test, feature = "test-util"))]
    pub fn change_switch_receipt_probe(&mut self) -> Result<(), BundleError> {
        for receipt in &mut self.active_bundle_mut()?.receipts.detailed {
            if let crate::receipts::ReceiptResult::Switch { result } = &mut receipt.result {
                result.new.history = result.old.history.clone();
                return Ok(());
            }
        }
        Err(BundleError::HistoryChanged)
    }
    pub fn switch_preview(&self) -> Result<crate::bundle_switch::SwitchPayload, BundleError> {
        let prepared = self.prepared.as_ref().ok_or(BundleError::HistoryChanged)?;
        Ok(crate::bundle_switch::SwitchPayload {
            history: (*prepared.restore.history).clone(),
            inputs: (*self
                .active_bundle()?
                .store
                .read_input_manifest(&prepared.restore.checkpoint.input_manifest_ref)?)
            .clone(),
            lineage: (*prepared.restore.lineage).clone(),
        })
    }
    pub fn prepare_gui_switch(
        &self,
        payload: crate::bundle_switch::SwitchPayload,
    ) -> Result<PreparedGuiSwitch, BundleError> {
        if !self.host_attached || self.host_retention_lost || self.host_cut.is_none() {
            return Err(BundleError::HistoryChanged);
        }
        let core = kinewright_core::Core::spawn(kinewright_core::Document::default())
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let attachment =
            crate::ProjectAttachment::new(&core).map_err(crate::bundle::history_error)?;
        attachment
            .restore_history(payload.history.clone())
            .map_err(crate::bundle::history_error)?;
        let cut = attachment.freeze().map_err(crate::bundle::history_error)?;
        Ok(PreparedGuiSwitch {
            core,
            attachment,
            cut,
            payload,
        })
    }
    pub fn activate_prepared_gui_switch(
        &mut self,
        cut: &kinewright_core::AcceptedCut,
        committed: &SwitchAttachment,
        prepared: PreparedGuiSwitch,
    ) -> Result<kinewright_core::Core, BundleError> {
        committed.verify()?;
        if !prepared.matches(committed) {
            return Err(BundleError::HistoryChanged);
        }
        let core = self.activate_host_cut_prepared(
            cut,
            committed.history.clone(),
            &committed.current.history.sha256,
            super::HostActivationKind::Switch,
            |_| Ok(()),
            Some(prepared),
        )?;
        self.inputs = committed.current.inputs.clone();
        Ok(core)
    }
    pub fn activate_host_switch(
        &mut self,
        cut: &kinewright_core::AcceptedCut,
        committed: &SwitchAttachment,
        prepare: impl FnOnce(&kinewright_core::Core) -> Result<(), BundleError>,
    ) -> Result<kinewright_core::Core, BundleError> {
        committed.verify()?;
        let core = self.activate_host_cut(
            cut,
            committed.history.clone(),
            &committed.current.history.sha256,
            super::HostActivationKind::Switch,
            prepare,
        )?;
        self.inputs = committed.current.inputs.clone();
        Ok(core)
    }
    pub fn finish_host_switch(
        &mut self,
        lease: ManualLease,
        inputs: crate::owned_inputs::OwnedInputManifest,
        live: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<kinewright_core::ManualPermit, BundleError> {
        let cut = self.host_cut.as_ref().ok_or(BundleError::HistoryChanged)?;
        if crate::manual_history::history_sha256(cut.history())? != lease.history_sha256
            || lease.inputs != self.inputs
        {
            return Err(BundleError::HistoryChanged);
        }
        let permit = self
            .attachment
            .resume_manual(cut, lease.into_encoder(inputs)?, live)
            .map_err(crate::bundle::history_error)?;
        self.host_cut = None;
        Ok(permit)
    }
    pub fn current_switch_identity(
        &self,
    ) -> Result<crate::bundle_switch::SwitchAttachmentIdentity, BundleError> {
        let bundle = self.active_bundle()?;
        if bundle.unsettled {
            return Err(BundleError::Unsettled);
        }
        let root = bundle.root().ok_or(BundleError::Unsettled)?;
        Ok(crate::bundle_switch::SwitchAttachmentIdentity {
            project: root.project_id.clone(),
            root_sha256: self.root_sha256()?,
            root_generation: root.generation,
            switch_generation: root.manual_switch_generation,
            current: SwitchBranch::from_checkpoint(
                bundle.checkpoint_reference(bundle.manual_branch_id())?,
                bundle.checkpoint(bundle.manual_branch_id())?,
            ),
        })
    }
    pub fn current_switch_attachment(&self) -> Result<SwitchAttachment, BundleError> {
        let bundle = self.active_bundle()?;
        if bundle.unsettled {
            return Err(BundleError::Unsettled);
        }
        let root = bundle.root().ok_or(BundleError::Unsettled)?;
        let checkpoint = bundle.checkpoint(bundle.manual_branch_id())?;
        let history = if let Some(cached) = bundle.cached_history(bundle.manual_branch_id()) {
            bundle.store.verify(&checkpoint.history_ref)?;
            (***cached).clone()
        } else {
            (*bundle.branch_history(bundle.manual_branch_id())?).clone()
        };
        Ok(SwitchAttachment {
            project: root.project_id.clone(),
            root_sha256: self.root_sha256()?,
            root_generation: root.generation,
            switch_generation: root.manual_switch_generation,
            current: SwitchBranch::from_checkpoint(
                bundle.checkpoint_reference(bundle.manual_branch_id())?,
                checkpoint,
            ),
            history,
            inputs: (*bundle
                .store
                .read_input_manifest(&checkpoint.input_manifest_ref)?)
            .clone(),
            lineage: (*bundle.branch_lineage(bundle.manual_branch_id())?).clone(),
        })
    }
    pub fn switch_envelope(
        &self,
        lease: &ManualLease,
        target: SwitchTarget,
    ) -> Result<CommandEnvelope, BundleError> {
        let bundle = self.active_bundle()?;
        let root = bundle.root().ok_or(BundleError::Unsettled)?;
        if lease.project != root.project_id
            || lease.branch != root.manual_branch_id
            || lease.switch_generation != root.manual_switch_generation
            || lease.owner_epoch != bundle.store.owner_epoch()
        {
            return Err(BundleError::HistoryChanged);
        }
        let checkpoint = bundle.checkpoint(bundle.manual_branch_id())?;
        let state = bundle
            .issuers
            .get(&lease.issuer)
            .filter(|s| !s.retired)
            .ok_or(BundleError::HistoryChanged)?;
        let envelope = CommandEnvelope {
            switch: Some(Box::new(SwitchRequest {
                target,
                source_checkpoint: bundle.checkpoint_reference(bundle.manual_branch_id())?,
                checkpoint_generation: checkpoint.checkpoint_generation,
                floor_generation: checkpoint.floor_generation,
                switch_generation: root.manual_switch_generation,
            })),
            manual: None,
            version: 1,
            action: "switch".into(),
            id: CommandId {
                issuer: lease.issuer.clone(),
                sequence: state.next_sequence,
            },
            project_id: root.project_id.clone(),
            target_branch: root.manual_branch_id.clone(),
            owner_epoch: lease.owner_epoch,
            lease: Some(lease.token.clone()),
            expected_head: Some(checkpoint.head_id.clone()),
            expected_revision: self
                .attachment
                .capture()
                .map_err(crate::bundle::history_error)?
                .revision(),
            command: None,
            comment_text: None,
            comment_strokes: None,
            evidence: vec![],
            transition: None,
        };
        envelope.encoded_bytes()?;
        Ok(envelope)
    }
    pub fn prepare_host_switch(
        &mut self,
        envelope: &CommandEnvelope,
    ) -> Result<ReceiptLookup, BundleError> {
        let lookup = self.active_bundle()?.receipt_lookup(envelope)?;
        if lookup != ReceiptLookup::Absent {
            return Ok(lookup);
        }
        if let Some(prepared) = &self.prepared {
            return if prepared.envelope.as_ref() == Some(envelope) {
                Ok(ReceiptLookup::Absent)
            } else {
                Err(BundleError::HistoryChanged)
            };
        }
        let request = envelope
            .switch
            .as_ref()
            .ok_or(BundleError::HistoryChanged)?;
        let bundle = self.active_bundle()?;
        let root = bundle.root().ok_or(BundleError::Unsettled)?;
        let checkpoint = bundle.checkpoint(bundle.manual_branch_id())?;
        if envelope.action != "switch"
            || envelope.project_id != root.project_id
            || envelope.target_branch != root.manual_branch_id
            || envelope.owner_epoch != bundle.store.owner_epoch()
            || envelope.expected_head.as_ref() != Some(&checkpoint.head_id)
            || request.source_checkpoint
                != bundle.checkpoint_reference(bundle.manual_branch_id())?
            || request.checkpoint_generation != checkpoint.checkpoint_generation
            || request.floor_generation != checkpoint.floor_generation
            || request.switch_generation != root.manual_switch_generation
            || envelope.expected_revision
                != self
                    .attachment
                    .capture()
                    .map_err(crate::bundle::history_error)?
                    .revision()
            || matches!(&request.target, SwitchTarget::WorkingBranch { branch } if branch == bundle.manual_branch_id())
        {
            return Err(BundleError::HistoryChanged);
        }
        let state = bundle
            .issuers
            .get(&envelope.id.issuer)
            .filter(|s| !s.retired)
            .ok_or(BundleError::HistoryChanged)?;
        if envelope.id.sequence != state.next_sequence {
            return Err(BundleError::HistoryChanged);
        }
        self.selection_result = None;
        self.prepare_recovery(request.target.selection())?;
        self.prepared
            .as_mut()
            .ok_or(BundleError::HistoryChanged)?
            .envelope = Some(envelope.clone());
        Ok(ReceiptLookup::Absent)
    }
    pub fn publish_host_switch(
        &mut self,
        envelope: &CommandEnvelope,
    ) -> Result<(ReceiptLookup, Option<BundleSaveReport>), BundleError> {
        // An unknown attempt must first settle its original preparation, including its pin.
        if !self
            .prepared
            .as_ref()
            .is_some_and(|p| p.envelope.as_ref() == Some(envelope))
        {
            let lookup = self.prepare_host_switch(envelope)?;
            if lookup != ReceiptLookup::Absent {
                return Ok((lookup, None));
            }
        }
        match self.select_local() {
            ProjectSelectionResult::Activated(report) => Ok((
                self.active_bundle()?.receipt_lookup(envelope)?,
                Some(report),
            )),
            ProjectSelectionResult::Unknown => Err(BundleError::DurabilityUnknown),
            ProjectSelectionResult::Refused(e) | ProjectSelectionResult::CommittedWithError(e) => {
                Err(e)
            }
        }
    }
}
