//! Safe native containment primitives. Only the guardian owns a lifetime Job.
//! Linux cgroup handles and Windows query-only process handles can be observed
//! independently without extending the Windows Job lifetime.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod namespace;
#[cfg(target_os = "linux")]
pub use namespace::*;
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod sandbox;
#[cfg(target_os = "linux")]
pub use sandbox::*;
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(target_os = "linux")]
mod credential;
#[cfg(target_os = "linux")]
pub use credential::*;
