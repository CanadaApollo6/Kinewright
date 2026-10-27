//! Director questions and runtime response identities.
//!
//! Rust adaptation of <https://github.com/pingdotgg/t3code> at
//! 5d91ab1117ad51ef1288a94717d85c70e7af09fe, apps/server/src/orchestration-v2/
//! DelegatedTaskQuestionReactor.ts, Orchestrator.ts and
//! apps/server/src/mcp/toolkits/thread/handlers.ts.
//! Notice: LICENSES/t3code-MIT.txt. Kinewright validates both wake boundaries,
//! retains answers by reference, and refuses consent until the H3 holder exists.
#![allow(clippy::missing_errors_doc)]
use super::orchestration_records::{
    AttemptState, ITEM_LIMIT, OrchestrationCommand, OrchestrationEnvelope, QuestionId,
    QuestionStatus, QuestionWake, Refusal, Role, RunOutcome, RunPhase, RunProjection,
    RuntimeRequest, TaskRecord, TaskState, TaskTurn,
};

pub fn live_turn(state: &RunProjection, turn: &TaskTurn) -> Result<(), Refusal> {
    live_identity(state, turn)?;
    if state
        .tasks
        .iter()
        .find(|task| task.id == turn.task)
        .is_none_or(|task| task.progress.turn_ended)
    {
        return Err(Refusal::InvalidState);
    }
    Ok(())
}

pub fn live_identity(state: &RunProjection, turn: &TaskTurn) -> Result<(), Refusal> {
    let task = state
        .tasks
        .iter()
        .find(|task| task.id == turn.task)
        .ok_or(Refusal::ScopeConflict)?;
    if state.outcome != RunOutcome::Pending
        || matches!(
            task.state,
            TaskState::Terminal(_)
                | TaskState::Cancelled
                | TaskState::Cancelling
                | TaskState::CleanupBlocked(_)
        )
        || task.progress.expired
        || task.progress.turn.as_ref() != Some(turn)
        || turn.expected_head != task.brief.scope.base.head
        || !state.attempts.iter().any(|attempt| {
            attempt.identity.task == turn.task
                && attempt.identity.attempt == turn.attempt
                && attempt.owner_epoch == turn.owner_epoch
                && !matches!(
                    attempt.state,
                    AttemptState::Stopping
                        | AttemptState::CleanupBlocked(_)
                        | AttemptState::Retired(_)
                )
        })
    {
        return Err(Refusal::InvalidState);
    }
    Ok(())
}

pub fn director(state: &RunProjection) -> Result<&TaskRecord, Refusal> {
    state
        .tasks
        .iter()
        .find(|task| {
            task.brief.role == Role::Director
                && !matches!(
                    task.state,
                    TaskState::Terminal(_)
                        | TaskState::Cancelled
                        | TaskState::Cancelling
                        | TaskState::CleanupBlocked(_)
                )
        })
        .ok_or(Refusal::InvalidState)
}

fn question_live(state: &RunProjection, id: &QuestionId) -> Result<usize, Refusal> {
    let index = state
        .requests
        .iter()
        .position(|request| matches!(request, RuntimeRequest::Question(q) if &q.id == id))
        .ok_or(Refusal::InvalidState)?;
    let RuntimeRequest::Question(question) = &state.requests[index] else {
        return Err(Refusal::InvalidState);
    };
    if question.status != QuestionStatus::Pending {
        return Err(Refusal::InvalidState);
    }
    live_identity(state, &question.origin)?;
    director(state)?;
    Ok(index)
}

