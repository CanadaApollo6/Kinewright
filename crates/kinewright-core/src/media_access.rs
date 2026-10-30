//! Authority to reopen locators belongs to a legacy project source, not a Document.
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// An immutable, process-local identity. Different grants never share it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LegacyMediaIdentity(u64);
impl std::fmt::Display for LegacyMediaIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// A revocable grant issued by a source that has no owned Studio envelope.
#[derive(Clone, Debug)]
pub struct LegacyMediaGrant(Arc<LegacyMediaState>);

impl PartialEq for LegacyMediaGrant {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for LegacyMediaGrant {}
#[derive(Debug)]
struct LegacyMediaState {
    identity: u64,
    revoked: AtomicBool,
    active: Mutex<usize>,
    #[cfg(feature = "test-util")]
    hold: Mutex<Option<Arc<WorkHoldState>>>,
    #[cfg(feature = "test-util")]
    opens: std::sync::atomic::AtomicUsize,
}

impl Default for LegacyMediaState {
    fn default() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            identity: NEXT.fetch_add(1, Ordering::Relaxed),
            revoked: AtomicBool::new(false),
            active: Mutex::new(0),
            #[cfg(feature = "test-util")]
            hold: Mutex::default(),
            #[cfg(feature = "test-util")]
            opens: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

/// An active locator operation. Activation waits for these leases to drain.
#[derive(Debug)]
pub struct LegacyMediaWork(LegacyMediaGrant);

impl Drop for LegacyMediaWork {
    fn drop(&mut self) {
        *self
            .0
            .0
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) -= 1;
    }
}

impl LegacyMediaGrant {
    /// Process-local identity for authority-partitioned derived caches.
    #[must_use]
    pub fn identity(&self) -> LegacyMediaIdentity {
        LegacyMediaIdentity(self.0.identity)
    }
    /// Refuse a request whose source has since become owned.
    /// # Errors
    /// Returns the named media-admission refusal after revocation.
    pub fn check(&self) -> Result<(), crate::MediaError> {
        if self.0.revoked.load(Ordering::Acquire) {
            Err(crate::MediaError::MediaAdmissionRequired)
        } else {
            Ok(())
        }
    }
    /// Start work atomically with respect to revocation.
    /// # Panics
    /// In test-util builds, panics if a fixture poisons its barrier.
    /// # Errors
    /// Returns the named admission refusal if the source was revoked.
    pub fn begin(&self) -> Result<LegacyMediaWork, crate::MediaError> {
        #[cfg(feature = "test-util")]
        {
            let hold = self.0.hold.lock().unwrap().clone();
            if let Some(hold) = hold {
                let mut state = hold.state.lock().unwrap();
                state.0 += 1;
                hold.changed.notify_all();
                while !state.1 {
                    state = hold.changed.wait(state).unwrap();
                }
            }
        }
        let mut active = self
            .0
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.check()?;
        *active += 1;
        Ok(LegacyMediaWork(self.clone()))
    }

    /// True when no operation can still be opening or reading a locator.
    #[must_use]
    pub fn is_quiescent(&self) -> bool {
        *self
            .0
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            == 0
    }
    /// Native locator-open count for retained-authority witnesses.
    #[cfg(feature = "test-util")]
    #[must_use]
    pub fn locator_opens(&self) -> usize {
        self.0.opens.load(Ordering::Acquire)
    }
    /// Record an authorized native locator open in a test build.
    #[cfg(feature = "test-util")]
    pub fn record_locator_open(&self) {
        self.0.opens.fetch_add(1, Ordering::AcqRel);
    }

    /// Pause worker admission for a deterministic queued-work witness.
    /// # Panics
    /// Panics if a fixture poisons its barrier.
    #[cfg(feature = "test-util")]
    #[must_use]
    pub fn hold_work(&self) -> LegacyMediaWorkHold {
        let state = Arc::new(WorkHoldState::default());
        *self.0.hold.lock().unwrap() = Some(Arc::clone(&state));
        LegacyMediaWorkHold {
            state,
            grant: self.clone(),
        }
    }
}

#[cfg(feature = "test-util")]
#[derive(Debug, Default)]
struct WorkHoldState {
    state: Mutex<(usize, bool)>,
    changed: std::sync::Condvar,
}

/// A test barrier that releases every waiting worker on drop.
#[cfg(feature = "test-util")]
pub struct LegacyMediaWorkHold {
    state: Arc<WorkHoldState>,
    grant: LegacyMediaGrant,
}
#[cfg(feature = "test-util")]
impl LegacyMediaWorkHold {
    /// Wait for the named number of worker threads to reach admission.
    /// # Panics
    /// Panics if workers do not arrive within thirty seconds.
    pub fn wait(&self, workers: usize) {
        let (state, timeout) = self
            .state
            .changed
            .wait_timeout_while(
                self.state.state.lock().unwrap(),
                std::time::Duration::from_secs(30),
                |state| state.0 < workers,
            )
            .unwrap();
        assert!(
            !timeout.timed_out() && state.0 >= workers,
            "locator workers reached barrier"
        );
    }
}
#[cfg(feature = "test-util")]
impl Drop for LegacyMediaWorkHold {
    fn drop(&mut self) {
        *self.grant.0.hold.lock().unwrap() = None;
        self.state.state.lock().unwrap().1 = true;
        self.state.changed.notify_all();
    }
}

/// Retained source provenance. It cannot be reconstructed from a Document.
#[derive(Clone, Debug)]
pub struct MediaSourceOrigin {
    owned: bool,
    legacy: Option<LegacyMediaGrant>,
}

impl MediaSourceOrigin {
    /// Start an independent, unsaved legacy project.
    #[must_use]
    pub fn new_project() -> Self {
        Self {
            owned: false,
            legacy: Some(LegacyMediaGrant(Arc::default())),
        }
    }

    /// Classify the retained envelope before its Studio field is removed.
    #[must_use]
    pub fn inspect(source: &serde_json::Value) -> Self {
        if source
            .as_object()
            .is_some_and(|object| object.contains_key("studio"))
        {
            Self {
                owned: true,
                legacy: None,
            }
        } else {
            Self::new_project()
        }
    }

    #[must_use]
    pub const fn is_owned(&self) -> bool {
        self.owned
    }

    #[must_use]
    pub fn legacy_media(&self) -> Option<LegacyMediaGrant> {
        self.legacy
            .as_ref()
            .filter(|grant| grant.check().is_ok())
            .cloned()
    }

    /// Revoke copies already handed to background work before activation.
    pub fn revoke_legacy_media(&self) {
        if let Some(grant) = &self.legacy {
            let _active = grant
                .0
                .active
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            grant.0.revoked.store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_envelopes_never_issue_a_legacy_grant_even_without_activation() {
        for studio in [
            serde_json::Value::Null,
            serde_json::json!({"owned_media": {}}),
        ] {
            let origin = MediaSourceOrigin::inspect(&serde_json::json!({"studio": studio}));
            assert!(origin.is_owned());
            assert!(origin.legacy_media().is_none());
        }
    }

    #[test]
    fn activation_revokes_grants_already_captured_by_background_work() {
        let origin = MediaSourceOrigin::new_project();
        let captured = origin.legacy_media().unwrap();
        assert!(captured.check().is_ok());
        origin.revoke_legacy_media();
        assert_eq!(
            captured.check(),
            Err(crate::MediaError::MediaAdmissionRequired)
        );
        assert!(origin.legacy_media().is_none());
    }
}
