//! Fixture commands preserve original envelopes; production owns every result and retry decision.
use super::{
    protocol::{Action, Call, CommandArgs, CommentArgs, CommentMaintenanceArgs, LoadedFixture},
    revised::Adapter,
};
use kinewright_project::{
    bundle_records::{ObjectRef, RecordKind},
    receipts::{CommandEnvelope, CommandId, ReceiptLookup, ReceiptResult},
};
use serde_json::{Value, json};
use std::{fs, io::Write};
fn reference(fixture: &LoadedFixture, role: &str) -> Result<ObjectRef, String> {
    let input = fixture
        .fixture
        .inputs
        .get(role)
        .ok_or("missing exact byte role")?;
    Ok(ObjectRef {
        sha256: input.sha256.clone(),
        byte_length: input.byte_length,
        record_kind: RecordKind::OwnedEvidence,
    })
}
fn write_request(path: &std::path::Path, request: &CommandEnvelope) -> Result<(), String> {
    fs::create_dir_all(path.parent().ok_or("request parent")?).map_err(|e| e.to_string())?;
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec(request).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}
fn read_request(path: &std::path::Path) -> Result<CommandEnvelope, String> {
    serde_json::from_value(super::protocol::unique_json(
        &super::protocol::read_bounded(path, super::protocol::CONTROL_BYTES as u64)?,
    )?)
    .map_err(|e| e.to_string())
}
impl Adapter {
    fn receipt_inputs(&mut self) -> Result<ObjectRef, String> {
        let core = self.baseline.core()?.clone();
        let bundle = self.bundle.as_mut().ok_or("bundle owner")?;
        if let Some(root) = bundle.root() {
            return Ok(root.input_manifest_ref.clone());
        }
        let inputs = bundle.empty_inputs().map_err(|e| e.to_string())?;
        let reference = inputs.reference.clone();
        bundle
            .save(
                &core.export_history().map_err(|e| e.to_string())?,
                &reference,
            )
            .map_err(|e| e.to_string())?;
        Ok(reference)
    }
    pub(super) fn receipt_edit(
        &mut self,
        action: &Action,
        args: &CommandArgs,
        fixture: &LoadedFixture,
    ) -> Result<Value, String> {
        let path = self.baseline.root.join("commands.kinewright");
        self.open_comment_bundle(&path)?;
        let inputs = self.receipt_inputs()?;
        let core = self.baseline.core()?.clone();
        let original = action.retry_of.unwrap_or(action.id);
        let request_path = self.comment_request_path(&format!("edit-{original}"))?;
        let request = if request_path.is_file() {
            read_request(&request_path)?
        } else {
            if action.retry_of.is_some() {
                return Err("preserved original envelope is unavailable".into());
            }
            let revision = self.baseline.snapshot()?.0;
            let bundle = self.bundle.as_mut().ok_or("bundle")?;
            let issuer = bundle
                .issue_issuer(&core, &inputs, u64::from(original))
                .map_err(|e| e.to_string())?;
            let request = CommandEnvelope {
                version: 1,
                action: "edit".into(),
                id: CommandId {
                    issuer,
                    sequence: u64::from(original),
                },
                project_id: bundle.project_id().clone(),
                target_branch: bundle.manual_branch_id().clone(),
                owner_epoch: bundle.store().owner_epoch(),
                lease: None,
                expected_head: bundle.manual_head().cloned(),
                expected_revision: args
                    .expected_revision
                    .map_or(revision, kinewright_core::TimelineRevision),
                command: Some(
                    serde_json::from_value(super::protocol::unique_json(super::baseline::input(
                        fixture,
                        &args.input,
                    )?)?)
                    .map_err(|e| e.to_string())?,
                ),
                comment_text: None,
                comment_strokes: None,
                evidence: vec![],
            };
            write_request(&request_path, &request)?;
            request
        };
        let result = self
            .bundle
            .as_mut()
            .ok_or("bundle")?
            .command(&request, &core, &inputs, &[])
            .map_err(|e| e.to_string())?;
        let (ReceiptLookup::Committed(receipt) | ReceiptLookup::Retired(receipt)) = result else {
            return Err(format!("edit was not reconciled: {result:?}"));
        };
        let original_epoch_is_current =
            request.owner_epoch == self.bundle.as_ref().ok_or("bundle")?.store().owner_epoch();
        Ok(
            json!({"backend":"linux-local-durable-receipts","receipt":receipt,"original_request":request,"original_epoch_is_current":original_epoch_is_current,"history":super::baseline::history_value(&core)?}),
        )
    }
    #[allow(clippy::too_many_lines)]
    pub(super) fn comment_receipt_submit(
        &mut self,
        args: &CommentArgs,
        fixture: &LoadedFixture,
    ) -> Result<Value, String> {
        let path = self.baseline.root.join(&args.path);
        self.open_comment_bundle(&path)?;
        let inputs = self.receipt_inputs()?;
        let text = fixture.files.get(&args.text).ok_or("missing text")?;
        let strokes = fixture.files.get(&args.strokes).ok_or("missing strokes")?;
        let text_ref = reference(fixture, &args.text)?;
        let strokes_ref = reference(fixture, &args.strokes)?;
        let key = args.original.as_deref().unwrap_or(&args.request);
        let request_path = self.comment_request_path(key)?;
        let core = self.baseline.core()?.clone();
        let original = if request_path.is_file() {
            read_request(&request_path)?
        } else {
            if args.original.is_some() {
                return Err("preserved original comment request unavailable".into());
            }
            let original_action=fixture.fixture.actions.iter().find(|a|matches!(&a.call,Call::CommentSubmit(c) if c.request==args.request && c.original.is_none())).ok_or("original action")?;
            let revision = self.baseline.snapshot()?.0;
            let bundle = self.bundle.as_mut().ok_or("bundle")?;
            let issuer = bundle
                .issue_issuer(&core, &inputs, u64::from(original_action.id))
                .map_err(|e| e.to_string())?;
            let request = CommandEnvelope {
                version: 1,
                action: "comment".into(),
                id: CommandId {
                    issuer,
                    sequence: u64::from(original_action.id),
                },
                project_id: bundle.project_id().clone(),
                target_branch: bundle.manual_branch_id().clone(),
                owner_epoch: bundle.store().owner_epoch(),
                lease: None,
                expected_head: bundle.manual_head().cloned(),
                expected_revision: revision,
                command: None,
                comment_text: Some(text_ref.clone()),
                comment_strokes: Some(strokes_ref.clone()),
                evidence: vec![],
            };
            write_request(&request_path, &request)?;
            request
        };
        let mut request = original.clone();
        request.comment_text = Some(text_ref.clone());
        request.comment_strokes = Some(strokes_ref.clone());
        let before = fs::read(&path).map_err(|e| e.to_string())?;
        let pending_bytes = fs::read(&request_path).map_err(|e| e.to_string())?;
        let history = core.export_history().map_err(|e| e.to_string())?;
        let bundle = self.bundle.as_mut().ok_or("bundle")?;
        let (result, trace) = kinewright_project::io_observer::observe(|| {
            bundle.command(
                &request,
                &core,
                &inputs,
                &[
                    (&text_ref, text.as_slice()),
                    (&strokes_ref, strokes.as_slice()),
                ],
            )
        });
        let result = result.map_err(|e| e.to_string())?;
        let root_unchanged = fs::read(&path).map_err(|e| e.to_string())? == before;
        let pending_preserved =
            fs::read(&request_path).map_err(|e| e.to_string())? == pending_bytes;
        match result {
            ReceiptLookup::Committed(receipt) | ReceiptLookup::Retired(receipt) => {
                let ReceiptResult::Comment {
                    record_id,
                    record_ref,
                    ..
                } = &receipt.result
                else {
                    return Err("wrong result kind".into());
                };
                let record = bundle
                    .store()
                    .read_evidence(record_ref)
                    .map_err(|e| e.to_string())?;
                let record: Value = super::protocol::unique_json(&record)?;
                let resolved = record["record_id"] == *record_id
                    && receipt.id == original.id
                    && receipt.digest == original.digest().map_err(|e| e.to_string())?;
                let reconciled_old_epoch = resolved
                    && original.owner_epoch != bundle.store().owner_epoch()
                    && root_unchanged
                    && trace.events.is_empty()
                    && core.export_history().map_err(|e| e.to_string())? == history;
                Ok(
                    json!({"backend":"linux-local-durable-receipts","durable_graph_published":true,"receipt_acknowledged":true,"receipt":receipt,"record_id":record_id,"record_ref":record_ref,"binding_present":true,"resolved_original_result":resolved,"reconciled_old_epoch":reconciled_old_epoch,"root_unchanged":root_unchanged,"original_request":original,"mutation_calls":trace.events.len()}),
                )
            }
            ReceiptLookup::IdPayloadConflict => Ok(
                json!({"id_payload_conflict":true,"root_unchanged":root_unchanged,"mutation_calls":trace.events.len(),"history_unchanged":core.export_history().map_err(|e|e.to_string())?==history}),
            ),
            ReceiptLookup::RetiredUnresolved => Ok(
                json!({"retired_unresolved":true,"pending_chain_preserved":pending_preserved,"root_unchanged":root_unchanged,"mutation_calls":trace.events.len(),"history_unchanged":core.export_history().map_err(|e|e.to_string())?==history}),
            ),
            value => Err(format!("unsettled comment lookup {value:?}")),
        }
    }
    pub(super) fn comment_receipt_inspect(
        &mut self,
        args: &CommentArgs,
        fixture: &LoadedFixture,
    ) -> Result<Value, String> {
        let path = self.baseline.root.join(&args.path);
        self.open_comment_bundle(&path)?;
        let request = read_request(&self.comment_request_path(&args.request)?)?;
        let bundle = self.bundle.as_ref().ok_or("bundle")?;
        let text = reference(fixture, &args.text)?;
        let strokes = reference(fixture, &args.strokes)?;
        let mut records = vec![];
        for reference in bundle.evidence_refs() {
            let bytes = bundle
                .store()
                .read_evidence(reference)
                .map_err(|e| e.to_string())?;
            let Ok(record) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            if record["tag"] == "s1-observer-comment"
                && record["text"] == serde_json::to_value(&text).map_err(|e| e.to_string())?
                && record["strokes"] == serde_json::to_value(&strokes).map_err(|e| e.to_string())?
            {
                records.push(record["record_id"].clone());
            }
        }
        if records.is_empty() {
            return Err("comment bytes are not reachable from the actual root".into());
        }
        let text = bundle
            .store()
            .read_evidence(&text)
            .map_err(|e| e.to_string())?;
        let strokes = bundle
            .store()
            .read_evidence(&strokes)
            .map_err(|e| e.to_string())?;
        let lookup = bundle.receipt_lookup(&request).map_err(|e| e.to_string())?;
        Ok(
            json!({"backend":"linux-local-durable-receipts","text_utf8":std::str::from_utf8(&text).map_err(|e|e.to_string())?,"strokes_utf8":std::str::from_utf8(&strokes).map_err(|e|e.to_string())?,"record_ids":records,"record_count":records.len(),"binding_present":matches!(lookup,ReceiptLookup::Committed(_)|ReceiptLookup::Retired(_)),"original_request":request}),
        )
    }
    pub(super) fn comment_receipt_maintenance(
        &mut self,
        args: &CommentMaintenanceArgs,
        forget: bool,
    ) -> Result<Value, String> {
        let path = self.baseline.root.join(&args.path);
        self.open_comment_bundle(&path)?;
        let inputs = self.receipt_inputs()?;
        let request = read_request(&self.comment_request_path(&args.request)?)?;
        let core = self.baseline.core()?.clone();
        let bundle = self.bundle.as_mut().ok_or("bundle")?;
        bundle
            .acknowledge_issuer(&request.id.issuer, request.id.sequence, &core, &inputs)
            .map_err(|e| e.to_string())?;
        bundle
            .retire_issuer(&request.id.issuer, forget, &core, &inputs)
            .map_err(|e| e.to_string())?;
        for _ in 0..args.rotations {
            bundle
                .save(&core.export_history().map_err(|e| e.to_string())?, &inputs)
                .map_err(|e| e.to_string())?;
        }
        Ok(json!({"retired":true,"forgotten":forget,"rotations":args.rotations}))
    }
}