/// Applied only inside the same proposed cut as the command receipt.
#[allow(
    clippy::too_many_lines,
    reason = "request transitions share one envelope boundary"
)]
pub fn apply(state: &mut RunProjection, envelope: &OrchestrationEnvelope) -> Result<bool, Refusal> {
    match &envelope.command {
        OrchestrationCommand::Request(RuntimeRequest::Approval(_) | RuntimeRequest::Secret(_)) => {
            return Err(Refusal::H3AttentionUnavailable);
        }
        OrchestrationCommand::Request(RuntimeRequest::Question(question)) => {
            live_turn(state, &question.origin)?;
            if envelope.authority.attempt.as_ref() != Some(&question.origin.attempt)
                || envelope.authority.owner_epoch != question.origin.owner_epoch
            {
                return Err(Refusal::ScopeConflict);
            }
            director(state)?;
            if question.task != question.origin.task
                || question.status != QuestionStatus::Pending
                || question.wake != QuestionWake::Pending
                || state.requests.len() >= ITEM_LIMIT
                || state.requests.iter().any(
                    |request| matches!(request, RuntimeRequest::Question(q) if q.id == question.id),
                )
            {
                return Err(Refusal::InvalidState);
            }
            state
                .tasks
                .iter_mut()
                .find(|task| task.id == question.task)
                .ok_or(Refusal::ScopeConflict)?
                .state = TaskState::WaitingForQuestion(question.id.clone());
            state
                .requests
                .push(RuntimeRequest::Question(question.clone()));
        }
        OrchestrationCommand::AdmitQuestionWake(id)
        | OrchestrationCommand::ExecuteQuestionWake(id) => {
            if matches!(state.phase, RunPhase::Paused | RunPhase::Interrupted) {
                return Err(Refusal::InvalidState);
            }
            // Repeated at execution because the task or request can die after admission.
            let index = question_live(state, id)?;
            let RuntimeRequest::Question(question) = &mut state.requests[index] else {
                return Err(Refusal::InvalidState);
            };
            match (&envelope.command, question.wake) {
                (OrchestrationCommand::AdmitQuestionWake(_), QuestionWake::Pending) => {
                    question.wake = QuestionWake::Admitted;
                }
                (OrchestrationCommand::ExecuteQuestionWake(_), QuestionWake::Admitted) => {
                    question.wake = QuestionWake::Executed;
                }
                _ => return Err(Refusal::InvalidState),
            }
        }
        OrchestrationCommand::AnswerQuestion(answer) => {
            let index = question_live(state, &answer.question)?;
            let director = director(state)?;
            let turn = director
                .progress
                .turn
                .as_ref()
                .ok_or(Refusal::ScopeConflict)?;
            live_turn(state, turn)?;
            if envelope.authority.attempt.as_ref() != Some(&turn.attempt)
                || envelope.authority.owner_epoch != turn.owner_epoch
            {
                return Err(Refusal::ScopeConflict);
            }
            let director_id = director.id.clone();
            if director.progress.decisions.len() >= ITEM_LIMIT {
                return Err(Refusal::Capacity);
            }
            state
                .tasks
                .iter_mut()
                .find(|task| task.id == director_id)
                .ok_or(Refusal::ScopeConflict)?
                .progress
                .decisions
                .push(answer.answer.clone());
            let RuntimeRequest::Question(question) = &mut state.requests[index] else {
                return Err(Refusal::InvalidState);
            };
            question.status = QuestionStatus::Answered(answer.answer.clone());
            question.wake = QuestionWake::Disposed;
            state
                .tasks
                .iter_mut()
                .find(|task| task.id == question.task)
                .ok_or(Refusal::ScopeConflict)?
                .state = TaskState::Running(question.origin.attempt.clone());
        }
        OrchestrationCommand::ExpireSession(session) => {
            if !state.tasks.iter().any(|task| {
                !task.progress.expired
                    && task
                        .progress
                        .turn
                        .as_ref()
                        .is_some_and(|turn| turn.session == *session)
                    && !matches!(
                        task.state,
                        TaskState::Terminal(_)
                            | TaskState::Cancelled
                            | TaskState::Cancelling
                            | TaskState::CleanupBlocked(_)
                    )
            }) {
                return Err(Refusal::InvalidState);
            }
            for request in &mut state.requests {
                if let RuntimeRequest::Question(q) = request
                    && q.origin.session == *session
                    && q.status == QuestionStatus::Pending
                {
                    q.status = QuestionStatus::Expired;
                    q.wake = QuestionWake::Disposed;
                }
            }
            for task in &mut state.tasks {
                if task
                    .progress
                    .turn
                    .as_ref()
                    .is_some_and(|turn| turn.session == *session)
                    && !matches!(
                        task.state,
                        TaskState::Terminal(_) | TaskState::Cancelled | TaskState::Cancelling
                    )
                {
                    task.progress.expired = true;
                    task.progress.turn_ended = false;
                    task.state = TaskState::Preparing;
                }
            }
            if state.phase != RunPhase::Finished {
                state.phase = RunPhase::Interrupted;
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// Runtime-only registry. Core and project records never hold a response callback.
#[derive(Default)]
pub struct LiveResponses {
    requests: std::collections::BTreeMap<String, kinewright_core::RuntimeRequest>,
}
impl LiveResponses {
    pub fn expire_stale(&mut self, current: Option<&kinewright_core::RuntimeRequestBinding>) {
        self.requests
            .retain(|_, request| current == Some(request.binding()));
    }
    #[must_use]
    pub fn contains(&self, request: &kinewright_core::RuntimeRequest) -> bool {
        self.requests.get(request.id()) == Some(request)
    }

    pub fn register(
        &mut self,
        request: &kinewright_core::RuntimeRequest,
    ) -> Result<(), kinewright_core::AgentError> {
        if self.requests.len() >= 64 || self.requests.contains_key(request.id()) {
            return Err(kinewright_core::AgentError::Protocol(
                "runtime_request_capacity_or_duplicate".into(),
            ));
        }
        self.requests
            .insert(request.id().to_owned(), request.clone());
        Ok(())
    }
    pub fn take(
        &mut self,
        request: &kinewright_core::RuntimeRequest,
        current: Option<&kinewright_core::RuntimeRequestBinding>,
    ) -> bool {
        self.requests.remove(request.id()).as_ref() == Some(request)
            && current == Some(request.binding())
    }
}
