//! Provisional storage bounds. Reservations cover simultaneous physical copies.
#![allow(clippy::missing_errors_doc)]
use crate::bundle::BundleError;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct StorageLimits {
    pub control_bytes: u64,
    pub pending_commands: u64,
    pub command_bytes: u64,
    pub branch_history_bytes: u64,
    pub live_history_bytes: u64,
    pub versions: u64,
    pub objects: u64,
    pub graph_depth: u64,
    pub decoded_record_bytes: u64,
    pub metadata_bytes: u64,
    pub comments: u64,
    pub comment_text_bytes: u64,
    pub comment_points: u64,
    pub voice_bytes: u64,
    pub receipts: u64,
    pub issuers: u64,
    pub owned_bytes: u64,
    pub copy_overlap_bytes: u64,
}
impl Default for StorageLimits {
    fn default() -> Self {
        Self {
            control_bytes: 64 << 20,
            pending_commands: 64,
            command_bytes: 256 << 10,
            branch_history_bytes: 32 << 20,
            live_history_bytes: 128 << 20,
            versions: 100,
            objects: 10_000,
            graph_depth: 32,
            decoded_record_bytes: 64 << 20,
            metadata_bytes: 512 << 20,
            comments: 2_000,
            comment_text_bytes: 16 << 10,
            comment_points: 512,
            voice_bytes: 256 << 20,
            receipts: 4_096,
            issuers: 128,
            owned_bytes: 20 << 30,
            copy_overlap_bytes: 20 << 30,
        }
    }
}

