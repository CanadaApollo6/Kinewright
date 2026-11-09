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

pub(crate) fn environment(
    scratch: &std::path::Path,
    credentials: &std::path::Path,
) -> BTreeMap<String, String> {
    BTreeMap::from([
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
    ])
}

/// C6: Claude Code's complete final environment. The guardian clears the
/// host's environment and sets exactly these (plus the MCP bearer), so no
/// API key, helper, base URL, third-party provider or config folder reaches
/// it. Tool search stays off so every tool schema the model sees is the
/// fixed `--tools ""` plus the host's own MCP tools.
pub(crate) fn claude_environment(
    scratch: &std::path::Path,
    store: &std::path::Path,
) -> BTreeMap<String, String> {
    BTreeMap::from(
        [
            ("PATH", "/usr/bin:/bin".to_owned()),
            ("LANG", "C.UTF-8".to_owned()),
            ("HOME", scratch.to_string_lossy().into_owned()),
            ("TMPDIR", scratch.to_string_lossy().into_owned()),
            ("CLAUDE_CONFIG_DIR", store.to_string_lossy().into_owned()),
            ("ENABLE_TOOL_SEARCH", "false".to_owned()),
            ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1".to_owned()),
            ("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1".to_owned()),
            ("CLAUDE_CODE_DISABLE_CLAUDE_MDS", "1".to_owned()),
            ("DISABLE_AUTOUPDATER", "1".to_owned()),
            ("DISABLE_TELEMETRY", "1".to_owned()),
            ("DISABLE_ERROR_REPORTING", "1".to_owned()),
        ]
        .map(|(name, value)| (name.to_owned(), value)),
    )
}

/// C6: the Claude-only system prompt. It replaces Claude Code's default
/// coding-agent prompt, which describes tools this session does not have.
/// The task itself is the shared native prompt, sent as the user message.
const CLAUDE_SYSTEM_PROMPT: &str = "You are a Kinewright task agent working on one video project. \
Your only tools are the kinewright MCP tools. Follow the task in the user message, \
use the tools it names, and end with a short plain-text summary.";

/// C6: the stream dialect a native provider writes. Codex's reader is unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StreamDialect {
    Codex,
    Claude,
}
impl StreamDialect {
    pub(crate) fn of(provider: &str) -> Self {
        match provider {
            "claude" | "scripted-claude" => Self::Claude,
            _ => Self::Codex,
        }
    }
}

/// C5 fix 7 and fix 8: while held, every native sandbox this process builds
/// carries the unpaid policy. The provider gets its own network namespace in
/// which only loopback exists, init relays `127.0.0.1:<MCP port>` to the
/// host, Landlock allows TCP only to that port, and the sign-in is a
/// read-only bind in a private home that holds nothing else of the
/// person's. The dry launch holds it around the unpaid
/// probe and launch so no provider request can leave the machine.
#[cfg(feature = "eval-harness")]
static DENY_PROVIDER_EGRESS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[cfg(feature = "eval-harness")]
pub(crate) struct DenyProviderEgress(());
#[cfg(feature = "eval-harness")]
impl DenyProviderEgress {
    /// # Panics
    /// Refuses a second concurrent holder.
    pub(crate) fn engage() -> Self {
        assert!(
            !DENY_PROVIDER_EGRESS.swap(true, std::sync::atomic::Ordering::SeqCst),
            "provider egress denial already held"
        );
        Self(())
    }
    pub(crate) fn engaged() -> bool {
        DENY_PROVIDER_EGRESS.load(std::sync::atomic::Ordering::SeqCst)
    }
}
#[cfg(feature = "eval-harness")]
impl Drop for DenyProviderEgress {
    fn drop(&mut self) {
        DENY_PROVIDER_EGRESS.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}
/// C5 fix 8: the host's view of the last unpaid launch's private home just
/// before launch, names and kinds only. Init checks the same home and its
/// mounts and refuses to launch any other shape; this is the report's list.
#[cfg(feature = "eval-harness")]
pub(crate) static UNPAID_HOME_BEFORE_LAUNCH: Mutex<Vec<String>> = Mutex::new(Vec::new());
#[cfg(feature = "eval-harness")]
fn record_unpaid_home(home: &std::path::Path) {
    let mut names = Vec::new();
    let mut pending = vec![home.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = entry.path();
            let kind = match entry.file_type() {
                Ok(kind) if kind.is_symlink() => "link",
                Ok(kind) if kind.is_dir() => "dir",
                Ok(kind) if kind.is_file() => "file",
                _ => "other",
            };
            if kind == "dir" && names.len() < 256 {
                pending.push(path.clone());
            }
            names.push(format!(
                "{kind} {}",
                path.strip_prefix(home).unwrap_or(&path).display()
            ));
        }
    }
    names.sort();
    if let Ok(mut recorded) = UNPAID_HOME_BEFORE_LAUNCH.lock() {
        *recorded = names;
    }
}
fn provider_ports(mcp_port: u16) -> Vec<u16> {
    if unpaid_policy(mcp_port).is_some() {
        return vec![mcp_port];
    }
    vec![443, mcp_port]
}
#[allow(
    clippy::unnecessary_wraps,
    reason = "shipping builds never build the unpaid policy"
)]
fn unpaid_policy(mcp_port: u16) -> Option<crate::job_guardian::protocol::UnpaidPolicy> {
    #[cfg(feature = "eval-harness")]
    if DenyProviderEgress::engaged() {
        return Some(crate::job_guardian::protocol::UnpaidPolicy { mcp_port });
    }
    let _ = mcp_port;
    None
}

/// The read-only system and install paths a provider is granted. The
/// contained `--version` probe gets exactly the same grants as a launch.
pub(crate) fn runtime_grants(
    executable: &adapters::ExecutableIdentity,
    user_local_runtime: bool,
) -> io::Result<Vec<std::path::PathBuf>> {
    Ok([
        "/usr",
        "/lib",
        "/lib64",
        "/dev/null",
        "/dev/urandom",
        "/etc/hosts",
        "/etc/resolv.conf",
        "/etc/nsswitch.conf",
        "/etc/ld.so.cache",
    ]
    .map(Into::into)
    .into_iter()
    .chain(super::native_runtime::network_files()?)
    .chain(if user_local_runtime {
        super::native_runtime::runtime_tree(executable)?
    } else {
        None
    })
    .collect())
}

