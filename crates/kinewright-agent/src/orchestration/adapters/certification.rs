//! Connector policy is compiled data. Host bindings and probe results are attempt-local.
#![allow(clippy::missing_errors_doc)]
use super::super::{
    AdmittedAttempt, ExecutionIdentity, orchestration_records::ProviderSelectionRecord,
};
use super::{EffectiveAuthEvidence, EffectiveLaunchContext, ProviderAuthContract};
use kinewright_core::SessionConfig;
use std::path::{Path, PathBuf};

use super::super::task_tool_names::TASK_TOOL_NAMES;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CliVersion(pub u32, pub u32, pub u32);
impl CliVersion {
    /// Only an observed release version, never a version requirement or prerelease.
    pub fn parse(value: &str) -> Result<Self, CertificationRefusal> {
        let parts: Vec<_> = value.split('.').collect();
        if parts.len() != 3
            || parts.iter().any(|part| {
                part.is_empty()
                    || !part.bytes().all(|b| b.is_ascii_digit())
                    || (part.len() > 1 && part.starts_with('0'))
            })
        {
            return Err(CertificationRefusal::UnqualifiedVersion);
        }
        let parse = |part: &str| {
            part.parse()
                .map_err(|_| CertificationRefusal::UnqualifiedVersion)
        };
        Ok(Self(parse(parts[0])?, parse(parts[1])?, parse(parts[2])?))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthenticationMode {
    Subscription,
    ApiKey,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpPolicy {
    HostEndpointOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxPolicy {
    LinuxFilesystemAndIpc,
    WindowsFilesystemAndIpc,
    #[cfg(any(test, feature = "test-util"))]
    Fixture,
}

#[derive(Debug)]
struct SessionPolicy {
    model: &'static str,
    options: &'static [(&'static str, &'static str)],
    effort: Option<&'static str>,
    service_tier: Option<&'static str>,
    max_turns: u32,
    tools: &'static [&'static str],
}

/// No deserializer or public constructor. Only connectors with a real adapter
/// and launch recipe exist; a newer CLI is allowed, and its unknown stream
/// events, sign-in modes or ambient sources fail closed at run time.
#[derive(Debug)]
pub struct ConnectorSpecification {
    provider: &'static str,
    /// The oldest CLI whose stream, flags and sign-in this adapter knows.
    minimum_version: CliVersion,
    session: SessionPolicy,
    default_token_ceiling: std::num::NonZeroU64,
    mcp: McpPolicy,
    sandbox: SandboxPolicy,
    authentication: AuthenticationMode,
    auth_contract: ProviderAuthContract,
}
impl ConnectorSpecification {
    #[must_use]
    pub fn provider(&self) -> &'static str {
        self.provider
    }
    #[must_use]
    pub fn selection(&self) -> ProviderSelectionRecord {
        ProviderSelectionRecord {
            connector: self.provider.into(),
            model: self.session.model.into(),
            options: self
                .session
                .options
                .iter()
                .map(
                    |(k, v)| super::super::orchestration_records::ProviderOptionRecord {
                        name: (*k).into(),
                        value: (*v).into(),
                    },
                )
                .collect(),
        }
    }
    #[must_use]
    pub fn default_token_ceiling(&self) -> std::num::NonZeroU64 {
        self.default_token_ceiling
    }
    #[must_use]
    pub fn minimum_version(&self) -> CliVersion {
        self.minimum_version
    }
    #[must_use]
    pub fn mcp_policy(&self) -> McpPolicy {
        self.mcp
    }
    #[must_use]
    pub fn sandbox_policy(&self) -> SandboxPolicy {
        self.sandbox
    }
    #[must_use]
    pub fn required_authentication(&self) -> AuthenticationMode {
        self.authentication
    }
    #[must_use]
    pub fn auth_contract(&self) -> ProviderAuthContract {
        self.auth_contract
    }
    pub(crate) fn session_config(&self, binding: &AttemptBinding) -> SessionConfig {
        let selection = self.bound_selection(binding);
        let option = |name: &str| {
            selection
                .options
                .iter()
                .find(|v| v.name == name)
                .map(|v| v.value.clone())
        };
        let tools = binding
            .task_policy
            .as_ref()
            .map_or(self.session.tools, |(_, role)| {
                crate::orchestration::task_manifest::names(*role)
            });
        SessionConfig {
            model: Some(selection.model),
            effort: option(if self.provider == "codex" {
                "model_reasoning_effort"
            } else {
                "effort"
            })
            .or_else(|| self.session.effort.map(Into::into)),
            service_tier: option("service_tier")
                .or_else(|| self.session.service_tier.map(Into::into)),
            max_turns: Some(self.session.max_turns),
            tool_names: Some(tools.iter().map(|s| (*s).into()).collect()),
            working_directory: Some(binding.working_directory.clone()),
            mcp_url: Some(binding.endpoint.clone()),
        }
    }

    fn bound_selection(&self, binding: &AttemptBinding) -> ProviderSelectionRecord {
        binding
            .task_policy
            .as_ref()
            .map_or_else(|| self.selection(), |(selection, _)| selection.clone())
    }

    fn registered(&self) -> bool {
        let known = execution_specifications()
            .iter()
            .any(|spec| std::ptr::eq(*spec, self));
        #[cfg(all(target_os = "linux", any(test, feature = "test-util")))]
        let known = known
            || std::ptr::eq(self, codex_candidate())
            || std::ptr::eq(self, claude_candidate());
        known
    }

    #[allow(
        dead_code,
        reason = "C2/C5 supply the trusted local probes and host binding"
    )]
    pub(crate) fn bind(
        &'static self,
        binding: AttemptBinding,
        checks: &dyn ConnectorRuntimeChecks,
    ) -> Result<CertifiedConnectorProfile, CertificationRefusal> {
        if !self.registered() {
            return Err(CertificationRefusal::UnqualifiedSpecification);
        }
        binding.validate()?;
        if let Some((selection, _)) = &binding.task_policy
            && (selection.connector != self.provider
                || !super::super::specialist_setting::specialist_choices().contains(selection))
        {
            return Err(CertificationRefusal::ModelUnavailable);
        }
        let runtime = checks.probe(self, &binding)?;
        self.validate_runtime(&runtime)?;
        Ok(CertifiedConnectorProfile {
            specification: self,
            config: self.session_config(&binding),
            binding,
            runtime,
        })
    }

