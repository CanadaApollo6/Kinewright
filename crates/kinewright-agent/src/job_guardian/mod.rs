//! Kinewright's shared native guardian protocol, following AW2 R28/R29.
//! This code is native containment work, not a translation from T3 Code.
//! The executable includes this module directly and links no media or UI code.

#[cfg(target_os = "linux")]
mod linux;
pub mod protocol;
mod service;
#[cfg(windows)]
mod windows;

pub use service::main_entry;