pub(crate) fn sandbox(
    executable: &adapters::ExecutableIdentity,
    scratch: &std::path::Path,
    scope: &adapters::CredentialStoreScope,
    mcp_port: u16,
    user_local_runtime: bool,
) -> io::Result<ProviderSandbox> {
    Ok(ProviderSandbox::LinuxAllowlist {
        credential_mount: scope.single_file.as_ref().map(|file| {
            Box::new(crate::job_guardian::protocol::CredentialMount {
                source: file.source.clone(),
                target: scope.root.join("auth.json"),
                identity: file.identity,
            })
        }),
        connect_tcp: provider_ports(mcp_port),
        runtime: runtime_grants(executable, user_local_runtime)?,
        scratch: scratch.to_path_buf(),
        credential_store: scope.root.clone(),
        executable_identity: executable.file_identity,
        unpaid: unpaid_policy(mcp_port),
    })
}

/// Claude Code's fixed argv. The user prompt is exclusively file-backed stdin;
/// every argv entry is an immutable launch setting included in certification.
/// `--tools ""` removes every built-in tool schema; `--allowedTools` only
/// grants permission to the host's MCP tools and filters no schema.
/// `--strict-mcp-config` loads only the host endpoint; `--restricted` skips
/// user, project and local settings; `--settings` turns hooks off and forces
/// subscription sign-in. The MCP bearer is expanded by Claude from the
/// environment the guardian sets; argv carries only the reference.
fn claude_arguments(
    binding: &adapters::AttemptBinding,
    config: &kinewright_core::SessionConfig,
) -> io::Result<Vec<String>> {
    let bootstrap = crate::writer_host::mcp_credentials(binding.endpoint())
        .ok_or_else(|| io::Error::other("private host MCP credentials unavailable"))?;
    let mcp = serde_json::json!({
        "mcpServers": {
            "kinewright": {
                "type": "http",
                "url": binding.endpoint(),
                "headers": {
                    "authorization": "Bearer ${KINEWRIGHT_HOST_MCP_TOKEN}",
                    "x-kinewright-project": bootstrap.endpoint.project,
                    "x-kinewright-protocol": crate::writer_host::HOST_PROTOCOL.to_string(),
                    "x-kinewright-roots": bootstrap.roots.wire_header().map_err(io::Error::other)?,
                }
            }
        }
    });
    claude_argv(&mcp, config)
}

/// The built-in plugins Claude Code 2.1.317 loads in a print-mode session,
/// as its own `system/init` lists them.
const CLAUDE_BUILTIN_PLUGINS: [&str; 3] = [
    "cc-plugin-agents-md",
    "cc-plugin-privacy",
    "cc-plugin-plugin-authoring",
];

