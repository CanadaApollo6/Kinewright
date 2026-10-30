//! Certified drivers release provider execution only through the common guardian launcher.
//!
//! Rust adaptation of <https://github.com/pingdotgg/t3code> at
//! 5d91ab1117ad51ef1288a94717d85c70e7af09fe, packages/provider-core/src/server/
//! ProviderAdapter.ts and apps/server/src/orchestration-v2/ProviderAdapterRegistry.ts.
//! Notice: LICENSES/t3code-MIT.txt. Kinewright requires the original durable
//! reservation and native containment before a driver can create a session.
#![allow(clippy::missing_errors_doc)]
use super::AdmittedAttempt;
use kinewright_core::{
    AgentDriver, AgentError, AgentSession, HarnessId, HarnessInfo, SessionConfig,
};
use std::sync::Mutex;

mod certification;
pub use certification::*;
mod authentication;
pub use authentication::*;
mod invocation;
pub use invocation::*;

#[derive(Debug)]
pub struct LaunchError(pub String);

pub struct ContainedSession {
    session: Box<dyn AgentSession>,
}
impl ContainedSession {
    /// C2 calls only after its OWNED/CONTAINED/COMMIT barriers.
    #[allow(dead_code, reason = "C2 supplies the native containment constructor")]
    pub(crate) fn from_guardian(session: Box<dyn AgentSession>) -> Self {
        Self { session }
    }
}

/// C2 implements this port. No uncontained driver fallback exists here.
pub trait GuardianLauncher: Send + Sync {
    fn launch(
        &self,
        attempt: AdmittedAttempt,
        invocation: AuthorizedInvocation,
    ) -> Result<ContainedSession, LaunchError>;
}

pub struct ContainedDriver<L> {
    launcher: L,
    reservation: Mutex<Option<(AdmittedAttempt, AuthorizedInvocation)>>,
    info: HarnessInfo,
}
impl<L: GuardianLauncher> ContainedDriver<L> {
    #[must_use]
    pub fn new(
        launcher: L,
        attempt: AdmittedAttempt,
        invocation: AuthorizedInvocation,
        info: HarnessInfo,
    ) -> Self {
        Self {
            launcher,
            reservation: Mutex::new(Some((attempt, invocation))),
            info,
        }
    }
}
impl<L: GuardianLauncher> AgentDriver for ContainedDriver<L> {
    fn id(&self) -> HarnessId {
        self.info.id.clone()
    }
    fn detect(&self) -> Option<HarnessInfo> {
        Some(self.info.clone())
    }
    fn start_session(&self, cfg: SessionConfig) -> Result<Box<dyn AgentSession>, AgentError> {
        let mut reservation = self
            .reservation
            .lock()
            .map_err(|_| AgentError::Unavailable("guardian reservation poisoned".into()))?;
        let (attempt, invocation) = reservation.as_ref().ok_or_else(|| {
            AgentError::Unavailable("guardian reservation already consumed".into())
        })?;
        invocation
            .validate(&cfg, attempt)
            .map_err(|refusal| AgentError::Unavailable(refusal.to_string()))?;
        let (attempt, invocation) = reservation.take().expect("validated reservation");
        drop(reservation);
        self.launcher
            .launch(attempt, invocation)
            .map(|contained| contained.session)
            .map_err(|error| AgentError::Unavailable(error.0))
    }
}