    pub(crate) fn validate_runtime(
        &self,
        runtime: &RuntimeQualification,
    ) -> Result<(), CertificationRefusal> {
        if runtime.version < self.minimum_version {
            return Err(CertificationRefusal::UnqualifiedVersion);
        }
        runtime.authentication.validate(
            self.auth_contract,
            &runtime.launch_context,
            &runtime.executable,
        )?;
        if self.authentication != AuthenticationMode::Subscription {
            return Err(CertificationRefusal::UnqualifiedSpecification);
        }
        if !runtime.executable.path.is_absolute() || runtime.executable.file_identity == [0; 2] {
            return Err(CertificationRefusal::ExecutableIdentity);
        }
        if runtime.sandbox != Some(self.sandbox) {
            return Err(CertificationRefusal::SandboxUnavailable);
        }
        if !runtime.model_available {
            return Err(CertificationRefusal::ModelUnavailable);
        }
        Ok(())
    }
}

/// A fixture runtime for tests: a real executable path with fixture
/// authentication and sandbox. Tests change one field to probe one check.
#[cfg(test)]
pub(crate) fn test_runtime() -> RuntimeQualification {
    let executable = ExecutableIdentity {
        path: std::env::current_exe().unwrap(),
        file_identity: [1, 2],
    };
    let (launch_context, authentication) = super::authentication::fixture(&executable);
    RuntimeQualification {
        version: CliVersion(0, 147, 0),
        authentication,
        launch_context,
        executable,
        sandbox: Some(SandboxPolicy::Fixture),
        model_available: true,
    }
}

/// Every connector this build can launch: a real adapter and launch recipe on
/// this platform. Independent of local detection; the person picks one
/// explicitly and nothing is substituted for a saved choice.
#[must_use]
pub fn available_connectors() -> Vec<&'static ConnectorSpecification> {
    #[cfg(target_os = "linux")]
    {
        vec![codex_candidate(), claude_candidate()]
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

/// Explicit fixture ports only. Never used by product choices or readiness.
#[cfg(any(test, feature = "test-util"))]
#[doc(hidden)]
#[must_use]
pub fn fixture_connectors() -> Vec<&'static ConnectorSpecification> {
    vec![&FIXTURE_SPECIFICATION]
}

fn execution_specifications() -> Vec<&'static ConnectorSpecification> {
    let specifications = available_connectors();
    #[cfg(any(test, feature = "test-util"))]
    let specifications: Vec<_> = specifications
        .into_iter()
        .chain(fixture_connectors())
        .chain([&CLAUDE_FIXTURE_SPECIFICATION])
        .collect();
    specifications
}

#[allow(
    dead_code,
    reason = "C2/C5 supply the trusted local probes and host binding"
)]
pub(crate) fn certify_connector(
    provider: &str,
    binding: AttemptBinding,
    checks: &dyn ConnectorRuntimeChecks,
) -> Result<CertifiedConnectorProfile, CertificationRefusal> {
    execution_specifications()
        .into_iter()
        .find(|spec| spec.provider == provider)
        .ok_or(CertificationRefusal::UnknownProvider)?
        .bind(binding, checks)
}

