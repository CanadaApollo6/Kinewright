//! Diagnostic counters outside history state and record identity.
//! Enable history-metrics for kernel and shared-payload serialization counters.
use std::cell::Cell;
#[cfg(feature = "history-metrics")]
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct PhaseCounters {
    pub document_copies: u64,
    pub exact_measurements: u64,
    pub document_bytes_serialised: u64,
    pub bytes_serialised: u64,
    pub kernel_validation_passes: u64,
    pub kernel_duration_passes: u64,
    pub kernel_identity_passes: u64,
}
thread_local! {static HISTORY:Cell<PhaseCounters>=const{Cell::new(PhaseCounters{document_copies:0,exact_measurements:0,document_bytes_serialised:0,bytes_serialised:0,kernel_validation_passes:0,kernel_duration_passes:0,kernel_identity_passes:0})};}
#[cfg(feature = "history-metrics")]
static VALIDATION: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "history-metrics")]
static DURATION: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "history-metrics")]
static IDENTITY: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "history-metrics")]
static PAYLOAD_BYTES: AtomicU64 = AtomicU64::new(0);
impl PhaseCounters {
    #[must_use]
    pub fn read() -> Self {
        #[allow(unused_mut)]
        let mut result = HISTORY.get();
        #[cfg(feature = "history-metrics")]
        {
            result.kernel_validation_passes = VALIDATION.load(Ordering::Relaxed);
            result.kernel_duration_passes = DURATION.load(Ordering::Relaxed);
            result.kernel_identity_passes = IDENTITY.load(Ordering::Relaxed);
            result.bytes_serialised += PAYLOAD_BYTES.load(Ordering::Relaxed);
        }
        result
    }
    #[must_use]
    pub fn since(self, earlier: Self) -> Self {
        Self {
            document_copies: self.document_copies - earlier.document_copies,
            exact_measurements: self.exact_measurements - earlier.exact_measurements,
            document_bytes_serialised: self.document_bytes_serialised
                - earlier.document_bytes_serialised,
            bytes_serialised: self.bytes_serialised - earlier.bytes_serialised,
            kernel_validation_passes: self.kernel_validation_passes
                - earlier.kernel_validation_passes,
            kernel_duration_passes: self.kernel_duration_passes - earlier.kernel_duration_passes,
            kernel_identity_passes: self.kernel_identity_passes - earlier.kernel_identity_passes,
        }
    }
}
pub(super) fn document_copy() {
    let mut n = HISTORY.get();
    n.document_copies += 1;
    HISTORY.set(n);
}
pub(super) fn exact(bytes: u64) {
    let mut n = HISTORY.get();
    n.exact_measurements += 1;
    n.document_bytes_serialised += bytes;
    HISTORY.set(n);
}
#[cfg(feature = "history-metrics")]
pub(crate) fn validation() {
    VALIDATION.fetch_add(1, Ordering::Relaxed);
}
#[cfg(feature = "history-metrics")]
pub(crate) fn duration() {
    DURATION.fetch_add(1, Ordering::Relaxed);
}
#[cfg(feature = "history-metrics")]
pub(crate) fn identity() {
    IDENTITY.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn serialized(bytes: u64) {
    let mut n = HISTORY.get();
    n.bytes_serialised += bytes;
    HISTORY.set(n);
}
#[cfg(feature = "history-metrics")]
pub(crate) fn payload_serialized(bytes: u64) {
    PAYLOAD_BYTES.fetch_add(bytes, Ordering::Relaxed);
}
