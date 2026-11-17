//! Runtime endpoint identity and attachment state.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ActorIdentity(pub(crate) u64);

impl ActorIdentity {
    /// Allocate a process-local endpoint identity. The project actor uses this
    /// domain for endpoint fencing.
    #[must_use]
    pub fn fresh() -> Option<Self> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_ACTOR: AtomicU64 = AtomicU64::new(1);
        #[allow(deprecated, reason = "MSRV predates AtomicU64::try_update")]
        NEXT_ACTOR
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .ok()
            .map(Self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentState {
    Live,
    Frozen,
    Sealed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CoreMutationRefusal {
    #[error("editing is paused while the project is checked")]
    Frozen,
    #[error("this editing session is read-only after the handoff")]
    Sealed,
    #[error("history installation requires the bound attachment owner")]
    OwnerRequired,
    #[error("a current manual producer permit is required")]
    ManualPermitRequired,
    #[error("Saving is catching up. Editing is paused; retry when the pending history is saved.")]
    ManualBackpressure,
    #[error("This input must be admitted by the project writer before the edit can be accepted.")]
    ManualInputRequired,
    #[error(
        "This edit exceeds the project's history capacity. Try a smaller edit or start a new project."
    )]
    ManualHistoryCapacity,
}
