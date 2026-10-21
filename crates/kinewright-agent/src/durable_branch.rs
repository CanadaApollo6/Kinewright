//! Frozen specialist bases bind durable identity before using a fresh Core revision.
#![allow(clippy::missing_errors_doc)]
use crate::TimelineBranch;
use kinewright_core::{
    Command, Core, Document, Event, JournalCommand, Query, QueryResult, TimelineRevision,
};
use kinewright_project::{
    bundle::{BundleError, ProjectBundle},
    bundle_records::{BranchCheckpoint, BranchId, ObjectRef, SpecialistBaseRecord},
    receipts::{CommandEnvelope, CommandId, ReceiptLookup},
    store::StorePin,
};
use std::sync::Arc;

pub struct DurableTimelineBranch {
    branch_id: BranchId,
    branch: TimelineBranch,
    base: SpecialistBaseRecord,
    pin: StorePin,
}

fn snapshot(core: &Core) -> Result<(TimelineRevision, Arc<Document>), BundleError> {
    let Event::QueryResult(QueryResult::Snapshot { revision, document }) = core
        .request(Command::Query(Query::Snapshot))
        .map_err(|e| BundleError::Corrupt(e.to_string()))?
    else {
        return Err(BundleError::Corrupt("Core snapshot response".into()));
    };
    Ok((revision, document))
}

impl DurableTimelineBranch {
    /// The live Core is quiescent and exactly the acknowledged target; S4 owns task quiescence.
    pub fn fork(bundle: &mut ProjectBundle, live: &Core, name: &str) -> Result<Self, BundleError> {
        let source = bundle.manual_branch_id().clone();
        let checkpoint = bundle.checkpoint(&source)?.clone();
        let frozen = bundle.branch_history(&source)?;
        if live
            .export_history()
            .map_err(|e| BundleError::Corrupt(e.to_string()))?
            != *frozen
        {
            return Err(BundleError::Unsupported(
                "live Core is not the durable base".into(),
            ));
        }
        let base = SpecialistBaseRecord {
            target_branch: source.clone(),
            base_head: checkpoint.head_id,
            base_document_hash: checkpoint.document_hash,
            original_baseline_ref: checkpoint.document_ref,
            frozen_checkpoint_ref: bundle.checkpoint_reference(&source)?,
            full_applied_delta: vec![],
        };
        let pin = bundle
            .store()
            .pin(std::slice::from_ref(&base.frozen_checkpoint_ref))?;
        let branch_id = bundle.fork_branch(&source)?;
        let (revision, document) = snapshot(live)?;
        let branch = TimelineBranch::new(name, revision, document)
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        Ok(Self {
            branch_id,
            branch,
            base,
            pin,
        })
    }
    #[must_use]
    pub fn core(&self) -> Core {
        self.branch.core()
    }
    #[must_use]
    pub const fn branch_id(&self) -> &BranchId {
        &self.branch_id
    }
    #[must_use]
    pub const fn base(&self) -> &SpecialistBaseRecord {
        &self.base
    }

