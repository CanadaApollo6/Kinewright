//! Intentional GUI detach retains the host's certified input closure and Producer.
use super::{
    AttachmentRole, HostCommandOwner, HostError, HostRefusal,
    orchestration::{HostRun, ProducerScope},
};
use crate::orchestration::orchestration_records::{RunId, TaskBrief};
use kinewright_project::{
    bundle_records::BranchCheckpoint,
    studio_records::{SnapshotDescriptor, SnapshotKey, VersionId},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

/// Constructed only from the host owner's admitted and charged frozen context.
/// Neither the wire nor a GUI callback can construct this value.
pub struct HostOwnedInputClosure {
    context: Arc<kinewright_project::studio_store::FrozenContext>,
    inputs: Arc<kinewright_media::owned_io::PlaybackInputs>,
}
impl HostOwnedInputClosure {
    fn ready(&self) -> Result<(), HostError> {
        let _verified = self.context.playback_inputs()?;
        if self.inputs.document() != &self.context.document {
            return Err(
                HostRefusal::new("continuation_inputs", "retained document changed").into(),
            );
        }
        self.context
            .verify_retained_inputs()
            .map_err(|error| HostRefusal::new("continuation_inputs", &error.to_string()))?;
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetachOutcome {
    pub retained_runs: Vec<RunId>,
}

impl HostCommandOwner {
    pub(super) fn retain_run(&mut self, run: &RunId, brief: &TaskBrief) -> Result<(), HostError> {
        if self.hooks.runs.contains_key(run) {
            return Ok(());
        }
        if self.hooks.runs.len() >= 64 {
            return Err(HostRefusal::new("capacity", "64 retained runs maximum").into());
        }
        self.owner()?
            .validate_orchestration_scope(&self.owner()?.active_manual_project()?, brief)?;
        let checkpoint = self
            .owner()?
            .store()?
            .read_record::<BranchCheckpoint>(&brief.scope.base.checkpoint)?;
        let descriptor = SnapshotDescriptor {
            key: SnapshotKey {
                project: self.owner()?.active_manual_project()?,
                version: VersionId::fresh()?,
                head: checkpoint.head_id.clone(),
                document_hash: checkpoint.document_hash.clone(),
                input_manifest_ref: checkpoint.input_manifest_ref.clone(),
            },
            checkpoint_ref: brief.scope.base.checkpoint.clone(),
            document_ref: checkpoint.document_ref.clone(),
            lineage_ref: checkpoint.lineage_ref.clone(),
            input_manifest_ref: checkpoint.input_manifest_ref.clone(),
            revision_refs: vec![],
            parent_version: None,
            run_provenance: None,
        };
        drop(checkpoint);
        let context = Arc::new(self.owner()?.frozen_context_descriptor(&descriptor)?);
        let inputs = context.playback_inputs()?;
        let producer = self.issue_attachment(
            format!("host-run:{}", run.0.as_str()),
            AttachmentRole::Producer,
            self.config.roots.clone(),
        )?;
        let index = self.resolve_attachment(Some(&producer.bearer()))?;
        self.attachments[index].producer_scope = Some(ProducerScope {
            run: run.clone(),
            task: None,
            attempt: None,
            endpoint: None,
            owner_epoch: self.owner()?.store()?.owner_epoch(),
            expires_at: None,
            scope: brief.scope.clone(),
        });
        self.hooks.runs.insert(
            run.clone(),
            HostRun {
                producer,
                inputs: HostOwnedInputClosure { context, inputs },
                tasks: BTreeMap::new(),
                continuable: true,
            },
        );
        Ok(())
    }
    pub(super) fn detach_for_continuation(
        &mut self,
        index: usize,
    ) -> Result<DetachOutcome, HostError> {
        if self.attachments[index].role != AttachmentRole::Editor
            || self.recovering
            || self.switch_frozen
            || self.hooks.pending.is_some()
            || self.prepared_owned_media.is_some()
        {
            return Err(HostRefusal::new(
                "continuation_refused",
                "the attachment remains paused until its current work is settled",
            )
            .into());
        }
        let mut retained_runs = vec![];
        for (id, run) in &self.hooks.runs {
            if !run.continuable {
                continue;
            }
            run.inputs.ready()?;
            let producer = self.resolve_attachment(Some(&run.producer.bearer()))?;
            if self.attachments[producer].role != AttachmentRole::Producer
                || self.attachments[producer]
                    .producer_scope
                    .as_ref()
                    .is_none_or(ProducerScope::expired)
            {
                return Err(HostRefusal::new(
                    "continuation_refused",
                    "independent host Producer is not live",
                )
                .into());
            }
            retained_runs.push(id.clone());
        }
        if retained_runs.is_empty() {
            return Err(
                HostRefusal::new("continuation_refused", "no independently retained run").into(),
            );
        }
        self.remote = None;
        Ok(DetachOutcome { retained_runs })
    }
    /// Only the trusted issuer marks an attachment as the person-facing holder.
    pub fn issue_person_facing_editor(
        &mut self,
        subject: String,
        roots: super::SessionRoots,
    ) -> Result<super::HostAuth, HostError> {
        let auth = self.issue_attachment(subject, AttachmentRole::Editor, roots)?;
        let index = self.resolve_attachment(Some(&auth.bearer()))?;
        self.attachments[index].person_facing = true;
        Ok(auth)
    }
}