/// The argv around a given MCP configuration; [`claude_arguments`] supplies
/// the host endpoint's.
pub(crate) fn claude_argv(
    mcp: &serde_json::Value,
    config: &kinewright_core::SessionConfig,
) -> io::Result<Vec<String>> {
    let allowed = config
        .tool_names
        .as_ref()
        .filter(|names| !names.is_empty())
        .ok_or_else(|| io::Error::other("provider tool list missing"))?
        .iter()
        .map(|name| format!("mcp__kinewright__{name}"))
        .collect::<Vec<_>>()
        .join(",");
    // Claude Code ships built-in plugins (`<name>@builtin`) that load even in
    // safe mode, and `--bare` would force API-key auth. They are turned off
    // here by name. A built-in this list does not name (a newer CLI's) still
    // shows in `system/init` and is refused there.
    let settings = serde_json::json!({
        "disableAllHooks": true,
        "forceLoginMethod": "claudeai",
        "enableAllProjectMcpServers": false,
        "enabledPlugins": CLAUDE_BUILTIN_PLUGINS
            .iter()
            .map(|name| (format!("{name}@builtin"), serde_json::Value::Bool(false)))
            .collect::<serde_json::Map<_, _>>(),
    });
    let mut arguments: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--input-format",
        "text",
        "--restricted",
        "--strict-mcp-config",
        "--mcp-config",
    ]
    .map(str::to_owned)
    .into();
    arguments.push(mcp.to_string());
    arguments.extend(["--tools".into(), String::new()]);
    arguments.extend(["--allowedTools".into(), allowed]);
    arguments.extend(
        [
            "--permission-mode",
            "dontAsk",
            "--permission-prompts",
            "none",
            "--no-session-persistence",
            "--disable-slash-commands",
            "--settings",
        ]
        .map(str::to_owned),
    );
    arguments.push(settings.to_string());
    arguments.extend(["--system-prompt".into(), CLAUDE_SYSTEM_PROMPT.into()]);
    if let Some(model) = &config.model {
        arguments.extend(["--model".into(), model.clone()]);
    }
    if let Some(effort) = &config.effort {
        arguments.extend(["--effort".into(), effort.clone()]);
    }
    Ok(arguments)
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
    if specification.provider() == "claude" {
        crate::claude_credentials::verify_private_store(&credentials)?;
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
    let dialect = StreamDialect::of(specification.provider());
    let environment = match dialect {
        StreamDialect::Codex => environment(&scratch, &credentials),
        StreamDialect::Claude => claude_environment(&scratch, &credentials),
    };
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
            arguments.push("-".into());
        }
        "claude" => arguments.extend(claude_arguments(binding, config)?),
        #[cfg(any(test, feature = "test-util"))]
        "scripted" => arguments.extend([
            scratch.to_string_lossy().into_owned(),
            "s4b-codex".into(),
            "-".into(),
        ]),
        #[cfg(any(test, feature = "test-util"))]
        "scripted-claude" => arguments.extend([
            scratch.to_string_lossy().into_owned(),
            "s4b-claude".into(),
            "-".into(),
        ]),
        _ => return Err(io::Error::other("provider recipe unavailable")),
    }
    // The guardian seals exactly the bytes measured here into its memfd.
    let digest =
        kinewright_media::sha256_file(&runtime.executable.path).map_err(io::Error::other)?;
    // C6: Claude's sign-in is a private copy inside scratch, so its person's
    // configuration folder is the root no grant may reach.
    let credential_root = match specification.provider() {
        "claude" => Some(crate::claude_credentials::config_root()?),
        _ => scope
            .single_file
            .as_ref()
            .and_then(|mount| mount.source.parent().map(std::path::Path::to_path_buf)),
    };
    let result = Program {
        input: Some(crate::job_guardian::protocol::InputBinding::new(
            prompt.as_bytes(),
        )?),
        image: Some(crate::job_guardian::protocol::ExecutableImage {
            digest,
            source: None,
        }),
        credential_root,
        executable: runtime.executable.path.clone(),
        arguments,
        environment,
        working_directory: scratch.clone(),
        pipe_directory: Some(scratch.join("io")),
        sandbox: sandbox(
            &runtime.executable,
            &scratch,
            scope,
            reqwest::Url::parse(binding.endpoint())
                .map_err(io::Error::other)?
                .port_or_known_default()
                .ok_or_else(|| io::Error::other("MCP port missing"))?,
            specification.provider() == "codex",
        )?,
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
    let options = &program.arguments;
    if hex(&context.environment_digest) != digest(&serde_json::to_vec(&program.environment)?)
        || hex(&context.settings_digest) != digest(&serde_json::to_vec(options)?)
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
    /// C6: raw events by dialect. An owner with no Claude accounting refuses
    /// Claude events, so the reader refuses the stream rather than counting
    /// them as Codex.
    fn raw_as(
        &self,
        dialect: StreamDialect,
        identity: &crate::orchestration::ExecutionIdentity,
        raw: &serde_json::Value,
        ordinal: u64,
        eof: bool,
    ) -> io::Result<()> {
        match dialect {
            StreamDialect::Codex => self.raw(identity, raw, ordinal, eof),
            StreamDialect::Claude => Err(io::Error::other("Claude usage owner unavailable")),
        }
    }
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
    /// The provider exited before its first event and nobody asked it to stop.
    /// The owner shows the reason at once instead of waiting for a deadline.
    fn launch_failed(
        &self,
        _identity: &crate::orchestration::ExecutionIdentity,
        _reason: &str,
    ) -> io::Result<()> {
        Ok(())
    }
    /// C5 fix 10: the reader refused the provider's stream, so the provider
    /// was killed. The owner shows a fixed reason once instead of leaving the
    /// run pending with nothing to wake it.
    fn stream_refused(
        &self,
        identity: &crate::orchestration::ExecutionIdentity,
        reason: &str,
    ) -> io::Result<()> {
        self.launch_failed(identity, reason)
    }
}

pub(crate) struct NativeGuardianLauncher {
    pub images: Arc<crate::executable_images::ExecutableImages>,
    pub claim: Arc<Mutex<MachineClaim>>,
    pub owner: Arc<dyn InvocationOwner>,
    pub guardian: PathBuf,
    pub checks: Arc<dyn adapters::ConnectorRuntimeChecks + Send + Sync>,
    /// See [`FIRST_PROGRESS`]; witnesses shorten it.
    pub first_progress: std::time::Duration,
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
            .map_err(adapters::LaunchError)
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
            .map_err(|error| {
                crate::writer_host::provider_readiness::certification_error(
                    invocation.profile().specification().provider(),
                    error,
                )
            })?;
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
        let mut actual = program(invocation.profile(), &prompt)?;
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
        let unpaid = actual.sandbox.unpaid().map(|policy| policy.mcp_port);
        #[cfg(feature = "eval-harness")]
        if let (
            Some(_),
            ProviderSandbox::LinuxAllowlist {
                credential_store, ..
            },
        ) = (unpaid, &actual.sandbox)
        {
            record_unpaid_home(credential_store);
        }
        let pipes = ProviderPipeListeners::bind(
            actual
                .pipe_directory
                .as_ref()
                .ok_or_else(|| io::Error::other("provider pipe directory"))?,
        )?;
        let pipes = if unpaid.is_some() {
            pipes.with_relay()?
        } else {
            pipes
        };
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
        let image = actual
            .image
            .as_mut()
            .ok_or_else(|| io::Error::other("qualified image missing"))?;
        let sealed = self.images.get(&actual.executable, &image.digest)?;
        image.source = Some(crate::executable_images::source(&sealed));
        let input_binding = actual
            .input
            .as_mut()
            .ok_or_else(|| io::Error::other("native input binding missing"))?;
        let input = crate::job_guardian::input::seal(immutable.bytes(), input_binding)?;
        input_binding.source = Some(crate::job_guardian::input::source(&input));
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
        drop(input); // Namespace init owns its verified read-only copy now.
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
        if let Some(port) = unpaid {
            pipes.start_relay(port)?;
        }
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
        // The unused socket reaches EOF before COMMIT. The provider receives
        // the verified sealed file instead, with no writer to hold EOF open.
        let pipes = pipes.into_reader();
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
        let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let first_progress = self.first_progress;
        let dialect = StreamDialect::of(invocation.profile().specification().provider());
        thread::Builder::new()
            .name("kinewright-contained-codex".into())
            .spawn(move || {
                let crate::provider_pipes::ProviderReader {
                    output,
                    diagnostics,
                    guardian,
                    relay,
                } = pipes;
                let diagnostics = crate::provider_stderr::stderr_summary(
                    diagnostics,
                    "kinewright-contained-stderr",
                );
                let mut output = FirstProgress::new(output, first_progress);
                let ProviderRead {
                    completed,
                    refusal,
                    interrupt,
                    failed_turn,
                } = read_provider_stream_as(dialect, &mut output, &sender, |raw, ordinal, eof| {
                    usage_owner.raw_as(dialect, &execution, raw, ordinal, eof)
                });
                let mut completed = completed;
                let silent = output.bytes == 0;
                let stalled = output.stalled;
                // C5 fix 10: the reader refused the stream. The one
                // publication of why comes before the interrupt (whose `Done`
                // lets the pump drop the session) and before the cleanup below
                // kills the provider, so the run is never seen with no live
                // provider and no reason. A person's Stop already said why.
                // C5 fix 11: a counted failed turn the accounting did not
                // refuse still ends this attempt; say so the same way, once.
                let refusal = refusal
                    .or((failed_turn && !completed).then_some(STREAM_TURN_FAILED))
                    .filter(|_| !worker_stopped.load(std::sync::atomic::Ordering::SeqCst));
                if let Some(reason) = refusal
                    && usage_owner.stream_refused(&execution, reason).is_err()
                {
                    sender.interrupt("provider stream refusal remains unpublished");
                }
                if let Some(message) = interrupt {
                    sender.interrupt(message);
                }
                let mut native = worker_control
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                native.close_lifetime();
                let cleanup = native.cleanup();
                let stopped = worker_stopped.load(std::sync::atomic::Ordering::SeqCst);
                match cleanup {
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
                // The run has ended; so does its MCP relay.
                drop(relay);
                let diagnostics = diagnostics
                    .ok()
                    .and_then(|reader| reader.join().ok())
                    .unwrap_or_default();
                let guardian = guardian.status();
                if refusal.is_none() && silent && !sender.failed() && !stopped {
                    let reason = if stalled {
                        first_progress_failure(first_progress)
                    } else {
                        launch_failure(&guardian, diagnostics)
                    };
                    if usage_owner.launch_failed(&execution, &reason).is_err() {
                        sender.interrupt("provider launch failure remains unpublished");
                    } else {
                        sender.interrupt(&reason);
                    }
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
                stopped,
            },
        )))
    }
}

