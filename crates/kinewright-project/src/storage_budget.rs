//! Provisional storage bounds. Reservations cover simultaneous physical copies.
#![allow(clippy::missing_errors_doc)]
use crate::bundle::BundleError;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub decoder_bytes: u64,
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
            decoder_bytes: 4 << 30,
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
            || self.decoder_bytes > ceiling.decoder_bytes
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

/// Shared physical metadata accounting for reader admission and store adoption.
pub(crate) fn object_metadata_charge(
    reference: &crate::bundle_records::ObjectRef,
    edges: usize,
) -> Result<u64, BundleError> {
    256_u64
        .checked_add(
            (edges as u64)
                .checked_mul(160)
                .ok_or(BundleError::Capacity("edge overflow"))?,
        )
        .and_then(|n| {
            n.checked_add(if reference.record_kind.is_owned_bytes() {
                0
            } else {
                reference.byte_length
            })
        })
        .ok_or(BundleError::Capacity("metadata overflow"))
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
    /// Subset of `decoded_bytes` retained by cursors, format probing, and codecs.
    pub decoder_bytes: u64,
    pub live_history_bytes: u64,
    pub objects: u64,
    pub edges: u64,
    pub unclassified_bytes: u64,
    pub verified_calls: u64,
    pub verified_bytes: u64,
    pub peak_physical_bytes: u64,
    pub peak_reserved_bytes: u64,
    pub peak_decoded_bytes: u64,
    pub peak_decoder_bytes: u64,
    pub peak_live_history_bytes: u64,
    pub damaged_accounting: bool,
    pub unsettled: bool,
}

#[derive(Clone, Copy)]
enum ReservationKind {
    Record,
    TransientDecoder,
}

pub struct StorageReservation<T = ()> {
    value: Option<T>,
    usage: Arc<Mutex<StorageUsage>>,
    bytes: u64,
    decoded: u64,
    decoder: u64,
    history: u64,
    copy: bool,
    active: bool,
}
impl StorageReservation {
    /// Reserve only additional snapshot overlap on this reservation's existing ledger.
    /// The extension remains independently active until admission commits it below.
    pub(crate) fn reserve_copy_growth(
        &self,
        target_bytes: u64,
        limits: &StorageLimits,
        free: u64,
    ) -> Result<Self, BundleError> {
        if !self.active || !self.copy || self.decoded != 0 || self.history != 0 {
            return Err(BundleError::Capacity(
                "snapshot overlap reservation is not active",
            ));
        }
        Self::reserve_for_retained_read(
            &self.usage,
            limits,
            target_bytes.saturating_sub(self.bytes),
            0,
            0,
            true,
            free,
            true,
        )
    }

    /// Combine two active charges without uncharging either one or changing the ledger.
    pub(crate) fn absorb_copy_growth(&mut self, mut extension: Self) -> Result<(), BundleError> {
        if !self.active
            || !extension.active
            || !self.copy
            || !extension.copy
            || self.decoded != 0
            || extension.decoded != 0
            || self.history != 0
            || extension.history != 0
            || !Arc::ptr_eq(&self.usage, &extension.usage)
        {
            return Err(BundleError::Capacity(
                "snapshot overlap extension owner differs",
            ));
        }
        let bytes = self
            .bytes
            .checked_add(extension.bytes)
            .ok_or(BundleError::Capacity("snapshot overlap growth overflow"))?;
        self.bytes = bytes;
        extension.active = false;
        Ok(())
    }

    pub(crate) fn verify_copy_charge(&self, bytes: u64) -> Result<(), BundleError> {
        let usage = self
            .usage
            .lock()
            .map_err(|_| BundleError::Capacity("accounting poisoned"))?;
        if !self.active
            || !self.copy
            || self.bytes < bytes
            || usage.damaged_accounting
            || usage.reserved_copy_bytes < self.bytes
            || usage.reserved_bytes < self.bytes
        {
            return Err(BundleError::Capacity(
                "snapshot overlap charge is not retained",
            ));
        }
        Ok(())
    }