impl StorageLimits {
    pub(crate) fn validate(&self) -> Result<(), BundleError> {
        let ceiling = Self::default();
        if self.control_bytes > ceiling.control_bytes
            || self.pending_commands > ceiling.pending_commands
            || self.command_bytes > ceiling.command_bytes
            || self.branch_history_bytes > ceiling.branch_history_bytes
            || self.live_history_bytes > ceiling.live_history_bytes
            || self.versions > ceiling.versions
            || self.objects > ceiling.objects
            || self.graph_depth > ceiling.graph_depth
            || self.decoded_record_bytes > ceiling.decoded_record_bytes
            || self.metadata_bytes > ceiling.metadata_bytes
            || self.comments > ceiling.comments
            || self.comment_text_bytes > ceiling.comment_text_bytes
            || self.comment_points > ceiling.comment_points
            || self.voice_bytes > ceiling.voice_bytes
            || self.receipts > ceiling.receipts
            || self.issuers > ceiling.issuers
            || self.owned_bytes > ceiling.owned_bytes
            || self.copy_overlap_bytes > ceiling.copy_overlap_bytes
        {
            return Err(BundleError::Unsupported(
                "provisional storage limit expansion is not admitted".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StorageUsage {
    pub physical_bytes: u64,
    pub owned_bytes: u64,
    pub metadata_bytes: u64,
    pub copy_bytes: u64,
    pub reserved_bytes: u64,
    pub reserved_copy_bytes: u64,
    pub decoded_bytes: u64,
    pub live_history_bytes: u64,
    pub objects: u64,
    pub edges: u64,
    pub verified_calls: u64,
    pub verified_bytes: u64,
    pub peak_physical_bytes: u64,
    pub peak_reserved_bytes: u64,
    pub peak_decoded_bytes: u64,
    pub damaged_accounting: bool,
    pub unsettled: bool,
}

pub struct StorageReservation<T = ()> {
    value: Option<T>,
    usage: Arc<Mutex<StorageUsage>>,
    bytes: u64,
    decoded: u64,
    history: u64,
    copy: bool,
    active: bool,
}
impl StorageReservation {
    pub(crate) fn reserve(
        usage: &Arc<Mutex<StorageUsage>>,
        limits: &StorageLimits,
        bytes: u64,
        decoded: u64,
        history: u64,
        copy: bool,
        free: u64,
    ) -> Result<Self, BundleError> {
        let mut state = usage
            .lock()
            .map_err(|_| BundleError::Capacity("accounting poisoned"))?;
        if state.unsettled {
            return Err(BundleError::Unsettled);
        }
        if state.damaged_accounting {
            return Err(BundleError::Capacity("accounting damaged"));
        }
        let physical = state
            .physical_bytes
            .checked_add(state.reserved_bytes)
            .and_then(|v| v.checked_add(bytes))
            .ok_or(BundleError::Capacity("physical overflow"))?;
        let allowance = limits
            .owned_bytes
            .checked_add(limits.metadata_bytes)
            .and_then(|n| n.checked_add(limits.copy_overlap_bytes))
            .and_then(|n| n.checked_add(limits.metadata_bytes))
            .ok_or(BundleError::Capacity("overlap overflow"))?;
        let reserved = state
            .reserved_bytes
            .checked_add(bytes)
            .ok_or(BundleError::Capacity("reservation overflow"))?;
        let decoded_total = state
            .decoded_bytes
            .checked_add(decoded)
            .ok_or(BundleError::Capacity("decoded overflow"))?;
        let history_total = state
            .live_history_bytes
            .checked_add(history)
            .ok_or(BundleError::Capacity("history overflow"))?;
        if reserved > free
            || physical > allowance
            || decoded > limits.decoded_record_bytes
            || state
                .metadata_bytes
                .checked_add(decoded_total)
                .is_none_or(|n| n > limits.metadata_bytes)
            || history_total > limits.live_history_bytes
        {
            return Err(BundleError::Capacity("simultaneous bytes or disk reserve"));
        }
        if !copy
            && state
                .physical_bytes
                .checked_sub(state.copy_bytes)
                .and_then(|v| v.checked_add(reserved))
                .is_none_or(|v| {
                    v > limits
                        .owned_bytes
                        .checked_add(limits.metadata_bytes)
                        .unwrap_or(0)
                })
        {
            return Err(BundleError::Capacity("owned media allowance"));
        }
        let reserved_copies = state
            .reserved_copy_bytes
            .checked_add(if copy { bytes } else { 0 })
            .ok_or(BundleError::Capacity("copy reserve overflow"))?;
        if state
            .copy_bytes
            .checked_add(reserved_copies)
            .is_none_or(|v| {
                v > limits
                    .copy_overlap_bytes
                    .checked_add(limits.metadata_bytes)
                    .unwrap_or(0)
            })
        {
            return Err(BundleError::Capacity("copy overlap"));
        }
        state.reserved_bytes = reserved;
        state.reserved_copy_bytes = reserved_copies;
        state.decoded_bytes = decoded_total;
        state.live_history_bytes = history_total;
        state.peak_reserved_bytes = state.peak_reserved_bytes.max(reserved);
        state.peak_decoded_bytes = state.peak_decoded_bytes.max(decoded_total);
        Ok(Self {
            value: Some(()),
            usage: Arc::clone(usage),
            bytes,
            decoded,
            history,
            copy,
            active: true,
        })
    }

    /// An uncertain on-disk stage stays charged after authority teardown.
    pub(crate) fn retain_charge(mut self) {
        self.active = false;
    }
    pub(crate) fn hold<T>(mut self, value: T) -> StorageReservation<T> {
        let held = StorageReservation {
            value: Some(value),
            usage: Arc::clone(&self.usage),
            bytes: self.bytes,
            decoded: self.decoded,
            history: self.history,
            copy: self.copy,
            active: self.active,
        };
        self.active = false;
        held
    }
    /// Convert exactly one staging charge to an immutable physical charge.
    pub(crate) fn seal(mut self, measured: u64) -> Result<(), BundleError> {
        if self.decoded != 0 || self.history != 0 {
            return Err(BundleError::Capacity(
                "a live decode reservation cannot be sealed as physical storage",
            ));
        }
        if measured > self.bytes {
            return Err(BundleError::Capacity("stream exceeded reservation"));
        }
        let mut state = self
            .usage
            .lock()
            .map_err(|_| BundleError::Capacity("accounting poisoned"))?;
        let physical = state
            .physical_bytes
            .checked_add(measured)
            .ok_or(BundleError::Capacity("seal overflow"))?;
        let reserved = state
            .reserved_bytes
            .checked_sub(self.bytes)
            .ok_or(BundleError::Capacity("reservation underflow"))?;
        let copies = state
            .copy_bytes
            .checked_add(if self.copy { measured } else { 0 })
            .ok_or(BundleError::Capacity("copy seal overflow"))?;
        let reserved_copies = state
            .reserved_copy_bytes
            .checked_sub(if self.copy { self.bytes } else { 0 })
            .ok_or(BundleError::Capacity("copy reservation underflow"))?;
        let decoded = state
            .decoded_bytes
            .checked_sub(self.decoded)
            .ok_or(BundleError::Capacity("decode underflow"))?;
        let history = state
            .live_history_bytes
            .checked_sub(self.history)
            .ok_or(BundleError::Capacity("history underflow"))?;
        state.physical_bytes = physical;
        state.reserved_bytes = reserved;
        state.copy_bytes = copies;
        state.reserved_copy_bytes = reserved_copies;
        state.decoded_bytes = decoded;
        state.live_history_bytes = history;
        state.peak_physical_bytes = state.peak_physical_bytes.max(physical);
        self.active = false;
        Ok(())
    }
}
impl<T> std::ops::Deref for StorageReservation<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.value
            .as_ref()
            .expect("reservation owns its live value")
    }
}
impl<T> Drop for StorageReservation<T> {
    fn drop(&mut self) {
        drop(self.value.take());
        if !self.active {
            return;
        }
        if let Ok(mut state) = self.usage.lock() {
            if let (Some(bytes), Some(decoded), Some(history), Some(copies)) = (
                state.reserved_bytes.checked_sub(self.bytes),
                state.decoded_bytes.checked_sub(self.decoded),
                state.live_history_bytes.checked_sub(self.history),
                state
                    .reserved_copy_bytes
                    .checked_sub(if self.copy { self.bytes } else { 0 }),
            ) {
                state.reserved_bytes = bytes;
                state.decoded_bytes = decoded;
                state.live_history_bytes = history;
                state.reserved_copy_bytes = copies;
            } else {
                state.damaged_accounting = true;
            }
        }
    }
}