/// C5 fix 7: how long a launched native provider may stay silent before its
/// first stdout byte. Codex `exec --json` writes `thread.started` within
/// seconds of a healthy start; the longest startup wait in the retained Codex
/// 0.166 source is one MCP server's 30 s startup timeout, so 60 s is twice
/// that. A provider still silent then (invocation 8: alive, blocked on a held
/// stdin for 897 s) is interrupted instead of holding the run deadline.
pub(crate) const FIRST_PROGRESS: std::time::Duration = std::time::Duration::from_secs(60);

/// The fixed reason for a provider that started but never spoke.
pub(crate) fn first_progress_failure(deadline: std::time::Duration) -> String {
    format!(
        "the provider started but sent nothing within {} s",
        deadline.as_secs()
    )
}

/// Counts provider stdout so a launch that never spoke is told apart from a
/// turn that ended. Until the first byte, each read is bounded by what is left
/// of the first-progress deadline; at the deadline the stream reads as ended
/// and `stalled` is set, so the ordinary end path closes the lifetime, cleans
/// up and retires the attempt once.
struct FirstProgress {
    inner: std::os::unix::net::UnixStream,
    deadline: std::time::Instant,
    bytes: u64,
    stalled: bool,
}
impl FirstProgress {
    fn new(inner: std::os::unix::net::UnixStream, within: std::time::Duration) -> Self {
        Self {
            inner,
            deadline: std::time::Instant::now() + within,
            bytes: 0,
            stalled: false,
        }
    }
}
impl std::io::Read for FirstProgress {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.stalled {
            return Ok(0);
        }
        if self.bytes == 0 {
            let left = self
                .deadline
                .saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                self.stalled = true;
                return Ok(0);
            }
            self.inner.set_read_timeout(Some(left))?;
        }
        let read = match self.inner.read(buffer) {
            Ok(read) => read,
            Err(error)
                if self.bytes == 0
                    && matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
            {
                self.stalled = true;
                return Ok(0);
            }
            Err(error) => return Err(error),
        };
        if self.bytes == 0 && read > 0 {
            self.inner.set_read_timeout(None)?;
        }
        self.bytes = self.bytes.saturating_add(read as u64);
        Ok(read)
    }
}

/// The persisted reason is built only from the guardian's own record and the
/// fixed known-error text. No provider-written byte reaches it.
pub(crate) fn launch_failure(
    guardian: &crate::provider_pipes::GuardianStatus,
    diagnostics: crate::provider_stderr::StderrSummary,
) -> String {
    use crate::provider_pipes::GuardianStatus;
    let base = match guardian {
        GuardianStatus::Exited(status) => {
            format!("the provider exited before its first event (status {status})")
        }
        GuardianStatus::Refused(kind) => {
            format!("the provider launch was refused after commit ({kind})")
        }
        GuardianStatus::Unknown => {
            "the provider exited before its first event (status unknown)".to_owned()
        }
    };
    crate::provider_stderr::with_hint(base, diagnostics)
}

