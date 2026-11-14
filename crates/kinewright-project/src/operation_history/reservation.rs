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
    pub const fn bounds(self) -> RoleBounds {
        match self {
            Self::Ordinary => RoleBounds::ORDINARY,
            Self::Task => RoleBounds::TASK,
            Self::Candidate => RoleBounds::CANDIDATE,
        }
    }
    #[must_use]
    pub const fn envelope(self) -> Envelope {
        let b = self.bounds();
        let owners = match self {
            Self::Ordinary => 5 << 20,
            Self::Task => 4 << 20,
            Self::Candidate => 512 << 10,
        };
        Envelope {
            retained: (2 + b.checkpoint_documents as u64) * b.document_live_bytes
                + b.decoded_history_bytes
                + owners,
            workspace: b.replay_workspace,
            owners,
        }
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
}
impl Default for PoolLimits {
    fn default() -> Self {
        Self {
            retained: [128 << 20, 328 << 20, 16 << 20],
            peak: [160 << 20, 392 << 20, 80 << 20],
        }
    }
}
#[derive(Debug, Default)]
struct Usage {
    retained: [u64; 3],
    workspace: [u64; 3],
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
    pub fn admit(&self, role: Role) -> Result<HistoryReservation, Error> {
        self.reserve(role, role.envelope().retained)
    }
    pub fn reserve(&self, role: Role, bytes: u64) -> Result<HistoryReservation, Error> {
        if bytes > role.envelope().retained {
            return Err(Error::Reservation);
        }
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
            return Err(Error::Reservation);
        }
        usage.retained[i] = next;
        Ok(HistoryReservation {
            shared: Arc::clone(&self.shared),
            role,
            bytes,
        })
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
    bytes: u64,
}
impl HistoryReservation {
    #[must_use]
    pub const fn role(&self) -> Role {
        self.role
    }
    pub fn grow_to(&mut self, bytes: u64) -> Result<(), Error> {
        if bytes > self.role.envelope().retained {
            return Err(Error::Reservation);
        }
        let mut usage = self.shared.usage.lock().map_err(|_| Error::Reservation)?;
        let i = self.role.index();
        let next = usage.retained[i]
            .checked_sub(self.bytes)
            .and_then(|n| n.checked_add(bytes))
            .ok_or(Error::Reservation)?;
        if next > self.shared.limits.retained[i]
            || next
                .checked_add(usage.workspace[i])
                .is_none_or(|n| n > self.shared.limits.peak[i])
        {
            return Err(Error::Reservation);
        }
        usage.retained[i] = next;
        self.bytes = bytes;
        Ok(())
    }
    pub fn workspace(&self) -> Result<Workspace, Error> {
        let bytes = self.role.envelope().workspace;
        let i = self.role.index();
        let mut usage = self.shared.usage.lock().map_err(|_| Error::Reservation)?;
        let next = usage.workspace[i]
            .checked_add(bytes)
            .ok_or(Error::Reservation)?;
        if next
            .checked_add(usage.retained[i])
            .is_none_or(|n| n > self.shared.limits.peak[i])
        {
            return Err(Error::Reservation);
        }
        usage.workspace[i] = next;
        Ok(Workspace {
            shared: Arc::clone(&self.shared),
            role: self.role,
            bytes,
        })
    }
}
impl Drop for HistoryReservation {
    fn drop(&mut self) {
        let mut usage = self
            .shared
            .usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        usage.retained[self.role.index()] -= self.bytes;
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