/// Constructed by the host after endpoint/roots authorization, never from settings.
/// These roots describe MCP authority; they grant no provider filesystem access.
#[derive(Clone, PartialEq, Eq)]
pub struct AttemptBinding {
    identity: ExecutionIdentity,
    endpoint: String,
    roots: Vec<PathBuf>,
    working_directory: PathBuf,
    task_policy: Option<(
        ProviderSelectionRecord,
        super::super::orchestration_records::Role,
    )>,
}
impl AttemptBinding {
    #[allow(
        dead_code,
        reason = "C2/C4 wire the authorized host endpoint and scratch directory"
    )]
    pub(crate) fn issue(
        attempt: &AdmittedAttempt,
        endpoint: String,
        roots: Vec<PathBuf>,
        working_directory: PathBuf,
    ) -> Result<Self, CertificationRefusal> {
        let binding = Self {
            identity: attempt.identity().clone(),
            endpoint,
            roots,
            working_directory,
            task_policy: None,
        };
        binding.validate()?;
        Ok(binding)
    }
    /// Only the host may bind the durable task's person-owned selection and role.
    #[cfg(target_os = "linux")]
    pub(crate) fn for_task(
        mut self,
        selection: ProviderSelectionRecord,
        role: super::super::orchestration_records::Role,
    ) -> Self {
        self.task_policy = Some((selection, role));
        self
    }
    fn validate(&self) -> Result<(), CertificationRefusal> {
        let url = reqwest::Url::parse(&self.endpoint)
            .map_err(|_| CertificationRefusal::InvalidBinding)?;
        if self.identity.validate().is_err()
            || self.endpoint.len() > 4096
            || url.scheme() != "http"
            || !matches!(url.host_str(), Some("127.0.0.1" | "[::1]"))
            || url.port().is_none_or(|port| port == 0)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || !self.working_directory.is_absolute()
            || self.roots.len() > 32
            || self.roots.iter().any(|root| !root.is_absolute())
        {
            return Err(CertificationRefusal::InvalidBinding);
        }
        Ok(())
    }
    #[must_use]
    pub fn identity(&self) -> &ExecutionIdentity {
        &self.identity
    }
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
    #[must_use]
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }
    #[must_use]
    pub fn working_directory(&self) -> &Path {
        &self.working_directory
    }
}

/// Native file identity (volume/file index on Windows, device/inode on Unix).
/// C2 must retain/recheck the opened executable through spawn; a path is insufficient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableIdentity {
    pub path: PathBuf,
    pub file_identity: [u64; 2],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeQualification {
    pub version: CliVersion,
    pub authentication: EffectiveAuthEvidence,
    pub launch_context: EffectiveLaunchContext,
    pub executable: ExecutableIdentity,
    pub sandbox: Option<SandboxPolicy>,
    pub model_available: bool,
}

/// Trusted local probes supplied by C2/C5. No CLI invocation or paid discovery here.
pub trait ConnectorRuntimeChecks {
    /// Unpaid local installation, version and subscription checks, without an attempt.
    fn preflight(&self, _: &ConnectorSpecification) -> Result<(), CertificationRefusal> {
        Err(CertificationRefusal::UnqualifiedSpecification)
    }
    fn probe(
        &self,
        specification: &ConnectorSpecification,
        binding: &AttemptBinding,
    ) -> Result<RuntimeQualification, CertificationRefusal>;
}

/// Readiness never inserts a release into the certification registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorReadiness {
    Install,
    SignIn,
    CheckAgain,
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CertificationRefusal {
    #[error("unknown connector provider")]
    UnknownProvider,
    #[error("connector specification is not available in this build")]
    UnqualifiedSpecification,
    #[error("connector profile was forged or modified")]
    ForgedProfile,
    #[error("CLI version is older than the oldest one this connector supports")]
    UnqualifiedVersion,
    #[error("API-key authentication is not allowed")]
    ApiAuthentication,
    #[error("subscription authentication could not be established")]
    UnknownAuthentication,
    #[error("subscription proof differs from the effective launch/refresh context")]
    EffectiveAuthentication,
    #[error("host attempt binding is invalid")]
    InvalidBinding,
    #[error("host attempt binding changed after certification")]
    BindingChanged,
    #[error("session policy differs from the connector specification")]
    FixedPolicyChanged,
    #[error("executable identity is unavailable or changed")]
    ExecutableIdentity,
    #[error("{0}")]
    ExecutableResolution(String),
    #[error("the connector's sandbox is unavailable")]
    SandboxUnavailable,
    #[error("the connector's model is unavailable")]
    ModelUnavailable,
    #[error(
        "an earlier CLI version check could not be proven stopped; it is stopped and cleaned up before another check is admitted"
    )]
    VersionProbePending,
}

/// Immutable and runtime-only. Only a registered specification can construct it.
///
/// ```compile_fail
/// use kinewright_agent::orchestration::adapters::CertifiedConnectorProfile;
/// let profile = CertifiedConnectorProfile {};
/// ```
/// ```compile_fail
/// use kinewright_agent::orchestration::adapters::CertifiedConnectorProfile;
/// let profile: CertifiedConnectorProfile = serde_json::from_str("{}").unwrap();
/// ```
#[derive(Clone)]
pub struct CertifiedConnectorProfile {
    specification: &'static ConnectorSpecification,
    config: SessionConfig,
    binding: AttemptBinding,
    runtime: RuntimeQualification,
}
impl CertifiedConnectorProfile {
    #[cfg(target_os = "linux")]
    pub(crate) fn runtime(&self) -> &RuntimeQualification {
        &self.runtime
    }

