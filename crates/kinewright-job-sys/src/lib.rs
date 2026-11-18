//! Safe native containment primitives. Only the guardian owns a lifetime Job.
//! Linux cgroup handles and Windows query-only process handles can be observed
//! independently without extending the Windows Job lifetime.

#[cfg(all(target_os = "linux", test))]
#[path = "../test_support/deadline.rs"]
mod deadline;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod namespace;
#[cfg(target_os = "linux")]
pub use namespace::*;
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
pub mod media_boundary;

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
pub mod media_scope;

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
pub mod media_host;
