//! Ordered effect decisions. The host commits each claim before dispatch.
//!
//! Rust adaptation of <https://github.com/pingdotgg/t3code> at
//! 5d91ab1117ad51ef1288a94717d85c70e7af09fe, apps/server/src/orchestration-v2/
//! {EffectOutbox,EffectWorker}.ts. Notice: LICENSES/t3code-MIT.txt.
//! Kinewright retains unknown process-bound claims until cleanup/reinspection;
//! no elapsed lease or disconnected client grants replacement execution.
#![allow(clippy::missing_errors_doc)]

use super::{
    bundle_records::BranchId,
    orchestration_records::{
        AttemptId, EffectId, EffectRecord, EffectSafety, EffectState, Refusal, RunProjection,
    },
};

/// A result wake must deliver every record with `delivery.wake == effect.id`.
/// The effect's `payload` alone does not include all coalesced sibling results.
#[must_use]
pub fn next_effect<'a>(state: &'a RunProjection, lane: &BranchId) -> Option<&'a EffectRecord> {
    if state.outcome != super::orchestration_records::RunOutcome::Pending
        || matches!(
            state.phase,
            super::orchestration_records::RunPhase::Paused
                | super::orchestration_records::RunPhase::Interrupted
        )
    {
        return None;
    }
    let first = state
        .effects
        .iter()
        .filter(|effect| {
            &effect.lane == lane
                && !matches!(effect.state, EffectState::Settled | EffectState::Disposed)
        })
        .min_by_key(|effect| effect.order)?;
    (first.state == EffectState::Pending).then_some(first)
}

/// Return a proposal only. Persist it through H1 before doing external work.
pub fn claim(
    state: &RunProjection,
    id: &EffectId,
    attempt: &AttemptId,
) -> Result<RunProjection, Refusal> {
    let effect = state
        .effects
        .iter()
        .find(|effect| &effect.id == id)
        .ok_or(Refusal::Unknown)?;
    if next_effect(state, &effect.lane).map(|effect| &effect.id) != Some(id) {
        return Err(Refusal::InvalidState);
    }
    let mut next = state.clone();
    next.effects
        .iter_mut()
        .find(|effect| &effect.id == id)
        .ok_or(Refusal::Unknown)?
        .state = EffectState::Claimed(attempt.clone());
    Ok(next)
}

/// An uncertain external outcome blocks the lane. Safe retries require an
/// explicit reconciliation decision, not a timer-based claim expiry.
pub fn outcome_unknown(
    state: &RunProjection,
    id: &EffectId,
    attempt: &AttemptId,
) -> Result<RunProjection, Refusal> {
    let mut next = state.clone();
    let effect = next
        .effects
        .iter_mut()
        .find(|effect| &effect.id == id)
        .ok_or(Refusal::Unknown)?;
    if effect.state != EffectState::Claimed(attempt.clone()) {
        return Err(Refusal::ScopeConflict);
    }
    effect.state = EffectState::OutcomeUnknown;
    Ok(next)
}

pub fn retry_reconciled_safe(
    state: &RunProjection,
    id: &EffectId,
) -> Result<RunProjection, Refusal> {
    let mut next = state.clone();
    let effect = next
        .effects
        .iter_mut()
        .find(|effect| &effect.id == id)
        .ok_or(Refusal::Unknown)?;
    if effect.state != EffectState::OutcomeUnknown || effect.safety != EffectSafety::ReplaySafe {
        return Err(Refusal::InvalidState);
    }
    effect.state = EffectState::Pending;
    Ok(next)
}
