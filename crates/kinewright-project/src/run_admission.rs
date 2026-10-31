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
pub const METADATA: u64 = 256 << 20;
pub const OWNED: u64 = 8 << 20;
pub const LIVE_HISTORY: u64 = 16 << 20;
pub const GRAPH_LEVELS: u64 = 8;
pub const REASON: &str = "Run limit reached. Stop this run to finish cleanup.";
thread_local! { static TERMINAL: RefCell<BTreeMap<usize,usize>> = const { RefCell::new(BTreeMap::new()) }; }
pub struct TerminalAdmission {
    key: usize,
    _lifetime: std::sync::Arc<()>,
    _thread: PhantomData<Rc<()>>,
}
impl Drop for TerminalAdmission {
    fn drop(&mut self) {
        TERMINAL.with_borrow_mut(|entries| {
            if let Some(depth) = entries.get_mut(&self.key) {
                *depth -= 1;
                if *depth == 0 {
                    entries.remove(&self.key);
                }
            }
        });
    }
}
pub(crate) fn terminal(store: &ProjectStore) -> TerminalAdmission {
    let key = store.admission_key();
    TERMINAL.with_borrow_mut(|entries| *entries.entry(key).or_default() += 1);
    TerminalAdmission {
        key,
        _lifetime: store.admission_lifetime(),
        _thread: PhantomData,
    }
}
pub(crate) fn is_terminal(key: usize) -> bool {
    TERMINAL.with_borrow(|entries| entries.contains_key(&key))
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
    #[must_use]
    pub fn terminal_admission(&self) -> TerminalAdmission {
        terminal(self)
    }
    pub(crate) fn admission_limits(&self) -> StorageLimits {
        if self.run_budget_active() && !is_terminal(self.admission_key()) {
            ordinary(self.limits())
        } else {
            self.limits().clone()
        }
    }
    pub(crate) fn run_error(&self, error: BundleError) -> BundleError {
        if self.run_budget_active() && matches!(error, BundleError::Capacity(_)) {
            self.note_run_limit();
            BundleError::RunLimit
        } else {
            error
        }
    }
    pub(crate) fn admit_run_write(&self) -> Result<(), BundleError> {
        if self.run_budget_active() && !is_terminal(self.admission_key()) && self.has_run_limit() {
            return Err(BundleError::RunLimit);
        }
        Ok(())
    }
    #[must_use]
    pub fn run_limit_reached(&self) -> bool {
        self.has_run_limit()
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
    fn check_run_start_capacity(
        &self,
        index: &crate::studio_records::RunIndex,
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
            || limits.live_history_bytes < HISTORY_BYTES
            || (index.runs.len() + index.orchestration.len()) as u64 + 1 > limits.versions
            || self.issuers.len() as u64 + 4 > limits.issuers
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
    pub(crate) fn run_admission(
        &mut self,
        update: &ValidatedRunUpdate,
        arm: bool,
    ) -> Result<Option<TerminalAdmission>, BundleError> {
        let reserved = is_terminal(self.store.admission_key());
        let reading = self.store.terminal_admission();
        let run = &update.projection().run;
        let index = self.run_index()?;
        let budget = index.budgets.get(run).cloned().unwrap_or_default();
        let before = self.orchestration_projection(run)?;
        let terminal = before
            .as_ref()
            .is_some_and(|before| terminal_progress(before, update));
        if terminal {
            crate::orchestration_accounting::encode(update.receipt(), TERMINAL_RECEIPT_BYTES)
                .map_err(|_| BundleError::RunLimit)?;
            crate::orchestration_accounting::encode(update, TERMINAL_TRANSITION_BYTES)
                .map_err(|_| BundleError::RunLimit)?;
            return Ok(Some(self.store.terminal_admission()));
        }
        if before.is_none() {
            if index.budgets.values().any(|b| b.active) {
                return Err(BundleError::RunLimit);
            }
            if !reserved {
                self.check_run_start_capacity(&index)?;
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
        let _terminal = self.store.terminal_admission();
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
    /// Check before staging a task freeze, using the same publication policy.
    pub fn run_transition_admission(
        &mut self,
        update: &ValidatedRunUpdate,
    ) -> Result<Option<TerminalAdmission>, BundleError> {
        self.active_bundle_mut()?.run_admission(update, false)
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
    pub fn run_terminal_admission(&self) -> Result<TerminalAdmission, BundleError> {
        Ok(self.store()?.terminal_admission())
    }
    pub fn run_publication_status(
        &self,
        run: &crate::orchestration_records::RunId,
    ) -> Result<Option<String>, BundleError> {
        let _reading = self.run_terminal_admission()?;
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
        let _terminal = store.terminal_admission();
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
        let _terminal = store.terminal_admission();
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
            let _terminal = store.terminal_admission();
            assert!(
                store.admit_document(&document).is_ok(),
                "C5_FIX1_TERMINAL_RECORD_HEADROOM decoded={decoded}"
            );
        }
    }
}
