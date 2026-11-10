//! Scoped support for the host's proof-bearing retirement transaction.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::BundleError,
    bundle_records::RecordKind,
    editing_session::Ja1EditingSession,
    orchestration_commit::ValidatedRunUpdate,
    orchestration_records::{AttemptState, ExecutionIdentity},
    store::ProjectStore,
};

#[derive(Clone)]
pub(crate) struct CleanupAdmission {
    key: usize,
    _lifetime: std::sync::Arc<()>,
    identity: ExecutionIdentity,
}
impl CleanupAdmission {
    pub(crate) fn check_transition(
        &self,
        update: &ValidatedRunUpdate,
        terminal: bool,
    ) -> Result<(), BundleError> {
        if !terminal
            || update.projection().run != self.identity.run
            || !update.projection().attempts.iter().any(|attempt| {
                attempt.identity == self.identity
                    && matches!(attempt.state, AttemptState::Retired(_))
            })
        {
            return Err(BundleError::HistoryChanged);
        }
        Ok(())
    }
}
/// Owned restoration capability. It cannot be serialized or supplied by a tool.
/// The trusted host retains its guardian proof for this capability's whole scope.
#[must_use = "the trusted host must restore ordinary admission on scope exit"]
pub struct CleanupPublication {
    ordinary: ProjectStore,
}
impl ProjectStore {
    pub(crate) fn is_cleanup_publication(&self) -> bool {
        self.cleanup_publication
            .as_ref()
            .is_some_and(|token| token.key == self.admission_key())
    }
    pub(crate) fn check_cleanup_kind(&self, kind: RecordKind) -> Result<(), BundleError> {
        if !self.is_cleanup_publication()
            || matches!(
                kind,
                RecordKind::Document
                    | RecordKind::History
                    | RecordKind::Lineage
                    | RecordKind::BranchCheckpoint
                    | RecordKind::BranchIndex
                    | RecordKind::SpecialistBase
                    | RecordKind::RootCheckpoint
                    | RecordKind::StoreManifest
                    | RecordKind::ReceiptIndex
                    | RecordKind::IssuerState
                    | RecordKind::RunIndex
                    // H1 metadata republishes its unchanged feedback index.
                    | RecordKind::FeedbackIndex
                    | RecordKind::OwnedEvidence
            )
        {
            Ok(())
        } else {
            Err(BundleError::Identity(format!(
                "record outside cleanup publication: {kind:?}"
            )))
        }
    }
}
impl Ja1EditingSession {
    /// Trusted retirement receiver only: the agent must first validate its exact
    /// guardian proof and durable launch fence. No tool handler calls this port.
    /// Each successful transaction retires the named attempt; repeated deliveries
    /// return before minting. Failed publications can retry the same bounded work.
    pub fn begin_cleanup_publication(
        &mut self,
        identity: &ExecutionIdentity,
    ) -> Result<CleanupPublication, BundleError> {
        let bundle = self.active_bundle_mut()?;
        let _reading = crate::run_admission::reading(&bundle.store);
        let state = bundle
            .orchestration_projection(&identity.run)?
            .ok_or(BundleError::HistoryChanged)?;
        let index = bundle.run_index()?;
        if identity.project != *bundle.project_id()
            || bundle.store.is_project_publication()
            || !index.budgets.get(&identity.run).is_some_and(|budget| {
                budget.active && (budget.terminal as u64) < crate::run_admission::TERMINAL_CUTS
            })
            || !state.attempts.iter().any(|attempt| {
                attempt.identity == *identity && !matches!(attempt.state, AttemptState::Retired(_))
            })
        {
            return Err(BundleError::HistoryChanged);
        }
        let mut writer = bundle.store.publication_clone_for_cleanup();
        writer.cleanup_publication = Some(CleanupAdmission {
            key: bundle.store.admission_key(),
            _lifetime: bundle.store.admission_lifetime(),
            identity: identity.clone(),
        });
        Ok(CleanupPublication {
            ordinary: std::mem::replace(&mut bundle.store, writer),
        })
    }
    /// Restore the exact ordinary publisher retained at entry.
    ///
    /// # Panics
    /// Panics if the trusted host replaced or lost its project owner while the
    /// proof-bearing cleanup transaction held exclusive access.
    pub fn end_cleanup_publication(&mut self, publication: CleanupPublication) {
        let bundle = self
            .active_bundle_mut()
            .expect("cleanup retains its project owner");
        assert_eq!(
            bundle.store.admission_key(),
            publication.ordinary.admission_key()
        );
        bundle.store = publication.ordinary;
    }
}
