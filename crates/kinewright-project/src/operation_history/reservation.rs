use super::Error;
use kinewright_core::undo::RoleBounds;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Ordinary,
    Task,
    Candidate,
}
impl Role {
    const fn index(self) -> usize {
        match self {
            Self::Ordinary => 0,
            Self::Task => 1,
            Self::Candidate => 2,
        }
    }
    #[must_use]
    pub const fn policy(self) -> kinewright_core::undo::RetentionPolicy {
        match self {
            Self::Ordinary => kinewright_core::undo::RetentionPolicy::Fold,
            Self::Task | Self::Candidate => kinewright_core::undo::RetentionPolicy::NeverFold,
        }
    }
    #[must_use]
    pub const fn bounds(self) -> RoleBounds {
        match self {
            Self::Ordinary => RoleBounds::ORDINARY,
            Self::Task => RoleBounds::TASK,
            Self::Candidate => RoleBounds::CANDIDATE,
        }
    }
    #[must_use]
    pub const fn envelope(self) -> Envelope {
        super::sizing::envelope(self.bounds(), 32 << 20)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Envelope {
    pub retained: u64,
    pub workspace: u64,
    pub owners: u64,
}
#[derive(Debug, Clone, Copy)]
pub struct PoolLimits {
    pub retained: [u64; 3],
    pub peak: [u64; 3],
    pub workspace: u64,
}
impl Default for PoolLimits {
    fn default() -> Self {
        Self {
            retained: [312 << 20, 400 << 20, 144 << 20],
            peak: [480 << 20, 568 << 20, 312 << 20],
            workspace: super::sizing::WORKSPACE_BYTES,
        }
    }
}
// Retained 312 + 400 + 144 MiB, plus one shared 168 MiB workspace: 1024 MiB.
#[derive(Debug, Default)]
struct Usage {
    retained: [u64; 3],
    workspace: [u64; 3],
    leases: [usize; 3],
    peak_leases: [usize; 3],
    cores: [std::collections::BTreeMap<u64, usize>; 3],
}
#[derive(Debug)]
struct Pool {
    limits: PoolLimits,
    usage: Mutex<Usage>,
}
#[derive(Debug, Clone)]
pub struct HistoryPool {
    // Independently dropped histories and captures share this single ledger;
    // the last reservation keeps it alive after the factory is dropped.
    shared: Arc<Pool>,
}
impl HistoryPool {
    #[must_use]
    pub fn new(limits: PoolLimits) -> Self {
        Self {
            shared: Arc::new(Pool {
                limits,
                usage: Mutex::new(Usage::default()),
            }),
        }
    }
    #[cfg(test)]
    pub(super) fn admit(&self, role: Role) -> Result<HistoryReservation, Error> {
        self.admit_sized(super::CreationSizing {
            kind: match role {
                Role::Ordinary => super::CreationKind::Ordinary,
                Role::Task => super::CreationKind::Task,
                Role::Candidate => super::CreationKind::LiveCandidate,
            },
            baseline_live: super::sizing::PRODUCT_DOCUMENT_BYTES,
            baseline_owners: if role == Role::Ordinary {
                32 << 20
            } else {
                29 << 20
            },
            initial_batch: kinewright_core::undo::Charge::default(),
        })
    }
    pub fn admit_sized(&self, sizing: super::CreationSizing) -> Result<HistoryReservation, Error> {
        self.reserve_limits(sizing.kind.role(), sizing.limits()?, sizing)
    }
    fn reserve_limits(
        &self,
        role: Role,
        limits: super::FixedLimits,
        sizing: super::CreationSizing,
    ) -> Result<HistoryReservation, Error> {
        let bytes = limits.envelope.retained;
        let mut usage = self.shared.usage.lock().map_err(|_| Error::Reservation)?;
        let i = role.index();
        let next = usage.retained[i]
            .checked_add(bytes)
            .ok_or(Error::Reservation)?;
        if next > self.shared.limits.retained[i]
            || next
                .checked_add(usage.workspace[i])
                .is_none_or(|n| n > self.shared.limits.peak[i])
        {
            return Err(Error::PoolFull(role));
        }
        usage.retained[i] = next;
        usage.leases[i] += 1;
        usage.peak_leases[i] = usage.peak_leases[i].max(usage.leases[i]);
        Ok(HistoryReservation {
            shared: Arc::clone(&self.shared),
            role,
            limits,
            sizing,
            core: false,
        })
    }
    /// Raw ingress is held in the shared scratch window before its descriptor is decoded.
    pub fn workspace(&self, role: Role) -> Result<Workspace, Error> {
        workspace(&self.shared, role, super::sizing::WORKSPACE_BYTES)
    }
    pub fn lease_counts(&self, role: Role) -> Result<(usize, usize), Error> {
        let usage = self.shared.usage.lock().map_err(|_| Error::Reservation)?;
        Ok((usage.leases[role.index()], usage.peak_leases[role.index()]))
    }
    pub fn usage(&self, role: Role) -> Result<(u64, u64), Error> {
        let usage = self.shared.usage.lock().map_err(|_| Error::Reservation)?;
        let i = role.index();
        Ok((usage.retained[i], usage.workspace[i]))
    }
}
#[derive(Debug)]
pub struct HistoryReservation {
    shared: Arc<Pool>,
    role: Role,
    limits: super::FixedLimits,
    sizing: super::CreationSizing,
    core: bool,
}
impl HistoryReservation {
    #[must_use]
    pub const fn role(&self) -> Role {
        self.role
    }
    #[must_use]
    pub const fn limits(&self) -> super::FixedLimits {
        self.limits
    }
    #[must_use]
    pub const fn sizing(&self) -> super::CreationSizing {
        self.sizing
    }
    /// Task admission leaves one full largest-task capture free, so a full pool
    /// can always checkpoint and release a task. Captures themselves use that room.
    pub(super) fn into_core(mut self) -> Result<Self, Error> {
        self.promote_replacing(None)?;
        Ok(self)
    }
    pub(super) fn promote_replacing(&mut self, old: Option<&Self>) -> Result<(), Error> {
        if self.core {
            return Ok(());
        }
        if self.needs_capture_headroom() {
            let mut usage = self.shared.usage.lock().map_err(|_| Error::Reservation)?;
            let retired =
                old.filter(|v| v.core && v.role == self.role && v.needs_capture_headroom());
            let largest = usage.cores[self.role.index()]
                .iter()
                .rev()
                .find_map(|(bytes, count)| {
                    let removed =
                        usize::from(retired.is_some_and(|v| v.limits.envelope.retained == *bytes));
                    (*count > removed).then_some(*bytes)
                })
                .unwrap_or(0)
                .max(self.limits.envelope.retained);
            let retained = usage.retained[self.role.index()]
                .checked_sub(retired.map_or(0, |v| v.limits.envelope.retained))
                .ok_or(Error::Reservation)?;
            if retained
                .checked_add(largest)
                .is_none_or(|n| n > self.shared.limits.retained[self.role.index()])
            {
                return Err(Error::PoolFull(self.role));
            }
            *usage.cores[self.role.index()]
                .entry(self.limits.envelope.retained)
                .or_default() += 1;
        }
        self.core = true;
        Ok(())
    }
    fn needs_capture_headroom(&self) -> bool {
        self.role != Role::Candidate
            || matches!(self.sizing.kind, super::CreationKind::LiveCandidate)
    }
    pub(super) fn pool(&self) -> HistoryPool {
        HistoryPool {
            shared: Arc::clone(&self.shared),
        }
    }
    pub fn capture(&self) -> Result<Self, Error> {
        HistoryPool {
            shared: Arc::clone(&self.shared),
        }
        .reserve_limits(self.role, self.limits, self.sizing)
    }
    #[must_use]
    pub fn shares_pool(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }
    pub fn workspace(&self) -> Result<Workspace, Error> {
        workspace(&self.shared, self.role, self.limits.envelope.workspace)
    }
}
fn workspace(shared: &Arc<Pool>, role: Role, bytes: u64) -> Result<Workspace, Error> {
    let i = role.index();
    let mut usage = shared.usage.lock().map_err(|_| Error::Reservation)?;
    let next = usage.workspace[i]
        .checked_add(bytes)
        .ok_or(Error::Reservation)?;
    let total = usage
        .workspace
        .iter()
        .try_fold(bytes, |n, v| n.checked_add(*v))
        .ok_or(Error::Reservation)?;
    if total > shared.limits.workspace
        || next
            .checked_add(usage.retained[i])
            .is_none_or(|n| n > shared.limits.peak[i])
    {
        return Err(Error::PoolFull(role));
    }
    usage.workspace[i] = next;
    Ok(Workspace {
        shared: Arc::clone(shared),
        role,
        bytes,
    })
}

impl Drop for HistoryReservation {
    fn drop(&mut self) {
        let mut usage = self
            .shared
            .usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        usage.retained[self.role.index()] -= self.limits.envelope.retained;
        usage.leases[self.role.index()] -= 1;
        if self.core && self.needs_capture_headroom() {
            let bytes = self.limits.envelope.retained;
            let count = usage.cores[self.role.index()]
                .get_mut(&bytes)
                .expect("live creation registered at admission");
            *count -= 1;
            if *count == 0 {
                usage.cores[self.role.index()].remove(&bytes);
            }
        }
    }
}
#[derive(Debug)]
pub struct Workspace {
    shared: Arc<Pool>,
    role: Role,
    bytes: u64,
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let mut usage = self
            .shared
            .usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        usage.workspace[self.role.index()] -= self.bytes;
    }
}
