use super::protocol::Program;
use kinewright_job_sys::{Cgroup, ProcessHandle};
use std::{
    fs,
    io::{self, Read, Write},
    os::fd::AsFd,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
};

pub(super) struct Tree {
    cgroup: Cgroup,
    service: Cgroup,
    child: Option<Child>,
    gate: Option<ChildStdin>,
}
impl Tree {
    pub(super) fn new(unit: &str) -> io::Result<Self> {
        kinewright_job_sys::require_nsdelegate()?;
        kinewright_job_sys::install_subreaper()?;
        let cgroup = Cgroup::create(unit)?;
        let service = Cgroup::open(
            cgroup
                .path()
                .parent()
                .ok_or_else(|| io::Error::other("service cgroup"))?,
        )?;
        service.require_deletion_signal()?;
        Ok(Self {
            cgroup,
            service,
            child: None,
            gate: None,
        })
    }
    pub(super) fn service_identity(&self) -> (u64, u64) {
        self.service.identity()
    }
    pub(super) fn identity(&self) -> (String, u64, u64) {
        let (device, inode) = self.cgroup.identity();
        (
            self.cgroup.path().to_string_lossy().into_owned(),
            device,
            inode,
        )
    }
    pub(super) fn contain(&mut self, root: &Path, program: &Program) -> io::Result<ProcessHandle> {
        let program_path = root.join("program.json");
        super::service::atomic_record(&program_path, program, super::protocol::WIRE_CAP)?;
        let mut child = Command::new(std::env::current_exe()?)
            .arg("--shim")
            .arg(self.cgroup.path())
            .arg(&program_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        self.gate = child.stdin.take();
        kinewright_job_sys::check_lifetime_writer(
            self.gate
                .as_ref()
                .ok_or_else(|| io::Error::other("shim gate"))?,
        )?;
        let ready = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("shim readiness"))?;
        self.child = Some(child);
        let result: Result<u32, String> = read_ready(ready)?;
        let process = ProcessHandle::open(result.map_err(io::Error::other)?)?;
        Ok(process)
    }
    pub(super) fn membership(&self, pid: u32) -> io::Result<bool> {
        Ok(self
            .cgroup
            .members()?
            .iter()
            .any(|member| member.pid() == pid))
    }
    pub(super) fn resume(&mut self) -> io::Result<()> {
        let mut gate = self
            .gate
            .take()
            .ok_or_else(|| io::Error::other("missing shim gate"))?;
        gate.write_all(b"C")
    }
    pub(super) fn exited(&mut self) -> io::Result<bool> {
        self.child
            .as_mut()
            .map_or(Ok(false), |child| child.try_wait().map(|s| s.is_some()))
    }
    pub(super) fn kill(&mut self) -> io::Result<()> {
        self.cgroup.kill()
    }
    pub(super) fn empty(&mut self) -> io::Result<bool> {
        Ok(self.cgroup.empty()? && self.service.no_processes_except(Some(std::process::id()))?)
    }
    #[allow(
        clippy::unused_self,
        reason = "the shared Tree API also serves Windows Job accounting"
    )]
    pub(super) fn reaped(&self) -> io::Result<bool> {
        kinewright_job_sys::reap_all()
    }
}

pub(super) fn shim(mut args: impl Iterator<Item = std::ffi::OsString>) -> io::Result<()> {
    let leaf = PathBuf::from(
        args.next()
            .ok_or_else(|| io::Error::other("missing leaf"))?,
    );
    let path = PathBuf::from(
        args.next()
            .ok_or_else(|| io::Error::other("missing program"))?,
    );
    if args.next().is_some() {
        return Err(io::Error::other("unexpected shim argument"));
    }
    let bytes = fs::read(&path)?;
    if bytes.len() > super::protocol::WIRE_CAP {
        return Err(io::Error::other("program cap"));
    }
    let program: Program = serde_json::from_slice(&bytes)?;
    program.validate()?;
    let cgroup = Cgroup::open(&leaf)?;
    cgroup.enter()?;
    drop(cgroup);
    let outcome = (|| -> io::Result<u32> {
        let (read, write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)?;
        kinewright_job_sys::enter_payload_boundary()?;
        let Some(pid) = kinewright_job_sys::fork_payload_init()? else {
            drop(read);
            let result = init(program, std::fs::File::from(write));
            std::process::exit(result.unwrap_or(1));
        };
        drop(write);
        let result: Result<(), String> = read_ready(std::fs::File::from(read))?;
        result.map_err(io::Error::other)?;
        Ok(pid)
    })();
    let boundary = outcome.as_ref().copied().map_err(ToString::to_string);
    write_ready(&mut std::io::stdout(), &boundary)?;
    kinewright_job_sys::close_payload_readiness()?;
    std::process::exit(kinewright_job_sys::wait_payload_init(outcome?)?)
}

fn read_ready<T: serde::de::DeserializeOwned>(mut reader: impl Read + AsFd) -> io::Result<T> {
    let flags = rustix::fs::fcntl_getfl(&reader)?;
    rustix::fs::fcntl_setfl(&reader, flags | rustix::fs::OFlags::NONBLOCK)?;
    let deadline = std::time::Instant::now() + super::protocol::STARTUP;
    let mut bytes = Vec::new();
    loop {
        if std::time::Instant::now() >= deadline {
            return Err(io::Error::other(
                "payload_namespace_unavailable: readiness deadline",
            ));
        }
        let mut buffer = [0; 1024];
        match reader.read(&mut buffer) {
            Ok(0) if bytes.is_empty() => {
                return Err(io::Error::other(
                    "payload_namespace_unavailable: readiness closed without result",
                ));
            }
            Ok(0) => return Ok(serde_json::from_slice(&bytes)?),
            Ok(n) if bytes.len() + n <= super::protocol::RECOVERY_CAP => {
                bytes.extend_from_slice(&buffer[..n]);
            }
            Ok(_) => return Err(io::Error::other("boundary readiness cap")),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

fn write_ready(writer: &mut impl Write, result: &impl serde::Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(result)?;
    if bytes.len() > super::protocol::RECOVERY_CAP {
        return Err(io::Error::other("boundary readiness cap"));
    }
    writer.write_all(&bytes)?;
    writer.flush()
}

fn init(program: Program, mut ready: std::fs::File) -> io::Result<i32> {
    kinewright_job_sys::close_payload_readiness()?;
    let boundary = kinewright_job_sys::mount_payload_proc().map_err(|error| error.to_string());
    write_ready(&mut ready, &boundary)?;
    drop(ready);
    boundary.map_err(io::Error::other)?;
    let mut commit = [0];
    std::io::stdin().read_exact(&mut commit)?;
    if commit != *b"C" {
        return Err(io::Error::other("missing shim COMMIT"));
    }
    let child = Command::new(program.executable)
        .args(program.arguments)
        .env(
            "XDG_RUNTIME_DIR",
            format!("/run/user/{}", rustix::process::getuid().as_raw()),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    kinewright_job_sys::reap_payload_until_exit(child.id())
}

#[cfg(test)]
mod tests {
    #[test]
    fn s4_empty_readiness_eof_has_named_reason() {
        let (reader, writer) = std::os::unix::net::UnixStream::pair().unwrap();
        drop(writer);
        let error = super::read_ready::<Result<u32, String>>(reader).unwrap_err();
        assert_eq!(
            error.to_string(),
            "payload_namespace_unavailable: readiness closed without result"
        );
    }
}
