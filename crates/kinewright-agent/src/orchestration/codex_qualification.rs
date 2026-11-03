//! Local probes for the installed Codex CLI, run at every native admission.
use crate::orchestration::adapters::{
    AttemptBinding, AuthenticationMode, CODEX_AUTH_CONTRACT, CertificationRefusal,
    ConnectorRuntimeChecks, ConnectorSpecification, CredentialFile, CredentialStoreScope,
    EffectiveAuthEvidence, EffectiveLaunchContext, ExecutableIdentity, RuntimeQualification,
    SandboxPolicy, codex_candidate,
};
use std::io;

pub(crate) struct InstalledCodexChecks;

pub(crate) fn executable() -> io::Result<ExecutableIdentity> {
    crate::native_runtime::installed_executable("codex", "Codex")
}

fn digest(value: &impl serde::Serialize) -> [u8; 32] {
    let hash = kinewright_media::sha256_bytes(&serde_json::to_vec(value).expect("recipe JSON"));
    std::array::from_fn(|i| u8::from_str_radix(&hash[i * 2..i * 2 + 2], 16).expect("sha256"))
}
impl ConnectorRuntimeChecks for InstalledCodexChecks {
    fn probe(
        &self,
        specification: &ConnectorSpecification,
        binding: &AttemptBinding,
    ) -> Result<RuntimeQualification, CertificationRefusal> {
        if !std::ptr::eq(specification, codex_candidate()) {
            return Err(CertificationRefusal::UnqualifiedSpecification);
        }
        kinewright_job_sys::require_nsdelegate()
            .and_then(|()| kinewright_job_sys::require_provider_landlock())
            .map_err(|_| CertificationRefusal::SandboxUnavailable)?;
        let executable = executable().map_err(|_| CertificationRefusal::ExecutableIdentity)?;
        let source = crate::codex_credentials::source_path()
            .map_err(|_| CertificationRefusal::UnknownAuthentication)?;
        let identity = crate::codex_credentials::inspect(&source, None)
            .map_err(|_| CertificationRefusal::UnknownAuthentication)?;
        let home = crate::codex_credentials::prepare_home(binding.working_directory())
            .map_err(|_| CertificationRefusal::EffectiveAuthentication)?;
        let context = EffectiveLaunchContext {
            environment_digest: [1; 32],
            settings_digest: [1; 32],
            environment_cleared: true,
            settings_isolated: true,
            executable: executable.clone(),
            credential_store: CredentialStoreScope {
                root: home,
                single_file: Some(CredentialFile { source, identity }),
                credential_files: vec!["auth.json".into()],
                lock_files: vec![],
                temporary_file_prefixes: vec![],
                refresh: CODEX_AUTH_CONTRACT.refresh,
            },
        };
        let mut runtime = RuntimeQualification {
            version: crate::orchestration::adapters::installed_version(&executable, &[])?,
            authentication: EffectiveAuthEvidence {
                contract: CODEX_AUTH_CONTRACT,
                context: context.clone(),
                at_launch: AuthenticationMode::Subscription,
                at_recheck: None,
                observed_store: context.credential_store.clone(),
            },
            launch_context: context,
            executable,
            sandbox: Some(SandboxPolicy::LinuxFilesystemAndIpc),
            model_available: true,
        };
        let program = crate::native_launcher::recipe(
            codex_candidate(),
            binding,
            &runtime,
            &codex_candidate().session_config(binding),
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

pub(crate) fn authentication_status() -> kinewright_core::AuthenticationStatus {
    use kinewright_core::AuthenticationStatus;
    if crate::codex_credentials::source_path()
        .and_then(|path| crate::codex_credentials::inspect(&path, None))
        .is_err()
    {
        return AuthenticationStatus::Unauthenticated;
    }
    // Readiness is local. The same exact executable and kernel prerequisites
    // are checked again by InstalledCodexChecks at every native admission.
    if executable().is_err()
        || kinewright_job_sys::require_nsdelegate().is_err()
        || kinewright_job_sys::require_provider_landlock().is_err()
    {
        return AuthenticationStatus::Unknown;
    }
    AuthenticationStatus::Authenticated
}
