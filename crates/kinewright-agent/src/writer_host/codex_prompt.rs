use super::codex_instructions::CODEX_INSTRUCTIONS;
// Included by the Linux native invocation owner. Render BEFORE durable charging.
impl HostCommandOwner {
    pub(super) fn codex_prompt(
        &self,
        task: &crate::orchestration::orchestration_records::TaskRecord,
        payload: &[u8],
        live: &serde_json::Value,
    ) -> Result<Vec<u8>, HostError> {
        use kinewright_project::studio_records::{FeedbackBatch, FeedbackRevision};
        let store = self.owner()?.store()?;
        let plan_bytes = store.read_evidence(&task.brief.plan)?;
        let plan: serde_json::Value =
            serde_json::from_slice(&plan_bytes).unwrap_or(serde_json::Value::Null);
        let mut notes = Vec::new();
        if plan["kind"] == "revise_cut_plan" {
            let reference: ObjectRef = serde_json::from_value(plan["batch_ref"].clone())?;
            let batch = store.read_record::<FeedbackBatch>(&reference)?;
            for reference in &batch.revision_refs {
                let revision = store.read_record::<FeedbackRevision>(reference)?;
                if revision.original_text_ref.byte_length
                    > (crate::protocol::limits::FRAME_BYTES / 2) as u64
                {
                    return Err(refused("feedback prompt capacity"));
                }
                let text = store.read_evidence(&revision.original_text_ref)?;
                notes.push(serde_json::json!({"anchor":revision.anchor,"text":std::str::from_utf8(&text).map_err(refused)?}));
            }
        }

        Ok(serde_json::to_vec(
            &serde_json::json!({"instructions":CODEX_INSTRUCTIONS,"live":live,"task":task.id,"brief":task.brief,"notes":notes,"payload":std::str::from_utf8(payload).map_err(refused)?}),
        )?)
    }
}
