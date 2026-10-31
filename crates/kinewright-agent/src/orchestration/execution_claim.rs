//! Machine-wide Studio execution ownership. A free OS lock never proves cleanup.
//! Private records contain recovery identities only, not project scheduling state.
use super::guardian::CleanupProof;
use crate::job_guardian::protocol::{AttemptIdentity, RECOVERY_CAP, RecoveryIdentity};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RunClaim {
    project: String,
    run: String,
}

/// The serialized host owns this guard until every native attempt is retired.
pub struct MachineClaim {
    lock: File,
    root: PathBuf,
    run: RunClaim,
    attempts: Vec<AttemptIdentity>,
    recovering: bool,
}

impl MachineClaim {
    /// Acquire the one account-wide native namespace, independent of the project.
    ///
    /// # Errors
    /// Refuses unavailable OS-local state or an active/unreconciled Studio run.
    pub fn acquire(project: &str, run: &str) -> io::Result<Self> {
        Self::acquire_at(&machine_root()?, project, run)
    }

    /// Recover the same machine namespace for inspection only.
    ///
    /// # Errors
    /// Refuses a held lock or invalid private recovery state.
    pub fn recover() -> io::Result<Self> {
        Self::recover_at(&machine_root()?)
    }

    /// # Errors
    /// Returns the production recovery refusals in an isolated fixture namespace.
    #[cfg(any(test, feature = "eval-harness"))]
    pub fn recover_fixture(root: &Path) -> io::Result<Self> {
        Self::recover_at(root)
    }

    /// Isolated namespaces are available only to the existing eval harness.
    ///
    /// # Errors
    /// Returns the same admission refusals as production acquisition.
    #[cfg(any(test, feature = "eval-harness"))]
    pub fn acquire_fixture(root: &Path, project: &str, run: &str) -> io::Result<Self> {
        Self::acquire_at(root, project, run)
    }

    /// # Errors
    /// Returns `studio_run_active` for a held lock or unreconciled recovery record.
    fn acquire_at(root: &Path, project: &str, run: &str) -> io::Result<Self> {
        let mut claim = Self::locked(root)?;
        if root.join("run.json").exists() || !claim.attempts.is_empty() {
            return Err(io::Error::other(
                "studio_run_active: cleanup reconciliation required",
            ));
        }
        claim.run = RunClaim {
            project: project.into(),
            run: run.into(),
        };
        write_record(&root.join("run.json"), &claim.run)?;
        claim.recovering = false;
        Ok(claim)
    }

    /// Acquire the original claim for inspection only. No new attempt is admitted.
    ///
    /// # Errors
    /// Returns `studio_run_active` or a malformed private-state refusal.
    fn recover_at(root: &Path) -> io::Result<Self> {
        let mut claim = Self::locked(root)?;
        claim.run = read_record(&root.join("run.json"))?;
        Ok(claim)
    }

