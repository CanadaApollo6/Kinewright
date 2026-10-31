//! The provider recipe and guardian release consume the same acknowledged manifest.
#![allow(
    dead_code,
    reason = "C5 registers qualified providers; C2 exercises protocol children"
)]
#![allow(clippy::missing_errors_doc)]
use crate::{
    execution_claim::MachineClaim,
    guardian::{CertifiedProfile, ContainedSession, DurableAttemptOwner, StartupControl},
    job_guardian::protocol::{AttemptIdentity, Program, ProviderSandbox, RecoveryIdentity},
    orchestration::{
        AdmittedAttempt,
        adapters::{self, AuthorizedInvocation, CertifiedConnectorProfile, SendKind},
    },
    provider_pipes::ProviderPipeListeners,
};
use kinewright_core::{AgentError, AgentEvent, AgentSession};
use kinewright_project::bundle_records::ObjectRef;
use std::{
    collections::BTreeMap,
    io,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};

/// One explicit TOML feature table stays within the guardian's argument bound.
pub(crate) fn codex_feature_policy() -> String {
    let entries = crate::drivers::CODEX_DISABLED_FEATURES
        .iter()
        .copied()
        .chain([
            "token_budget",
            "sleep_tool",
            "current_time_reminder",
            "send_message_to_user_async",
        ])
        .map(|name| format!("{name}=false"))
        .collect::<Vec<_>>()
        .join(",");
    format!("features={{{entries}}}")
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub identity: crate::orchestration::ExecutionIdentity,
    pub endpoint: String,
    pub roots: Vec<PathBuf>,
    pub kind: SendKind,
    pub program: Program,
    pub payload: ObjectRef,
    pub turn: u64,
}

#[allow(
    clippy::too_many_lines,
    reason = "the qualified recipe and filesystem policy are audited together"
)]
pub(crate) fn program(profile: &CertifiedConnectorProfile, prompt: &str) -> io::Result<Program> {
    recipe(
        profile.specification(),
        profile.binding(),
        profile.runtime(),
        profile.session_config(),
        prompt,
    )
}