    #[must_use]
    pub fn selection(&self) -> ProviderSelectionRecord {
        self.specification.bound_selection(&self.binding)
    }
    #[must_use]
    pub fn specification(&self) -> &'static ConnectorSpecification {
        self.specification
    }
    #[must_use]
    pub fn session_config(&self) -> &SessionConfig {
        &self.config
    }
    #[must_use]
    pub fn binding(&self) -> &AttemptBinding {
        &self.binding
    }

    pub fn validate_binding(&self, binding: &AttemptBinding) -> Result<(), CertificationRefusal> {
        if binding != &self.binding {
            return Err(CertificationRefusal::BindingChanged);
        }
        binding.validate()
    }

    /// Shared driver boundary, before consuming the durable reservation or launching.
    pub fn validate_session(
        &self,
        cfg: &SessionConfig,
        identity: &ExecutionIdentity,
    ) -> Result<(), CertificationRefusal> {
        if !self.specification.registered()
            || self.config != self.specification.session_config(&self.binding)
        {
            return Err(CertificationRefusal::ForgedProfile);
        }
        self.specification.validate_runtime(&self.runtime)?;
        self.binding.validate()?;
        if cfg.mcp_url != self.config.mcp_url
            || cfg.working_directory != self.config.working_directory
            || identity != self.binding.identity()
        {
            return Err(CertificationRefusal::BindingChanged);
        }
        let mut fixed = cfg.clone();
        fixed.mcp_url.clone_from(&self.config.mcp_url);
        fixed
            .working_directory
            .clone_from(&self.config.working_directory);
        if fixed != self.config {
            return Err(CertificationRefusal::FixedPolicyChanged);
        }
        Ok(())
    }

    /// C2's launcher rechecks in its spawn critical section and retains the native handle.
    pub fn recheck_runtime(
        &self,
        checks: &dyn ConnectorRuntimeChecks,
    ) -> Result<(), CertificationRefusal> {
        self.validate_session(&self.config, self.binding.identity())?;
        let runtime = checks.probe(self.specification, &self.binding)?;
        self.specification.validate_runtime(&runtime)?;
        if runtime.executable != self.runtime.executable {
            return Err(CertificationRefusal::ExecutableIdentity);
        }
        if runtime.launch_context != self.runtime.launch_context {
            return Err(CertificationRefusal::EffectiveAuthentication);
        }
        Ok(())
    }

    #[cfg(any(test, all(target_os = "linux", feature = "test-util")))]
    #[allow(
        dead_code,
        reason = "isolated kernel integration fixture uses the production factory"
    )]
    pub(crate) fn fixture(attempt: &AdmittedAttempt) -> Self {
        struct Checks;
        impl ConnectorRuntimeChecks for Checks {
            fn probe(
                &self,
                _: &ConnectorSpecification,
                binding: &AttemptBinding,
            ) -> Result<RuntimeQualification, CertificationRefusal> {
                let executable = ExecutableIdentity {
                    path: binding.working_directory.join("scripted"),
                    file_identity: [1, 2],
                };
                let (launch_context, authentication) = super::authentication::fixture(&executable);
                Ok(RuntimeQualification {
                    version: CliVersion(0, 147, 0),
                    authentication,
                    launch_context,
                    executable,
                    sandbox: Some(SandboxPolicy::Fixture),
                    model_available: true,
                })
            }
        }
        let root = std::env::current_dir().unwrap();
        let binding = AttemptBinding::issue(
            attempt,
            "http://127.0.0.1:12345/mcp".into(),
            vec![root.clone()],
            root,
        )
        .unwrap();
        certify_connector("scripted", binding, &Checks).unwrap()
    }
}

#[cfg(any(test, feature = "test-util"))]
static FIXTURE_SPECIFICATION: ConnectorSpecification = ConnectorSpecification {
    provider: "scripted",
    minimum_version: CliVersion(0, 147, 0),
    session: SessionPolicy {
        model: "fixture",
        options: &[],
        effort: None,
        service_tier: None,
        max_turns: 1,
        tools: TASK_TOOL_NAMES,
    },
    mcp: McpPolicy::HostEndpointOnly,
    default_token_ceiling: std::num::NonZeroU64::new(2_000_000).unwrap(),
    sandbox: SandboxPolicy::Fixture,
    authentication: AuthenticationMode::Subscription,
    auth_contract: ProviderAuthContract {
        provider: "scripted",
        proof: super::SubscriptionProof::Fixture,
        refresh: super::authentication::REFRESH,
    },
};

