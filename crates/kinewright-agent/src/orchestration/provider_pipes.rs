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
}
pub struct ProviderPipes {
    pub input: UnixStream,
    pub output: UnixStream,
    pub diagnostics: UnixStream,
    credentials: UnixStream,
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
        })
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
        })
    }
}
impl Drop for ProviderPipeListeners {
    fn drop(&mut self) {
        for name in ["stdin", "stdout", "stderr", "credentials"] {
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
