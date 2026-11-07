// Included by the Linux native invocation owner. Render before durable charging.
impl HostCommandOwner {
    pub(super) fn codex_prompt(
        &self,
        task: &crate::orchestration::orchestration_records::TaskRecord,
        payload: &[u8],
        live: &serde_json::Value,
    ) -> Result<Vec<u8>, HostError> {
        let notes = self.prompt_notes(task)?;
        let overview = self.hooks.runs.iter().find(|(_,run)|run.tasks.contains_key(&task.id))
            .map(|(run,_)| {
                let (revision,document)=self.context_snapshot(run,&task.id)?;
                self.context_overview(run,task,revision,&document)
            }).transpose()?.unwrap_or_else(||serde_json::json!({"omitted":true,"reason":"task Core unavailable","read_route":"get_timeline_state"}));
        let mut prefix = super::task_context::application_prefix(task.brief.role)?;
        prefix.extend(serde_json::to_vec(&serde_json::json!({"live":live,"task":task.id,"brief":task.brief,"notes":notes,"overview":overview,"payload":std::str::from_utf8(payload).map_err(refused)?}))?);
        prefix.push(b'}');
        Ok(prefix)
    }
}
