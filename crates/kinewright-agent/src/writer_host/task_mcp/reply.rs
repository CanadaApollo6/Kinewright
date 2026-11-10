use super::{HostCommandOwner, HostError, Role, TaskEndpoint, TaskMcp, Value, json, refused};

impl TaskMcp {
    /// Neither a cached manifest nor a caller's role can outlive this binding.
    pub(crate) fn role(&self) -> Result<Role, HostError> {
        let host = self.host.upgrade().ok_or_else(|| refused("host closed"))?;
        let host = self.lock_owner(&host)?;
        let (_, state, task) = host.resolve_task_mcp(&self.auth, &self.endpoint)?;
        Ok(state
            .tasks
            .iter()
            .find(|record| record.id == task)
            .ok_or_else(|| refused("task missing"))?
            .brief
            .role)
    }
    /// Capture the terminal state under the same writer lock as the commit,
    /// before retirement removes its live attachment or a project switch runs.
    pub(super) fn remember_terminal_state(&self, host: &HostCommandOwner) {
        use crate::orchestration::orchestration_records::TaskState;
        if let Ok(owner) = host.owner()
            && let Ok(Some(state)) = owner.orchestration_projection(&self.endpoint.authority().run)
            && let Some(task) = state
                .tasks
                .iter()
                .find(|task| task.id == self.endpoint.authority().task)
            && matches!(
                task.state,
                TaskState::Terminal(_)
                    | TaskState::Cancelled
                    | TaskState::Cancelling
                    | TaskState::CleanupBlocked(_)
            )
            && let Ok(mut terminal) = self.terminal_state.lock()
        {
            *terminal = Some(
                json!({"project":self.endpoint.authority().project,"run":state.run,"phase":state.phase,"outcome":state.outcome,
                "sequence":state.sequence,"task":task.id,"task_state":task.state}),
            );
        }
    }
    /// Refresh failure cannot change the operation result or its replay key.
    pub(crate) fn reply(&self, outcome: Value, receipt: Option<Value>) -> Value {
        let receipt = receipt.unwrap_or(Value::Null);
        let mut reply = json!({"receipt":receipt});
        reply["outcome"] = outcome;
        if let Ok(state) = self.terminal_state.lock()
            && let Some(state) = state.as_ref()
        {
            reply["state"] = state.clone();
        }
        if let Some(host) = self.host.upgrade()
            && let Ok(mut host) = self.lock_reply_owner(&host)
            && let Ok(mut issuer) = self.issuer.lock()
        {
            let context = host.task_reply_context(&self.auth, &self.endpoint, &mut issuer);
            reply
                .as_object_mut()
                .unwrap()
                .extend(context.as_object().unwrap().clone());
        }
        reply
    }
}
impl HostCommandOwner {
    /// Shared by tool replies, prompt/wake seeds and MCP initialization. State
    /// remains readable after terminal success; controls require live authority.
    pub(in crate::writer_host) fn task_reply_context(
        &mut self,
        auth: &super::HostAuth,
        endpoint: &TaskEndpoint,
        issuer: &mut Option<kinewright_project::receipts::IssuerId>,
    ) -> Value {
        let mut context = json!({"version":super::manifest::VERSION});
        let _ = (|| -> Result<(), HostError> {
            let index = self.resolve_attachment(Some(&auth.bearer()))?;
            let authority = endpoint.authority();
            let scope = self.attachments[index]
                .producer_scope
                .as_ref()
                .ok_or_else(|| refused("missing task scope"))?;
            if scope.run != authority.run
                || scope.task.as_ref() != Some(&authority.task)
                || scope.attempt.as_ref() != Some(&authority.attempt)
                || scope.endpoint.as_ref() != Some(&authority.endpoint)
                || authority.project != self.owner()?.active_manual_project()?
            {
                return Err(refused("reply attachment changed"));
            }
            let state = self
                .owner()?
                .orchestration_projection(&authority.run)?
                .ok_or_else(|| refused("run missing"))?;
            let task = state
                .tasks
                .iter()
                .find(|task| task.id == authority.task)
                .ok_or_else(|| refused("task missing"))?;
            context["state"] = json!({"project":authority.project,"run":state.run,"phase":state.phase,"outcome":state.outcome,
                "sequence":state.sequence,"task":task.id,"task_state":task.state});
            let (index, _, _) = self.resolve_task_mcp(auth, endpoint)?;
            self.prove_retention()?;
            let metadata = self.issue_metadata(index, issuer.as_ref())?;
            *issuer = Some(metadata.envelope.id.issuer.clone());
            context["controls"] = json!({"id":metadata.envelope.id,"attempt":endpoint.authority().attempt,
                "expected_generation":metadata.root_generation,"owner_epoch":metadata.envelope.owner_epoch,
                "expected_head":endpoint.head()});
            Ok(())
        })();
        context
    }
}
