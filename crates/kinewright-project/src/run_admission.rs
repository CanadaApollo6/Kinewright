//! One store-wide escrow protects the terminal publications of admitted native work.
#![allow(clippy::missing_errors_doc)]
use crate::{bundle::BundleError, storage_budget::StorageLimits, store::ProjectStore};
use std::{cell::RefCell, collections::BTreeMap, marker::PhantomData, rc::Rc};
pub const TASKS: usize = 8;
pub const SENDS: usize = 128;
pub const ATTEMPTS: usize = 24;
pub const USAGE_SAMPLES: usize = 512;
pub const ACCOUNTING_BYTES: usize = 1 << 20;
pub const ORDINARY_CUTS: usize = 1024;
pub const TERMINAL_CUTS: u64 = 32;
pub const MINIMAL_CUTS: u64 = 16;
pub const TRANSITION_BYTES: usize = 192 << 10;
pub const TERMINAL_RECEIPT_BYTES: usize = 6 << 10;
pub const TERMINAL_TRANSITION_BYTES: usize = 512 << 10;
pub const HISTORY_BYTES: u64 = 1 << 20;
pub const RECORD_BYTES: u64 = 8 << 20;
pub const DECODED_BYTES: u64 = 32 << 20;
pub const OBJECTS: u64 = 256;
pub const SPECIALIST_BASES: u64 = TASKS as u64 * 2;
pub const METADATA: u64 = 256 << 20;
pub const OWNED: u64 = 8 << 20;
// At most two branches per task: retained base/delta, retained capture and
// staged decoded history (each bounded to 1 MiB), plus 16 MiB for serial
// replay/old-capture overlap. Ordinary writes cannot spend this allowance.
pub const LIVE_HISTORY: u64 = TASKS as u64 * 2 * 3 * HISTORY_BYTES + (16 << 20);
pub const GRAPH_LEVELS: u64 = 8;
pub const REASON: &str = "Run limit reached. Stop this run to finish cleanup.";
thread_local! { static READ_HEADROOM: RefCell<BTreeMap<usize,usize>> = const { RefCell::new(BTreeMap::new()) }; }
#[derive(Clone)]
pub(crate) struct TerminalAdmission {
    key: usize,
    _lifetime: std::sync::Arc<()>,
    publication: PublicationIdentity,
}
#[derive(Clone)]
enum PublicationIdentity {
    Cut {
        run: crate::orchestration_records::RunId,
        command: crate::receipts::CommandId,
        digest: String,
    },
    Accounting {
        run: crate::orchestration_records::RunId,
        revision: u64,
        digest: String,
        root: Option<crate::bundle_records::ObjectRef>,
    },
}
impl TerminalAdmission {
    pub(crate) fn matches_store(&self, store: &ProjectStore) -> bool {
        self.key == store.admission_key()
    }
    pub(crate) fn matches_update(&self, update: &ValidatedRunUpdate) -> bool {
        matches!(&self.publication, PublicationIdentity::Cut { run, command, digest }
            if run == &update.projection().run && command == &update.envelope().id && digest == &update.receipt().cut_digest)
    }
    pub(crate) fn matches_pending(&self, bundle: &crate::bundle::ProjectBundle) -> bool {
        match &self.publication {
            PublicationIdentity::Cut { command, .. } => bundle
                .pending_command
                .as_ref()
                .is_some_and(|pending| &pending.id == command),
            PublicationIdentity::Accounting {
                root: Some(root), ..
            } => {
                bundle.pending_command.is_none()
                    && bundle
                        .root
                        .as_ref()
                        .and_then(|current| current.run_index_ref.as_ref())
                        == Some(root)
            }
            PublicationIdentity::Accounting { root: None, .. } => false,
        }
    }
    pub(crate) fn accounting_root(&mut self, root: crate::bundle_records::ObjectRef) {
        if let PublicationIdentity::Accounting { root: current, .. } = &mut self.publication {
            *current = Some(root);
        }
    }
    pub(crate) fn matches_accounting(
        &self,
        run: &crate::orchestration_records::RunId,
        next: &crate::orchestration_accounting::AccountingRecord,
    ) -> bool {
        matches!(&self.publication, PublicationIdentity::Accounting { run: expected, revision, digest, .. }
            if expected == run && *revision == next.revision
                && crate::orchestration_accounting::encode(next, ACCOUNTING_BYTES).is_ok_and(|bytes| kinewright_media::sha256_bytes(&bytes) == *digest))
    }
}
pub(crate) struct RunReadHeadroom {
    key: usize,
    _lifetime: std::sync::Arc<()>,
    _thread: PhantomData<Rc<()>>,
}
impl Drop for RunReadHeadroom {
    fn drop(&mut self) {
        READ_HEADROOM.with_borrow_mut(|entries| {
            if let Some(depth) = entries.get_mut(&self.key) {
                *depth -= 1;
                if *depth == 0 {
                    entries.remove(&self.key);
                }
            }
        });
    }
}
pub(crate) fn reading(store: &ProjectStore) -> RunReadHeadroom {
    let key = store.admission_key();
    READ_HEADROOM.with_borrow_mut(|entries| *entries.entry(key).or_default() += 1);
    RunReadHeadroom {
        key,
        _lifetime: store.admission_lifetime(),
        _thread: PhantomData,
    }
}
pub(crate) fn is_reading(key: usize) -> bool {
    READ_HEADROOM.with_borrow(|entries| entries.contains_key(&key))
}
fn mint_terminal(store: &ProjectStore, publication: PublicationIdentity) -> TerminalAdmission {
    TerminalAdmission {
        key: store.admission_key(),
        _lifetime: store.admission_lifetime(),
        publication,
    }
}
pub(crate) fn ordinary(raw: &StorageLimits) -> StorageLimits {
    let mut limits = raw.clone();
    limits.metadata_bytes = limits.metadata_bytes.saturating_sub(METADATA);
    limits.objects = limits.objects.saturating_sub(OBJECTS);
    limits.owned_bytes = limits.owned_bytes.saturating_sub(OWNED);
    limits.copy_overlap_bytes = limits.copy_overlap_bytes.saturating_sub(OWNED);
    limits.receipts = limits.receipts.saturating_sub(TERMINAL_CUTS);
    limits.pending_commands = limits.pending_commands.saturating_sub(1);
    limits.issuers = limits.issuers.saturating_sub(4);
    limits.versions = limits.versions.saturating_sub(1);
    limits.control_bytes = limits.control_bytes.min(RECORD_BYTES);
    limits.branch_history_bytes = limits.branch_history_bytes.min(HISTORY_BYTES);
    limits.live_history_bytes = limits.live_history_bytes.saturating_sub(LIVE_HISTORY);
    limits
}
impl ProjectStore {
    pub(crate) fn is_terminal_publication(&self) -> bool {
        self.publication
            .as_ref()
            .is_some_and(|token| token.matches_store(self))
    }
    pub(crate) fn is_person_publication(&self) -> bool {
        self.person_publication
            .as_ref()
            .is_some_and(|token| token.matches_store(self))
    }
    pub(crate) fn is_project_publication(&self) -> bool {
        self.is_terminal_publication()
            || self.is_person_publication()
            || self.is_cleanup_publication()
    }
    pub(crate) fn specialist_version_limit(&self) -> u64 {
        if self.run_budget_active() && !self.is_project_publication() {
            self.limits().versions.saturating_sub(SPECIALIST_BASES)
        } else {
            self.limits().versions
        }
    }
    /// Return an owned H1 record; headroom cannot escape as a retained reservation.
    pub fn read_run_record(
        &self,
        reference: &crate::bundle_records::ObjectRef,
    ) -> Result<Box<crate::orchestration_records::OrchestrationRecord>, BundleError> {
        let _reading = reading(self);
        Ok(Box::new(
            (*self.read_record::<crate::orchestration_records::OrchestrationRecord>(reference)?)
                .clone(),
        ))
    }
    pub(crate) fn admission_limits(&self) -> StorageLimits {
        if self.run_budget_active() && !self.is_project_publication() {
            ordinary(self.limits())
        } else {
            self.limits().clone()
        }
    }
    pub(crate) fn run_error(&self, error: BundleError) -> BundleError {
        if self.run_budget_active()
            && !self.is_person_publication()
            && !self.is_cleanup_publication()
            && matches!(error, BundleError::Capacity(_))
        {
            self.note_run_limit();
            BundleError::RunLimit
        } else {
            error
        }
    }
    pub(crate) fn admit_run_write(&self) -> Result<(), BundleError> {
        if self.run_budget_active() && !self.is_project_publication() && self.has_run_limit() {
            return Err(BundleError::RunLimit);
        }
        Ok(())
    }
    #[must_use]
    pub fn run_limit_reached(&self) -> bool {
        self.has_run_limit()
    }
}