struct RestrictedSession {
    control: Arc<Mutex<ContainedSession>>,
    events: crossbeam_channel::Receiver<AgentEvent>,
    owner: Arc<dyn InvocationOwner>,
    identity: crate::orchestration::ExecutionIdentity,
    first: Option<String>,
    stopped: Arc<std::sync::atomic::AtomicBool>,
}
impl AgentSession for RestrictedSession {
    fn send_user_message(&mut self, text: String) -> Result<(), AgentError> {
        if text.is_empty() {
            return Err(AgentError::Protocol("provider prompt capacity".into()));
        }
        if text.len() > crate::job_guardian::protocol::INPUT_BYTES {
            return Err(AgentError::Protocol(
                crate::job_guardian::protocol::INPUT_TOO_LONG.into(),
            ));
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
        self.stopped
            .store(true, std::sync::atomic::Ordering::SeqCst);
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

/// C5 fix 10: fixed reasons the reader refuses a provider's stream with.
/// None carries the provider's bytes.
pub(crate) const STREAM_ACCOUNTING_REFUSED: &str =
    "provider stream refused: the host's accounting refused a provider event";
pub(crate) const STREAM_NATIVE_TOOL_REFUSED: &str =
    "provider stream refused: a provider-native tool is not allowed";
pub(crate) const STREAM_MALFORMED: &str = "provider stream refused: malformed provider event";
/// An event type or subtype this build does not know, typically from a newer
/// CLI. It is refused rather than skipped, so nothing unread passes.
pub(crate) const STREAM_UNKNOWN_EVENT: &str = "provider stream refused: unknown provider event";
pub(crate) const STREAM_USAGE_UNRESOLVED: &str =
    "provider stream refused: usage publication remains unresolved";
/// C5 fix 11: the provider ended after a counted `turn.failed` that paused
/// nothing. The fixed reason is published once, before cleanup, instead of
/// the generic stall. A Director failure delivery is the next stage's.
pub(crate) const STREAM_TURN_FAILED: &str = "the provider reported a failed turn";

/// What reading one provider's stream ended with.
pub(crate) struct ProviderRead {
    pub completed: bool,
    /// The fixed reason the reader refused the stream, if it did.
    pub refusal: Option<&'static str>,
    /// The interrupt the caller owes for that refusal. It is not sent here,
    /// so the native reader can publish why before `Done` lets the pump drop
    /// the session.
    pub interrupt: Option<&'static str>,
    /// C5 fix 11: the stream carried a `turn.failed` and no later
    /// `turn.completed`.
    pub failed_turn: bool,
}

/// The native launch path and uncertified CI children share bounded framing,
/// raw H1 accounting before display parsing, and terminal failure handling.
pub(crate) fn read_provider_output(
    output: impl std::io::Read,
    sender: &crate::protocol::limits::EventSender,
    record: impl FnMut(&serde_json::Value, u64, bool) -> io::Result<()>,
) -> bool {
    let read = read_provider_stream(output, sender, record);
    if let Some(message) = read.interrupt {
        sender.interrupt(message);
    }
    read.completed
}

pub(crate) fn read_provider_stream(
    output: impl std::io::Read,
    sender: &crate::protocol::limits::EventSender,
    record: impl FnMut(&serde_json::Value, u64, bool) -> io::Result<()>,
) -> ProviderRead {
    read_provider_stream_as(StreamDialect::Codex, output, sender, record)
}

/// C6: one reader for both dialects. Framing, accounting before display,
/// refusal and end-of-stream handling are shared; only the allowlist and the
/// terminal event differ. A Claude refusal takes the same path as a Codex
/// one: a fixed reason, then guardian cleanup by the caller.
pub(crate) fn read_provider_stream_as(
    dialect: StreamDialect,
    output: impl std::io::Read,
    sender: &crate::protocol::limits::EventSender,
    mut record: impl FnMut(&serde_json::Value, u64, bool) -> io::Result<()>,
) -> ProviderRead {
    let mut parser = crate::protocol::CodexProtocol::native_display();
    let mut claude = crate::protocol::ClaudeProtocol::native_display();
    let mut initialized = false;
    let mut ordinal = 0_u64;
    let mut completed = false;
    let mut refusal = None;
    let mut interrupt = None;
    let mut failed_turn = false;
    for line in crate::protocol::limits::Frames::new(output) {
        let result = (|| {
            let line = line.map_err(|_| STREAM_MALFORMED)?;
            ordinal = ordinal.checked_add(1).ok_or(STREAM_ACCOUNTING_REFUSED)?;
            let raw: serde_json::Value =
                serde_json::from_str(&line).map_err(|_| STREAM_MALFORMED)?;
            // Judged first, so a refused frame never becomes a usage sample.
            if dialect == StreamDialect::Claude {
                claude_event_allowed(&raw, &mut initialized)?;
            } else if !codex_event_known(&raw) {
                return Err(STREAM_UNKNOWN_EVENT);
            } else if !native_item_allowed(&raw) {
                return Err(STREAM_NATIVE_TOOL_REFUSED);
            }
            record(&raw, ordinal, false).map_err(|_| STREAM_ACCOUNTING_REFUSED)?;
            if dialect == StreamDialect::Claude {
                if raw.get("type").and_then(serde_json::Value::as_str) == Some("result") {
                    if initialized
                        && raw.get("subtype").and_then(serde_json::Value::as_str) == Some("success")
                        && raw.get("is_error").and_then(serde_json::Value::as_bool) != Some(true)
                    {
                        completed = true;
                        failed_turn = false;
                    } else {
                        failed_turn = true;
                    }
                }
                return claude.parse_line(&line).map_err(|_| STREAM_MALFORMED);
            }
            match raw.get("type").and_then(serde_json::Value::as_str) {
                Some("turn.completed") => {
                    completed = true;
                    failed_turn = false;
                }
                Some("turn.started") => {
                    completed = false;
                    failed_turn = false;
                }
                Some("turn.failed") => {
                    failed_turn = true;
                    completed = false;
                }
                _ => {}
            }
            parser.parse_line(&line).map_err(|_| STREAM_MALFORMED)
        })();
        match result {
            Ok(parsed) => {
                for event in parsed {
                    let event = match event {
                        AgentEvent::Error(message) if transport_error(&message) => {
                            AgentEvent::Error(
                                "network: provider transport failed or is reconnecting".into(),
                            )
                        }
                        other => other,
                    };
                    if !matches!(event, AgentEvent::Done | AgentEvent::Cost { .. })
                        && sender.send(event).is_err()
                    {
                        break;
                    }
                }
            }
            Err(reason) => {
                refusal = Some(reason);
                interrupt = Some("provider stream refused; cleanup pending");
                break;
            }
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
        refusal.get_or_insert(STREAM_USAGE_UNRESOLVED);
        interrupt.get_or_insert("usage publication remains unresolved");
    }
    ProviderRead {
        completed,
        refusal,
        interrupt,
        failed_turn,
    }
}

/// The top-level event types `codex exec --json` writes (its `ThreadEvent`,
/// [rust-v0.166.0](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/exec/src/exec_events.rs)).
/// A newer CLI is admitted by version floor, so an event type it adds is
/// refused instead of passing unread.
fn codex_event_known(raw: &serde_json::Value) -> bool {
    matches!(
        raw.get("type").and_then(serde_json::Value::as_str),
        Some(
            "thread.started"
                | "turn.started"
                | "turn.completed"
                | "turn.failed"
                | "item.started"
                | "item.updated"
                | "item.completed"
                | "error"
        )
    )
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
                        crate::orchestration::task_tool_names::TASK_TOOL_NAMES
                            .contains(&crate::protocol::display_tool_name(tool).as_str())
                    })
        }
        _ => false,
    }
}