/// C6: the protocol fixture child speaking Claude's `stream-json` dialect,
/// under the same guardian, sealed image and sandbox as `scripted`.
#[cfg(any(test, feature = "test-util"))]
static CLAUDE_FIXTURE_SPECIFICATION: ConnectorSpecification = ConnectorSpecification {
    provider: "scripted-claude",
    minimum_version: FIXTURE_SPECIFICATION.minimum_version,
    session: SessionPolicy {
        model: "fixture",
        options: &[],
        effort: None,
        service_tier: None,
        max_turns: 1,
        tools: FIXTURE_SPECIFICATION.session.tools,
    },
    mcp: McpPolicy::HostEndpointOnly,
    default_token_ceiling: std::num::NonZeroU64::new(2_000_000).unwrap(),
    sandbox: SandboxPolicy::Fixture,
    authentication: AuthenticationMode::Subscription,
    auth_contract: ProviderAuthContract {
        provider: "scripted",
        proof: super::SubscriptionProof::Fixture,
        refresh: super::authentication::REFRESH,
    },
};

#[cfg(target_os = "linux")]
pub(crate) fn codex_candidate() -> &'static ConnectorSpecification {
    &CODEX_CANDIDATE
}

/// Explicit candidate-binding entry point retained for containment witnesses.
#[cfg(all(target_os = "linux", any(test, feature = "test-util")))]
pub(crate) fn certify_candidate(
    candidate: &'static ConnectorSpecification,
    binding: AttemptBinding,
    checks: &dyn ConnectorRuntimeChecks,
) -> Result<CertifiedConnectorProfile, CertificationRefusal> {
    if !std::ptr::eq(candidate, codex_candidate()) && !std::ptr::eq(candidate, claude_candidate()) {
        return Err(CertificationRefusal::UnqualifiedSpecification);
    }
    candidate.bind(binding, checks)
}

/// The hardened Claude Code connector, available on Linux. Local runtime
/// checks still refuse unknown sign-in modes, versions and ambient authority.
#[cfg(target_os = "linux")]
pub(crate) fn claude_candidate() -> &'static ConnectorSpecification {
    &CLAUDE_CANDIDATE
}

#[cfg(target_os = "linux")]
static CLAUDE_CANDIDATE: ConnectorSpecification = ConnectorSpecification {
    provider: "claude",
    minimum_version: CliVersion(2, 1, 316),
    session: SessionPolicy {
        model: "claude-opus-5-5",
        options: &[("effort", "high")],
        effort: Some("high"),
        service_tier: None,
        max_turns: 1,
        tools: CODEX_CANDIDATE.session.tools,
    },
    mcp: McpPolicy::HostEndpointOnly,
    default_token_ceiling: std::num::NonZeroU64::new(2_000_000).unwrap(),
    sandbox: SandboxPolicy::LinuxFilesystemAndIpc,
    authentication: AuthenticationMode::Subscription,
    auth_contract: super::CLAUDE_AUTH_CONTRACT,
};

