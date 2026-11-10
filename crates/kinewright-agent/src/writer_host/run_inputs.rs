//! Certify pinned run inputs outside the owner mutex, then bind under that mutex.
#[cfg(target_os = "linux")]
use super::HostAuth;
use super::{
    HostCommand, HostCommandOwner, HostError, HostOwnedInputClosure, HostRefusal, HostReply,
    HostRequest,
};
use crate::orchestration::orchestration_records::OrchestrationCommand;
#[cfg(target_os = "linux")]
use crate::orchestration::orchestration_records::{RunId, RunProjection};
use kinewright_project::{
    bundle_records::{BranchCheckpoint, ObjectRef},
    studio_records::{SnapshotDescriptor, SnapshotKey, VersionId},
    studio_store::FrozenContextRequest,
};
use std::sync::{Arc, Mutex};

pub(super) struct PreparedRunInputs {
    request: FrozenContextRequest,
}
impl PreparedRunInputs {
    pub(super) fn open(self) -> Result<HostOwnedInputClosure, HostError> {
        let context = Arc::new(self.request.open()?);
        let inputs = context.playback_inputs()?;
        Ok(HostOwnedInputClosure { context, inputs })
    }
}
impl HostOwnedInputClosure {
    pub(super) fn recheck(
        &self,
        descriptor: &SnapshotDescriptor,
        epoch: u64,
    ) -> Result<(), HostError> {
        let certified = &self.context.snapshot.descriptor;
        if certified.key.project != descriptor.key.project
            || certified.key.document_hash != descriptor.key.document_hash
            || certified.document_ref != descriptor.document_ref
            || certified.input_manifest_ref != descriptor.input_manifest_ref
            || self
                .context
                .certified_input_leases
                .iter()
                .any(|lease| lease.owner_epoch() != epoch)
        {
            return Err(HostRefusal::new(
                "continuation_inputs",
                "Run inputs changed while they were certified. No execution was started. Try again with the current inputs.",
            )
            .into());
        }
        self.context.verify_certification_current().map_err(|_| {
            HostRefusal::new(
                "continuation_inputs",
                "Run inputs could not be rechecked after certification. No execution was started.",
            )
        })?;
        Ok(())
    }
}
impl HostCommandOwner {
    pub(super) fn run_input_descriptor(
        &self,
        reference: &ObjectRef,
    ) -> Result<SnapshotDescriptor, HostError> {
        let checkpoint = self
            .owner()?
            .store()?
            .read_record::<BranchCheckpoint>(reference)?;
        Ok(SnapshotDescriptor {
            key: SnapshotKey {
                project: self.owner()?.active_manual_project()?,
                version: VersionId::fresh()?,
                head: checkpoint.head_id.clone(),
                document_hash: checkpoint.document_hash.clone(),
                input_manifest_ref: checkpoint.input_manifest_ref.clone(),
            },
            checkpoint_ref: reference.clone(),
            document_ref: checkpoint.document_ref.clone(),
            lineage_ref: checkpoint.lineage_ref.clone(),
            input_manifest_ref: checkpoint.input_manifest_ref.clone(),
            revision_refs: vec![],
            parent_version: None,
            run_provenance: None,
        })
    }
    pub(super) fn prepare_run_inputs(
        &self,
        reference: &ObjectRef,
    ) -> Result<PreparedRunInputs, HostError> {
        Ok(PreparedRunInputs {
            request: self
                .owner()?
                .prepare_frozen_context_descriptor(&self.run_input_descriptor(reference)?)?,
        })
    }
    pub(super) fn command_run_inputs(
        &self,
        bearer: &str,
        request: &HostRequest,
    ) -> Result<Option<PreparedRunInputs>, HostError> {
        let (index, _, _) = self.authorize_request(Some(bearer), request)?;
        match &request.command {
            HostCommand::DispatchOrchestration { request } if self.hooks.pending.is_none() => {
                self.authorize_orchestration(index, request)?;
                if self
                    .owner()?
                    .orchestration_replay(&request.envelope, request.consume_batch.as_ref())?
                    .is_some()
                {
                    return Ok(None);
                }
                if let OrchestrationCommand::StartRun(start) = &request.envelope.command {
                    return self
                        .prepare_run_inputs(&start.director.scope.base.checkpoint)
                        .map(Some);
                }
                Ok(None)
            }
            HostCommand::ReviseCut { request }
                if self.hooks.pending.is_none()
                    && self.attachments[index].role == super::AttachmentRole::Editor =>
            {
                let feedback = self.owner()?.feedback_index()?;
                if !feedback
                    .batches
                    .get(&request.batch_id)
                    .is_some_and(|batch| batch.request == *request && batch.consumed_by.is_none())
                {
                    return Ok(None);
                }
                drop(feedback);
                let (reference, _) = self.owner()?.manual_checkpoint()?;
                self.prepare_run_inputs(&reference).map(Some)
            }
            _ => Ok(None),
        }
    }
}
pub(super) fn prepare(
    host: &Arc<Mutex<HostCommandOwner>>,
    bearer: &str,
    request: &HostRequest,
) -> Result<Option<HostOwnedInputClosure>, HostError> {
    let start = matches!(&request.command, HostCommand::DispatchOrchestration { request }
        if matches!(&request.envelope.command, OrchestrationCommand::StartRun(_)));
    if !start && !matches!(&request.command, HostCommand::ReviseCut { .. }) {
        return Ok(None);
    }
    let prepared = {
        let owner = host
            .lock()
            .map_err(|_| HostRefusal::new("retention_lost", super::RETENTION_LOST))?;
        owner.command_run_inputs(bearer, request)?
    };
    prepared.map(PreparedRunInputs::open).transpose()
}
pub(super) fn execute_prepared(
    owner: &mut HostCommandOwner,
    bearer: &str,
    request: HostRequest,
    prepared: Result<Option<HostOwnedInputClosure>, HostError>,
) -> HostReply {
    match prepared {
        Ok(prepared) => {
            owner.hooks.prepared_run_inputs = prepared;
            let reply = owner.execute_bearer(Some(bearer), request);
            owner.hooks.prepared_run_inputs = None;
            reply
        }
        Err(error) => HostReply {
            status: owner.status(),
            error: Some(error.refusal()),
            ..HostReply::default()
        },
    }
}
pub(super) fn execute(
    host: &Arc<Mutex<HostCommandOwner>>,
    bearer: &str,
    request: HostRequest,
) -> HostReply {
    let prepared = prepare(host, bearer, &request);
    let Ok(mut owner) = host.lock() else {
        return HostReply {
            error: Some(HostRefusal::new("retention_lost", super::RETENTION_LOST)),
            ..HostReply::default()
        };
    };
    execute_prepared(&mut owner, bearer, request, prepared)
}
#[cfg(target_os = "linux")]
pub(crate) fn restore(
    host: &Arc<Mutex<HostCommandOwner>>,
    run: &RunId,
) -> Result<(HostAuth, RunProjection), HostError> {
    let prepared = {
        let owner = host
            .lock()
            .map_err(|_| HostRefusal::new("retention_lost", super::RETENTION_LOST))?;
        if owner.hooks.runs.contains_key(run) {
            None
        } else {
            let state = owner
                .owner()?
                .orchestration_projection(run)?
                .ok_or_else(|| HostRefusal::new("task_scope", "missing recovery run"))?;
            let brief = &state
                .tasks
                .iter()
                .find(|t| t.parent.is_none())
                .ok_or_else(|| HostRefusal::new("task_scope", "missing Director"))?
                .brief;
            Some(owner.prepare_run_inputs(&brief.scope.base.checkpoint)?)
        }
    }
    .map(PreparedRunInputs::open)
    .transpose()?;
    let mut owner = host
        .lock()
        .map_err(|_| HostRefusal::new("retention_lost", super::RETENTION_LOST))?;
    owner.hooks.prepared_run_inputs = prepared;
    let result = owner.restore_native_run(run);
    owner.hooks.prepared_run_inputs = None;
    result
}
