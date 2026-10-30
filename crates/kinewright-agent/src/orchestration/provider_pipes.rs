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
}
