//! The branch publication names one owner record and its projection. The
//! accepted stream, receipt window and owner reference are one immutable value.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::BundleError,
    bundle_records::{BranchId, HeadId, ObjectRef, ProjectId, RecordKind},
    operation_history::{
        HistoryPool, Role, SnapshotIdentity, StoredOwnerRecord, ValidatedOwnerSnapshot,
    },
    operation_protocol::StreamState,
    receipts::CommandId,
    storage_budget::StorageLimits,
    store::{BundleRecord, ProjectStore, record_sealed},
};
use kinewright_core::{TimelineRevision, undo::CommandKey};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub branch_id: BranchId,
    pub head_id: HeadId,
    pub owner_ref: ObjectRef,
    pub identity: SnapshotIdentity,
    pub role: Role,
    pub document_ref: ObjectRef,
    pub revision: TimelineRevision,
    pub position: CommandKey,
    pub input_manifest_ref: ObjectRef,
    pub generation: u64,
    pub predecessor: Option<HeadId>,
    pub command_cut: Option<CommandId>,
    pub take_map_revision_ref: Option<ObjectRef>,
    pub stream: Option<StreamState>,
}
/// Publication scope comes from the actual frozen actor cut or prepared
/// update, rather than from a second observation of the live endpoint.
pub struct Header {
    pub branch: BranchId,
    pub head: HeadId,
    pub revision: TimelineRevision,
    pub inputs: ObjectRef,
    pub generation: u64,
    pub predecessor: Option<HeadId>,
    pub command_cut: Option<CommandId>,
    pub take_map: Option<ObjectRef>,
    pub stream: Option<StreamState>,
}
impl Checkpoint {
    pub fn from_sealed(
        header: Header,
        owner: &crate::operation_history::SealedOwner,
    ) -> Result<Self, BundleError> {
        let checkpoint = Self {
            branch_id: header.branch,
            head_id: header.head,
            owner_ref: owner.object.reference.clone(),
            identity: owner.identity,
            role: owner.role,
            document_ref: owner.document.clone(),
            revision: header.revision,
            position: owner.position,
            input_manifest_ref: header.inputs,
            generation: header.generation,
            predecessor: header.predecessor,
            command_cut: header.command_cut,
            take_map_revision_ref: header.take_map,
            stream: header.stream,
        };
        checkpoint.validate(&StorageLimits::default())?;
        Ok(checkpoint)
    }
    pub fn validate_scope(&self, project: &ProjectId) -> Result<(), BundleError> {
        if self
            .stream
            .as_ref()
            .is_some_and(|stream| &stream.seed().project != project)
        {
            return Err(BundleError::Corrupt("editing stream project scope".into()));
        }
        Ok(())
    }
    pub fn open(
        &self,
        store: &ProjectStore,
        pool: &HistoryPool,
    ) -> Result<ValidatedOwnerSnapshot, crate::operation_history::Error> {
        self.validate(store.limits())
            .map_err(|_| crate::operation_history::Error::InvalidRecord)?;
        let validated = StoredOwnerRecord::read_projected(
            store,
            &self.owner_ref,
            pool,
            self.role,
            &self.document_ref,
            self.identity,
        )?;
        if validated.position() != self.position {
            return Err(crate::operation_history::Error::InvalidRecord);
        }
        Ok(validated)
    }
}
impl record_sealed::Sealed for Checkpoint {}
impl BundleRecord for Checkpoint {
    const KIND: RecordKind = RecordKind::BranchCheckpoint;
    fn dependencies(&self) -> Vec<ObjectRef> {
        let mut references = vec![self.owner_ref.clone(), self.input_manifest_ref.clone()];
        references.extend(self.take_map_revision_ref.iter().cloned());
        references
    }
    fn validate(&self, _: &StorageLimits) -> Result<(), BundleError> {
        self.owner_ref.validate()?;
        self.document_ref.validate()?;
        self.input_manifest_ref.validate()?;
        if self.owner_ref.record_kind != RecordKind::OperationHistory
            || self.document_ref.record_kind != RecordKind::Document
            || self.input_manifest_ref.record_kind != RecordKind::InputManifest
            || self.generation == 0
        {
            return Err(BundleError::Corrupt("operation checkpoint metadata".into()));
        }
        if let Some(reference) = &self.take_map_revision_ref {
            reference.validate()?;
            if reference.record_kind != RecordKind::TakeMap {
                return Err(BundleError::Corrupt("checkpoint take map".into()));
            }
        }
        if let Some(command) = &self.command_cut {
            command.issuer.validate()?;
            if command.sequence == 0 {
                return Err(BundleError::Corrupt("checkpoint command cut".into()));
            }
        }
        if let Some(stream) = &self.stream {
            stream
                .validate()
                .map_err(|error| BundleError::Corrupt(error.to_string()))?;
            if stream.seed().branch != self.branch_id || stream.sequence() > self.position.position
            {
                return Err(BundleError::Corrupt(
                    "checkpoint editing stream scope".into(),
                ));
            }
            if stream.sequence() == 0 && stream.seed().installed != self.identity {
                return Err(BundleError::Corrupt("installed snapshot identity".into()));
            }
            if let Some(last) = stream.receipts().last()
                && (last.accepted.epoch != self.position.epoch
                    || last.accepted.position.checked_add(1) != Some(self.position.position)
                    || self.command_cut.as_ref() != Some(&last.id))
            {
                return Err(BundleError::Corrupt("checkpoint receipt cut".into()));
            }
            let mut previous: Option<CommandKey> = None;
            for receipt in stream.receipts() {
                if receipt.accepted.epoch != self.position.epoch
                    || previous.is_some_and(|key| {
                        key.position.checked_add(1) != Some(receipt.accepted.position)
                    })
                {
                    return Err(BundleError::Corrupt("checkpoint receipt positions".into()));
                }
                previous = Some(receipt.accepted);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "operation_checkpoint_tests.rs"]
mod tests;