struct PublicationBundle<'a> {
    bundle: &'a mut crate::bundle::ProjectBundle,
    ordinary: Option<ProjectStore>,
}
impl std::ops::Deref for PublicationBundle<'_> {
    type Target = crate::bundle::ProjectBundle;
    fn deref(&self) -> &Self::Target {
        self.bundle
    }
}
impl std::ops::DerefMut for PublicationBundle<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.bundle
    }
}
impl Drop for PublicationBundle<'_> {
    fn drop(&mut self) {
        self.bundle.store = self.ordinary.take().expect("publication store owner");
    }
}
impl crate::bundle::ProjectBundle {
    /// Manual publishers keep project admission for their complete transaction,
    /// including reconciliation. The ordinary run store is restored on exit.
    pub(crate) fn with_person_publication<T>(
        &mut self,
        publish: impl FnOnce(&mut Self) -> Result<T, BundleError>,
    ) -> Result<T, BundleError> {
        let _reading = reading(&self.store);
        let writer = self.store.person_publication_store();
        let ordinary = std::mem::replace(&mut self.store, writer);
        let mut publication = PublicationBundle {
            bundle: self,
            ordinary: Some(ordinary),
        };
        publish(&mut publication)
    }
    pub(crate) fn with_publication_admission<T>(
        &mut self,
        admission: Option<TerminalAdmission>,
        publish: impl FnOnce(&mut Self) -> Result<T, BundleError>,
    ) -> Result<T, BundleError> {
        let Some(admission) = admission else {
            return publish(self);
        };
        let _reading = reading(&self.store);
        let writer = self.store.publication_store(admission)?;
        let ordinary = std::mem::replace(&mut self.store, writer);
        let mut publication = PublicationBundle {
            bundle: self,
            ordinary: Some(ordinary),
        };
        publish(&mut publication)
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunBudget {
    pub active: bool,
    pub exhausted: bool,
    pub ordinary: usize,
    pub terminal: usize,
    pub attempts: usize,
}

use crate::{
    orchestration_commit::{CommandDecision, ValidatedRunUpdate},
    orchestration_records::{
        AttemptState, OrchestrationCommand, RunAction, RunCleanup, RunOutcome, RunPhase,
        RunProjection,
    },
};

/// Terminal authority for one terminal cut (C5 fix 13, narrowed by fix 14).
///
/// A run with no admitted budget, or one that has used its 32 terminal cuts,
/// never gets it. While the budget is active, fix 2's rule holds unchanged: the
/// cut is bound to its one publication. Once the run has finished and its budget
/// is inactive, only validated archival progress gets a terminal cut, so a
/// finished run can still archive its retired attempts after Stop but cannot
/// spend its reserve on any other change. `archival` is evaluated only then.
fn terminal_authority(
    admitted: bool,
    budget: &RunBudget,
    finished: bool,
    archival: impl FnOnce() -> Result<bool, BundleError>,
) -> Result<(), BundleError> {
    if !admitted || budget.terminal as u64 >= TERMINAL_CUTS {
        return Err(BundleError::RunLimit);
    }
    if budget.active || (finished && archival()?) {
        return Ok(());
    }
    Err(BundleError::RunLimit)
}

/// C5 fix 14: a recorded refusal on a finished run changes nothing but the
/// receipt's place in the history.
fn refusal_only(before: &RunProjection, update: &ValidatedRunUpdate) -> bool {
    let after = update.projection();
    update.receipt().decision != CommandDecision::Applied
        && after.run == before.run
        && after.phase == before.phase
        && after.outcome == before.outcome
        && after.wait == before.wait
        && after.cleanup == before.cleanup
        && after.tasks == before.tasks
        && after.effects == before.effects
        && after.attempts == before.attempts
        && after.deliveries == before.deliveries
        && after.requests == before.requests
        && after.results == before.results
        && after.usage == before.usage
}

pub(crate) fn terminal_progress(before: &RunProjection, update: &ValidatedRunUpdate) -> bool {
    let after = update.projection();
    if update.receipt().decision != CommandDecision::Applied {
        return false;
    }
    if after.tasks.len() != before.tasks.len()
        || after
            .tasks
            .iter()
            .any(|a| !before.tasks.iter().any(|b| a.id == b.id))
        || after.effects.len() > before.effects.len()
        || after
            .effects
            .iter()
            .any(|a| !before.effects.iter().any(|b| a.id == b.id))
        || after
            .attempts
            .iter()
            .any(|a| !before.attempts.iter().any(|b| a.identity == b.identity))
    {
        return false;
    }
    if matches!(&update.envelope().command, OrchestrationCommand::ControlRun(intent) if intent.action==RunAction::Stop)
        && before.outcome == RunOutcome::Pending
        && after.phase == RunPhase::Finished
        && matches!(after.outcome, RunOutcome::Cancelled(_))
    {
        return true;
    }
    let live = |s: &RunProjection| {
        s.attempts
            .iter()
            .filter(|a| !matches!(a.state, AttemptState::Retired(_)))
            .count()
    };
    live(after) < live(before)
        || after.attempts.len() < before.attempts.len()
        || before.cleanup != RunCleanup::Settled && after.cleanup == RunCleanup::Settled
        || before.phase != RunPhase::Interrupted && after.phase == RunPhase::Interrupted
}
impl crate::bundle::ProjectBundle {
    pub(crate) fn accounting_recovery_admission(
        &self,
        state: &RunProjection,
        before: &crate::orchestration_accounting::AccountingRecord,
        after: &crate::orchestration_accounting::AccountingRecord,
    ) -> Result<Option<TerminalAdmission>, BundleError> {
        after
            .validate_successor(before)
            .map_err(|_| BundleError::HistoryChanged)?;
        let bytes = crate::orchestration_accounting::encode(after, ACCOUNTING_BYTES)
            .map_err(|_| BundleError::RunLimit)?;
        let _reading = reading(&self.store);
        let index = self.run_index()?;
        let budget = index
            .budgets
            .get(&state.run)
            .ok_or(BundleError::HistoryChanged)?;
        if !budget.active
            || !(budget.exhausted
                || self.store.run_limit_reached()
                || state.outcome != RunOutcome::Pending)
        {
            return Ok(None);
        }
        // Recover only clock/usage for already charged work. New work and
        // growing wait/repair identities continue to use ordinary admission.
        if before.tasks.len() != after.tasks.len()
            || before.tasks.iter().zip(&after.tasks).any(|(old, new)| {
                old.task != new.task
                    || old.token_limit != new.token_limit
                    || old.sends != new.sends
                    || old.providers != new.providers
                    || old.repairs != new.repairs
                    || old.last_failure != new.last_failure
                    || old.repeated_failures != new.repeated_failures
                    || old.wait_reason != new.wait_reason
                    || old.wait_until_unix_ms != new.wait_until_unix_ms
            })
        {
            return Err(BundleError::RunLimit);
        }
        Ok(Some(mint_terminal(
            &self.store,
            PublicationIdentity::Accounting {
                run: state.run.clone(),
                revision: after.revision,
                digest: kinewright_media::sha256_bytes(&bytes),
                root: None,
            },
        )))
    }
    fn check_run_start_capacity(
        &self,
        index: &crate::studio_records::RunIndex,
        incoming_issuer: bool,
    ) -> Result<(), BundleError> {
        let raw = self.store.limits();
        let limits = ordinary(raw);
        let usage = self.store.usage()?;
        if raw.control_bytes < RECORD_BYTES + (1 << 20)
            || raw.decoded_record_bytes < DECODED_BYTES + (8 << 20)
            || raw.command_bytes < 256 << 10
            || raw.branch_history_bytes < HISTORY_BYTES
            || raw.graph_depth < 2 * GRAPH_LEVELS
            || limits.pending_commands == 0
            || self
                .evidence
                .iter()
                .filter(|reference| {
                    reference.record_kind == crate::bundle_records::RecordKind::SpecialistBase
                })
                .count() as u64
                + SPECIALIST_BASES
                > raw.versions
            || limits.live_history_bytes < HISTORY_BYTES
            || (index.runs.len() + index.orchestration.len()) as u64 + 1 > limits.versions
            || self.issuers.len() as u64 + u64::from(incoming_issuer) + 4 > limits.issuers
            || self.receipts.detailed.len() as u64
                + self.receipts.retired.len() as u64
                + MINIMAL_CUTS
                > limits.receipts
            || usage.objects + 64 > limits.objects
            || usage.metadata_bytes
                + usage.decoded_bytes.saturating_sub(usage.decoder_bytes)
                + (96 << 20)
                > limits.metadata_bytes
            || usage.owned_bytes + HISTORY_BYTES > limits.owned_bytes
            || usage.live_history_bytes > limits.live_history_bytes
        {
            return Err(BundleError::RunAdmission(
                "Project storage is too full to start a run safely.",
            ));
        }
        Ok(())
    }
    pub(crate) fn advance_run_budget(
        &self,
        index: &mut crate::studio_records::RunIndex,
        transition: &ValidatedRunUpdate,
    ) -> Result<(), BundleError> {
        let run = &transition.projection().run;
        let before = self.orchestration_projection(run)?;
        let reserved = before
            .as_ref()
            .is_some_and(|before| crate::run_admission::terminal_progress(before, transition));
        let budget = index.budgets.entry(run.clone()).or_default();
        budget.attempts +=
            before
                .as_ref()
                .map_or(transition.projection().attempts.len(), |before| {
                    transition
                        .projection()
                        .attempts
                        .iter()
                        .filter(|a| !before.attempts.iter().any(|b| a.identity == b.identity))
                        .count()
                });
        let escrow = reserved
            && (budget.exhausted
                || self.store.run_limit_reached()
                || budget.ordinary >= crate::run_admission::ORDINARY_CUTS
                || before.as_ref().is_some_and(|s| {
                    s.outcome != crate::orchestration_records::RunOutcome::Pending
                }));
        if escrow {
            budget.terminal += 1;
        } else {
            budget.ordinary += 1;
        }
        if budget.terminal as u64 > crate::run_admission::TERMINAL_CUTS {
            return Err(BundleError::RunLimit);
        }
        budget.exhausted |= self.store.run_limit_reached()
            || budget.ordinary >= crate::run_admission::ORDINARY_CUTS;
        budget.active = transition.projection().outcome
            == crate::orchestration_records::RunOutcome::Pending
            || transition.projection().cleanup != crate::orchestration_records::RunCleanup::Settled;
        Ok(())
    }
    /// C5 fix 14: which cuts a run may make, by its state before the update.
    /// A first cut and a pending or still-cleaning run are unchanged. A
    /// terminal cut needs `terminal_authority`. A finished run whose budget is
    /// inactive may otherwise only record a refusal that changes nothing.
    fn cut_authority(
        &self,
        index: &crate::studio_records::RunIndex,
        before: Option<&RunProjection>,
        update: &ValidatedRunUpdate,
        terminal: bool,
    ) -> Result<(), BundleError> {
        let Some(before) = before else {
            return Ok(());
        };
        let budget = index.budgets.get(&before.run);
        let finished = before.outcome != RunOutcome::Pending;
        if terminal {
            return terminal_authority(
                budget.is_some(),
                &budget.cloned().unwrap_or_default(),
                finished,
                || self.archival_progress(before, update.projection()),
            );
        }
        if finished && budget.is_some_and(|b| !b.active) && !refusal_only(before, update) {
            return Err(BundleError::RunLimit);
        }
        Ok(())
    }
    /// C5 fix 14: the update only archives retired attempts. Outcome, phase,
    /// wait, cleanup, tasks, effects, deliveries, requests and results are
    /// unchanged; the only attempts removed are retired; and the usage list
    /// changes only by replacing the attempt archive with a new sealed archive
    /// that holds the previous archive's attempts followed by exactly the
    /// removed attempts. Accounting references in usage are therefore unchanged.
    fn archival_progress(
        &self,
        before: &RunProjection,
        after: &RunProjection,
    ) -> Result<bool, BundleError> {
        use crate::orchestration_records::OrchestrationPayload;
        if after.run != before.run
            || after.phase != before.phase
            || after.outcome != before.outcome
            || after.wait != before.wait
            || after.cleanup != before.cleanup
            || after.tasks != before.tasks
            || after.effects != before.effects
            || after.deliveries != before.deliveries
            || after.requests != before.requests
            || after.results != before.results
        {
            return Ok(false);
        }
        let removed = before
            .attempts
            .iter()
            .filter(|a| !after.attempts.contains(a))
            .cloned()
            .collect::<Vec<_>>();
        if removed.is_empty()
            || removed
                .iter()
                .any(|a| !matches!(a.state, AttemptState::Retired(_)))
            || !before
                .attempts
                .iter()
                .filter(|a| after.attempts.contains(a))
                .eq(after.attempts.iter())
        {
            return Ok(false);
        }
        let archive = |reference| -> Result<_, BundleError> {
            Ok(match self.store.read_run_record(reference)?.payload {
                OrchestrationPayload::AttemptArchive(archive) => Some(archive),
                _ => None,
            })
        };
        let mut previous = None;
        for reference in before.usage.iter().rev() {
            if let Some(found) = archive(reference)? {
                previous = Some((reference, found));
                break;
            }
        }
        let Some((sealed, rest)) = after.usage.split_last() else {
            return Ok(false);
        };
        let kept = before
            .usage
            .iter()
            .filter(|r| previous.as_ref().is_none_or(|(p, _)| p != r));
        if before.usage.contains(sealed) || !kept.eq(rest.iter()) {
            return Ok(false);
        }
        let Some(next) = archive(sealed)? else {
            return Ok(false);
        };
        let mut expected = previous.map(|(_, a)| a.attempts).unwrap_or_default();
        expected.extend(removed);
        Ok(next.run == before.run && next.attempts == expected)
    }
    pub(crate) fn run_admission(
        &mut self,
        update: &ValidatedRunUpdate,
        arm: bool,
    ) -> Result<Option<TerminalAdmission>, BundleError> {
        let reserved = self.store.is_terminal_publication();
        let reading = reading(&self.store);
        let run = &update.projection().run;
        let index = self.run_index()?;
        let budget = index.budgets.get(run).cloned().unwrap_or_default();
        let before = self.orchestration_projection(run)?;
        let terminal = before
            .as_ref()
            .is_some_and(|before| terminal_progress(before, update));
        if let Some(cleanup) = &self.store.cleanup_publication {
            cleanup.check_transition(update, terminal)?;
        }
        self.cut_authority(&index, before.as_ref(), update, terminal)?;
        if terminal {
            crate::orchestration_accounting::encode(update.receipt(), TERMINAL_RECEIPT_BYTES)
                .map_err(|_| BundleError::RunLimit)?;
            crate::orchestration_accounting::encode(update, TERMINAL_TRANSITION_BYTES)
                .map_err(|_| BundleError::RunLimit)?;
            return Ok(arm.then(|| {
                mint_terminal(
                    &self.store,
                    PublicationIdentity::Cut {
                        run: run.clone(),
                        command: update.envelope().id.clone(),
                        digest: update.receipt().cut_digest.clone(),
                    },
                )
            }));
        }
        if before.is_none() {
            if index.budgets.values().any(|b| b.active) {
                return Err(BundleError::RunLimit);
            }
            if !reserved {
                self.check_run_start_capacity(
                    &index,
                    !self.issuers.contains_key(&update.envelope().id.issuer),
                )?;
            }
            let director = update
                .projection()
                .tasks
                .first()
                .ok_or(BundleError::RunLimit)?;
            let checkpoint = self
                .store
                .read_record::<crate::bundle_records::BranchCheckpoint>(
                    &director.brief.scope.base.checkpoint,
                )?;
            if checkpoint.history_ref.byte_length > HISTORY_BYTES {
                return Err(BundleError::RunAdmission(
                    "Task history is too large to start this run safely.",
                ));
            }
            let history = self
                .store
                .read_record::<kinewright_core::HistorySnapshot>(&checkpoint.history_ref)?;
            let bytes =
                serde_json::to_vec(&*history).map_err(|e| BundleError::Corrupt(e.to_string()))?;
            if crate::store::preflight_record(&bytes, self.store.limits())? > HISTORY_BYTES {
                return Err(BundleError::RunAdmission(
                    "Task history is too large to start this run safely.",
                ));
            }
            if arm {
                self.store.set_run_budget(true);
            }
        }
        drop(index);
        drop(reading);
        let added = before
            .as_ref()
            .map_or(update.projection().attempts.len(), |before| {
                update
                    .projection()
                    .attempts
                    .iter()
                    .filter(|a| !before.attempts.iter().any(|b| a.identity == b.identity))
                    .count()
            });
        if budget.attempts + added > ATTEMPTS {
            self.store.note_run_limit();
            return Err(BundleError::RunLimit);
        }
        if budget.ordinary >= ORDINARY_CUTS
            || self.store.run_limit_reached()
            || crate::orchestration_accounting::encode(update, TRANSITION_BYTES).is_err()
            || (!reserved && {
                let usage = self.store.usage()?;
                usage.metadata_bytes
                    + usage.decoded_bytes.saturating_sub(usage.decoder_bytes)
                    + (96 << 20)
                    > ordinary(self.store.limits()).metadata_bytes
            })
        {
            self.store.note_run_limit();
            return Err(BundleError::RunLimit);
        }
        Ok(None)
    }
    pub(crate) fn refresh_run_budget(&self) -> Result<(), BundleError> {
        let _reading = reading(&self.store);
        let index = self.run_index()?;
        let active = index.budgets.values().any(|b| b.active);
        self.store.set_run_budget(active);
        let limits = ordinary(self.store.limits());
        let usage = self.store.usage()?;
        if active
            && (self.receipts.detailed.len() as u64 + self.receipts.retired.len() as u64
                >= limits.receipts
                || usage.objects > limits.objects
                || usage.metadata_bytes > limits.metadata_bytes
                || usage.owned_bytes > limits.owned_bytes
                || usage.live_history_bytes > limits.live_history_bytes)
        {
            self.store.note_run_limit();
        }
        Ok(())
    }
}

impl crate::editing_session::Ja1EditingSession {
    #[cfg(any(test, feature = "test-util"))]
    pub fn run_budget_probe(
        &self,
        run: &crate::orchestration_records::RunId,
    ) -> Result<RunBudget, BundleError> {
        let _reading = reading(self.store()?);
        self.active_bundle()?
            .run_index()?
            .budgets
            .get(run)
            .cloned()
            .ok_or(BundleError::HistoryChanged)
    }
    /// Check before staging a task freeze, using the same publication policy.
    pub fn run_transition_admission(
        &mut self,
        update: &ValidatedRunUpdate,
    ) -> Result<(), BundleError> {
        self.active_bundle_mut()?
            .run_admission(update, false)
            .map(|_| ())
    }
    #[cfg(target_os = "linux")]
    pub fn check_run_send_budget(
        &self,
        record: &crate::orchestration_accounting::AccountingRecord,
    ) -> Result<(), BundleError> {
        if record.tasks.iter().map(|t| t.sends.len()).sum::<usize>() >= SENDS {
            self.store()?.note_run_limit();
            return Err(BundleError::RunLimit);
        }
        Ok(())
    }
    pub fn run_publication_status(
        &self,
        run: &crate::orchestration_records::RunId,
    ) -> Result<Option<String>, BundleError> {
        let _reading = reading(self.store()?);
        let index = self.active_bundle()?.run_index()?;
        let exhausted = index
            .budgets
            .get(run)
            .is_some_and(|b| b.exhausted || b.ordinary >= ORDINARY_CUTS);
        Ok((exhausted || self.store()?.run_limit_reached()).then(|| REASON.into()))
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one table compares all three ordinary completion updates"
    )]
    fn te13_fix1_finish_freeze_publish_are_never_terminal_progress() {
        use crate::{
            bundle_records::{ObjectRef, RecordKind},
            orchestration_commit::{CommandDecision, ValidatedRunUpdate},
            orchestration_records::*,
            receipts::CommandId,
        };
        fn id<T: serde::de::DeserializeOwned>() -> T {
            serde_json::from_value(serde_json::json!("1".repeat(32))).unwrap()
        }
        let reference = ObjectRef {
            sha256: "0".repeat(64),
            byte_length: 0,
            record_kind: RecordKind::OwnedEvidence,
        };
        let state = RunProjection {
            edit_consent: None,
            edit_approvals: Vec::new(),
            edit_applications: vec![],
            run: id(),
            sequence: 0,
            control_revision: id(),
            phase: RunPhase::Running,
            outcome: RunOutcome::Pending,
            wait: RunWait::None,
            cleanup: RunCleanup::Settled,
            tasks: vec![],
            effects: vec![],
            attempts: vec![],
            deliveries: vec![],
            requests: vec![],
            results: vec![],
            usage: vec![],
        };
        for command in [
            OrchestrationCommand::FinishTask(FinishTask {
                task: id(),
                expected_revision: kinewright_core::TimelineRevision(0),
            }),
            OrchestrationCommand::FreezeTask(id()),
            OrchestrationCommand::PublishResult(Box::new(PublishResult {
                task: id(),
                result: TaskResult {
                    outcome: AttemptOutcome::Completed,
                    base: None,
                    head: None,
                    delta_ref: None,
                    artifacts: vec![],
                    evidence: vec![],
                    usage_ref: reference.clone(),
                    explanation: reference.clone(),
                },
            })),
        ] {
            let mut envelope = OrchestrationEnvelope {
                id: CommandId {
                    issuer: id(),
                    sequence: 1,
                },
                digest: serde_json::from_value(serde_json::json!("0".repeat(64))).unwrap(),
                project: id(),
                expected_generation: 1,
                authority: AuthorityBinding {
                    grant_ref: reference.clone(),
                    owner_epoch: 1,
                    endpoint: None,
                    attempt: None,
                },
                command,
            };
            envelope.digest = envelope.computed_digest().unwrap();
            let mut next = state.clone();
            next.sequence += 1;
            let segment = EventSegment {
                run: state.run.clone(),
                first_sequence: next.sequence,
                previous: None,
                events: vec![OrchestrationEvent {
                    command: envelope.id.clone(),
                    sequence: next.sequence,
                    projection: next.clone(),
                }],
            };
            let update = ValidatedRunUpdate::new(
                &envelope.project,
                1,
                &envelope,
                next,
                segment,
                CommandDecision::Applied,
            )
            .unwrap();
            assert!(
                !terminal_progress(&state, &update),
                "TE13_FIX1_NO_TERMINAL_COMPLETION {:?}",
                envelope.command
            );
        }
    }
    #[test]
    fn s4b_c5_fix1_ordinary_reads_cannot_spend_terminal_memory() {
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "run-read-budget",
        );
        let mut store = ProjectStore::new(
            crate::namespace::ProjectNamespace::acquire(&temp.path("budget.kinewright")).unwrap(),
            Some(crate::bundle_records::ProjectId::fresh().unwrap()),
            None,
            StorageLimits::default(),
        )
        .unwrap();
        let document = kinewright_core::Document {
            markers: (1..=40_000)
                .map(|n| kinewright_core::Marker {
                    id: kinewright_core::MarkerId(n),
                    position: kinewright_core::TimeCode(0),
                    label: String::new(),
                    color_token: 0,
                })
                .collect(),
            ..Default::default()
        };
        let sealed = store.admit_document(&document).unwrap();
        store.set_run_budget(true);
        let mut held = vec![];
        let mut refused = false;
        for _ in 0..24 {
            match store.read_record::<kinewright_core::Document>(&sealed.reference) {
                Ok(record) => held.push(record),
                Err(BundleError::RunLimit) => {
                    refused = true;
                    break;
                }
                Err(error) => panic!("C5_FIX1_READ_WRONG_REFUSAL {error}"),
            }
        }
        assert!(refused && !held.is_empty(), "C5_FIX1_READ_REFUSAL");
        let usage = store.usage().unwrap();
        assert!(
            usage.metadata_bytes + usage.decoded_bytes < store.limits().metadata_bytes - METADATA,
            "C5_FIX1_READ_RESERVE {usage:?}"
        );
        assert!(
            !store.run_limit_reached(),
            "a refused read is not a spending decision"
        );
        let _reading = reading(&store);
        let terminal = store
            .read_record::<kinewright_core::Document>(&sealed.reference)
            .expect("C5_FIX1_TERMINAL_READ_MEMORY");
        assert_eq!(*terminal, document);
    }
    #[test]
    fn s4b_c5_fix1_graph_admission_keeps_terminal_headroom() {
        use crate::{bundle_records::ObjectRef, namespace::ProjectNamespace};
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "run-graph-budget",
        );
        let mut store = ProjectStore::new(
            ProjectNamespace::acquire(&temp.path("budget.kinewright")).unwrap(),
            Some(crate::bundle_records::ProjectId::fresh().unwrap()),
            None,
            StorageLimits::default(),
        )
        .unwrap();
        let append = |store: &mut ProjectStore, n: u64, previous: &[ObjectRef]| {
            let bytes = n.to_le_bytes();
            store.admit_blob(
                &mut &bytes[..],
                &ObjectRef {
                    sha256: kinewright_media::sha256_bytes(&bytes),
                    byte_length: bytes.len() as u64,
                    record_kind: crate::bundle_records::RecordKind::OwnedEvidence,
                },
                previous,
            )
        };
        store.set_run_budget(true);
        let mut previous = vec![];
        let mut refused = None;
        for n in 0..40 {
            match append(&mut store, n, &previous) {
                Ok(sealed) => previous = vec![sealed.reference],
                Err(BundleError::RunLimit) => {
                    refused = Some(n);
                    break;
                }
                Err(error) => panic!("C5_FIX1_GRAPH_WRONG_REFUSAL {error}"),
            }
        }
        let refused = refused.expect("C5_FIX1_GRAPH_RESERVED");
        assert_eq!(refused, store.limits().graph_depth - GRAPH_LEVELS + 1);
        assert!(append(&mut store, 100, &previous).is_err());
        // This unit store has no H1 run. Its inactive control retains the raw cap.
        store.set_run_budget(false);
        for n in refused..refused + GRAPH_LEVELS {
            previous = vec![
                append(&mut store, n, &previous)
                    .expect("C5_FIX1_TERMINAL_GRAPH_HEADROOM")
                    .reference,
            ];
        }
        assert!(store.closure(&previous).is_ok());
        assert!(
            append(&mut store, 99, &previous).is_err(),
            "C5_FIX1_RAW_GRAPH_UNCHANGED"
        );
        assert_eq!(store.limits().graph_depth, 32);
    }
    #[test]
    fn s4b_c5_fix1_record_limits_keep_terminal_headroom() {
        use crate::namespace::ProjectNamespace;
        use kinewright_media::test_support::TempDirectory;
        for decoded in [false, true] {
            let temp = TempDirectory::new_in(
                &kinewright_media::test_support::workspace_target_dir(),
                "run-record-budget",
            );
            let mut store = ProjectStore::new(
                ProjectNamespace::acquire(&temp.path("budget.kinewright")).unwrap(),
                Some(crate::bundle_records::ProjectId::fresh().unwrap()),
                None,
                StorageLimits::default(),
            )
            .unwrap();
            let document = kinewright_core::Document {
                markers: (1..=if decoded { 40_000 } else { 1 })
                    .map(|n| kinewright_core::Marker {
                        id: kinewright_core::MarkerId(n),
                        position: kinewright_core::TimeCode(0),
                        label: if decoded {
                            String::new()
                        } else {
                            "x".repeat(9 << 20)
                        },
                        color_token: 0,
                    })
                    .collect(),
                ..Default::default()
            };
            document.validate().unwrap();
            let bytes = serde_json::to_vec(&document).unwrap();
            let cost = crate::store::preflight_record(&bytes, store.limits()).unwrap();
            if decoded {
                assert!(
                    (bytes.len() as u64) < RECORD_BYTES && cost > DECODED_BYTES,
                    "C5_FIX1_DECODED_PRECONDITION {cost}"
                );
            } else {
                assert!(
                    bytes.len() as u64 > RECORD_BYTES && cost < store.limits().decoded_record_bytes
                );
            }
            store.set_run_budget(true);
            let before = store.usage().unwrap().objects;
            assert!(
                matches!(store.admit_document(&document), Err(BundleError::RunLimit)),
                "C5_FIX1_RECORD_RESERVE decoded={decoded}"
            );
            assert_eq!(store.usage().unwrap().objects, before);
            let headroom = reading(&store);
            assert!(
                matches!(store.admit_document(&document), Err(BundleError::RunLimit)),
                "C5_FIX2_READING_IS_NOT_WRITE_AUTHORITY"
            );
            drop(headroom);
            // No H1 run was admitted in this unit fixture.
            store.set_run_budget(false);
            assert!(
                store.admit_document(&document).is_ok(),
                "C5_FIX1_TERMINAL_RECORD_HEADROOM decoded={decoded}"
            );
        }
    }

    /// C5 fix 14: each condition of `terminal_authority` refuses on its own.
    #[test]
    fn s4b_c5_fix14_terminal_authority_conditions() {
        let active = RunBudget {
            active: true,
            ..RunBudget::default()
        };
        let inactive = RunBudget::default();
        let spent = RunBudget {
            active: true,
            terminal: usize::try_from(TERMINAL_CUTS).unwrap(),
            ..RunBudget::default()
        };
        let yes = || Ok(true);
        let no = || Ok(false);
        assert!(
            terminal_authority(true, &active, false, no).is_ok(),
            "C5_FIX14_ACTIVE_KEEPS_FIX2_RULE"
        );
        assert!(
            terminal_authority(false, &active, true, yes).is_err(),
            "C5_FIX14_UNADMITTED_REFUSED"
        );
        assert!(
            terminal_authority(true, &spent, true, yes).is_err(),
            "C5_FIX14_TERMINAL_BOUND_REFUSED"
        );
        assert!(
            terminal_authority(true, &inactive, true, no).is_err(),
            "C5_FIX14_FINISHED_NON_ARCHIVE_REFUSED"
        );
        assert!(
            terminal_authority(true, &inactive, false, yes).is_err(),
            "C5_FIX14_INACTIVE_PENDING_REFUSED"
        );
        assert!(
            terminal_authority(true, &inactive, true, yes).is_ok(),
            "C5_FIX14_FINISHED_ARCHIVE_ADMITTED"
        );
    }
}
