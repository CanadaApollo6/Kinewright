use super::{BundleError, BundleSaveReport, Ja1EditingSession};
use crate::{
    manual_history::{AcceptedManualCommand, ManualLease, history_sha256},
    receipts::ReceiptLookup,
};
use kinewright_core::AcceptedHistory;
impl Ja1EditingSession {
    pub fn active_manual_project(&self) -> Result<crate::bundle_records::ProjectId, BundleError> {
        Ok(self.active_bundle()?.project_id().clone())
    }

    pub fn acknowledge_manual(
        &self,
        permit: &kinewright_core::ManualPermit,
        prefix: Vec<AcceptedHistory>,
    ) -> Result<(), BundleError> {
        self.attachment
            .acknowledge_manual(permit, prefix)
            .map_err(crate::bundle::history_error)
    }
    pub fn bind_manual_producer(
        &self,
        lease: ManualLease,
        inputs: crate::owned_inputs::OwnedInputManifest,
    ) -> Result<kinewright_core::ManualPermit, BundleError> {
        if history_sha256(
            &self
                .core
                .export_history()
                .map_err(crate::bundle::history_error)?,
        )? != lease.history_sha256
        {
            return Err(BundleError::HistoryChanged);
        }
        self.attachment
            .manual_permit(lease.into_encoder(inputs)?)
            .map_err(crate::bundle::history_error)
    }
    pub fn manual_input_context(
        &self,
    ) -> Result<crate::owned_inputs::OwnedInputManifest, BundleError> {
        let bundle = self.active_bundle()?;
        let inputs = &bundle
            .checkpoint(bundle.manual_branch_id())?
            .input_manifest_ref;
        Ok((*bundle.store.read_input_manifest(inputs)?).clone())
    }
    pub fn issue_manual_lease(
        &mut self,
        client: String,
        token: String,
    ) -> Result<ManualLease, BundleError> {
        self.require_local_writer()?;
        let core = self.core.clone();
        let inputs = self.inputs.clone();
        let history = core
            .export_history()
            .map_err(crate::bundle::history_error)?;
        let bundle = self
            .destination
            .as_mut()
            .ok_or(BundleError::HistoryChanged)?
            .bundle_mut()?;
        let branch = bundle.manual_branch_id().clone();
        if *bundle.branch_history(&branch)? != history {
            return Err(BundleError::HistoryChanged);
        }
        let issuer = bundle.issue_issuer(&core, &inputs, 1)?;
        let checkpoint = bundle.checkpoint(&branch)?;
        Ok(ManualLease {
            client,
            token,
            project: bundle.project_id().clone(),
            branch: branch.clone(),
            switch_generation: bundle
                .root()
                .ok_or(BundleError::HistoryChanged)?
                .manual_switch_generation,
            owner_epoch: bundle.store.owner_epoch(),
            generation: 1,
            issuer,
            anchor_head: checkpoint.head_id.clone(),
            predecessor: checkpoint.batch_ids.last().cloned(),
            history_sha256: checkpoint.history_ref.sha256.clone(),
            inputs: checkpoint.input_manifest_ref.clone(),
            lineage: checkpoint.lineage_ref.clone(),
        })
    }
    pub fn commit_manual_history(
        &mut self,
        lease: &ManualLease,
        accepted: &AcceptedHistory,
    ) -> Result<(ReceiptLookup, Option<BundleSaveReport>), BundleError> {
        self.require_local_writer()?;
        let envelope = AcceptedManualCommand::decode(accepted)?.envelope;
        if envelope.manual.as_ref().map(|m| &m.binding) != Some(lease) {
            return Err(BundleError::HistoryChanged);
        }
        let core = self.core.clone();
        let bundle = self
            .destination
            .as_mut()
            .ok_or(BundleError::HistoryChanged)?
            .bundle_mut()?;
        let result = crate::bundle_commit::BundleCommit::admitted_history(
            bundle,
            &envelope,
            &core,
            &lease.inputs,
        )?;
        if let Some(report) = &result.1 {
            self.association
                .as_mut()
                .ok_or(BundleError::HistoryChanged)?
                .saved = Some(report.history_sha256.clone());
        }
        Ok(result)
    }
}
