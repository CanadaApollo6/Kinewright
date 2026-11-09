use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    path::PathBuf,
    time::Duration,
};

pub const STARTUP: Duration = Duration::from_secs(5);
pub const GRACE: Duration = Duration::from_millis(500);
pub const CLEANUP: Duration = Duration::from_secs(2);
pub const WIRE_CAP: usize = 64 * 1024;
pub const RECOVERY_CAP: usize = 1024;
pub const VERSION: u32 = 1;
pub const INPUT_BYTES: usize = 256 * 1024;
pub const INPUT_TOO_LONG: &str = "This feedback is too long to send in one run. Shorten or split the notes, then start the run again.";

/// Namespace init's own exit record. Init writes it on the private credential
/// socket, which the provider never holds, so provider output cannot forge it.
#[cfg(target_os = "linux")]
pub const GUARDIAN_STATUS: &str = "kinewright-guardian/1";

#[cfg(target_os = "linux")]
#[must_use]
pub fn guardian_status_record(result: &io::Result<i32>) -> String {
    match result {
        Ok(status) => format!("{GUARDIAN_STATUS} exited {status}\n"),
        Err(error) => format!("{GUARDIAN_STATUS} refused {:?}\n", error.kind()),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttemptIdentity {
    pub project: String,
    pub run: String,
    pub task: String,
    pub attempt: String,
    pub nonce: String,
    pub owner_instance: String,
}
impl AttemptIdentity {
    /// # Errors
    /// Refuses missing, oversized, or path-like identity components.
    pub fn validate(&self) -> io::Result<()> {
        for part in [
            &self.project,
            &self.run,
            &self.task,
            &self.attempt,
            &self.nonce,
            &self.owner_instance,
        ] {
            if part.is_empty()
                || part.len() > 64
                || !part.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
            {
                return Err(io::Error::other("invalid guardian identity"));
            }
        }
        if self.nonce.len() != 32
            || self.owner_instance.len() != 32
            || !self.nonce.bytes().all(|b| b.is_ascii_hexdigit())
            || !self.owner_instance.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(io::Error::other(
                "guardian nonce/owner must contain 128 bits",
            ));
        }
        Ok(())
    }
    #[must_use]
    pub fn unit(&self) -> String {
        format!("kinewright-job-{}.service", self.nonce)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Program {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<InputBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<ExecutableImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_root: Option<PathBuf>,
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub environment: std::collections::BTreeMap<String, String>,
    pub working_directory: PathBuf,
    pub sandbox: ProviderSandbox,
    pub pipe_directory: Option<PathBuf>,
}

/// Exact immutable stdin, independently bounded from launch metadata.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InputBinding {
    pub sha256: String,
    pub byte_length: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<PathBuf>,
}
impl InputBinding {
    #[cfg(target_os = "linux")]
    #[allow(
        dead_code,
        reason = "the standalone guardian receives bindings constructed by the host"
    )]
    pub(crate) fn new(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > INPUT_BYTES {
            return Err(io::Error::other(INPUT_TOO_LONG));
        }
        Ok(Self {
            sha256: kinewright_hash::sha256_bytes(bytes),
            byte_length: bytes.len() as u64,
            source: None,
        })
    }
    pub(crate) fn validate(&self) -> io::Result<()> {
        if self.byte_length > INPUT_BYTES as u64 {
            return Err(io::Error::other(INPUT_TOO_LONG));
        }
        if self.sha256.len() != 64
            || !self.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || self.source.as_ref().is_some_and(|p| !p.is_absolute())
        {
            return Err(io::Error::other("invalid native input binding"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutableImage {
    pub digest: String,
    pub source: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CredentialMount {
    pub source: PathBuf,
    pub target: PathBuf,
    pub identity: [u64; 2],
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "policy", deny_unknown_fields)]
pub enum ProviderSandbox {
    ContainmentOnly,
    LinuxAllowlist {
        credential_mount: Option<Box<CredentialMount>>,
        connect_tcp: Vec<u16>,
        runtime: Vec<PathBuf>,
        scratch: PathBuf,
        credential_store: PathBuf,
        executable_identity: [u64; 2],
        /// Absent from every production manifest.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unpaid: Option<UnpaidPolicy>,
    },
}

/// C5 fix 8: the unpaid dry policy. The payload gets its own network
/// namespace with only loopback. Namespace init listens on
/// `127.0.0.1:mcp_port` there and hands each connection it accepts to the
/// host over the private `relay` pipe; the host copies bytes to its real MCP
/// listener. The mounted sign-in is a read-only bind, and init refuses to
/// launch unless the private per-attempt home holds nothing else of the
/// person's. The home itself stays writable for the provider's own state.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UnpaidPolicy {
    pub mcp_port: u16,
}

/// The fifth provider pipe, present only under [`UnpaidPolicy`].
#[cfg(target_os = "linux")]
pub const RELAY_PIPE: &str = "relay";

#[cfg(target_os = "linux")]
impl ProviderSandbox {
    #[must_use]
    pub fn unpaid(&self) -> Option<&UnpaidPolicy> {
        match self {
            Self::LinuxAllowlist { unpaid, .. } => unpaid.as_ref(),
            Self::ContainmentOnly => None,
        }
    }
}
impl Program {
    #[cfg(all(target_os = "linux", any(test, feature = "eval-harness")))]
    #[allow(
        dead_code,
        reason = "the guardian binary includes the shared test protocol surface"
    )]
    pub(crate) fn fixture_image(&mut self) -> io::Result<Option<std::fs::File>> {
        if matches!(self.sandbox, ProviderSandbox::ContainmentOnly) {
            return Ok(None);
        }
        let digest = self.image.as_ref().map_or_else(
            || kinewright_media::sha256_file(&self.executable).map_err(io::Error::other),
            |i| Ok(i.digest.clone()),
        )?;
        let image = kinewright_job_sys::seal_executable(&self.executable, &digest)?;
        self.image = Some(ExecutableImage {
            digest,
            source: Some(PathBuf::from(format!(
                "/proc/{}/fd/{}",
                std::process::id(),
                std::os::fd::AsRawFd::as_raw_fd(&image)
            ))),
        });
        if let ProviderSandbox::LinuxAllowlist {
            credential_mount: Some(mount),
            ..
        } = &self.sandbox
        {
            self.credential_root = mount.source.parent().map(std::path::Path::to_path_buf);
        }
        Ok(Some(image))
    }
    /// # Errors
    /// Refuses relative executables, excessive arguments, or protocol overflow.
    pub fn validate(&self) -> io::Result<()> {
        if let Some(input) = &self.input {
            input.validate()?;
            #[cfg(not(target_os = "linux"))]
            return Err(io::Error::other(
                "native stdin transport is not qualified on this platform",
            ));
        }
        if !self.executable.is_absolute()
            || !self.working_directory.is_absolute()
            || self.image.as_ref().is_some_and(|i| {
                i.digest.len() != 64
                    || !i.digest.bytes().all(|b| b.is_ascii_hexdigit())
                    || i.source.as_ref().is_some_and(|p| !p.is_absolute())
            })
            || self.environment.len() > 16
            || self.environment.iter().any(|(key, value)| {
                !matches!(
                    key.as_str(),
                    "PATH"
                        | "HOME"
                        | "LANG"
                        | "LC_ALL"
                        | "TMPDIR"
                        | "CODEX_HOME"
                        | "XDG_RUNTIME_DIR"
                        // S4b C6: Claude Code's fixed isolation switches.
                        | "CLAUDE_CONFIG_DIR"
                        | "ENABLE_TOOL_SEARCH"
                        | "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"
                        | "CLAUDE_CODE_DISABLE_AUTO_MEMORY"
                        | "CLAUDE_CODE_DISABLE_CLAUDE_MDS"
                        | "DISABLE_AUTOUPDATER"
                        | "DISABLE_TELEMETRY"
                        | "DISABLE_ERROR_REPORTING"
                ) || value.contains('\0')
            })
            || self
                .pipe_directory
                .as_ref()
                .is_some_and(|path| !path.is_absolute())
            || self.arguments.len() > 128
            || serde_json::to_vec(self)?.len() > WIRE_CAP / 2
            || self.arguments.iter().any(|s| s.contains('\0'))
        {
            return Err(io::Error::other("invalid native program"));
        }
        if let ProviderSandbox::LinuxAllowlist {
            runtime,
            scratch,
            credential_store,
            executable_identity,
            connect_tcp,
            credential_mount,
            unpaid,
        } = &self.sandbox
            && (credential_mount.as_ref().is_some_and(|mount| {
                self.credential_root.as_deref() != mount.source.parent()
                    || !mount.source.is_absolute()
                    || mount.target != credential_store.join("auth.json")
                    || !credential_store.starts_with(scratch)
                    || mount.identity == [0, 0]
            }) || self.pipe_directory.is_none()
                || connect_tcp.is_empty()
                || connect_tcp.len() > 2
                || connect_tcp.contains(&0)
                || unpaid
                    .as_ref()
                    .is_some_and(|policy| connect_tcp != &[policy.mcp_port])
                || runtime.len() > 32
                || runtime
                    .iter()
                    .chain([scratch, credential_store])
                    .any(|path| !path.is_absolute())
                || scratch != &self.working_directory
                || executable_identity == &[0, 0])
        {
            return Err(io::Error::other("invalid provider sandbox"));
        }
        #[cfg(target_os = "linux")]
        if let (
            ProviderSandbox::LinuxAllowlist {
                runtime,
                scratch,
                credential_store,
                ..
            },
            Some(root),
        ) = (&self.sandbox, &self.credential_root)
        {
            for grant in runtime.iter().chain([
                scratch,
                credential_store,
                &self.executable,
                &self.working_directory,
            ]) {
                kinewright_job_sys::validate_credential_grant(grant, root)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecoveryIdentity {
    pub version: u32,
    pub attempt: AttemptIdentity,
    pub guardian_pid: u32,
    pub guardian_start: u64,
    pub containment: String,
    pub device: u64,
    pub inode: u64,
    pub service_device: u64,
    pub service_inode: u64,
}
impl RecoveryIdentity {
    /// # Errors
    /// Refuses unsupported or oversized private recovery identities.
    pub fn validate(&self) -> io::Result<()> {
        self.attempt.validate()?;
        if self.version != VERSION
            || serde_json::to_vec(self)?.len() > RECOVERY_CAP
            || self.guardian_pid == 0
        {
            return Err(io::Error::other("invalid recovery identity"));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum HostMessage {
    Prepare {
        identity: AttemptIdentity,
        program: Box<Program>,
    },
    Contain {
        nonce: String,
    },
    Commit {
        nonce: String,
    },
    Cancel {
        nonce: String,
    },
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum GuardianMessage {
    Owned {
        recovery: RecoveryIdentity,
        at_us: u64,
    },
    Contained {
        nonce: String,
        pid: u32,
        start: u64,
        at_us: u64,
    },
    Executing {
        nonce: String,
        at_us: u64,
    },
    Cleaned {
        nonce: String,
        killed_at_us: u64,
        empty_at_us: u64,
        reaped_at_us: u64,
    },
    CleanupBlocked {
        nonce: String,
        reason: String,
    },
    Refused {
        reason: String,
    },
}

/// Reads one bounded frame before JSON allocation. EOF is lifetime loss.
///
/// # Errors
/// Returns EOF, malformed JSON, or a byte-cap refusal.
pub fn read_frame<T: for<'de> Deserialize<'de>>(reader: &mut impl Read) -> io::Result<T> {
    let mut bytes = Vec::with_capacity(1024);
    let mut byte = [0];
    loop {
        if reader.read(&mut byte)? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        if byte[0] == b'\n' {
            break;
        }
        if bytes.len() == WIRE_CAP {
            return Err(io::Error::other("guardian frame cap"));
        }
        bytes.push(byte[0]);
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

/// # Errors
/// Refuses oversized frames or a closed control pipe.
pub fn write_frame(writer: &mut impl Write, message: &impl Serialize) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(message)?;
    if bytes.len() > WIRE_CAP {
        return Err(io::Error::other("guardian frame cap"));
    }
    bytes.push(b'\n');
    writer.write_all(&bytes)?;
    writer.flush()
}
/// Durable fence around Linux manager submission; PID/start is reopened through a pidfd.
#[cfg(target_os = "linux")]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransportIntent {
    pub attempt: AttemptIdentity,
    pub state: TransportState,
}
#[cfg(target_os = "linux")]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum TransportState {
    Unstarted,
    Starting,
    Started { pid: u32, start: u64 },
    Closed,
}