    fn locked(root: &Path) -> io::Result<Self> {
        if !root.is_absolute() {
            return Err(io::Error::other("absolute machine state root required"));
        }
        fs::create_dir_all(root)?;
        if fs::symlink_metadata(root)?.file_type().is_symlink() {
            return Err(io::Error::other("machine root symlink"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let metadata = fs::metadata(root)?;
            if metadata.uid() != rustix::process::getuid().as_raw() {
                return Err(io::Error::other("machine root owner"));
            }
            fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits().cast_signed())
                .mode(0o600);
        }
        #[cfg(windows)]
        let lock_name = kinewright_job_sys::execution_namespace()?.1;
        #[cfg(not(windows))]
        let lock_name = "execution.lock";
        let lock = options.open(root.join(lock_name))?;
        lock.try_lock()
            .map_err(|_| io::Error::other("studio_run_active"))?;
        let mut attempts = Vec::new();
        for entry in fs::read_dir(root)? {
            let path = entry?.path();
            if path
                .file_name()
                .and_then(|p| p.to_str())
                .is_some_and(|p| p.starts_with("attempt-"))
                && path
                    .extension()
                    .is_some_and(|extension| extension == "json")
            {
                if attempts.len() == 6 {
                    return Err(io::Error::other("cleanup record capacity"));
                }
                let identity: AttemptIdentity = read_record(&path)?;
                identity.validate()?;
                if path != record_path(root, &identity) {
                    return Err(io::Error::other("recovery filename identity"));
                }
                attempts.push(identity);
            }
        }
        Ok(Self {
            lock,
            root: root.into(),
            run: RunClaim {
                project: String::new(),
                run: String::new(),
            },
            attempts,
            recovering: true,
        })
    }

    /// # Errors
    /// Refuses recovery-time execution, another run, duplicates, or a fourth attempt.
    pub fn reserve(&mut self, identity: &AttemptIdentity) -> io::Result<()> {
        identity.validate()?;
        if self.recovering || identity.project != self.run.project || identity.run != self.run.run {
            return Err(io::Error::other("studio_run_active: claim scope/recovery"));
        }
        if self.attempts.len() >= 3 {
            return Err(io::Error::other("attempt_capacity"));
        }
        if self
            .attempts
            .iter()
            .any(|old| old.attempt == identity.attempt || old.nonce == identity.nonce)
        {
            return Err(io::Error::other("duplicate native reservation"));
        }
        write_record(&record_path(&self.root, identity), identity)?;
        #[cfg(target_os = "linux")]
        self.write_transport(
            identity,
            crate::job_guardian::protocol::TransportState::Unstarted,
        )?;
        self.attempts.push(identity.clone());
        Ok(())
    }

    /// # Errors
    /// Refuses a recovery identity outside the exact reserved attempt.
    pub fn bind(&self, recovery: &RecoveryIdentity) -> io::Result<()> {
        recovery.validate()?;
        if !self.attempts.contains(&recovery.attempt) {
            return Err(io::Error::other("unreserved native identity"));
        }
        let path = self
            .root
            .join(format!("native-{}.json", recovery.attempt.nonce));
        if path.exists() {
            let previous: RecoveryIdentity = read_record(&path)?;
            if previous == *recovery {
                return Ok(());
            }
            return Err(io::Error::other("native reservation identity changed"));
        }
        write_record(&path, recovery)
    }

    #[cfg(target_os = "linux")]
    fn transport_path(&self, identity: &AttemptIdentity) -> PathBuf {
        self.root.join(format!("launch-{}.json", identity.nonce))
    }
    #[cfg(target_os = "linux")]
    fn write_transport(
        &self,
        identity: &AttemptIdentity,
        state: crate::job_guardian::protocol::TransportState,
    ) -> io::Result<()> {
        write_record(
            &self.transport_path(identity),
            &crate::job_guardian::protocol::TransportIntent {
                attempt: identity.clone(),
                state,
            },
        )
    }
    #[cfg(target_os = "linux")]
    pub(crate) fn transport_intent(
        &self,
        identity: &AttemptIdentity,
    ) -> io::Result<crate::job_guardian::protocol::TransportState> {
        if !self.attempts.contains(identity) {
            return Err(io::Error::other("unretained transport"));
        }
        let record: crate::job_guardian::protocol::TransportIntent =
            read_record(&self.transport_path(identity))?;
        if record.attempt != *identity {
            return Err(io::Error::other("foreign transport"));
        }
        Ok(record.state)
    }
    #[cfg(target_os = "linux")]
    pub(crate) fn begin_transport(&mut self, identity: &AttemptIdentity) -> io::Result<PathBuf> {
        if !matches!(
            self.transport_intent(identity)?,
            crate::job_guardian::protocol::TransportState::Unstarted
        ) {
            return Err(io::Error::other("transport already started or fenced"));
        }
        self.write_transport(
            identity,
            crate::job_guardian::protocol::TransportState::Starting,
        )?;
        Ok(self.transport_path(identity))
    }
    #[cfg(target_os = "linux")]
    pub(crate) fn cancel_unspawned_transport(
        &mut self,
        identity: &AttemptIdentity,
    ) -> io::Result<()> {
        self.write_transport(
            identity,
            crate::job_guardian::protocol::TransportState::Unstarted,
        )
    }
    #[cfg(target_os = "linux")]
    pub(crate) fn fence_transport(&mut self, identity: &AttemptIdentity) -> io::Result<()> {
        self.write_transport(
            identity,
            crate::job_guardian::protocol::TransportState::Closed,
        )
    }

    /// Proofs are produced by independent native observation, never pipe EOF.
    ///
    /// # Errors
    /// Refuses a stale/foreign proof or duplicate retirement.
    pub fn retire(&mut self, proof: &CleanupProof) -> io::Result<()> {
        let bound: RecoveryIdentity = read_record(
            &self
                .root
                .join(format!("native-{}.json", proof.identity().nonce)),
        )?;
        if bound != *proof.recovery() {
            return Err(io::Error::other("retirement native identity mismatch"));
        }
        let index = self
            .attempts
            .iter()
            .position(|id| id == proof.identity())
            .ok_or_else(|| io::Error::other("retirement identity/already retired"))?;
        let job_root = self.root.join(format!("job-{}", proof.identity().nonce));
        match fs::symlink_metadata(&job_root) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(io::Error::other("native state directory replaced"));
            }
            Ok(_) => fs::remove_dir_all(job_root)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        fs::remove_file(record_path(&self.root, proof.identity()))?;
        let recovery = self
            .root
            .join(format!("native-{}.json", proof.identity().nonce));
        fs::remove_file(recovery)?;
        #[cfg(target_os = "linux")]
        self.remove_transport(proof.identity())?;
        sync_directory(&self.root)?;
        self.attempts.remove(index);
        Ok(())
    }