/// C6: Claude Code loaded a tool, MCP server, plugin, skill, hook, subagent
/// or permission mode that its recipe does not give it.
pub(crate) const STREAM_AMBIENT_REFUSED: &str =
    "provider stream refused: the provider loaded configuration outside its recipe";
/// C6: Claude Code reported a sign-in other than the subscription.
pub(crate) const STREAM_AUTH_REFUSED: &str =
    "provider stream refused: the provider is not using subscription sign-in";

fn claude_tool_allowed(name: &str) -> bool {
    name.strip_prefix("mcp__kinewright__")
        .is_some_and(|tool| crate::orchestration::task_tool_names::TASK_TOOL_NAMES.contains(&tool))
}

/// C6: Claude's `system/init` lists what the session actually loaded. Only
/// subscription sign-in (`apiKeySource` "none" with the private OAuth store
/// and every API override cleared), only the host's MCP tools, only the host
/// server, no plugins or skills, and the recipe's permission mode pass.
fn claude_init_allowed(raw: &serde_json::Value) -> Result<(), &'static str> {
    use serde_json::Value;
    if raw.get("apiKeySource").and_then(Value::as_str) != Some("none") {
        return Err(STREAM_AUTH_REFUSED);
    }
    let tools = raw
        .get("tools")
        .and_then(Value::as_array)
        .ok_or(STREAM_MALFORMED)?;
    let servers = raw
        .get("mcp_servers")
        .and_then(Value::as_array)
        .ok_or(STREAM_MALFORMED)?;
    let empty = |key: &str| {
        raw.get(key)
            .is_none_or(|value| value.as_array().is_some_and(Vec::is_empty))
    };
    if !tools
        .iter()
        .all(|tool| tool.as_str().is_some_and(claude_tool_allowed))
        || servers.len() != 1
        || servers[0].get("name").and_then(Value::as_str) != Some("kinewright")
        || !empty("plugins")
        || !empty("skills")
        || raw.get("permissionMode").and_then(Value::as_str) != Some("dontAsk")
    {
        return Err(STREAM_AMBIENT_REFUSED);
    }
    // F3: a field this build has not read is refused, so ambient surface a
    // newer CLI reports under a new name never passes unread.
    if raw
        .as_object()
        .ok_or(STREAM_MALFORMED)?
        .keys()
        .any(|key| !CLAUDE_INIT_FIELDS.contains(&key.as_str()))
    {
        return Err(STREAM_UNKNOWN_EVENT);
    }
    Ok(())
}

/// Every field of Claude Code 2.1.317's `system/init`, as the S4b C6 dry
/// launch observed it on 2026-11-03.
const CLAUDE_INIT_FIELDS: [&str; 27] = [
    "additional_directories",
    "agents",
    "analytics_disabled",
    "apiKeySource",
    "capabilities",
    "claude_code_version",
    "cwd",
    "fast_mode_disabled_reason",
    "fast_mode_state",
    "mcp_servers",
    "model",
    "original_cwd",
    "output_style",
    "per_turn_effort_active",
    "permissionMode",
    "plugins",
    "product_feedback_disabled",
    "session_id",
    "skills",
    "slash_commands",
    "subtype",
    "tools",
    "type",
    "ultrathink_available",
    "ultrathink_typed_raises",
    "uuid",
    "view_mode",
];

/// Claude Code's `system`/`ui_invalidate` render notice; 2.1.317 sends one
/// before `init`. Bare, it names only the `ui.render` event and carries no
/// tool, text or configuration. `prompts_screened`, `instances` or
/// `client_modules` say a mod or plugin acts on the session, which the
/// recipe never loads. Any other field is unknown.
fn claude_ui_invalidate_allowed(raw: &serde_json::Value) -> Result<(), &'static str> {
    let object = raw.as_object().ok_or(STREAM_MALFORMED)?;
    if ["prompts_screened", "instances", "client_modules"]
        .iter()
        .any(|key| object.contains_key(*key))
    {
        return Err(STREAM_AMBIENT_REFUSED);
    }
    if raw.get("event").and_then(serde_json::Value::as_str) == Some("ui.render")
        && object.keys().all(|key| {
            matches!(
                key.as_str(),
                "type" | "subtype" | "event" | "session_id" | "uuid"
            )
        })
    {
        Ok(())
    } else {
        Err(STREAM_UNKNOWN_EVENT)
    }
}

