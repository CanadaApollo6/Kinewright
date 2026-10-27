use super::protocol::Program;
use kinewright_job_sys::{Job, ProcessHandle};
use std::{io, path::Path};
pub(super) struct Tree {
    job: Job,
}
impl Tree {
    pub(super) fn new(_unit: &str) -> io::Result<Self> {
        Ok(Self {
            job: Job::create()?,
        })
    }
    #[allow(
        clippy::unused_self,
        reason = "the shared Tree API reads the retained cgroup on Linux"
    )]
    pub(super) fn identity(&self) -> (String, u64, u64) {
        ("exclusive-unnamed-job".into(), 0, 0)
    }
    pub(super) fn contain(&mut self, _root: &Path, program: &Program) -> io::Result<ProcessHandle> {
        let pid = self
            .job
            .spawn_suspended(&program.executable, &program.arguments)?;
        ProcessHandle::open(pid)
    }
    pub(super) fn membership(&self, pid: u32) -> io::Result<bool> {
        Ok(self.job.members()?.iter().any(|member| member.pid() == pid))
    }
    pub(super) fn resume(&mut self) -> io::Result<()> {
        self.job.resume()
    }
    pub(super) fn exited(&mut self) -> io::Result<bool> {
        self.job.direct_exited()
    }
    pub(super) fn kill(&mut self) -> io::Result<()> {
        self.job.kill()
    }
    pub(super) fn empty(&mut self) -> io::Result<bool> {
        self.job.empty()
    }
    pub(super) fn reaped(&self) -> io::Result<bool> {
        Ok(self.job.empty()? && self.job.direct_exited()?)
    }
}
