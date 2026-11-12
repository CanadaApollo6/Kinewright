//! The persisted history graph and its resolved, document-bearing counterpart.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::BundleError,
    bundle_records::{ObjectRef, RecordKind},
    storage_budget::{StorageLimits, StorageReservation, StorageUsage},
    store::{BundleRecord, ProjectStore, record_sealed},
};
use kinewright_core::{Document, HistoryJournal, HistorySnapshot};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

/// Persisted ownership determines admission; an ordinary writer cannot claim run escrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryRole {
    Ordinary,
    Task,
    Candidate,
}
impl HistoryRole {
    #[must_use]
    pub const fn persisted_limit(self, limits: &StorageLimits) -> u64 {
        match self {
            Self::Ordinary => limits.branch_history_bytes,
            Self::Task => TASK_BYTES,
            Self::Candidate => CANDIDATE_BYTES,
        }
    }
    #[must_use]
    pub fn live_limit(self, limits: &StorageLimits) -> u64 {
        match self {
            // Preserve the existing single-history decoded bound. The shared
            // ordinary/manual pool remains limits.live_history_bytes (128 MiB).
            Self::Ordinary => limits.branch_history_bytes.min(limits.live_history_bytes),
            Self::Task => TASK_LIVE,
            Self::Candidate => CANDIDATE_LIVE,
        }
    }
    /// Retain the policy that a completed replay proved. The shared pool's
    /// total is checked by each reservation and by cold-root admission; the
    /// history proof depends on its effective role limit.
    pub(crate) fn admission_profile(self, limits: &StorageLimits) -> StorageLimits {
        let mut profile = limits.clone();
        profile.live_history_bytes = self.live_limit(limits);
        profile
    }
    pub(crate) fn replay_limit(self, limits: &StorageLimits) -> u64 {
        match self {
            Self::Ordinary => self.live_limit(limits).min(limits.decoded_record_bytes),
            // Native retained state uses its role budget. Its one serial
            // replay uses the existing decoded window and raw total ledger.
            Self::Task | Self::Candidate => limits.decoded_record_bytes,
        }
    }
}
// Final stage-one role caps. The 568 MiB resident envelope remains unqualified;
// resident native actors are not yet charged to the shared raw ledger.
pub const TASK_BYTES: u64 = 1_111_040;
pub const CANDIDATE_BYTES: u64 = 8 * TASK_BYTES;
pub const TASK_LIVE: u64 = 41 << 20;
pub const CANDIDATE_LIVE: u64 = 2 << 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryRecord {
    pub role: HistoryRole,
    pub baseline_ref: ObjectRef,
    pub current_ref: ObjectRef,
    pub journal: HistoryJournal,
}

impl HistoryRecord {
    pub(crate) fn verify_checkpoint(
        checkpoint: &crate::bundle_records::BranchCheckpoint,
        history: &HistorySnapshot,
    ) -> Result<(), BundleError> {
        let record = Self::capture(history, checkpoint.history_role)?;
        let bytes = serde_json::to_vec(&record).map_err(crate::bundle::encoding_error)?;
        if bytes.len() as u64 != checkpoint.history_ref.byte_length
            || kinewright_media::sha256_bytes(&bytes) != checkpoint.history_ref.sha256
            || crate::manual_history::history_sha256(history)? != checkpoint.history_digest
        {
            return Err(BundleError::Corrupt("checkpoint history identity".into()));
        }
        Ok(())
    }
    pub fn capture(history: &HistorySnapshot, role: HistoryRole) -> Result<Self, BundleError> {
        let reference = |document: &Document| -> Result<ObjectRef, BundleError> {
            let bytes = serde_json::to_vec(document).map_err(crate::bundle::encoding_error)?;
            Ok(ObjectRef {
                sha256: kinewright_media::sha256_bytes(&bytes),
                byte_length: bytes.len() as u64,
                record_kind: RecordKind::Document,
            })
        };
        Ok(Self {
            role,
            baseline_ref: reference(&history.baseline)?,
            current_ref: reference(&history.document)?,
            journal: HistoryJournal::capture(history).map_err(crate::bundle::history_error)?,
        })
    }