/// C6: the Claude stream allowlist. Hooks, background tasks, subagent
/// messages, host control requests and provider-native tools are refused;
/// the listed informational events pass unchanged; any other type, system
/// subtype or content block is unknown and refused, since a newer CLI is
/// admitted by version floor rather than by exact build.
fn claude_event_allowed(
    raw: &serde_json::Value,
    initialized: &mut bool,
) -> Result<(), &'static str> {
    use serde_json::Value;
    let kind = raw
        .get("type")
        .and_then(Value::as_str)
        .ok_or(STREAM_MALFORMED)?;
    let blocks = |allowed: &dyn Fn(&Value) -> Result<(), &'static str>| {
        if raw
            .get("parent_tool_use_id")
            .is_some_and(|parent| !parent.is_null())
        {
            return Err(STREAM_AMBIENT_REFUSED);
        }
        if !*initialized {
            return Err(STREAM_MALFORMED);
        }
        match raw.pointer("/message/content") {
            Some(Value::String(_)) if kind == "user" => Ok(()),
            Some(Value::Array(content)) => content.iter().try_for_each(allowed),
            _ => Err(STREAM_MALFORMED),
        }
    };
    match kind {
        "system" => match raw.get("subtype").and_then(Value::as_str) {
            Some("init") if *initialized => Err(STREAM_MALFORMED),
            Some("init") => {
                claude_init_allowed(raw)?;
                *initialized = true;
                Ok(())
            }
            Some("api_retry" | "status" | "compact_boundary") => Ok(()),
            Some("ui_invalidate") => claude_ui_invalidate_allowed(raw),
            Some(subtype)
                if subtype.starts_with("hook_")
                    || subtype.starts_with("task_")
                    || subtype.starts_with("remote_")
                    || matches!(subtype, "local_command" | "plugin_install") =>
            {
                Err(STREAM_AMBIENT_REFUSED)
            }
            Some(_) => Err(STREAM_UNKNOWN_EVENT),
            None => Err(STREAM_MALFORMED),
        },
        "assistant" => blocks(&|block| match block.get("type").and_then(Value::as_str) {
            Some("text" | "thinking" | "redacted_thinking") => Ok(()),
            Some("tool_use")
                if block
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(claude_tool_allowed) =>
            {
                Ok(())
            }
            Some("tool_use" | "server_tool_use" | "mcp_tool_use") => {
                Err(STREAM_NATIVE_TOOL_REFUSED)
            }
            Some(kind) if kind.ends_with("_tool_result") => Err(STREAM_NATIVE_TOOL_REFUSED),
            Some(_) => Err(STREAM_UNKNOWN_EVENT),
            None => Err(STREAM_MALFORMED),
        }),
        "user" => blocks(&|block| match block.get("type").and_then(Value::as_str) {
            Some("text" | "tool_result") => Ok(()),
            _ => Err(STREAM_MALFORMED),
        }),
        // Informational events that carry no tool, text or usage the reader
        // acts on; accounting reads `result` and `assistant` only. Partial
        // `stream_event`s are never requested, so they are unknown here.
        "result" | "rate_limit_event" | "keep_alive" | "tool_progress" | "tool_use_summary"
        | "prompt_suggestion" => Ok(()),
        "auth_status" => Err(STREAM_AUTH_REFUSED),
        "control_request" => Err(STREAM_AMBIENT_REFUSED),
        _ => Err(STREAM_UNKNOWN_EVENT),
    }
}