#[cfg(target_os = "linux")]
static CODEX_CANDIDATE: ConnectorSpecification = ConnectorSpecification {
    provider: "codex",
    minimum_version: CliVersion(0, 166, 0),
    session: SessionPolicy {
        model: "gpt-6.1-sol",
        options: &[("model_reasoning_effort", "high")],
        effort: Some("high"),
        service_tier: None,
        max_turns: 1,
        tools: TASK_TOOL_NAMES,
    },
    mcp: McpPolicy::HostEndpointOnly,
    default_token_ceiling: std::num::NonZeroU64::new(2_000_000).unwrap(),
    sandbox: SandboxPolicy::LinuxFilesystemAndIpc,
    authentication: AuthenticationMode::Subscription,
    auth_contract: super::CODEX_AUTH_CONTRACT,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> AttemptBinding {
        let root = std::env::current_dir().unwrap();
        AttemptBinding {
            identity: serde_json::from_value(serde_json::json!({
                "version": 1, "project": "00000000000000000000000000000001",
                "run": "00000000000000000000000000000002",
                "task": "00000000000000000000000000000003",
                "attempt": "00000000000000000000000000000004",
                "nonce": "00000000000000000000000000000005",
                "owner_instance": "00000000000000000000000000000006"
            }))
            .unwrap(),
            endpoint: "http://127.0.0.1:12345/mcp".into(),
            roots: vec![root.clone()],
            working_directory: root,
            task_policy: None,
        }
    }
    #[cfg(target_os = "linux")]
    fn native_runtime(spec: &ConnectorSpecification) -> RuntimeQualification {
        let mut runtime = test_runtime();
        runtime.version = spec.minimum_version();
        runtime.sandbox = Some(SandboxPolicy::LinuxFilesystemAndIpc);
        let scope = &mut runtime.launch_context.credential_store;
        scope.refresh = spec.auth_contract().refresh;
        scope.lock_files.clear();
        scope.temporary_file_prefixes.clear();
        scope.credential_files = vec![
            if spec.provider() == "codex" {
                "auth.json"
            } else {
                ".credentials.json"
            }
            .into(),
        ];
        if spec.provider() == "codex" {
            scope.single_file = Some(super::super::CredentialFile {
                source: scope.root.join("fixture-auth"),
                identity: [1, 2],
            });
        }
        runtime.authentication.contract = spec.auth_contract();
        runtime.authentication.context = runtime.launch_context.clone();
        runtime.authentication.observed_store = runtime.launch_context.credential_store.clone();
        runtime
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn te15_b1_native_profiles_pin_model_options_role_and_reject_tampering() {
        use super::super::super::orchestration_records::Role;
        for (spec, priority) in [
            (codex_candidate(), false),
            (codex_candidate(), true),
            (claude_candidate(), false),
        ] {
            for role in [
                Role::Director,
                Role::Editor,
                Role::Reviewer,
                Role::Motion,
                Role::Audio,
                Role::Colour,
            ] {
                let mut selection = if spec.provider() == "claude" {
                    super::super::super::specialist_setting::specialist_choices()
                        .into_iter()
                        .find(|s| s.model == "claude-sonnet-5-5" && s.options[0].value == "low")
                        .unwrap()
                } else {
                    spec.selection()
                };
                if priority {
                    selection.options.push(
                        super::super::super::orchestration_records::ProviderOptionRecord {
                            name: "service_tier".into(),
                            value: "priority".into(),
                        },
                    );
                }
                let binding = binding().for_task(selection.clone(), role);
                let runtime = native_runtime(spec);
                let profile = spec.bind(binding.clone(), &runtime).unwrap();
                assert_eq!(profile.selection(), selection, "TE15_SELECTION_PIN");
                assert_eq!(
                    profile.session_config().tool_names.as_ref().unwrap(),
                    crate::orchestration::task_manifest::names(role),
                    "TE15_ROLE_PIN"
                );
                assert_eq!(
                    profile.session_config().effort.as_deref(),
                    Some(selection.options[0].value.as_str())
                );
                profile
                    .validate_session(profile.session_config(), binding.identity())
                    .unwrap();
                profile.recheck_runtime(&runtime).unwrap();
                assert_eq!(
                    profile.session_config().service_tier.as_deref(),
                    priority.then_some("priority"),
                    "TE15_FIX_TIER_PIN"
                );
                for changed in ["model", "effort", "tier", "tools"] {
                    let mut config = profile.session_config().clone();
                    match changed {
                        "model" => config.model = Some("other".into()),
                        "effort" => config.effort = Some("ultracode".into()),
                        "tier" => config.service_tier = Some("other".into()),
                        _ => config.tool_names = Some(vec!["invoke_capability".into()]),
                    }
                    assert_eq!(
                        profile.validate_session(&config, binding.identity()),
                        Err(CertificationRefusal::FixedPolicyChanged),
                        "TE15_POLICY_TAMPER {changed}"
                    );
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn te15_b1_unavailable_task_policy_refuses_before_probe() {
        struct NoProbe;
        impl ConnectorRuntimeChecks for NoProbe {
            fn probe(
                &self,
                _: &ConnectorSpecification,
                _: &AttemptBinding,
            ) -> Result<RuntimeQualification, CertificationRefusal> {
                panic!("TE15_UNAVAILABLE_PROBED");
            }
        }
        let mut selection = codex_candidate().selection();
        selection.model = "unavailable-model".into();
        assert_eq!(
            codex_candidate()
                .bind(
                    binding().for_task(
                        selection,
                        super::super::super::orchestration_records::Role::Editor
                    ),
                    &NoProbe
                )
                .err(),
            Some(CertificationRefusal::ModelUnavailable),
            "TE15_UNAVAILABLE_POLICY"
        );
    }

    fn runtime() -> RuntimeQualification {
        test_runtime()
    }
    impl ConnectorRuntimeChecks for RuntimeQualification {
        fn probe(
            &self,
            _: &ConnectorSpecification,
            _: &AttemptBinding,
        ) -> Result<RuntimeQualification, CertificationRefusal> {
            Ok(self.clone())
        }
    }
    fn refusal(runtime: &RuntimeQualification) -> CertificationRefusal {
        certify_connector("scripted", binding(), runtime)
            .err()
            .expect("must refuse")
    }

    #[test]
    fn s4b_c1_fresh_endpoints_bind_the_same_fixed_specification() {
        let original = binding();
        let first = certify_connector("scripted", original.clone(), &runtime()).unwrap();
        let mut fresh = original.clone();
        fresh.endpoint = "http://127.0.0.1:23456/mcp".into();
        fresh.identity.attempt =
            serde_json::from_str("\"00000000000000000000000000000007\"").unwrap();
        fresh.working_directory = fresh.working_directory.join("next-scratch");
        let second = certify_connector("scripted", fresh.clone(), &runtime()).unwrap();
        assert!(std::ptr::eq(first.specification(), second.specification()));
        assert_eq!(first.selection(), second.selection());
        assert_ne!(first.session_config(), second.session_config());
        first
            .validate_session(first.session_config(), original.identity())
            .unwrap();
        second
            .validate_session(second.session_config(), fresh.identity())
            .unwrap();
        second.validate_binding(&fresh).unwrap();
        second.recheck_runtime(&runtime()).unwrap();
    }

    #[test]
    fn s4b_c1_forged_profile_is_refused() {
        let mut profile = certify_connector("scripted", binding(), &runtime()).unwrap();
        profile.config.tool_names = Some(vec!["unrestricted_shell".into()]);
        assert_eq!(
            profile.validate_session(profile.session_config(), profile.binding().identity()),
            Err(CertificationRefusal::ForgedProfile),
            "S4B_C1_FORGED_PROFILE_REFUSED"
        );
    }

    #[test]
    fn s4b_c1_binding_change_is_refused() {
        let original = binding();
        let profile = certify_connector("scripted", original.clone(), &runtime()).unwrap();
        let mut cfg = profile.session_config().clone();
        cfg.mcp_url = Some("http://127.0.0.1:23456/mcp".into());
        assert_eq!(
            profile.validate_session(&cfg, original.identity()),
            Err(CertificationRefusal::BindingChanged),
            "S4B_C1_BINDING_CHANGE_REFUSED"
        );
        cfg = profile.session_config().clone();
        cfg.working_directory = Some(original.working_directory.join("elsewhere"));
        assert_eq!(
            profile.validate_session(&cfg, original.identity()),
            Err(CertificationRefusal::BindingChanged)
        );
        let mut changed = original.clone();
        changed.identity.owner_instance =
            serde_json::from_str("\"00000000000000000000000000000009\"").unwrap();
        assert_eq!(
            profile.validate_session(profile.session_config(), changed.identity()),
            Err(CertificationRefusal::BindingChanged)
        );
        changed = original.clone();
        changed
            .roots
            .push(original.working_directory.join("another-project"));
        assert_eq!(
            profile.validate_binding(&changed),
            Err(CertificationRefusal::BindingChanged)
        );
        assert_eq!(profile.validate_binding(&original), Ok(()));
    }

    #[test]
    fn s4b_c1_fixed_policy_change_is_refused() {
        let profile = certify_connector("scripted", binding(), &runtime()).unwrap();
        for field in 0..5 {
            let mut cfg = profile.session_config().clone();
            match field {
                0 => cfg.model = Some("other".into()),
                1 => cfg.effort = Some("other".into()),
                2 => cfg.service_tier = Some("other".into()),
                3 => cfg.max_turns = None,
                _ => cfg.tool_names = None,
            }
            assert_eq!(
                profile.validate_session(&cfg, profile.binding().identity()),
                Err(CertificationRefusal::FixedPolicyChanged)
            );
        }
    }

    #[test]
    fn s4b_c1_versions_below_the_minimum_are_refused_and_newer_ones_pass() {
        for version in [CliVersion(0, 146, 9), CliVersion(0, 0, 1)] {
            let mut check = runtime();
            check.version = version;
            assert_eq!(refusal(&check), CertificationRefusal::UnqualifiedVersion);
        }
        // A newer CLI is allowed: its stream is held to the same fail-closed
        // allowlists at run time.
        for version in [
            CliVersion(0, 147, 0),
            CliVersion(0, 147, 2),
            CliVersion(1, 0, 0),
        ] {
            let mut check = runtime();
            check.version = version;
            assert!(
                certify_connector("scripted", binding(), &check).is_ok(),
                "NEWER_CLI_ACCEPTED {version:?}"
            );
        }
        assert_eq!(CliVersion::parse("0.147.0"), Ok(CliVersion(0, 147, 0)));
        for version in [
            ">= 0.147.0",
            "0.147.0-beta",
            "0.147",
            "00.147.0",
            "0.+147.0",
            "4294967296.0.0",
        ] {
            assert_eq!(
                CliVersion::parse(version),
                Err(CertificationRefusal::UnqualifiedVersion)
            );
        }
    }

    #[test]
    fn s4b_c1_api_and_unknown_authentication_are_refused() {
        for (authentication, expected) in [
            (
                AuthenticationMode::ApiKey,
                CertificationRefusal::ApiAuthentication,
            ),
            (
                AuthenticationMode::Unknown,
                CertificationRefusal::UnknownAuthentication,
            ),
        ] {
            let mut check = runtime();
            check.authentication.at_launch = authentication;
            assert_eq!(refusal(&check), expected);
            check.authentication.at_launch = AuthenticationMode::Subscription;
            check.authentication.at_recheck = Some(authentication);
            assert_eq!(refusal(&check), expected);
        }
    }

    #[test]
    fn s4b_c1_runtime_requires_executable_sandbox_and_model() {
        let mut check = runtime();
        check.executable.file_identity = [0; 2];
        check.launch_context.executable = check.executable.clone();
        check.authentication.context = check.launch_context.clone();
        assert_eq!(refusal(&check), CertificationRefusal::ExecutableIdentity);
        check = runtime();
        check.sandbox = None;
        assert_eq!(refusal(&check), CertificationRefusal::SandboxUnavailable);
        check = runtime();
        check.sandbox = Some(SandboxPolicy::WindowsFilesystemAndIpc);
        assert_eq!(refusal(&check), CertificationRefusal::SandboxUnavailable);
        check = runtime();
        check.model_available = false;
        assert_eq!(refusal(&check), CertificationRefusal::ModelUnavailable);
        let profile = certify_connector("scripted", binding(), &runtime()).unwrap();
        check = runtime();
        check.executable.file_identity[1] += 1;
        check.launch_context.executable = check.executable.clone();
        check.authentication.context = check.launch_context.clone();
        assert_eq!(
            profile.recheck_runtime(&check),
            Err(CertificationRefusal::ExecutableIdentity)
        );
        check = runtime();
        check.authentication.at_launch = AuthenticationMode::ApiKey;
        assert_eq!(
            profile.recheck_runtime(&check),
            Err(CertificationRefusal::ApiAuthentication)
        );
    }

    #[test]
    fn s4b_c2_runtime_context_cannot_change_between_binding_and_spawn() {
        let profile = certify_connector("scripted", binding(), &runtime()).unwrap();
        let mut changed = runtime();
        changed.launch_context.environment_digest = [3; 32];
        changed.authentication.context = changed.launch_context.clone();
        assert_eq!(
            profile.recheck_runtime(&changed),
            Err(CertificationRefusal::EffectiveAuthentication),
            "S4B_C2_FRESH_CONTEXT_MATCHES_SPAWN"
        );
    }

    #[test]
    fn s4b_c1_unknown_provider_and_unregistered_specification_are_refused() {
        static FORGED: ConnectorSpecification = ConnectorSpecification {
            provider: "scripted",
            minimum_version: CliVersion(0, 147, 0),
            session: SessionPolicy {
                model: "fixture",
                options: &[],
                effort: None,
                service_tier: None,
                max_turns: 1,
                tools: &[],
            },
            default_token_ceiling: std::num::NonZeroU64::new(2_000_000).unwrap(),
            mcp: McpPolicy::HostEndpointOnly,
            sandbox: SandboxPolicy::Fixture,
            authentication: AuthenticationMode::Subscription,
            auth_contract: super::super::CODEX_AUTH_CONTRACT,
        };
        assert_eq!(
            certify_connector("unknown", binding(), &runtime()).err(),
            Some(CertificationRefusal::UnknownProvider)
        );
        assert_eq!(
            certify_connector("codex", binding(), &runtime()).err(),
            Some(if cfg!(target_os = "linux") {
                CertificationRefusal::UnqualifiedVersion
            } else {
                CertificationRefusal::UnknownProvider
            })
        );
        assert_eq!(
            FORGED.bind(binding(), &runtime()).err(),
            Some(CertificationRefusal::UnqualifiedSpecification)
        );
        let mut profile = certify_connector("scripted", binding(), &runtime()).unwrap();
        profile.specification = &FORGED;
        assert_eq!(
            profile.validate_session(profile.session_config(), profile.binding().identity()),
            Err(CertificationRefusal::ForgedProfile)
        );
    }

    #[test]
    fn s4b_c1_invalid_runtime_binding_is_refused() {
        for endpoint in [
            "https://127.0.0.1:12345/mcp",
            "http://example.com:12345/mcp",
            "http://127.0.0.1/mcp",
            "http://u:p@127.0.0.1:12345/mcp",
            "garbage",
        ] {
            let mut invalid = binding();
            invalid.endpoint = endpoint.into();
            assert_eq!(
                certify_connector("scripted", invalid, &runtime()).err(),
                Some(CertificationRefusal::InvalidBinding)
            );
        }
        let mut invalid = binding();
        invalid.roots.push("relative".into());
        assert_eq!(
            certify_connector("scripted", invalid, &runtime()).err(),
            Some(CertificationRefusal::InvalidBinding)
        );
    }

    #[test]
    fn s4b_c1_readiness_never_changes_the_registry() {
        let registered: Vec<_> = available_connectors()
            .iter()
            .map(|spec| spec.provider())
            .collect();
        assert_eq!(
            registered,
            if cfg!(target_os = "linux") {
                vec!["codex", "claude"]
            } else {
                vec![]
            }
        );
        for _readiness in [
            ConnectorReadiness::Install,
            ConnectorReadiness::SignIn,
            ConnectorReadiness::CheckAgain,
            ConnectorReadiness::Ready,
        ] {
            assert_eq!(
                available_connectors()
                    .iter()
                    .map(|spec| spec.provider())
                    .collect::<Vec<_>>(),
                registered
            );
        }
    }
}