#[allow(
    clippy::too_many_lines,
    reason = "one provider recipe is fingerprinted before certification and spawn"
)]
pub(crate) fn recipe(
    specification: &adapters::ConnectorSpecification,
    binding: &adapters::AttemptBinding,
    runtime: &adapters::RuntimeQualification,
    config: &kinewright_core::SessionConfig,
    prompt: &str,
) -> io::Result<Program> {
    let scratch = binding.working_directory().to_path_buf();
    let credentials = runtime.launch_context.credential_store.root.clone();
    let canonical = std::fs::canonicalize(&scratch)?;
    for directory in [&canonical, &credentials] {
        let metadata = std::fs::symlink_metadata(directory)?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.permissions().mode() & 0o077 != 0
            || metadata.uid() != rustix::process::getuid().as_raw()
        {
            return Err(io::Error::other("provider directory is not private"));
        }
    }
    let scope = &runtime.launch_context.credential_store;
    if let Some(mount) = &scope.single_file {
        crate::codex_credentials::inspect(&mount.source, Some(mount.identity))?;
    }
    let mut pending = if scope.single_file.is_some() {
        vec![]
    } else {
        vec![credentials.clone()]
    };
    let mut entries = 0;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            entries += 1;
            if entries > 96 {
                return Err(io::Error::other("credential store capacity"));
            }
            let path = entry.path();
            let relative = path.strip_prefix(&credentials).map_err(io::Error::other)?;
            let metadata = std::fs::symlink_metadata(&path)?;
            let named = scope
                .credential_files
                .iter()
                .chain(&scope.lock_files)
                .any(|name| name == relative);
            let temporary = scope.temporary_file_prefixes.iter().any(|prefix| {
                relative.parent() == prefix.parent()
                    && relative
                        .file_name()
                        .zip(prefix.file_name())
                        .is_some_and(|(name, prefix)| {
                            name.as_encoded_bytes()
                                .starts_with(prefix.as_encoded_bytes())
                        })
            });
            let parent = scope
                .credential_files
                .iter()
                .chain(&scope.lock_files)
                .chain(&scope.temporary_file_prefixes)
                .any(|name| name.starts_with(relative));
            if metadata.file_type().is_symlink()
                || metadata.uid() != rustix::process::getuid().as_raw()
                || (metadata.is_file()
                    && (!named && !temporary
                        || metadata.nlink() != 1
                        || metadata.len() > 64 * 1024))
                || (metadata.is_dir() && !parent)
                || (!metadata.is_file() && !metadata.is_dir())
            {
                return Err(io::Error::other("credential store exceeds qualified scope"));
            }
            if metadata.is_dir() {
                pending.push(path);
            }
        }
    }
    if credentials == canonical || credentials == std::path::Path::new("/") {
        return Err(io::Error::other(
            "provider scratch overlaps project authority",
        ));
    }
    let environment = BTreeMap::from([
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("LANG".into(), "C.UTF-8".into()),
        (
            "XDG_RUNTIME_DIR".into(),
            format!("/run/user/{}", rustix::process::getuid().as_raw()),
        ),
        ("HOME".into(), scratch.to_string_lossy().into_owned()),
        ("TMPDIR".into(), scratch.to_string_lossy().into_owned()),
        (
            "CODEX_HOME".into(),
            credentials.to_string_lossy().into_owned(),
        ),
    ]);
    let mut arguments = vec![];
    match specification.provider() {
        "codex" => {
            super::native_runtime::write_model_catalog(&scratch, config.model.as_deref())?;
            arguments.extend(
                [
                    "exec",
                    "--ignore-user-config",
                    "--ignore-rules",
                    "--strict-config",
                    "--ephemeral",
                    "--skip-git-repo-check",
                    "--json",
                    "--color",
                    "never",
                    "--sandbox",
                    "read-only",
                ]
                .map(str::to_owned),
            );
            arguments.extend(["-c".into(), codex_feature_policy()]);
            let cfg = config;
            if let Some(model) = &cfg.model {
                arguments.extend(["--model".into(), model.clone()]);
            }
            for (name, value) in [
                ("model_reasoning_effort", &cfg.effort),
                ("service_tier", &cfg.service_tier),
            ] {
                if let Some(value) = value {
                    arguments.extend([
                        "-c".into(),
                        format!("{name}={}", serde_json::to_string(value)?),
                    ]);
                }
            }
            arguments.extend(["-C".into(), scratch.to_string_lossy().into_owned()]);
            for option in [
                "approval_policy='never'",
                "forced_login_method='chatgpt'",
                "cli_auth_credentials_store='file'",
                "web_search='disabled'",
                "tools.update_plan.enabled=false",
                "agents.enabled=false",
                "analytics.enabled=false",
                "feedback.enabled=false",
                "history.persistence='none'",
                "shell_environment_policy.inherit='none'",
                "project_doc_max_bytes=0",
                "mcp_servers.kinewright.required=true",
                "mcp_servers.kinewright.default_tools_approval_mode='approve'",
                "mcp_servers.kinewright.bearer_token_env_var='KINEWRIGHT_HOST_MCP_TOKEN'",
            ] {
                arguments.extend(["-c".into(), option.into()]);
            }
            arguments.extend([
                "-c".into(),
                format!(
                    "mcp_servers.kinewright.url={}",
                    serde_json::to_string(binding.endpoint())?
                ),
                "-c".into(),
                format!(
                    "mcp_servers.kinewright.enabled_tools={}",
                    serde_json::to_string(&cfg.tool_names)?
                ),
            ]);
            arguments.extend([
                "-c".into(),
                format!(
                    "model_catalog_json={}",
                    serde_json::to_string(&scratch.join("models-direct.json"))?
                ),
            ]);
            if let Some(bootstrap) = crate::writer_host::mcp_credentials(binding.endpoint()) {
                let fields = [
                    ("x-kinewright-project", bootstrap.endpoint.project),
                    (
                        "x-kinewright-protocol",
                        crate::writer_host::HOST_PROTOCOL.to_string(),
                    ),
                    (
                        "x-kinewright-roots",
                        bootstrap.roots.wire_header().map_err(io::Error::other)?,
                    ),
                ];
                let fields = fields
                    .iter()
                    .map(|(name, value)| {
                        Ok(format!(
                            "{}={}",
                            serde_json::to_string(name)?,
                            serde_json::to_string(value)?
                        ))
                    })
                    .collect::<Result<Vec<_>, serde_json::Error>>()?
                    .join(",");
                arguments.extend([
                    "-c".into(),
                    format!("mcp_servers.kinewright.http_headers={{{fields}}}"),
                ]);
            } else {
                return Err(io::Error::other("private host MCP credentials unavailable"));
            }
            arguments.push(prompt.into());
        }
        #[cfg(any(test, feature = "test-util"))]
        "scripted" => arguments.extend([
            scratch.to_string_lossy().into_owned(),
            "s4b-codex".into(),
            prompt.into(),
        ]),
        _ => return Err(io::Error::other("provider recipe unavailable")),
    }
    let result = Program {
        executable: runtime.executable.path.clone(),
        arguments,
        environment,
        working_directory: scratch.clone(),
        pipe_directory: Some(scratch.join("io")),
        sandbox: ProviderSandbox::LinuxAllowlist {
            credential_mount: scope.single_file.as_ref().map(|file| {
                Box::new(crate::job_guardian::protocol::CredentialMount {
                    source: file.source.clone(),
                    target: credentials.join("auth.json"),
                    identity: file.identity,
                })
            }),
            connect_tcp: vec![
                443,
                reqwest::Url::parse(binding.endpoint())
                    .map_err(io::Error::other)?
                    .port_or_known_default()
                    .ok_or_else(|| io::Error::other("MCP port missing"))?,
            ],
            runtime: [
                "/usr",
                "/lib",
                "/lib64",
                "/dev/null",
                "/dev/urandom",
                "/etc/ssl",
                "/etc/hosts",
                "/etc/resolv.conf",
                "/etc/nsswitch.conf",
                "/etc/ld.so.cache",
            ]
            .map(Into::into)
            .into_iter()
            .chain(if specification.provider() == "codex" {
                super::native_runtime::runtime_tree(&runtime.executable)?
            } else {
                None
            })
            .collect(),
            scratch,
            credential_store: credentials,
            executable_identity: runtime.executable.file_identity,
        },
    };
    for denied in binding.roots() {
        for grant in runtime_paths(&result) {
            if overlaps(grant, denied)? {
                return Err(io::Error::other(
                    "provider allowlist overlaps project authority",
                ));
            }
        }
    }
    result.validate()?;
    Ok(result)
}