    pub(crate) fn reserve(
        usage: &Arc<Mutex<StorageUsage>>,
        limits: &StorageLimits,
        bytes: u64,
        decoded: u64,
        history: u64,
        copy: bool,
        free: u64,
    ) -> Result<Self, BundleError> {
        Self::reserve_for_retained_read(usage, limits, bytes, decoded, history, copy, free, false)
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "same physical ledger and limits; only read admission may tolerate cleanup uncertainty"
    )]
    pub(crate) fn reserve_for_retained_read(
        usage: &Arc<Mutex<StorageUsage>>,
        limits: &StorageLimits,
        bytes: u64,
        decoded: u64,
        history: u64,
        copy: bool,
        free: u64,
        cleanup_only: bool,
    ) -> Result<Self, BundleError> {
        Self::reserve_kind(
            usage,
            limits,
            bytes,
            decoded,
            history,
            copy,
            free,
            cleanup_only,
            ReservationKind::Record,
        )
    }
    pub(crate) fn reserve_decoder(
        usage: &Arc<Mutex<StorageUsage>>,
        limits: &StorageLimits,
        decoded: u64,
        free: u64,
    ) -> Result<Self, BundleError> {
        Self::reserve_kind(
            usage,
            limits,
            0,
            decoded,
            0,
            false,
            free,
            true,
            ReservationKind::TransientDecoder,
        )
    }
    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "one ledger applies unchanged aggregate limits to typed record and decoder reservations"
    )]
    fn reserve_kind(
        usage: &Arc<Mutex<StorageUsage>>,
        limits: &StorageLimits,
        bytes: u64,
        decoded: u64,
        history: u64,
        copy: bool,
        free: u64,
        cleanup_only: bool,
        kind: ReservationKind,
    ) -> Result<Self, BundleError> {
        let mut state = usage
            .lock()
            .map_err(|_| BundleError::Capacity("accounting poisoned"))?;
        if state.unsettled && !cleanup_only {
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
        let codec_charge = if matches!(kind, ReservationKind::TransientDecoder) {
            decoded
        } else {
            0
        };
        let codec_total = state
            .decoder_bytes
            .checked_add(codec_charge)
            .ok_or(BundleError::Capacity("decoder overflow"))?;
        let metadata_decoded =
            decoded_total
                .checked_sub(codec_total)
                .ok_or(BundleError::Capacity(
                    "decoder subset exceeds decoded bytes",
                ))?;
        let history_total = state
            .live_history_bytes
            .checked_add(history)
            .ok_or(BundleError::Capacity("history overflow"))?;
        if matches!(kind, ReservationKind::Record) && reserved > free {
            return Err(BundleError::Capacity("disk reserve"));
        }
        if physical > allowance {
            return Err(BundleError::Capacity("physical overlap bytes"));
        }
        if matches!(kind, ReservationKind::Record) && decoded > limits.decoded_record_bytes {
            return Err(BundleError::Capacity("decoded record bytes"));
        }
        if state
            .metadata_bytes
            .checked_add(metadata_decoded)
            .is_none_or(|n| n > limits.metadata_bytes)
        {
            return Err(BundleError::Capacity("metadata and decoded bytes"));
        }
        if codec_total > limits.decoder_bytes {
            return Err(BundleError::Capacity("decoder bytes"));
        }
        if history_total > limits.live_history_bytes {
            return Err(BundleError::Capacity("live history bytes"));
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
        state.decoder_bytes = codec_total;
        state.live_history_bytes = history_total;
        state.peak_live_history_bytes = state.peak_live_history_bytes.max(history_total);
        state.peak_reserved_bytes = state.peak_reserved_bytes.max(reserved);
        state.peak_decoded_bytes = state.peak_decoded_bytes.max(decoded_total);
        state.peak_decoder_bytes = state.peak_decoder_bytes.max(codec_total);
        Ok(Self {
            value: Some(()),
            usage: Arc::clone(usage),
            bytes,
            decoded,
            decoder: codec_charge,
            history,
            copy,
            active: true,
        })
    }

    #[cfg(any(test, feature = "test-util"))]
    pub(crate) const fn reserved_bytes_probe(&self) -> u64 {
        self.bytes
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
            decoder: self.decoder,
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
impl<T> StorageReservation<T> {
    pub(crate) fn map<U>(mut self, transform: impl FnOnce(T) -> U) -> StorageReservation<U> {
        let value = transform(self.value.take().expect("reservation owns value"));
        let mapped = StorageReservation {
            value: Some(value),
            usage: Arc::clone(&self.usage),
            bytes: self.bytes,
            decoded: self.decoded,
            decoder: self.decoder,
            history: self.history,
            copy: self.copy,
            active: self.active,
        };
        self.active = false;
        mapped
    }

    pub(crate) const fn decoded_charge(&self) -> u64 {
        self.decoded
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
            if let (Some(bytes), Some(decoded), Some(decoder), Some(history), Some(copies)) = (
                state.reserved_bytes.checked_sub(self.bytes),
                state.decoded_bytes.checked_sub(self.decoded),
                state.decoder_bytes.checked_sub(self.decoder),
                state.live_history_bytes.checked_sub(self.history),
                state
                    .reserved_copy_bytes
                    .checked_sub(if self.copy { self.bytes } else { 0 }),
            ) {
                state.reserved_bytes = bytes;
                state.decoded_bytes = decoded;
                state.decoder_bytes = decoder;
                state.live_history_bytes = history;
                state.reserved_copy_bytes = copies;
            } else {
                state.damaged_accounting = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s3_decoder_budget_exact_limit_one_beyond_is_atomic_and_separate() {
        let limits = StorageLimits::default();
        assert_eq!(limits.metadata_bytes, 512 << 20);
        assert_eq!(limits.decoded_record_bytes, 64 << 20);
        let usage = Arc::new(Mutex::new(StorageUsage {
            metadata_bytes: limits.metadata_bytes,
            ..StorageUsage::default()
        }));
        let exact = StorageReservation::reserve_decoder(&usage, &limits, limits.decoder_bytes, 0)
            .expect("C0_DECODER_EXACT_LIMIT_ADMITTED");
        let before = usage.lock().unwrap().clone();
        assert_eq!(before.decoded_bytes, limits.decoder_bytes);
        assert_eq!(before.decoder_bytes, limits.decoder_bytes);
        assert_eq!(before.reserved_bytes, 0, "C0_DECODER_IS_NOT_DISK_BYTES");
        assert!(
            matches!(
                StorageReservation::reserve_decoder(&usage, &limits, 1, 0),
                Err(BundleError::Capacity("decoder bytes"))
            ),
            "C0_DECODER_ONE_BEYOND_REFUSED"
        );
        assert_eq!(*usage.lock().unwrap(), before, "C0_DECODER_REFUSAL_ATOMIC");
        assert!(
            matches!(
                StorageReservation::reserve(&usage, &limits, 0, 1, 0, false, u64::MAX),
                Err(BundleError::Capacity("metadata and decoded bytes"))
            ),
            "C0_METADATA_LIMIT_PRESERVED"
        );
        assert!(
            matches!(
                StorageReservation::reserve(
                    &usage,
                    &limits,
                    0,
                    limits.decoded_record_bytes + 1,
                    0,
                    false,
                    u64::MAX
                ),
                Err(BundleError::Capacity("decoded record bytes"))
            ),
            "C0_RECORD_LIMIT_PRESERVED"
        );
        drop(exact);
        let after = usage.lock().unwrap();
        assert_eq!(after.decoder_bytes, 0);
        assert_eq!(after.decoded_bytes, 0);
        assert!(!after.damaged_accounting);
    }

    #[test]
    fn s3_decoder_budget_hold_map_and_final_resource_drop_retain_same_charge() {
        struct Resource {
            usage: Arc<Mutex<StorageUsage>>,
        }
        impl Drop for Resource {
            fn drop(&mut self) {
                let usage = self.usage.lock().unwrap();
                assert_eq!(
                    usage.decoder_bytes, 123,
                    "C0_DECODER_CHARGE_THROUGH_RESOURCE_DROP"
                );
                assert_eq!(usage.decoded_bytes, 130);
            }
        }
        let limits = StorageLimits::default();
        let usage = Arc::new(Mutex::new(StorageUsage::default()));
        let disk = StorageReservation::reserve(&usage, &limits, 1024, 7, 0, false, 1024).unwrap();
        let decoder = StorageReservation::reserve_decoder(&usage, &limits, 123, 0).unwrap();
        let resource = decoder.hold(Resource {
            usage: Arc::clone(&usage),
        });
        let mapped = resource.map(|resource| (resource, 17));
        assert_eq!(usage.lock().unwrap().decoder_bytes, 123);
        assert_eq!(
            usage.lock().unwrap().reserved_bytes,
            1024,
            "C0_DECODER_IGNORES_DISK_RESERVATION"
        );
        drop(mapped);
        assert_eq!(usage.lock().unwrap().decoder_bytes, 0);
        assert_eq!(usage.lock().unwrap().decoded_bytes, 7);
        drop(disk);
        assert_eq!(usage.lock().unwrap().decoded_bytes, 0);
        assert!(!usage.lock().unwrap().damaged_accounting);
    }
}
