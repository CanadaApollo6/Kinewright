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
    pub executable: PathBuf,
    pub arguments: Vec<String>,
}
impl Program {
    /// # Errors
    /// Refuses relative executables, excessive arguments, or protocol overflow.
    pub fn validate(&self) -> io::Result<()> {
        if !self.executable.is_absolute()
            || self.arguments.len() > 128
            || serde_json::to_vec(self)?.len() > WIRE_CAP / 2
            || self.arguments.iter().any(|s| s.contains('\0'))
        {
            return Err(io::Error::other("invalid native program"));
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
        program: Program,
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
