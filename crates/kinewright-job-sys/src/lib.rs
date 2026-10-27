//! Safe native containment primitives. Only the guardian owns a lifetime Job.
//! Linux cgroup handles and Windows query-only process handles can be observed
//! independently without extending the Windows Job lifetime.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;
#[cfg(windows)]
pub use windows::*;