/// A provider message about its network transport, for the display's
/// `network:` category only. It is broad on purpose and never decides
/// accounting: see `native_spending::codex_retry_notice` (C5 fix 11).
fn transport_error(message: &str) -> bool {
    let has = |needle: &[u8]| {
        message
            .as_bytes()
            .windows(needle.len())
            .any(|part| part.eq_ignore_ascii_case(needle))
    };
    !has(b"mcp")
        && [
            b"reconnect".as_slice(),
            b"network",
            b"tls",
            b"dns",
            b"connection",
            b"stream disconnected",
        ]
        .iter()
        .any(|needle| has(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Riel 2026-11-02: no exact CLI pin. A newer CLI passes the version
    /// floor, and an event type it adds is refused by the stream reader
    /// instead of passing unread.
    #[test]
    fn s4b_newer_cli_passes_the_floor_and_its_unknown_event_is_refused() {
        use crate::orchestration::adapters::{
            CertificationRefusal, CliVersion, claude_candidate, codex_candidate, test_runtime,
        };
        for (spec, newer, older) in [
            (
                codex_candidate(),
                CliVersion(0, 200, 0),
                CliVersion(0, 165, 9),
            ),
            (
                claude_candidate(),
                CliVersion(2, 2, 0),
                CliVersion(2, 1, 315),
            ),
        ] {
            let mut check = test_runtime();
            check.version = newer;
            assert_ne!(
                spec.validate_runtime(&check),
                Err(CertificationRefusal::UnqualifiedVersion),
                "NEWER_CLI_PASSES_THE_FLOOR {}",
                spec.provider()
            );
            check.version = older;
            assert_eq!(
                spec.validate_runtime(&check),
                Err(CertificationRefusal::UnqualifiedVersion),
                "OLDER_CLI_REFUSED {}",
                spec.provider()
            );
        }
        let init = r#"{"type":"system","subtype":"init","apiKeySource":"none","tools":["mcp__kinewright__get_timeline_state"],"mcp_servers":[{"name":"kinewright","status":"connected"}],"permissionMode":"dontAsk","plugins":[],"skills":[]}"#;
        for (dialect, lines) in [
            (
                StreamDialect::Codex,
                vec![
                    r#"{"type":"thread.started","thread_id":"t"}"#,
                    r#"{"type":"thread.summarized","usage":{"input_tokens":7,"output_tokens":3}}"#,
                ],
            ),
            (
                StreamDialect::Claude,
                vec![init, r#"{"type":"session_title","title":"x"}"#],
            ),
            (
                StreamDialect::Claude,
                vec![
                    r#"{"type":"system","subtype":"init","apiKeySource":"none","tools":["mcp__kinewright__get_timeline_state"],"mcp_servers":[{"name":"kinewright","status":"connected"}],"permissionMode":"dontAsk","plugins":[],"skills":[],"hooks_loaded":["person"]}"#,
                ],
            ),
            (
                StreamDialect::Claude,
                vec![init, r#"{"type":"system","subtype":"memory_saved"}"#],
            ),
            (
                StreamDialect::Claude,
                vec![
                    init,
                    r#"{"type":"assistant","message":{"content":[{"type":"citation_card"}]},"parent_tool_use_id":null}"#,
                ],
            ),
        ] {
            let (sender, _events) = crate::protocol::limits::event_channel();
            let mut bytes = lines.join("\n").into_bytes();
            bytes.push(b'\n');
            let mut accounted = Vec::new();
            let read = read_provider_stream_as(
                dialect,
                std::io::Cursor::new(bytes),
                &sender,
                |raw, _, _| {
                    accounted.push(raw.to_string());
                    Ok(())
                },
            );
            assert_eq!(
                read.refusal,
                Some(STREAM_UNKNOWN_EVENT),
                "UNKNOWN_EVENT_REFUSED {dialect:?} {lines:?}"
            );
            assert!(!read.completed);
            // F4: the refused frame is judged before accounting, so it never
            // becomes a usage sample; only the frames before it and the EOF are.
            let refused: serde_json::Value = serde_json::from_str(lines[lines.len() - 1]).unwrap();
            assert!(
                !accounted.contains(&refused.to_string()),
                "C6_REFUSED_FRAME_ACCOUNTED {dialect:?} {lines:?}"
            );
            assert_eq!(
                accounted.len(),
                lines.len(),
                "C6_ACCOUNTED_FRAMES {accounted:?}"
            );
        }
    }

    fn c5_fix4_reason(stderr: &[u8], record: &[u8]) -> String {
        let (mut writer, reader) = std::os::unix::net::UnixStream::pair().unwrap();
        let summary = crate::provider_stderr::stderr_summary(reader, "fix4-test").unwrap();
        // A real socket delivers stderr in pieces, as the provider writes it.
        for piece in stderr.chunks(4093) {
            std::io::Write::write_all(&mut writer, piece).unwrap();
        }
        drop(writer);
        launch_failure(
            &crate::provider_pipes::parse_guardian_status(record),
            summary.join().unwrap(),
        )
    }
    #[test]
    fn s4b_c5_fix4_reason_never_carries_provider_bytes() {
        // Astra probe case 1: a credential-looking value before the exit.
        let reason = c5_fix4_reason(
            b"auth error: refresh_token=synthetic-subscription-secret\nkinewright-guardian: provider exited with status 1\n",
            b"kinewright-guardian/1 exited 1\n",
        );
        assert_eq!(
            reason, "the provider exited before its first event (status 1)",
            "C5_FIX4_CREDENTIAL_TEXT_PERSISTED"
        );
        // Astra probe case 2: a bearer split by the old 64 KiB capture boundary.
        let mut split = vec![b'e'; crate::protocol::limits::FRAME_BYTES - 10];
        split.extend_from_slice(b"\nsynthetic-bearer-0123456789abcdef\n");
        let reason = c5_fix4_reason(&split, b"kinewright-guardian/1 exited 1\n");
        assert!(
            !reason.contains("synthetic") && !reason.contains("eeee"),
            "C5_FIX4_BEARER_FRAGMENT_PERSISTED {reason}"
        );
        // Long stderr with a forged guardian line keeps the real status.
        let mut long = vec![b'x'; 1 << 20];
        long.extend_from_slice(b"\nkinewright-guardian/1 exited 0\n");
        assert_eq!(
            c5_fix4_reason(&long, b"kinewright-guardian/1 exited 3\n"),
            "the provider exited before its first event (status 3)",
            "C5_FIX4_LONG_STDERR_STATUS"
        );
        // The known Codex error maps to its fixed text, even split late in
        // a long stream, and the provider's own wording is not kept.
        let mut codex = vec![b'y'; 100_000 - 7];
        codex.extend_from_slice(b"\nError: Codex executable path is not configured\n");
        let reason = c5_fix4_reason(&codex, b"kinewright-guardian/1 exited 1\n");
        assert_eq!(
            reason,
            "the provider exited before its first event (status 1): Codex could not resolve its own executable path",
            "C5_FIX4_KNOWN_CODEX_ERROR"
        );
        assert_eq!(
            c5_fix4_reason(&[], b"kinewright-guardian/1 refused PermissionDenied\n"),
            "the provider launch was refused after commit (PermissionDenied)"
        );
        for record in [
            b"".as_slice(),
            b"kinewright-guardian/1 exited 3",
            b"kinewright-guardian/1 exited 3 sk-synthetic\n",
            b"kinewright-guardian/1 refused sk_synthetic-1\n",
            b"kinewright-guardian/2 exited 3\n",
        ] {
            assert_eq!(
                c5_fix4_reason(b"refresh_token=synthetic\n", record),
                "the provider exited before its first event (status unknown)",
                "C5_FIX4_MALFORMED_RECORD {record:?}"
            );
        }
        assert_eq!(
            crate::job_guardian::protocol::guardian_status_record(&Ok(3)),
            "kinewright-guardian/1 exited 3\n"
        );
    }
    #[test]
    fn s4b_c5c_retries_surface_network_before_eof() {
        use std::io::Write;
        assert!(!transport_error("MCP connection closed"));
        let (mut writer, reader) = std::os::unix::net::UnixStream::pair().unwrap();
        let (sender, events) = crate::protocol::limits::event_channel();
        let eof = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = eof.clone();
        let worker = thread::spawn(move || {
            read_provider_output(reader, &sender, |_, _, end| {
                if end {
                    seen.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                Ok(())
            })
        });
        writer.write_all(b"{\"type\":\"error\",\"message\":\"Reconnecting... 2/5\"}\n{\"type\":\"error\",\"message\":\"Reconnecting... waiting for network\"}\n").unwrap();
        let first = events.recv_timeout(std::time::Duration::from_secs(2));
        let second = events.recv_timeout(std::time::Duration::from_secs(2));
        let before_eof = !eof.load(std::sync::atomic::Ordering::SeqCst);
        drop(writer);
        let completed = worker.join().unwrap();
        assert!(!completed);
        assert!(eof.load(std::sync::atomic::Ordering::SeqCst));
        assert!(before_eof, "C5C_NETWORK_BEFORE_EOF");
        for observed in [first, second] {
            assert!(
                matches!(observed, Ok(AgentEvent::Error(message)) if message.starts_with("network:")),
                "C5C_RECONNECT_CATEGORY"
            );
        }
    }
}
