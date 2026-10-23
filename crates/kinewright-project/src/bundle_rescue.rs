//! A1 read-only observations and bounded admission; rescue publication lands separately.
#![allow(clippy::missing_errors_doc)]
use crate::{
    AttemptBinding,
    bundle::{BundleError, PublicationState},
    bundle_records::ObjectRef,
};
use kinewright_core::{AcceptedCut, HistorySaveTag};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReconcilePhase {
    #[default]
    Namespace,
    Authority,
    RootRead,
    Graph,
    Barrier,
    Cleanup,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RescueCause {
    Unavailable,
    RequiredBytes,
    Persistence,
    Interrupted,
    Contention,
    Invalid,
    Unsupported,
    CleanupOnly,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RescueEligibility {
    Eligible,
    Ineligible,
}

/// The original error is retained, including its native errno, rather than a synthesized path error.
#[derive(Debug)]
pub struct RescueProbe {
    pub tag: HistorySaveTag,
    pub ticket: Option<AttemptBinding>,
    pub phase: ReconcilePhase,
    pub operation: &'static str,
    pub cause: RescueCause,
    pub eligibility: RescueEligibility,
    pub error: BundleError,
}
impl RescueProbe {
    pub(crate) fn failed(
        tag: HistorySaveTag,
        ticket: Option<AttemptBinding>,
        phase: ReconcilePhase,
        state: PublicationState,
        error: BundleError,
    ) -> Self {
        let cause = classify(phase, &error);
        let eligibility = if state == PublicationState::Unknown
            && matches!(
                cause,
                RescueCause::Unavailable | RescueCause::RequiredBytes | RescueCause::Persistence
            ) {
            RescueEligibility::Eligible
        } else {
            RescueEligibility::Ineligible
        };
        crate::namespace::note("rescue_probe", &tag.destination);
        let operation = match phase {
            ReconcilePhase::Namespace => "namespace identity",
            ReconcilePhase::Authority => "verify retained authority",
            ReconcilePhase::RootRead => "read root",
            ReconcilePhase::Graph => "validate required graph/receipts",
            ReconcilePhase::Barrier => "reconcile publication barrier",
            ReconcilePhase::Cleanup => "cleanup",
        };
        Self {
            tag,
            ticket,
            phase,
            operation,
            cause,
            eligibility,
            error,
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    #[test]
    fn w03_cause_table_preserves_native_errors_and_exclusions() {
        let tag = HistorySaveTag {
            destination: "a.kinewright".into(),
            parent_device: 1,
            parent_inode: 1,
            root_sha256: "a".repeat(64),
            previous_root_sha256: None,
        };
        for (errno, phase, eligible) in [
            (libc::ENOENT, ReconcilePhase::Namespace, true),
            (libc::EACCES, ReconcilePhase::RootRead, true),
            (libc::ENODEV, ReconcilePhase::Namespace, true),
            (libc::EIO, ReconcilePhase::Graph, true),
            (libc::ESTALE, ReconcilePhase::Namespace, true),
            (libc::ETIMEDOUT, ReconcilePhase::Barrier, true),
            (libc::EROFS, ReconcilePhase::Barrier, true),
            (libc::ENOSPC, ReconcilePhase::Barrier, true),
            (libc::EDQUOT, ReconcilePhase::Barrier, true),
            (libc::EUCLEAN, ReconcilePhase::Graph, true),
            (libc::EINTR, ReconcilePhase::RootRead, false),
            (libc::EAGAIN, ReconcilePhase::RootRead, false),
            (libc::EBUSY, ReconcilePhase::Namespace, false),
            (libc::EBADF, ReconcilePhase::RootRead, false),
            (libc::EINVAL, ReconcilePhase::Barrier, false),
            (libc::ENOSPC, ReconcilePhase::RootRead, false),
            (libc::EIO, ReconcilePhase::Cleanup, false),
        ] {
            let probe = RescueProbe::failed(
                tag.clone(),
                None,
                phase,
                PublicationState::Unknown,
                BundleError::Io(std::io::Error::from_raw_os_error(errno)),
            );
            assert_eq!(
                probe.eligibility == RescueEligibility::Eligible,
                eligible,
                "errno={errno}, phase={phase:?}"
            );
            assert!(
                matches!(probe.error, BundleError::Io(ref error) if error.raw_os_error() == Some(errno))
            );
        }
        for state in [
            PublicationState::NeverAttempted,
            PublicationState::DefinitelyRefused,
            PublicationState::Committed,
        ] {
            let probe = RescueProbe::failed(
                tag.clone(),
                None,
                ReconcilePhase::Namespace,
                state,
                BundleError::Io(std::io::Error::from_raw_os_error(libc::EIO)),
            );
            assert_eq!(probe.eligibility, RescueEligibility::Ineligible);
        }
        for (phase, expected) in [
            (ReconcilePhase::Graph, RescueEligibility::Eligible),
            (ReconcilePhase::RootRead, RescueEligibility::Ineligible),
            (ReconcilePhase::Cleanup, RescueEligibility::Ineligible),
        ] {
            assert_eq!(
                RescueProbe::failed(
                    tag.clone(),
                    None,
                    phase,
                    PublicationState::Unknown,
                    BundleError::Corrupt("required bytes".into())
                )
                .eligibility,
                expected
            );
        }
        for error in [
            BundleError::DurabilityUnsupported,
            BundleError::HistoryBarrierInvariant,
            BundleError::Contention("held".into()),
            BundleError::CleanupPending("held".into()),
        ] {
            assert_eq!(
                RescueProbe::failed(
                    tag.clone(),
                    None,
                    ReconcilePhase::Barrier,
                    PublicationState::Unknown,
                    error
                )
                .eligibility,
                RescueEligibility::Ineligible
            );
        }
    }
}
fn classify(phase: ReconcilePhase, error: &BundleError) -> RescueCause {
    use RescueCause as C;
    if phase == ReconcilePhase::Cleanup {
        return C::CleanupOnly;
    }
    match error {
        BundleError::Io(error) => {
            use std::io::ErrorKind as K;
            match error.kind() {
                K::Interrupted => C::Interrupted,
                K::WouldBlock | K::ResourceBusy => C::Contention,
                K::NotFound | K::PermissionDenied | K::TimedOut => C::Unavailable,
                _ => {
                    #[cfg(target_os = "linux")]
                    {
                        match error.raw_os_error() {
                            Some(
                                libc::ENODEV
                                | libc::ENXIO
                                | libc::EIO
                                | libc::ESTALE
                                | libc::ESHUTDOWN,
                            ) => return C::Unavailable,
                            Some(libc::EROFS | libc::EUCLEAN) => return C::Persistence,
                            Some(libc::ENOSPC | libc::EDQUOT)
                                if phase == ReconcilePhase::Barrier =>
                            {
                                return C::Persistence;
                            }
                            _ => {}
                        }
                    }
                    C::Invalid
                }
            }
        }
        BundleError::Corrupt(_) if phase == ReconcilePhase::Graph => C::RequiredBytes,
        BundleError::Identity(_) if phase == ReconcilePhase::Namespace => C::Unavailable,
        BundleError::DurabilityUnknown if phase == ReconcilePhase::Barrier => C::Persistence,
        BundleError::Contention(_) => C::Contention,
        BundleError::Unsupported(_) | BundleError::DurabilityUnsupported => C::Unsupported,
        BundleError::CleanupPending(_) => C::CleanupOnly,
        _ => C::Invalid,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub enum MissingOwnedUse {
    Current,
    Undo { branch: String, cut: u64 },
    Redo { branch: String, cut: u64 },
    Branch { id: String },
    Recovery { root_sha256: String },
    Manifest,
    Evidence { name: String },
}

/// A page over a frozen inventory. A continuation is diagnostic, never a publishable graph.
#[derive(Debug)]
pub struct RescueMissingOwnedBytes {
    pub entries: Vec<(ObjectRef, Vec<MissingOwnedUse>, String)>,
    pub omitted: usize,
    pub next: Option<usize>,
    pub dependencies_complete: bool,
}
impl std::fmt::Display for RescueMissingOwnedBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "rescue_missing_owned_bytes: {} items; {} omitted; dependencies_complete={}",
            self.entries.len(),
            self.omitted,
            self.dependencies_complete
        )
    }
}
impl std::error::Error for RescueMissingOwnedBytes {}

/// No new durable record: this owns one actor cut and the exact admitted source reads.
pub struct FrozenRescueCapture {
    pub(crate) cut: AcceptedCut,
    pub(crate) tag: HistorySaveTag,
    pub(crate) digest: String,
    pub(crate) reads: crate::store::RetainedReadAdmission,
    pub(crate) was_frozen: bool,
}
impl FrozenRescueCapture {
    #[must_use]
    pub const fn cut(&self) -> &AcceptedCut {
        &self.cut
    }
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }
    #[must_use]
    pub const fn reads(&self) -> &crate::store::RetainedReadAdmission {
        &self.reads
    }
}
/// An explicit local choice, bound to one exact capture. Background probes cannot create it.
pub struct RescueConsent {
    pub(crate) tag: HistorySaveTag,
    pub(crate) digest: String,
}
impl FrozenRescueCapture {
    #[must_use]
    pub fn consent(&self) -> RescueConsent {
        RescueConsent {
            tag: self.tag.clone(),
            digest: self.digest.clone(),
        }
    }
    pub fn check_consent(&self, consent: &RescueConsent) -> Result<(), BundleError> {
        if self.tag != consent.tag || self.digest != consent.digest {
            return Err(BundleError::HistoryChanged);
        }
        Ok(())
    }
}
