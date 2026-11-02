//! Private provider data pipes never carry guardian lifetime or control frames.
use std::{
    fs, io,
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::Instant,
};

pub struct ProviderPipeListeners {
    root: PathBuf,
    listeners: [UnixListener; 4],
    relay: Option<UnixListener>,
}
pub struct ProviderPipes {
    pub input: UnixStream,
    pub output: UnixStream,
    pub diagnostics: UnixStream,
    credentials: UnixStream,
    relay_channel: Option<UnixStream>,
    relay: Option<McpRelay>,
}
impl ProviderPipeListeners {
    /// # Errors
    /// Refuses reuse of an existing directory or unavailable private sockets.
    pub fn bind(root: &Path) -> io::Result<Self> {
        fs::create_dir(root)?;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
        let listeners = [
            UnixListener::bind(root.join("stdin"))?,
            UnixListener::bind(root.join("stdout"))?,
            UnixListener::bind(root.join("stderr"))?,
            UnixListener::bind(root.join("credentials"))?,
        ];
        for listener in &listeners {
            listener.set_nonblocking(true)?;
        }
        Ok(Self {
            root: root.into(),
            listeners,
            relay: None,
        })
    }
    /// C5 fix 8: also bind the unpaid dry policy's relay pipe.
    ///
    /// # Errors
    /// Refuses an unavailable socket.
    pub fn with_relay(mut self) -> io::Result<Self> {
        let relay = UnixListener::bind(self.root.join(crate::job_guardian::protocol::RELAY_PIPE))?;
        relay.set_nonblocking(true)?;
        self.relay = Some(relay);
        Ok(self)
    }
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.root
    }

    /// # Errors
    /// Refuses missing provider connections at the startup deadline.
    pub fn accept(self) -> io::Result<ProviderPipes> {
        fn accept(listener: &UnixListener, deadline: Instant) -> io::Result<UnixStream> {
            loop {
                match listener.accept() {
                    Ok((pipe, _)) => return Ok(pipe),
                    Err(error)
                        if error.kind() == io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        let deadline = Instant::now() + crate::job_guardian::protocol::STARTUP;
        Ok(ProviderPipes {
            input: accept(&self.listeners[0], deadline)?,
            output: accept(&self.listeners[1], deadline)?,
            diagnostics: accept(&self.listeners[2], deadline)?,
            credentials: accept(&self.listeners[3], deadline)?,
            relay_channel: self
                .relay
                .as_ref()
                .map(|relay| accept(relay, deadline))
                .transpose()?,
            relay: None,
        })
    }
}
impl Drop for ProviderPipeListeners {
    fn drop(&mut self) {
        for name in [
            "stdin",
            "stdout",
            "stderr",
            "credentials",
            crate::job_guardian::protocol::RELAY_PIPE,
        ] {
            let _ = fs::remove_file(self.root.join(name));
        }
        let _ = fs::remove_dir(&self.root);
    }
}
impl ProviderPipes {
    /// # Errors
    /// Refuses an oversized secret or a broken private credential channel.
    pub fn credentials(&mut self, secret: &[u8]) -> io::Result<()> {
        use std::io::Write;
        if secret.len() > 4096 {
            return Err(io::Error::other("provider credential capacity"));
        }
        self.credentials
            .set_write_timeout(Some(crate::job_guardian::protocol::STARTUP))?;
        self.credentials.write_all(secret)?;
        self.credentials.shutdown(std::net::Shutdown::Write)
    }
    /// The guardian's exit record arrives on the credential socket after the
    /// host has shut down its own write side.
    ///
    /// # Errors
    /// Refuses an unavailable descriptor clone.
    pub fn guardian_channel(&self) -> io::Result<GuardianChannel> {
        self.credentials.try_clone().map(GuardianChannel)
    }
    /// C5 fix 7: ends the provider's stdin at once and keeps only what the
    /// host reads. Codex `exec` reads a non-TTY stdin to EOF before its turn,
    /// so a held stdin is a provider that never starts. The returned set has
    /// no stdin, so no later closure can hold it open again.
    #[must_use]
    pub fn into_reader(self) -> ProviderReader {
        // Shutdown reaches the peer even if a descriptor copy survives.
        let _ = self.input.shutdown(std::net::Shutdown::Both);
        drop(self.input);
        ProviderReader {
            output: self.output,
            diagnostics: self.diagnostics,
            guardian: GuardianChannel(self.credentials),
            relay: self.relay,
        }
    }
    /// C5 fix 8: start copying each loopback connection init hands over to
    /// the host's MCP listener on `127.0.0.1:port`.
    ///
    /// # Errors
    /// Refuses a missing relay pipe or an unavailable thread.
    pub fn start_relay(&mut self, port: u16) -> io::Result<()> {
        let channel = self
            .relay_channel
            .take()
            .ok_or_else(|| io::Error::other("unpaid relay pipe missing"))?;
        self.relay = Some(McpRelay::start(
            channel,
            std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port)),
        )?);
        Ok(())
    }
}

/// What the host owns once the provider runs: its stdout, its stderr, the
/// guardian's status channel and, under the unpaid policy, the MCP relay.
/// The provider's stdin is not part of it. Dropping it ends the relay.
pub struct ProviderReader {
    pub output: UnixStream,
    pub diagnostics: UnixStream,
    pub guardian: GuardianChannel,
    pub relay: Option<McpRelay>,
}