    /// # Errors
    /// Only a complete unbound proof and an absent native file authorize retirement.
    #[cfg(target_os = "linux")]
    pub fn retire_unbound(
        &mut self,
        proof: &super::guardian::UnboundCleanupProof,
    ) -> io::Result<()> {
        let identity = proof.identity();
        if !matches!(
            self.transport_intent(identity)?,
            crate::job_guardian::protocol::TransportState::Closed
        ) {
            return Err(io::Error::other("unbound transport not fenced"));
        }
        match self.recovery(identity) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            _ => {
                return Err(io::Error::other(
                    "unbound retirement needs absent native identity",
                ));
            }
        }
        let index = self
            .attempts
            .iter()
            .position(|a| a == identity)
            .ok_or_else(|| io::Error::other("unretained unbound attempt"))?;
        let root = self.root.join(format!("job-{}", identity.nonce));
        match fs::symlink_metadata(&root) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(io::Error::other("native state directory replaced"));
            }
            Ok(_) => fs::remove_dir_all(root)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        fs::remove_file(record_path(&self.root, identity))?;
        self.remove_transport(identity)?;
        sync_directory(&self.root)?;
        self.attempts.remove(index);
        Ok(())
    }
    #[cfg(target_os = "linux")]
    fn remove_transport(&self, identity: &AttemptIdentity) -> io::Result<()> {
        match fs::remove_file(self.transport_path(identity)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
    /// Inspect the exact private record while holding the account-wide lock.
    /// # Errors
    /// Refuses missing, foreign or replaced native identities.
    pub fn recovery(&self, identity: &AttemptIdentity) -> io::Result<RecoveryIdentity> {
        if !self.attempts.contains(identity) {
            return Err(io::Error::other("unretained recovery"));
        }
        let record: RecoveryIdentity =
            read_record(&self.root.join(format!("native-{}.json", identity.nonce)))?;
        record.validate()?;
        if record.attempt != *identity {
            return Err(io::Error::other("foreign recovery"));
        }
        Ok(record)
    }
    /// # Errors
    /// Refuses recovery against a different project or run before H1 is changed.
    #[cfg(target_os = "linux")]
    pub(crate) fn claimed_run(&self) -> (&str, &str) {
        (&self.run.project, &self.run.run)
    }
    /// # Errors
    /// Refuses recovery against a different project or run.
    pub fn check_run(&self, project: &str, run: &str) -> io::Result<()> {
        if self.run.project != project || self.run.run != run {
            return Err(io::Error::other("foreign machine run"));
        }
        Ok(())
    }
    /// Resume the SAME run without releasing its lock between recovery and admission.
    /// # Errors
    /// Refuses any unproved retained attempt or another project's run.
    pub fn resume(mut self, project: &str, run: &str) -> io::Result<Self> {
        if !self.attempts.is_empty() || self.run.project != project || self.run.run != run {
            return Err(io::Error::other("cleanup_blocked: recovery scope"));
        }
        self.recovering = false;
        Ok(self)
    }

    #[must_use]
    pub fn retained_attempts(&self) -> usize {
        self.attempts.len()
    }
    #[must_use]
    pub fn recovery_identities(&self) -> &[AttemptIdentity] {
        &self.attempts
    }
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// # Errors
    /// Refuses to release a run with any outstanding native reservation.
    pub fn finish(self) -> io::Result<()> {
        if !self.attempts.is_empty() {
            return Err(io::Error::other("cleanup_blocked"));
        }
        fs::remove_file(self.root.join("run.json"))?;
        sync_directory(&self.root)?;
        self.lock.unlock()?;
        Ok(())
    }
}
#[allow(
    clippy::unnecessary_wraps,
    reason = "Windows can lack its OS-local state directory"
)]
fn machine_root() -> io::Result<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        Ok(PathBuf::from(format!(
            "/run/user/{}/kinewright-studio-execution",
            rustix::process::getuid().as_raw()
        )))
    }
    #[cfg(windows)]
    {
        kinewright_job_sys::execution_namespace().map(|(root, _)| root)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        Err(io::Error::other("external_supervision_unavailable"))
    }
}
fn record_path(root: &Path, identity: &AttemptIdentity) -> PathBuf {
    root.join(format!("attempt-{}.json", identity.nonce))
}
fn read_record<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<T> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(RECOVERY_CAP as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > RECOVERY_CAP {
        return Err(io::Error::other("recovery record cap"));
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}
#[allow(clippy::unnecessary_wraps, reason = "Unix directory flush can fail")]
fn sync_directory(root: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(root)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = root;
    Ok(())
}
fn write_record(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > RECOVERY_CAP {
        return Err(io::Error::other("recovery record cap"));
    }
    let partial = path.with_extension("partial");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(partial, path)?;
    sync_directory(
        path.parent()
            .ok_or_else(|| io::Error::other("record parent"))?,
    )
}