    /// One replay resolver for store reads and pre-adoption namespace recovery.
    /// The caller owns/pins the graph while the documents are being resolved.
    pub(crate) fn resolve(
        &self,
        limits: &StorageLimits,
        mut read_document: impl FnMut(&ObjectRef) -> Result<StorageReservation<Document>, BundleError>,
        reserve: impl FnOnce(u64, HistoryRole) -> Result<StorageReservation, BundleError>,
    ) -> Result<StorageReservation<HistorySnapshot>, BundleError> {
        let baseline = read_document(&self.baseline_ref)?;
        let document = read_document(&self.current_ref)?;
        let reservation = reserve(self.role.replay_limit(limits), self.role)?;
        let document = document.take();
        let (history, _) = self
            .journal
            .resolve_with_budget(
                Arc::new(baseline.take()),
                &document,
                self.role.replay_limit(limits),
            )
            .map_err(crate::bundle::history_error)?;
        let retained = crate::history_size::check_retained_history(&history, self.role, limits)?;
        let reservation = reservation.finish_history_replay(retained)?;
        Ok(reservation.hold(history))
    }
}

impl record_sealed::Sealed for HistoryRecord {}
impl BundleRecord for HistoryRecord {
    const KIND: RecordKind = RecordKind::History;
    fn dependencies(&self) -> Vec<ObjectRef> {
        vec![self.baseline_ref.clone(), self.current_ref.clone()]
    }
    fn validate(&self, limits: &StorageLimits) -> Result<(), BundleError> {
        if self.journal.batches.len() > kinewright_core::HISTORY_COMMANDS {
            return Err(BundleError::Capacity("branch history commands"));
        }
        if self.baseline_ref.record_kind != RecordKind::Document
            || self.current_ref.record_kind != RecordKind::Document
            || self.baseline_ref.sha256 != self.journal.baseline_sha256
            || self.current_ref.sha256 != self.journal.document_sha256
        {
            return Err(BundleError::Corrupt(
                "history document reference agreement".into(),
            ));
        }
        let bytes = serde_json::to_vec(self).map_err(crate::bundle::encoding_error)?;
        if bytes.len() as u64 > self.role.persisted_limit(limits)
            || crate::store::preflight_record(&bytes, limits)? > self.role.persisted_limit(limits)
        {
            return Err(BundleError::Capacity("branch history"));
        }
        Ok(())
    }
}

impl ProjectStore {
    pub(crate) fn with_history_admission<T>(
        &self,
        role: HistoryRole,
        limits: &StorageLimits,
        work: impl FnOnce() -> Result<T, BundleError>,
    ) -> Result<T, BundleError> {
        self.replay_gate().with(|| {
            let window = role.replay_limit(limits);
            let _replay = self.reserve_copy_read(
                0,
                window,
                if role == HistoryRole::Ordinary {
                    window
                } else {
                    0
                },
                false,
                true,
            )?;
            work()
        })?
    }

    pub(crate) fn reserve_history_anchor(
        &self,
        anchor: &kinewright_core::HistoryTransport,
    ) -> Result<StorageReservation, BundleError> {
        let documents = crate::history_size::document_live_bytes(&anchor.baseline)?
            .checked_add(if Arc::ptr_eq(&anchor.baseline, &anchor.document) {
                0
            } else {
                crate::history_size::document_live_bytes(&anchor.document)?
            })
            .ok_or(BundleError::Capacity("anchor memory overflow"))?;
        let bytes = documents
            .checked_add(crate::history_size::payload(&anchor.journal)?)
            .ok_or(BundleError::Capacity("anchor memory overflow"))?;
        self.reserve_copy_read(0, bytes, 0, false, true)
    }
    pub(crate) fn reserve_history_capture(
        &self,
        history: &HistorySnapshot,
        role: HistoryRole,
    ) -> Result<StorageReservation, BundleError> {
        self.reserve_role_history_shadow(crate::history_size::history_live_bytes(history)?, role)
    }
    pub(crate) fn reserve_history_replay(
        &self,
        live: u64,
        role: HistoryRole,
    ) -> Result<StorageReservation, BundleError> {
        if live > role.replay_limit(self.limits()) {
            return Err(BundleError::Capacity("history replay memory"));
        }
        self.reserve_copy_read(
            0,
            live,
            if role == HistoryRole::Ordinary {
                live
            } else {
                0
            },
            false,
            true,
        )
    }
    pub(crate) fn reserve_role_history_shadow(
        &self,
        live: u64,
        role: HistoryRole,
    ) -> Result<StorageReservation, BundleError> {
        if live > role.live_limit(self.limits()) {
            return Err(BundleError::Capacity("role history memory"));
        }
        self.reserve_copy_read(
            0,
            live,
            if role == HistoryRole::Ordinary {
                live
            } else {
                0
            },
            false,
            role != HistoryRole::Ordinary,
        )
    }
    pub fn resolve_history(
        &self,
        reference: &ObjectRef,
    ) -> Result<StorageReservation<HistorySnapshot>, BundleError> {
        self.resolve_copy_history(reference, false)
    }

