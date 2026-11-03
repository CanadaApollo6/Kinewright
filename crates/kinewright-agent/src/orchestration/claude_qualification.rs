//! C6: local probes for the installed Claude Code CLI, run at every native
//! admission. Nothing here registers or advertises Claude: it is not in
//! `available_connectors()`.
use crate::orchestration::adapters::{
    AttemptBinding, AuthenticationMode, CLAUDE_AUTH_CONTRACT, CertificationRefusal,
    ConnectorRuntimeChecks, ConnectorSpecification, CredentialStoreScope, EffectiveAuthEvidence,
    EffectiveLaunchContext, ExecutableIdentity, RuntimeQualification, SandboxPolicy,
    claude_candidate,
};
use std::io;

pub(crate) struct InstalledClaudeChecks {
    /// The guardian that contains the `--version` probe.
    pub guardian: std::path::PathBuf,
}

pub(crate) fn executable() -> io::Result<ExecutableIdentity> {
    crate::native_runtime::installed_executable("claude", "Claude Code")
}

fn digest(value: &impl serde::Serialize) -> [u8; 32] {
    let hash = kinewright_media::sha256_bytes(&serde_json::to_vec(value).expect("recipe JSON"));
    std::array::from_fn(|i| u8::from_str_radix(&hash[i * 2..i * 2 + 2], 16).expect("sha256"))
}

fn refusal(refusal: crate::claude_credentials::Refusal) -> CertificationRefusal {
    use crate::claude_credentials::Refusal;
    match refusal {
        Refusal::Api => CertificationRefusal::ApiAuthentication,
        Refusal::Unknown | Refusal::Expiring => CertificationRefusal::UnknownAuthentication,
        Refusal::Store => CertificationRefusal::EffectiveAuthentication,
    }
}

impl ConnectorRuntimeChecks for InstalledClaudeChecks {
    fn probe(
        &self,
        specification: &ConnectorSpecification,
        binding: &AttemptBinding,
    ) -> Result<RuntimeQualification, CertificationRefusal> {
        if !std::ptr::eq(specification, claude_candidate()) {
            return Err(CertificationRefusal::UnqualifiedSpecification);
        }
        kinewright_job_sys::require_nsdelegate()
            .and_then(|()| kinewright_job_sys::require_provider_landlock())
            .map_err(|_| CertificationRefusal::SandboxUnavailable)?;
        let executable = executable().map_err(|_| CertificationRefusal::ExecutableIdentity)?;
        let version = crate::version_probe::installed_version(
            &crate::version_probe::VersionProbe {
                guardian: &self.guardian,
                form: crate::version_probe::VersionForm::Claude,
                user_local_runtime: false,
                credential_root: Some(
                    crate::claude_credentials::config_root()
                        .map_err(|_| CertificationRefusal::UnknownAuthentication)?,
                ),
            },
            &executable,
        )?;
        let source = crate::claude_credentials::source_path()
            .map_err(|_| CertificationRefusal::UnknownAuthentication)?;
        // At launch and at the launch-time recheck the person's store must
        // hold a subscription sign-in and no API mode may be configured.
        let sign_in = crate::claude_credentials::inspect(&source, None).map_err(refusal)?;
        let store = crate::claude_credentials::prepare_store(
            binding.working_directory(),
            &source,
            sign_in.identity,
        )
        .map_err(refusal)?;
        let context = EffectiveLaunchContext {
            environment_digest: [1; 32],
            settings_digest: [1; 32],
            environment_cleared: true,
            settings_isolated: true,
            executable: executable.clone(),
            credential_store: CredentialStoreScope {
                root: store,
                single_file: None,
                credential_files: vec![".credentials.json".into()],
                lock_files: vec![],
                temporary_file_prefixes: vec![],
                refresh: CLAUDE_AUTH_CONTRACT.refresh,
            },
        };
        let mut runtime = RuntimeQualification {
            version,
            authentication: EffectiveAuthEvidence {
                contract: CLAUDE_AUTH_CONTRACT,
                context: context.clone(),
                at_launch: AuthenticationMode::Subscription,
                at_recheck: Some(AuthenticationMode::Subscription),
                observed_store: context.credential_store.clone(),
            },
            launch_context: context,
            executable,
            sandbox: Some(SandboxPolicy::LinuxFilesystemAndIpc),
            model_available: true,
        };
        let program = crate::native_launcher::recipe(
            claude_candidate(),
            binding,
            &runtime,
            &claude_candidate().session_config(binding),
            "probe recipe",
        )
        .map_err(|_| CertificationRefusal::EffectiveAuthentication)?;
        let mut arguments = program.arguments;
        arguments.pop();
        runtime.launch_context.environment_digest = digest(&program.environment);
        runtime.launch_context.settings_digest = digest(&arguments);
        runtime.authentication.context = runtime.launch_context.clone();
        Ok(runtime)
    }
}