/// Most relayed connections open at once.
pub const RELAY_OPEN: usize = 16;

type RelayStreams = std::sync::Mutex<std::collections::BTreeMap<u64, [std::net::TcpStream; 2]>>;

/// C5 fix 8: the host half of the unpaid dry policy's MCP bridge. Each
/// descriptor init passes is one TCP connection the provider opened to
/// `127.0.0.1:<port>` inside its own network namespace. The host connects to
/// its real MCP listener and copies bytes both ways, nothing else: the bearer
/// and every header check stay end to end. At most [`RELAY_OPEN`] are open
/// at once; dropping the relay closes the channel and every connection.
pub struct McpRelay {
    channel: UnixStream,
    open: std::sync::Arc<RelayStreams>,
}
impl McpRelay {
    fn start(channel: UnixStream, target: std::net::SocketAddr) -> io::Result<Self> {
        let open = std::sync::Arc::new(RelayStreams::default());
        let receiver = channel.try_clone()?;
        let table = std::sync::Arc::clone(&open);
        std::thread::Builder::new()
            .name("kinewright-mcp-relay".into())
            .spawn(move || {
                let mut next = 0_u64;
                while let Ok(Some(descriptor)) = kinewright_job_sys::receive_descriptor(&receiver) {
                    let inner = std::net::TcpStream::from(descriptor);
                    match table.lock() {
                        Ok(streams) if streams.len() < RELAY_OPEN => {}
                        Ok(_) => continue,
                        Err(_) => break,
                    }
                    let Ok(outer) = std::net::TcpStream::connect_timeout(
                        &target,
                        crate::job_guardian::protocol::STARTUP,
                    ) else {
                        continue;
                    };
                    let (Ok(a), Ok(b), Ok(c), Ok(d)) = (
                        inner.try_clone(),
                        outer.try_clone(),
                        inner.try_clone(),
                        outer.try_clone(),
                    ) else {
                        continue;
                    };
                    next += 1;
                    let Ok(mut streams) = table.lock() else {
                        break;
                    };
                    streams.insert(next, [a, b]);
                    drop(streams);
                    let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    for (from, to) in [(inner, outer), (d, c)] {
                        let finished = std::sync::Arc::clone(&finished);
                        let shared = std::sync::Arc::clone(&table);
                        let key = next;
                        let spawned = std::thread::Builder::new()
                            .name("kinewright-mcp-relay-copy".into())
                            .spawn(move || {
                                let mut reader = &from;
                                let mut writer = &to;
                                let _ = io::copy(&mut reader, &mut writer);
                                let _ = to.shutdown(std::net::Shutdown::Write);
                                if finished.swap(true, std::sync::atomic::Ordering::SeqCst)
                                    && let Ok(mut streams) = shared.lock()
                                {
                                    streams.remove(&key);
                                }
                            });
                        if spawned.is_err()
                            && let Ok(mut streams) = table.lock()
                            && let Some(pair) = streams.remove(&key)
                        {
                            for stream in pair {
                                let _ = stream.shutdown(std::net::Shutdown::Both);
                            }
                        }
                    }
                }
            })?;
        Ok(Self { channel, open })
    }
}
impl Drop for McpRelay {
    fn drop(&mut self) {
        let _ = self.channel.shutdown(std::net::Shutdown::Both);
        if let Ok(streams) = self.open.lock() {
            for stream in streams.values().flatten() {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        }
    }
}

/// Init's private status channel. Only the guardian writes to it.
pub struct GuardianChannel(UnixStream);
impl GuardianChannel {
    /// Read the bounded record once init has exited. A missing, oversized or
    /// malformed record is `Unknown`, never text.
    #[must_use]
    pub fn status(self) -> GuardianStatus {
        use std::io::Read;
        let cap = GUARDIAN_STATUS_CAP;
        let mut record = Vec::new();
        let read = self
            .0
            .set_read_timeout(Some(crate::job_guardian::protocol::CLEANUP))
            .and_then(|()| (&self.0).take(cap as u64 + 1).read_to_end(&mut record));
        if read.is_err() || record.len() > cap {
            return GuardianStatus::Unknown;
        }
        parse_guardian_status(&record)
    }
}

pub const GUARDIAN_STATUS_CAP: usize = 64;

/// What the guardian says happened after COMMIT. It carries no provider bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuardianStatus {
    Exited(i32),
    /// An `io::ErrorKind` name: ASCII letters only, at most 32 bytes.
    Refused(String),
    Unknown,
}

#[must_use]
pub fn parse_guardian_status(record: &[u8]) -> GuardianStatus {
    let Some(line) = std::str::from_utf8(record)
        .ok()
        .and_then(|text| text.strip_suffix('\n'))
        .and_then(|text| text.strip_prefix(crate::job_guardian::protocol::GUARDIAN_STATUS))
        .and_then(|text| text.strip_prefix(' '))
    else {
        return GuardianStatus::Unknown;
    };
    match line.split_once(' ') {
        Some(("exited", status)) => status
            .parse()
            .map_or(GuardianStatus::Unknown, GuardianStatus::Exited),
        Some(("refused", kind))
            if !kind.is_empty()
                && kind.len() <= 32
                && kind.bytes().all(|byte| byte.is_ascii_alphabetic()) =>
        {
            GuardianStatus::Refused(kind.to_owned())
        }
        _ => GuardianStatus::Unknown,
    }
}