    /// Store the full applied delta and its original base independently of manual floors.
    pub fn persist(&mut self, bundle: &mut ProjectBundle) -> Result<ObjectRef, BundleError> {
        let frozen = self
            .branch
            .core()
            .export_history()
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        bundle.save_branch(&self.branch_id, &frozen)?;
        self.base.full_applied_delta = frozen.applied_operations();
        let reference = bundle.retain_specialist_base(&self.base)?;
        self.pin = bundle.store().pin(&[
            reference.clone(),
            bundle.checkpoint_reference(&self.branch_id)?,
        ])?;
        Ok(reference)
    }
    pub fn reopen(
        bundle: &ProjectBundle,
        branch_id: BranchId,
        base_ref: &ObjectRef,
        name: &str,
    ) -> Result<Self, BundleError> {
        let base = bundle
            .store()
            .read_record::<SpecialistBaseRecord>(base_ref)?;
        let checkpoint = bundle
            .store()
            .read_record::<BranchCheckpoint>(&base.frozen_checkpoint_ref)?;
        if checkpoint.branch_id != base.target_branch
            || checkpoint.head_id != base.base_head
            || checkpoint.document_hash != base.base_document_hash
            || base.original_baseline_ref != checkpoint.document_ref
        {
            return Err(BundleError::Corrupt(
                "specialist base identity mismatch".into(),
            ));
        }
        let baseline = bundle
            .store()
            .read_record::<Document>(&base.original_baseline_ref)?;
        let history = bundle.branch_history(&branch_id)?;
        if history.baseline != Arc::new((*baseline).clone())
            || history.applied_operations() != base.full_applied_delta
        {
            return Err(BundleError::Corrupt(
                "frozen specialist delta differs from checkpoint".into(),
            ));
        }
        let branch = TimelineBranch::new(name, TimelineRevision(0), Arc::new((*baseline).clone()))
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        branch
            .core()
            .restore_history((*history).clone())
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let pin = bundle
            .store()
            .pin(&[base_ref.clone(), bundle.checkpoint_reference(&branch_id)?])?;
        Ok(Self {
            branch_id,
            branch,
            base: (*base).clone(),
            pin,
        })
    }
    fn bind(&self, bundle: &ProjectBundle, live: &Core) -> Result<TimelineRevision, BundleError> {
        let checkpoint = bundle.checkpoint(&self.base.target_branch)?;
        if checkpoint.branch_id != self.base.target_branch
            || checkpoint.head_id != self.base.base_head
            || checkpoint.document_hash != self.base.base_document_hash
        {
            return Err(BundleError::Unsupported("durable base conflict".into()));
        }
        let current = live
            .export_history()
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        if current != *bundle.branch_history(&self.base.target_branch)? {
            return Err(BundleError::Unsupported(
                "runtime Core is not the bound durable head".into(),
            ));
        }
        Ok(snapshot(live)?.0)
    }
    pub fn merge_envelope(
        &self,
        bundle: &ProjectBundle,
        live: &Core,
        id: CommandId,
    ) -> Result<CommandEnvelope, BundleError> {
        let revision = self.bind(bundle, live)?;
        let frozen = self
            .branch
            .core()
            .export_history()
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        if frozen.applied_operations() != self.base.full_applied_delta {
            return Err(BundleError::Unsupported(
                "specialist changed after freezing".into(),
            ));
        }
        Ok(CommandEnvelope {
            version: 1,
            action: "edit".into(),
            id,
            project_id: bundle.project_id().clone(),
            target_branch: self.base.target_branch.clone(),
            owner_epoch: bundle.store().owner_epoch(),
            lease: None,
            expected_head: Some(self.base.base_head.clone()),
            expected_revision: revision,
            command: Some(JournalCommand::DoBatch(
                self.base.full_applied_delta.clone(),
            )),
            comment_text: None,
            comment_strokes: None,
            evidence: vec![],
            transition: None,
        })
    }
    /// Lookup the original result before rebinding; a completed merge has already changed its base head.
    pub fn merge(
        &self,
        bundle: &mut ProjectBundle,
        live: &Core,
        envelope: &CommandEnvelope,
    ) -> Result<ReceiptLookup, BundleError> {
        let previous = bundle.receipt_lookup(envelope)?;
        if previous != ReceiptLookup::Absent {
            return Ok(previous);
        }
        let revision = self.bind(bundle, live)?;
        if envelope.expected_revision != revision
            || envelope.expected_head.as_ref() != Some(&self.base.base_head)
            || envelope.target_branch != self.base.target_branch
            || envelope.command
                != Some(JournalCommand::DoBatch(
                    self.base.full_applied_delta.clone(),
                ))
        {
            return Err(BundleError::Unsupported(
                "merge envelope differs from frozen durable binding".into(),
            ));
        }
        let inputs = bundle
            .checkpoint(&self.base.target_branch)?
            .input_manifest_ref
            .clone();
        bundle.command(envelope, live, &inputs, &[])
    }
}
