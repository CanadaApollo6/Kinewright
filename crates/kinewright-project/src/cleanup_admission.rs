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
use std::sync::{Arc, Weak};

#[derive(Clone)]
pub(crate) struct CleanupAdmission {
    key: usize,
    _lifetime: std::sync::Arc<()>,
    identity: ExecutionIdentity,
    scope: Weak<()>,
}
impl CleanupAdmission {
    pub(crate) fn check_transition(
        &self,
        update: &ValidatedRunUpdate,
        terminal: bool,
    ) -> Result<(), BundleError> {
        if self.scope.upgrade().is_none() {
            return Ok(());
        }
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
/// The sole strong lease keeps cleanup authority active in every store view.
/// Dropping it restores ordinary admission, including during unwinding.
#[must_use = "retain this lease throughout the proof-bearing cleanup transaction"]
pub struct CleanupPublication {
    _scope: Arc<()>,
}
impl CleanupPublication {
    fn begin(store: &mut ProjectStore, identity: &ExecutionIdentity) -> Self {
        let scope = Arc::new(());
        let mut writer = store.publication_clone_for_cleanup();
        writer.cleanup_publication = Some(CleanupAdmission {
            key: store.admission_key(),
            _lifetime: store.admission_lifetime(),
            identity: identity.clone(),
            scope: Arc::downgrade(&scope),
        });
        *store = writer;
        Self { _scope: scope }
    }
}
impl ProjectStore {
    pub(crate) fn is_cleanup_publication(&self) -> bool {
        self.cleanup_publication.as_ref().is_some_and(|token| {
            token.key == self.admission_key() && token.scope.upgrade().is_some()
        })
    }
    pub(crate) fn check_cleanup_kind(&self, kind: RecordKind) -> Result<(), BundleError> {
        // Cleanup uses exact task cuts and committed manual history, never floors or recovery-close transitions.
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
        Ok(CleanupPublication::begin(&mut bundle.store, identity))
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn run_limit_cleanup_handle_drop_restores_ordinary_admission() {
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "cleanup-handle-drop",
        );
        let project = crate::bundle_records::ProjectId::fresh().unwrap();
        let mut store = ProjectStore::new(
            crate::namespace::ProjectNamespace::acquire(&temp.path("project.kinewright")).unwrap(),
            Some(project.clone()),
            None,
            crate::storage_budget::StorageLimits::default(),
        )
        .unwrap();
        let identity: ExecutionIdentity = serde_json::from_value(serde_json::json!({
            "version":1, "project":project, "run":"1".repeat(32),
            "task":"2".repeat(32), "attempt":"3".repeat(32),
            "nonce":"4".repeat(32), "owner_instance":"5".repeat(32),
        }))
        .unwrap();
        store.set_run_budget(true);
        store.note_run_limit();
        let lease = CleanupPublication::begin(&mut store, &identity);
        assert!(store.is_cleanup_publication());
        store
            .admit_document(&kinewright_core::Document::default())
            .unwrap();
        drop(lease);
        assert!(!store.is_cleanup_publication());
        assert!(store.run_limit_reached());
        assert!(matches!(
            store.admit_document(&kinewright_core::Document::default()),
            Err(BundleError::RunLimit)
        ));
    }
}
