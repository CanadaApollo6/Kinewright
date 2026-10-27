//! Bounded restart references, with explicit uncertain native injection.
//!
//! Rust adaptation of <https://github.com/pingdotgg/t3code> at
//! 5d91ab1117ad51ef1288a94717d85c70e7af09fe, apps/server/src/orchestration-v2/
//! ContextHandoffService.ts and ContextHandoffDelivery.ts.
//! Notice: LICENSES/t3code-MIT.txt. Kinewright keeps exact project references
//! instead of copying provider transcripts or silently retrying native injection.
#![allow(clippy::missing_errors_doc)]
use super::{
    bundle_records::ObjectRef,
    orchestration_records::{
        COMMAND_BYTES, DeliveryState, ITEM_LIMIT, QuestionStatus, Refusal, RunProjection,
        RuntimeRequest, TaskBrief, TaskId, encoded, validate_reference,
    },
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemainingLimits {
    pub sends: u32,
    pub elapsed_millis: u64,
    pub token_ceiling: Option<u64>,
    pub repairs: u32,
    pub rate_waits: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestartBrief {
    pub task: TaskId,
    pub assignment: TaskBrief,
    pub feedback: Vec<ObjectRef>,
    pub decisions: Vec<ObjectRef>,
    pub findings_and_responses: Vec<ObjectRef>,
    pub pending_results: Vec<ObjectRef>,
    pub remaining: RemainingLimits,
}
impl RestartBrief {
    pub fn build(
        state: &RunProjection,
        task: &TaskId,
        remaining: RemainingLimits,
    ) -> Result<Self, Refusal> {
        let task = state
            .tasks
            .iter()
            .find(|candidate| &candidate.id == task)
            .ok_or(Refusal::ScopeConflict)?;
        let mut findings = Vec::new();
        for request in &state.requests {
            if let RuntimeRequest::Question(q) = request {
                if q.task != task.id
                    && task.brief.role != super::orchestration_records::Role::Director
                {
                    continue;
                }
                findings.push(q.prompt.clone());
                if let QuestionStatus::Answered(answer) = &q.status {
                    findings.push(answer.clone());
                }
            }
        }
        let brief =
            Self {
                task: task.id.clone(),
                assignment: task.brief.clone(),
                feedback: task.progress.feedback.clone(),
                decisions: task.progress.decisions.clone(),
                findings_and_responses: findings,
                pending_results: state
                    .deliveries
                    .iter()
                    .filter(|d| {
                        state.tasks.iter().any(|child| {
                            child.id == d.task && child.parent.as_ref() == Some(&task.id)
                        }) && !matches!(
                            d.state,
                            DeliveryState::Observed(_) | DeliveryState::Disposed
                        )
                    })
                    .map(|d| d.result.clone())
                    .collect(),
                remaining,
            };
        for list in [
            &brief.feedback,
            &brief.decisions,
            &brief.findings_and_responses,
            &brief.pending_results,
        ] {
            if list.len() > ITEM_LIMIT {
                return Err(Refusal::Capacity);
            }
            for reference in list {
                validate_reference(reference)?;
            }
        }
        encoded(&brief, COMMAND_BYTES)?;
        Ok(brief)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeInjection {
    Fresh,
    Pending,
    Accepted,
    Unknown,
}
impl NativeInjection {
    pub fn begin(&mut self) -> Result<(), Refusal> {
        if *self != Self::Fresh {
            return Err(Refusal::InvalidState);
        }
        *self = Self::Pending;
        Ok(())
    }
    pub fn finish(&mut self, accepted: bool) -> Result<(), Refusal> {
        if *self != Self::Pending {
            return Err(Refusal::InvalidState);
        }
        *self = if accepted {
            Self::Accepted
        } else {
            Self::Unknown
        };
        Ok(())
    }
}