    pub(crate) fn resolve_copy_history(
        &self,
        reference: &ObjectRef,
        cleanup: bool,
    ) -> Result<StorageReservation<HistorySnapshot>, BundleError> {
        self.replay_gate()
            .with(|| self.resolve_copy_history_serial(reference, cleanup, false))?
    }

    pub(crate) fn resolve_manual_history(
        &self,
        reference: &ObjectRef,
    ) -> Result<StorageReservation<HistorySnapshot>, BundleError> {
        self.replay_gate()
            .with(|| self.resolve_copy_history_serial(reference, false, true))?
    }

    fn resolve_copy_history_serial(
        &self,
        reference: &ObjectRef,
        cleanup: bool,
        manual: bool,
    ) -> Result<StorageReservation<HistorySnapshot>, BundleError> {
        let pin = self.pin(std::slice::from_ref(reference))?;
        let record = self.read_copy_record::<HistoryRecord>(reference, cleanup)?;
        let resolved = record.resolve(
            self.limits(),
            |reference| self.read_copy_record(reference, cleanup),
            |live, role| {
                if live > role.replay_limit(self.limits()) {
                    return Err(BundleError::Capacity("history replay memory"));
                }
                self.reserve_copy_read(
                    0,
                    live,
                    if manual || role == HistoryRole::Ordinary {
                        live
                    } else {
                        0
                    },
                    false,
                    cleanup || role != HistoryRole::Ordinary,
                )
            },
        )?;
        Ok(resolved.pin(pin))
    }
}

impl crate::bundle::ProjectBundle {
    pub(crate) fn history_role(
        &self,
        branch: &crate::bundle_records::BranchId,
        specialist: Option<&ObjectRef>,
    ) -> Result<HistoryRole, BundleError> {
        if branch == self.manual_branch_id() {
            return Ok(HistoryRole::Ordinary);
        }
        if let Some(reference) = specialist {
            let base = self
                .store()
                .read_record::<crate::bundle_records::SpecialistBaseRecord>(reference)?;
            return Ok(base.history_role);
        }
        match self.branches.get(branch) {
            Some(checkpoint) => Ok(self
                .store()
                .read_record::<HistoryRecord>(&checkpoint.history_ref)?
                .role),
            None => Ok(HistoryRole::Ordinary),
        }
    }
}

pub(crate) fn resolve_namespace_history(
    namespace: &crate::namespace::ProjectNamespace,
    reference: &ObjectRef,
    usage: &Arc<Mutex<StorageUsage>>,
    fields: &mut BTreeSet<String>,
    limits: &StorageLimits,
    manual: bool,
) -> Result<StorageReservation<HistorySnapshot>, BundleError> {
    let record = crate::bundle_recovery::read_record::<HistoryRecord>(
        namespace, reference, usage, fields, limits,
    )?;
    record.resolve(
        limits,
        |reference| {
            crate::bundle_recovery::read_record(namespace, reference, usage, fields, limits)
        },
        |live, role| {
            if live > role.replay_limit(limits) {
                return Err(BundleError::Capacity("history replay memory"));
            }
            StorageReservation::reserve(
                usage,
                limits,
                0,
                live,
                if manual || role == HistoryRole::Ordinary {
                    live
                } else {
                    0
                },
                false,
                u64::MAX,
            )
        },
    )
}