pub(crate) fn runtime_paths(program: &Program) -> Vec<&std::path::Path> {
    let mut paths = vec![
        program.executable.as_path(),
        program.working_directory.as_path(),
    ];
    if let ProviderSandbox::LinuxAllowlist {
        runtime,
        credential_store,
        ..
    } = &program.sandbox
    {
        paths.extend(runtime.iter().map(PathBuf::as_path));
        paths.push(credential_store);
        if let ProviderSandbox::LinuxAllowlist {
            credential_mount: Some(mount),
            ..
        } = &program.sandbox
        {
            paths.push(&mount.source);
        }
    }
    paths
}
pub(crate) fn overlaps(first: &std::path::Path, second: &std::path::Path) -> io::Result<bool> {
    fn identity(path: &std::path::Path) -> io::Result<(u64, u64)> {
        let metadata = std::fs::metadata(path)?;
        Ok((metadata.dev(), metadata.ino()))
    }
    let first = std::fs::canonicalize(first)?;
    let second = std::fs::canonicalize(second)?;
    let first_identity = identity(&first)?;
    let second_identity = identity(&second)?;
    for ancestor in first.ancestors() {
        if identity(ancestor)? == second_identity {
            return Ok(true);
        }
    }
    for ancestor in second.ancestors() {
        if identity(ancestor)? == first_identity {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn context_matches(
    profile: &CertifiedConnectorProfile,
    program: &Program,
) -> io::Result<()> {
    fn digest(bytes: &[u8]) -> String {
        kinewright_media::sha256_bytes(bytes)
    }
    fn hex(bytes: &[u8; 32]) -> String {
        use std::fmt::Write;
        bytes
            .iter()
            .fold(String::with_capacity(64), |mut result, byte| {
                let _ = write!(result, "{byte:02x}");
                result
            })
    }
    let context = &profile.runtime().launch_context;
    let mut options = program.arguments.clone();
    options.pop();
    if hex(&context.environment_digest) != digest(&serde_json::to_vec(&program.environment)?)
        || hex(&context.settings_digest) != digest(&serde_json::to_vec(&options)?)
    {
        return Err(io::Error::other(
            "provider effective launch context changed",
        ));
    }
    Ok(())
}

pub(crate) trait InvocationOwner: Send + Sync {
    fn reserve(&self, invocation: &AuthorizedInvocation) -> io::Result<()>;
    fn release(
        &self,
        invocation: &AuthorizedInvocation,
        recovery: &RecoveryIdentity,
        control: &StartupControl,
    ) -> io::Result<()>;
    fn commit(
        &self,
        invocation: &AuthorizedInvocation,
        recovery: &RecoveryIdentity,
        send: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<()>;
    fn started(&self, identity: &crate::orchestration::ExecutionIdentity) -> io::Result<()>;
    fn raw(
        &self,
        identity: &crate::orchestration::ExecutionIdentity,
        raw: &serde_json::Value,
        ordinal: u64,
        eof: bool,
    ) -> io::Result<()>;
    fn completed(
        &self,
        identity: &crate::orchestration::ExecutionIdentity,
        completed: bool,
        proof: &crate::guardian::CleanupProof,
    ) -> io::Result<()>;
    fn unbound_completed(
        &self,
        identity: &crate::orchestration::ExecutionIdentity,
        proof: &crate::guardian::UnboundCleanupProof,
    ) -> io::Result<()>;
    fn steering(
        &self,
        identity: &crate::orchestration::ExecutionIdentity,
        text: &str,
    ) -> io::Result<()>;
}

pub(crate) struct NativeGuardianLauncher {
    pub claim: Arc<Mutex<MachineClaim>>,
    pub owner: Arc<dyn InvocationOwner>,
    pub guardian: PathBuf,
    pub checks: Arc<dyn adapters::ConnectorRuntimeChecks + Send + Sync>,
    #[cfg(test)]
    pub lose_commit_ack: bool,
    #[cfg(test)]
    pub after_release: Option<Arc<dyn Fn() + Send + Sync>>,
    #[cfg(test)]
    pub startup_failure: Option<StartupFailure>,
}
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartupFailure {
    Pipes,
    Credentials,
    Reader,
}

struct StartupCleanup {
    owner: Arc<dyn InvocationOwner>,
    identity: crate::orchestration::ExecutionIdentity,
    control: Option<Arc<Mutex<ContainedSession>>>,
    claim: Arc<Mutex<MachineClaim>>,
    credential_mount: Option<Box<crate::job_guardian::protocol::CredentialMount>>,
}
impl Drop for StartupCleanup {
    fn drop(&mut self) {
        let Some(control) = self.control.take() else {
            return;
        };
        let mut native = control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        native.close_lifetime();
        let proof = native.cleanup();
        if let Some(mount) = &self.credential_mount {
            let _ = crate::codex_credentials::verify_mount(mount);
        }
        if let Ok(proof) = proof
            && self.owner.completed(&self.identity, false, &proof).is_ok()
            && let Ok(mut claim) = self.claim.lock()
        {
            let _ = claim.retire(&proof);
        }
    }
}
struct Owner<'a> {
    port: &'a dyn InvocationOwner,
    invocation: &'a AuthorizedInvocation,
    control: Option<StartupControl>,
    #[cfg(test)]
    after_release: Option<Arc<dyn Fn() + Send + Sync>>,
}
impl DurableAttemptOwner for Owner<'_> {
    fn cleanup_completed(&mut self, proof: &crate::guardian::CleanupProof) -> io::Result<()> {
        self.port.completed(
            self.invocation.invocation().binding().identity(),
            false,
            proof,
        )
    }
    fn cleanup_unbound(&mut self, proof: &crate::guardian::UnboundCleanupProof) -> io::Result<()> {
        self.port
            .unbound_completed(self.invocation.invocation().binding().identity(), proof)
    }
    fn reserve(&mut self, identity: &AttemptIdentity) -> io::Result<()> {
        if identity != &native_identity(self.invocation.invocation().binding().identity()) {
            return Err(io::Error::other("native reservation identity changed"));
        }
        self.port.reserve(self.invocation)
    }
    fn authorize_commit(&mut self, recovery: &RecoveryIdentity) -> io::Result<()> {
        self.port.release(
            self.invocation,
            recovery,
            self.control
                .as_ref()
                .ok_or_else(|| io::Error::other("startup control missing"))?,
        )?;
        #[cfg(test)]
        if let Some(barrier) = &self.after_release {
            barrier();
        }
        Ok(())
    }
    fn commit_authorized(
        &mut self,
        recovery: &RecoveryIdentity,
        send: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<()> {
        self.port.commit(self.invocation, recovery, send)
    }
}
pub(crate) fn native_identity(
    identity: &crate::orchestration::ExecutionIdentity,
) -> AttemptIdentity {
    AttemptIdentity {
        project: identity.project.as_str().into(),
        run: identity.run.0.as_str().into(),
        task: identity.task.0.as_str().into(),
        attempt: identity.attempt.0.as_str().into(),
        nonce: identity.nonce.as_str().into(),
        owner_instance: identity.owner_instance.as_str().into(),
    }
}
impl adapters::GuardianLauncher for NativeGuardianLauncher {
    fn launch(
        &self,
        attempt: AdmittedAttempt,
        invocation: AuthorizedInvocation,
    ) -> Result<adapters::ContainedSession, adapters::LaunchError> {
        self.launch_native(&attempt, &invocation)
            .map_err(|error| adapters::LaunchError(error.to_string()))
    }
}
impl NativeGuardianLauncher {
    #[cfg(test)]
    fn fail_startup(&self, point: StartupFailure) -> io::Result<()> {
        if self.startup_failure == Some(point) {
            Err(io::Error::other("fixture startup failure after prepare"))
        } else {
            Ok(())
        }
    }
    #[allow(
        clippy::too_many_lines,
        reason = "one immutable invocation crosses the ordered native barriers"
    )]
    fn launch_native(
        &self,
        attempt: &AdmittedAttempt,
        invocation: &AuthorizedInvocation,
    ) -> io::Result<adapters::ContainedSession> {
        invocation
            .validate(invocation.profile().session_config(), attempt)
            .map_err(io::Error::other)?;
        invocation
            .profile()
            .recheck_runtime(self.checks.as_ref())
            .map_err(io::Error::other)?;
        let immutable = invocation.invocation();
        let manifest: Manifest = serde_json::from_slice(immutable.manifest_bytes())?;
        if manifest.identity != *attempt.identity()
            || manifest.endpoint != immutable.binding().endpoint()
            || manifest.roots != immutable.binding().roots()
            || manifest.kind != immutable.kind()
            || manifest.payload != *immutable.payload()
            || manifest.turn != invocation.acknowledgment().consumption().turn
        {
            return Err(io::Error::other("native invocation binding changed"));
        }
        let prompt = std::str::from_utf8(immutable.bytes())
            .map_err(|_| io::Error::other("provider prompt encoding"))?
            .to_owned();
        let actual = program(invocation.profile(), &prompt)?;
        let actual_bytes = serde_json::to_vec(&Manifest {
            program: actual.clone(),
            ..manifest
        })?;
        let actual_reference = ObjectRef {
            sha256: kinewright_media::sha256_bytes(&actual_bytes),
            byte_length: actual_bytes.len() as u64,
            record_kind: kinewright_project::bundle_records::RecordKind::OwnedEvidence,
        };
        invocation
            .verify_payload(&actual_reference, immutable.bytes())
            .map_err(io::Error::other)?;
        context_matches(invocation.profile(), &actual)?;
        let pipes = ProviderPipeListeners::bind(
            actual
                .pipe_directory
                .as_ref()
                .ok_or_else(|| io::Error::other("provider pipe directory"))?,
        )?;
        let mut owner = Owner {
            port: self.owner.as_ref(),
            invocation,
            control: None,
            #[cfg(test)]
            after_release: self.after_release.clone(),
        };
        let policy = invocation.profile().specification().sandbox_policy();
        let qualified = policy == adapters::SandboxPolicy::LinuxFilesystemAndIpc;
        #[cfg(any(test, feature = "test-util"))]
        let qualified = qualified || policy == adapters::SandboxPolicy::Fixture;
        if !qualified {
            return Err(io::Error::other("provider sandbox not qualified"));
        }
        let profile = CertifiedProfile::linux_allowlist();
        let mut claim_guard = self
            .claim
            .lock()
            .map_err(|_| io::Error::other("machine claim poisoned"))?;
        let credential_mount = match &actual.sandbox {
            ProviderSandbox::LinuxAllowlist {
                credential_mount, ..
            } => credential_mount.clone(),
            ProviderSandbox::ContainmentOnly => None,
        };
        let native = ContainedSession::prepare(
            &mut claim_guard,
            &mut owner,
            native_identity(attempt.identity()),
            actual,
            &self.guardian,
            &profile,
        )?;
        drop(claim_guard);
        let control = Arc::new(Mutex::new(native));
        let mut startup = StartupCleanup {
            owner: self.owner.clone(),
            identity: attempt.identity().clone(),
            control: Some(control.clone()),
            claim: self.claim.clone(),
            credential_mount: credential_mount.clone(),
        };
        owner.control = Some(
            control
                .lock()
                .map_err(|_| io::Error::other("native session poisoned"))?
                .startup_control(),
        );
        #[cfg(test)]
        self.fail_startup(StartupFailure::Pipes)?;
        let mut pipes = pipes.accept()?;
        #[cfg(test)]
        self.fail_startup(StartupFailure::Credentials)?;
        let credentials = crate::writer_host::mcp_credentials(immutable.binding().endpoint());
        let secret = credentials
            .as_ref()
            .map(|bootstrap| bootstrap.auth.bearer());
        pipes.credentials(
            secret
                .as_deref()
                .and_then(|s| s.strip_prefix("Bearer "))
                .unwrap_or_default()
                .as_bytes(),
        )?;
        #[cfg(test)]
        if self.lose_commit_ack {
            let mut native = control
                .lock()
                .map_err(|_| io::Error::other("native session poisoned"))?;
            native.commit_without_reply(&mut owner)?;
            let deadline = std::time::Instant::now() + crate::job_guardian::protocol::STARTUP;
            let marker = immutable
                .binding()
                .working_directory()
                .join("execution-ready");
            while !marker.exists() {
                if std::time::Instant::now() >= deadline {
                    return Err(io::Error::other("execution observation unavailable"));
                }
                thread::sleep(std::time::Duration::from_millis(1));
            }
            return Err(io::Error::other("fixture lost EXECUTING acknowledgment"));
        }
        let committed = {
            let mut native = control
                .lock()
                .map_err(|_| io::Error::other("native session poisoned"))?;
            native.commit(&mut owner)
        };
        committed?;
        self.owner.started(attempt.identity())?;
        #[cfg(test)]
        self.fail_startup(StartupFailure::Reader)?;
        let worker_control = Arc::clone(&control);
        let claim = Arc::clone(&self.claim);
        let usage_owner = Arc::clone(&self.owner);
        let execution = attempt.identity().clone();
        let (sender, events) = crate::protocol::limits::event_channel();
        thread::Builder::new()
            .name("kinewright-contained-codex".into())
            .spawn(move || {
                let diagnostics = crate::protocol::limits::stderr_capture(
                    pipes.diagnostics,
                    "kinewright-contained-stderr",
                );
                let mut completed =
                    read_provider_output(pipes.output, &sender, |raw, ordinal, eof| {
                        usage_owner.raw(&execution, raw, ordinal, eof)
                    });
                let mut native = worker_control
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                native.close_lifetime();
                match native.cleanup() {
                    Ok(proof) => {
                        if credential_mount.as_ref().is_some_and(|mount| {
                            crate::codex_credentials::verify_mount(mount).is_err()
                        }) {
                            completed = false;
                            sender.interrupt(
                                "Sign in: credential identity or authentication changed",
                            );
                        }
                        if usage_owner
                            .completed(&execution, completed && !sender.failed(), &proof)
                            .is_err()
                        {
                            sender.interrupt("native completion publication remains unresolved");
                        } else if claim
                            .lock()
                            .map_err(|_| ())
                            .and_then(|mut claim| claim.retire(&proof).map_err(|_| ()))
                            .is_err()
                        {
                            sender.interrupt("native retirement remains blocked");
                        }
                    }
                    Err(_) => sender.interrupt("native cleanup remains blocked"),
                }
                drop(native);
                if let Ok(reader) = diagnostics {
                    let _ = reader.join();
                }
                let _ = sender.send(AgentEvent::Done);
            })?;
        startup.control.take();
        Ok(adapters::ContainedSession::from_guardian(Box::new(
            RestrictedSession {
                control,
                events,
                owner: Arc::clone(&self.owner),
                identity: attempt.identity().clone(),
                first: Some(prompt),
            },
        )))
    }
}
struct RestrictedSession {
    control: Arc<Mutex<ContainedSession>>,
    events: crossbeam_channel::Receiver<AgentEvent>,
    owner: Arc<dyn InvocationOwner>,
    identity: crate::orchestration::ExecutionIdentity,
    first: Option<String>,
}
impl AgentSession for RestrictedSession {
    fn send_user_message(&mut self, text: String) -> Result<(), AgentError> {
        if text.is_empty() || text.len() > crate::protocol::limits::FRAME_BYTES / 2 {
            return Err(AgentError::Protocol("provider prompt capacity".into()));
        }
        if let Some(first) = self.first.take() {
            return if first == text {
                Ok(())
            } else {
                Err(AgentError::Protocol(
                    "opening send differs from its charge".into(),
                ))
            };
        }
        self.owner
            .steering(&self.identity, &text)
            .map_err(|_| AgentError::Protocol("queued steering refused".into()))
    }
    fn events(&self) -> crossbeam_channel::Receiver<AgentEvent> {
        self.events.clone()
    }
    fn interrupt(&mut self) {
        self.control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .close_lifetime();
    }
}
impl Drop for RestrictedSession {
    fn drop(&mut self) {
        self.interrupt();
    }
}

/// The native launch path and uncertified CI children share bounded framing,
/// raw H1 accounting before display parsing, and terminal failure handling.
pub(crate) fn read_provider_output(
    output: impl std::io::Read,
    sender: &crate::protocol::limits::EventSender,
    mut record: impl FnMut(&serde_json::Value, u64, bool) -> io::Result<()>,
) -> bool {
    let mut parser = crate::protocol::CodexProtocol::default();
    let mut ordinal = 0_u64;
    let mut completed = false;
    for line in crate::protocol::limits::Frames::new(output) {
        let result = line.map_err(io::Error::other).and_then(|line| {
            ordinal = ordinal
                .checked_add(1)
                .ok_or_else(|| io::Error::other("usage event capacity"))?;
            let raw: serde_json::Value = serde_json::from_str(&line)?;
            record(&raw, ordinal, false)?;
            if !native_item_allowed(&raw) {
                return Err(io::Error::other("provider-native tool refused"));
            }
            completed |=
                raw.get("type").and_then(serde_json::Value::as_str) == Some("turn.completed");
            parser.parse_line(&line).map_err(io::Error::other)
        });
        if let Ok(parsed) = result {
            for event in parsed {
                if !matches!(event, AgentEvent::Done | AgentEvent::Cost { .. })
                    && sender.send(event).is_err()
                {
                    break;
                }
            }
        } else {
            sender.interrupt("provider stream refused; cleanup pending");
            break;
        }
        if sender.failed() {
            break;
        }
    }
    if record(
        &serde_json::json!({"type":"native.eof"}),
        ordinal.saturating_add(1),
        true,
    )
    .is_err()
    {
        sender.interrupt("usage publication remains unresolved");
    }
    completed
}

fn native_item_allowed(raw: &serde_json::Value) -> bool {
    let Some(item) = raw.get("item") else {
        return true;
    };
    match item.get("type").and_then(serde_json::Value::as_str) {
        Some("agent_message" | "reasoning" | "error") => true,
        Some("mcp_tool_call") => {
            item.get("server").and_then(serde_json::Value::as_str) == Some("kinewright")
                && item
                    .get("tool")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|tool| {
                        crate::runtime::COMPACT_TOOL_NAMES
                            .contains(&crate::protocol::display_tool_name(tool).as_str())
                    })
        }
        _ => false,
    }
}
