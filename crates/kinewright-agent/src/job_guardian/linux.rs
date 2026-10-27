use super::protocol::Program;
use kinewright_job_sys::{Cgroup, ProcessHandle};
use std::{
    fs,
    io::{self, Read, Write},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
};

pub(super) struct Tree {
    cgroup: Cgroup,
    child: Option<Child>,
    gate: Option<ChildStdin>,
}
impl Tree {
    pub(super) fn new(unit: &str) -> io::Result<Self> {
        kinewright_job_sys::install_subreaper()?;
        Ok(Self {
            cgroup: Cgroup::create(unit)?,
            child: None,
            gate: None,
        })
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
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        self.gate = child.stdin.take();
        kinewright_job_sys::check_lifetime_writer(
            self.gate
                .as_ref()
                .ok_or_else(|| io::Error::other("shim gate"))?,
        )?;
        let process = ProcessHandle::open(child.id())?;
        self.child = Some(child);
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
        self.cgroup.empty()
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
    let bytes = fs::read(path)?;
    if bytes.len() > super::protocol::WIRE_CAP {
        return Err(io::Error::other("program cap"));
    }
    let program: Program = serde_json::from_slice(&bytes)?;
    program.validate()?;
    let cgroup = Cgroup::open(&leaf)?;
    cgroup.enter()?;
    let mut commit = [0];
    std::io::stdin().read_exact(&mut commit)?;
    if commit != *b"C" {
        return Err(io::Error::other("missing shim COMMIT"));
    }
    drop(cgroup);
    Err(Command::new(program.executable)
        .args(program.arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .exec())
}
