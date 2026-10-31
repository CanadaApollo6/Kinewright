//! Local probes for one candidate release. Registry publication follows the live gate.
use crate::orchestration::adapters::{
    AttemptBinding, AuthenticationMode, CODEX_AUTH_CONTRACT, CertificationRefusal, CliVersion,
    ConnectorRuntimeChecks, ConnectorSpecification, CredentialFile, CredentialStoreScope,
    EffectiveAuthEvidence, EffectiveLaunchContext, ExecutableIdentity, RuntimeQualification,
    SandboxPolicy, codex_candidate,
};
use std::{
    fs,
    io::{self, Read},
    os::unix::fs::MetadataExt,
};

pub(crate) const CODEX_BINARY_SHA256: &str =
    "b8a11fedf24b2dfb59a70c9e52b030f9a508dcd49cd2dd0d41956f4c785156b8";
pub(crate) struct InstalledCodexChecks;

pub(crate) fn executable() -> io::Result<ExecutableIdentity> {
    let path =
        crate::drivers::find_on_path("codex").ok_or_else(|| io::Error::other("Install Codex"))?;
    let mut path = fs::canonicalize(path)?;
    if path.file_name().is_some_and(|name| name == "mise") {
        let output = std::process::Command::new(&path)
            .args(["which", "codex"])
            .output()?;
        if !output.status.success() || output.stdout.len() > 4096 {
            return Err(io::Error::other("Check again: Codex resolution failed"));
        }
        path = fs::canonicalize(
            String::from_utf8(output.stdout)
                .map_err(io::Error::other)?
                .trim(),
        )?;
    }
    let mut file = fs::File::open(&path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > 512 * 1024 * 1024 {
        return Err(io::Error::other(
            "Check again: Codex executable size refused",
        ));
    }
    let hash = kinewright_media::sha256_reader(&mut file.by_ref().take(512 * 1024 * 1024 + 1))?;
    let after = file.metadata()?;
    if hash != CODEX_BINARY_SHA256
        || metadata.len() != after.len()
        || metadata.modified()? != after.modified()?
    {
        return Err(io::Error::other(
            "Check again: Codex executable is not the qualified release",
        ));
    }
    Ok(ExecutableIdentity {
        path,
        file_identity: [metadata.dev(), metadata.ino()],
    })
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
            version: CliVersion(0, 166, 0),
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
