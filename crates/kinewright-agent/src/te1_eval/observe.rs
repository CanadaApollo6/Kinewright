//! Observes the real server boundary. Fixture scheduling is transport-independent.
use super::{Call, Mcp, SendContext, Trace, error};
use rmcp::model::CallToolResult;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
pub(crate) struct Observations {
    pub trace: Arc<Mutex<Trace>>,
    contexts: Mutex<BTreeMap<(String, String), SendContext>>,
    case: u8,
    conflict: AtomicBool,
    injecting: AtomicBool,
}
impl Observations {
    pub fn new(case: u8, trace: Arc<Mutex<Trace>>) -> Self {
        Self {
            trace,
            case,
            contexts: Mutex::default(),
            conflict: AtomicBool::new(false),
            injecting: AtomicBool::new(false),
        }
    }
    pub fn bind(&self, context: SendContext) -> io::Result<()> {
        let key = (
            context.identity["task"]
                .as_str()
                .ok_or_else(|| error("task id missing"))?
                .into(),
            context.identity["attempt"]
                .as_str()
                .ok_or_else(|| error("attempt id missing"))?
                .into(),
        );
        let mut contexts = self.contexts.lock().map_err(error)?;
        if contexts.len() >= 32 {
            return Err(error("eval attempt capacity"));
        }
        contexts.insert(key, context);
        Ok(())
    }
    pub fn call(
        &self,
        task: &str,
        attempt: &str,
        name: &str,
        args: Value,
        result: &CallToolResult,
    ) -> io::Result<()> {
        let context = self
            .contexts
            .lock()
            .map_err(error)?
            .get(&(task.into(), attempt.into()))
            .cloned()
            .ok_or_else(|| error("unbound eval MCP observation"))?;
        let refused = result.is_error == Some(true);
        let reply = result
            .structured_content
            .clone()
            .unwrap_or(serde_json::to_value(result)?);
        let call = Call {
            task: json!(task),
            tool: name.into(),
            arguments: args,
            reply,
            refused,
            fixture: self.injecting.load(Ordering::Acquire),
        };
        {
            let mut trace = self.trace.lock().map_err(error)?;
            if trace.calls.len() >= 256 || serde_json::to_vec(&call)?.len() > 256 * 1024 {
                return Err(error("eval trace capacity"));
            }
            trace.calls.push(call);
        }
        if self.case == 6
            && context.brief["role"] != "director"
            && name == "prepare_edit_plan"
            && !refused
            && !self.conflict.swap(true, Ordering::AcqRel)
        {
            self.injecting.store(true, Ordering::Release);
            let result: io::Result<()> = (|| {
                let mcp = Mcp::connect(&context)?;
                mcp.edit(json!([{"op":"move_marker","marker":2,"to":94}]))?;
                let revision = mcp.revision()?;
                mcp.call("undo_task_edit", json!({"expected_revision":revision}))?;
                Ok(())
            })();
            self.injecting.store(false, Ordering::Release);
            if let Err(error) = &result {
                self.trace
                    .lock()
                    .map_err(super::error)?
                    .errors
                    .push(format!("fixture scheduling: {error}"));
            }
            result?;
        }
        Ok(())
    }
}