/// Serializes temporary replay documents across all task endpoints and publication.
/// A publication may resolve another record on the same thread; that nested work
/// belongs to the same overlap, so this gate is reentrant for its owning thread.
#[derive(Default)]
pub(crate) struct ReplayGate {
    state: Mutex<Option<(std::thread::ThreadId, usize)>>,
    ready: std::sync::Condvar,
}
impl ReplayGate {
    pub(crate) fn with<T>(&self, work: impl FnOnce() -> T) -> Result<T, BundleError> {
        struct Release<'a>(&'a ReplayGate);
        impl Drop for Release<'_> {
            fn drop(&mut self) {
                if let Ok(mut state) = self.0.state.lock()
                    && let Some((_, depth)) = &mut *state
                {
                    *depth -= 1;
                    if *depth == 0 {
                        *state = None;
                        self.0.ready.notify_one();
                    }
                }
            }
        }
        let thread = std::thread::current().id();
        let mut state = self
            .state
            .lock()
            .map_err(|_| BundleError::Capacity("history replay gate unavailable"))?;
        while state.as_ref().is_some_and(|(owner, _)| *owner != thread) {
            state = self
                .ready
                .wait(state)
                .map_err(|_| BundleError::Capacity("history replay gate unavailable"))?;
        }
        match &mut *state {
            Some((_, depth)) => *depth += 1,
            None => *state = Some((thread, 1)),
        }
        drop(state);
        let _release = Release(self);
        Ok(work())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn role_histories_leave_the_ordinary_pool_but_manual_selection_uses_it() {
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "history-role-pool",
        );
        let mut store = ProjectStore::new(
            crate::namespace::ProjectNamespace::acquire(&temp.path("project.kinewright")).unwrap(),
            Some(crate::bundle_records::ProjectId::fresh().unwrap()),
            None,
            StorageLimits::default(),
        )
        .unwrap();
        let core = kinewright_core::Core::spawn(Document::default()).unwrap();
        core.request(kinewright_core::Command::Do(
            kinewright_core::Operation::AddMarker {
                marker: kinewright_core::Marker {
                    id: kinewright_core::MarkerId(1),
                    position: kinewright_core::TimeCode(0),
                    label: "edit".into(),
                    color_token: 0,
                },
            },
        ))
        .unwrap();
        let history = core.export_history().unwrap();
        let ordinary = store.admit_history(&history).unwrap();
        for role in [HistoryRole::Task, HistoryRole::Candidate] {
            let sealed = store
                .admit_record(&HistoryRecord::capture(&history, role).unwrap())
                .unwrap();
            let before = store.usage().unwrap();
            let resolved = store.resolve_history(&sealed.reference).unwrap();
            let during = store.usage().unwrap();
            assert_eq!(
                during.live_history_bytes, before.live_history_bytes,
                "ROLE_HAS_OWN_POOL"
            );
            assert!(
                during.decoded_bytes > before.decoded_bytes,
                "ROLE_STILL_COUNTS_UNDER_TOTAL"
            );
            assert_eq!(*resolved, history);
            drop(resolved);
            let manual = store.resolve_manual_history(&sealed.reference).unwrap();
            assert!(
                store.usage().unwrap().live_history_bytes > before.live_history_bytes,
                "MANUAL_SELECTION_USES_ORDINARY_POOL"
            );
            drop(manual);
            assert_eq!(
                store.usage().unwrap().live_history_bytes,
                before.live_history_bytes
            );
        }
        let resolved = store.resolve_history(&ordinary.reference).unwrap();
        assert!(store.usage().unwrap().live_history_bytes > 0);
        assert_eq!(*resolved, history);
    }

    #[test]
    fn replay_gate_serializes_threads_and_allows_same_publication_nesting() {
        let gate = Arc::new(ReplayGate::default());
        let active = std::sync::atomic::AtomicUsize::new(0);
        let together = std::sync::Barrier::new(4);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let gate = Arc::clone(&gate);
                let active = &active;
                let together = &together;
                scope.spawn(move || {
                    together.wait();
                    for _ in 0..32 {
                        gate.with(|| {
                            assert_eq!(active.fetch_add(1, std::sync::atomic::Ordering::SeqCst), 0);
                            gate.with(std::thread::yield_now).unwrap();
                            assert_eq!(active.fetch_sub(1, std::sync::atomic::Ordering::SeqCst), 1);
                        })
                        .unwrap();
                    }
                });
            }
        });
        assert_eq!(active.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}
