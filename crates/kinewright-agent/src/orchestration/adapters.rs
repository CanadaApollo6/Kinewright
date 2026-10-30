//! Certified drivers release provider execution only through the common guardian launcher.
//!
//! Rust adaptation of <https://github.com/pingdotgg/t3code> at
//! 5d91ab1117ad51ef1288a94717d85c70e7af09fe, packages/provider-core/src/server/
//! ProviderAdapter.ts and apps/server/src/orchestration-v2/ProviderAdapterRegistry.ts.
//! Notice: LICENSES/t3code-MIT.txt. Kinewright requires the original durable
//! reservation and native containment before a driver can create a session.
#![allow(clippy::missing_errors_doc)]
use super::{AdmittedAttempt, orchestration_records::ProviderSelectionRecord};
use kinewright_core::{
    AgentDriver, AgentError, AgentSession, HarnessId, HarnessInfo, SessionConfig,
};
use std::sync::Mutex;

/// No deserializer or public constructor can turn CLI installation into certification.
#[derive(Clone)]
pub struct CertifiedConnectorProfile {
    selection: ProviderSelectionRecord,
    config: SessionConfig,
}
impl CertifiedConnectorProfile {
    #[must_use]
    pub fn selection(&self) -> &ProviderSelectionRecord {
        &self.selection
    }
    #[must_use]
    pub fn session_config(&self) -> &SessionConfig {
        &self.config
    }
    #[cfg(test)]
    #[allow(
        dead_code,
        reason = "integration fixture compiles the exact module through its isolated kernel"
    )]
    pub(crate) fn fixture(selection: ProviderSelectionRecord, config: SessionConfig) -> Self {
        Self { selection, config }
    }
}

/// Every connector profile this build certifies: the one list S4 offers for the
/// Director setting. Certification registers each shipping profile here; none is
/// certified yet. Fixture builds also certify the scripted Director, which no
/// shipping launcher can start.
#[must_use]
pub fn certified_connectors() -> Vec<CertifiedConnectorProfile> {
    let shipping: [CertifiedConnectorProfile; 0] = [];
    shipping.into_iter().chain(fixture_connectors()).collect()
}
#[cfg(any(test, feature = "eval-harness"))]
fn fixture_connectors() -> Vec<CertifiedConnectorProfile> {
    vec![CertifiedConnectorProfile {
        selection: ProviderSelectionRecord {
            connector: "scripted".into(),
            model: "fixture".into(),
            options: vec![],
        },
        config: SessionConfig {
            model: Some("fixture".into()),
            ..SessionConfig::default()
        },
    }]
}
#[cfg(not(any(test, feature = "eval-harness")))]
fn fixture_connectors() -> Vec<CertifiedConnectorProfile> {
    Vec::new()
}

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
        profile: CertifiedConnectorProfile,
    ) -> Result<ContainedSession, LaunchError>;
}

pub struct ContainedDriver<L> {
    launcher: L,
    profile: CertifiedConnectorProfile,
    attempt: Mutex<Option<AdmittedAttempt>>,
    info: HarnessInfo,
}
impl<L: GuardianLauncher> ContainedDriver<L> {
    #[must_use]
    pub fn new(
        launcher: L,
        attempt: AdmittedAttempt,
        profile: CertifiedConnectorProfile,
        info: HarnessInfo,
    ) -> Self {
        Self {
            launcher,
            attempt: Mutex::new(Some(attempt)),
            profile,
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
        if cfg != self.profile.config
            || cfg.model.as_deref() != Some(self.profile.selection.model.as_str())
        {
            return Err(AgentError::Unavailable(
                "session differs from certified connector profile".into(),
            ));
        }
        let attempt = self
            .attempt
            .lock()
            .map_err(|_| AgentError::Unavailable("guardian reservation poisoned".into()))?
            .take()
            .ok_or_else(|| {
                AgentError::Unavailable("guardian reservation already consumed".into())
            })?;
        self.launcher
            .launch(attempt, self.profile.clone())
            .map(|contained| contained.session)
            .map_err(|error| AgentError::Unavailable(error.0))
    }
}
