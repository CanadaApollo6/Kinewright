//! Bundle admission errors shared by the namespace and object owner.
use std::{fmt, io};

#[derive(Debug)]
pub enum BundleError {
    Io(io::Error),
    Identity(String),
    Contention(String),
    DurabilityUnsupported,
    DurabilityUnknown,
    Capacity(&'static str),
    Unsupported(String),
    Corrupt(String),
    Unsettled,
}
impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => e.fmt(f),
            Self::Identity(e) => write!(f, "identity_refused: {e}"),
            Self::Contention(e) => write!(f, "contention: {e}"),
            Self::DurabilityUnsupported => f.write_str("durability_unsupported"),
            Self::DurabilityUnknown => f.write_str("durability_unknown"),
            Self::Capacity(e) => write!(f, "storage_backpressure: {e}"),
            Self::Unsupported(e) => write!(f, "unsupported_bundle: {e}"),
            Self::Corrupt(e) => write!(f, "damaged_bundle: {e}"),
            Self::Unsettled => f.write_str("publication requires reconciliation"),
        }
    }
}
impl std::error::Error for BundleError {}
impl From<io::Error> for BundleError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
#[cfg(target_os = "linux")]
impl From<rustix::io::Errno> for BundleError {
    fn from(e: rustix::io::Errno) -> Self {
        Self::Io(e.into())
    }
}
