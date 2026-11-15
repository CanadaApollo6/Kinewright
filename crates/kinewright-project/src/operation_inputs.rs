//! A bound source retains its admitted manifest under the store's existing
//! decoded reservation. Payload checks use an in-place sorted index; they
//! neither replay retained operations nor traverse the candidate document.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::BundleError,
    bundle_records::{ObjectRef, RecordKind},
    operation_history::Error,
    owned_inputs::{OwnedInputEntry, OwnedInputManifest},
    storage_budget::StorageReservation,
    store::{BundleRecord, ProjectStore},
};
use kinewright_core::{ClipContent, Operation};

pub struct ManualInputs {
    reference: ObjectRef,
    manifest: StorageReservation<OwnedInputManifest>,
    font: String,
}
impl ManualInputs {
    pub fn read(store: &ProjectStore, reference: &ObjectRef) -> Result<Self, BundleError> {
        Self::from_reserved(reference.clone(), store.read_record(reference)?)
    }
    fn from_reserved(
        reference: ObjectRef,
        manifest: StorageReservation<OwnedInputManifest>,
    ) -> Result<Self, BundleError> {
        use std::fmt::Write as _;
        struct HashWriter {
            hash: kinewright_hash::Sha256,
            bytes: u64,
        }
        impl std::io::Write for HashWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.bytes = self
                    .bytes
                    .checked_add(bytes.len() as u64)
                    .ok_or_else(|| std::io::Error::other("input length"))?;
                self.hash.update(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        reference.validate()?;
        if reference.record_kind != RecordKind::InputManifest {
            return Err(BundleError::Corrupt("manual input manifest kind".into()));
        }
        manifest.validate(&crate::storage_budget::StorageLimits::default())?;
        // The reference is checked before sorting the retained value. This
        // derived order is used only for membership, never written as a record.
        let mut writer = HashWriter {
            hash: kinewright_hash::Sha256::new(),
            bytes: 0,
        };
        serde_json::to_writer(&mut writer, &*manifest)
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let mut digest = String::with_capacity(64);
        for byte in writer.hash.finalize() {
            let _ = write!(digest, "{byte:02x}");
        }
        if digest != reference.sha256 || writer.bytes != reference.byte_length {
            return Err(BundleError::HistoryChanged);
        }
        let manifest = manifest.map(|mut manifest| {
            manifest.entries.sort_unstable_by(|left, right| {
                (
                    &left.object.record_kind,
                    left.object.sha256.as_str(),
                    left.object.byte_length,
                )
                    .cmp(&(
                        &right.object.record_kind,
                        right.object.sha256.as_str(),
                        right.object.byte_length,
                    ))
                    .then_with(|| left.entry_id.cmp(&right.entry_id))
            });
            manifest
        });
        Ok(Self {
            reference,
            manifest,
            font: kinewright_media::sha256_bytes(kinewright_core::title_font_bytes()),
        })
    }
    #[must_use]
    pub const fn reference(&self) -> &ObjectRef {
        &self.reference
    }
    fn entries(&self, kind: RecordKind, hash: &str, length: u64) -> &[OwnedInputEntry] {
        let key = (&kind, hash, length);
        let compare = |entry: &OwnedInputEntry| {
            (
                &entry.object.record_kind,
                entry.object.sha256.as_str(),
                entry.object.byte_length,
            )
                .cmp(&key)
        };
        let first = self
            .manifest
            .entries
            .partition_point(|entry| compare(entry).is_lt());
        let last = self
            .manifest
            .entries
            .partition_point(|entry| !compare(entry).is_gt());
        &self.manifest.entries[first..last]
    }
    fn font(&self) -> bool {
        self.entries(
            RecordKind::OwnedFont,
            &self.font,
            kinewright_core::title_font_bytes().len() as u64,
        )
        .iter()
        .any(|entry| entry.entry_id == "font:Inter")
    }
    pub(crate) fn check(&self, operations: &[Operation]) -> Result<(), Error> {
        for operation in operations {
            let admitted = match operation {
                Operation::AddAsset { asset } => match (
                    &asset.source_fingerprint.content_sha256,
                    asset.source_fingerprint.byte_len,
                ) {
                    (Some(hash), Some(length)) => self
                        .entries(RecordKind::OwnedMedia, hash, length)
                        .iter()
                        .any(|entry| entry.matches_media_asset(asset)),
                    // An unhashed source is an external locator, never a claim
                    // to bytes owned by the store (decision 9).
                    _ => true,
                },
                Operation::RelinkAsset {
                    asset, candidate, ..
                } => match (
                    &candidate.fingerprint.content_sha256,
                    candidate.fingerprint.byte_len,
                ) {
                    (Some(hash), Some(length)) => self
                        .entries(RecordKind::OwnedMedia, hash, length)
                        .iter()
                        .any(|entry| {
                            entry.entry_id.starts_with(&format!("media:{}:", asset.0))
                                && entry.chunk_table_ref().is_ok()
                                && entry.media_interpretation().is_ok_and(|interpretation| {
                                    interpretation.kind == candidate.kind
                                        && interpretation.fps == candidate.fps
                                        && interpretation.duration == candidate.duration
                                        && interpretation.resolution == candidate.resolution
                                })
                        }),
                    _ => true,
                },
                Operation::AddLutAsset { asset } => self
                    .entries(RecordKind::OwnedLut, &asset.sha256, asset.byte_len)
                    .iter()
                    .any(|entry| {
                        crate::bundle_commit::lut_entry_id_matches(&entry.entry_id, asset)
                    }),
                Operation::AddTitle { .. } => self.font(),
                Operation::AddTrack { track }
                    if track
                        .clips
                        .iter()
                        .any(|clip| matches!(clip.content, ClipContent::Title(_))) =>
                {
                    self.font()
                }
                _ => true,
            };
            if !admitted {
                return Err(Error::InputRequired);
            }
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn fixture() -> Self {
        let manifest = OwnedInputManifest {
            project_id: None,
            session_id: None,
            owner_epoch: 1,
            entries: vec![],
        };
        let bytes = serde_json::to_vec(&manifest).unwrap();
        let reference = ObjectRef {
            sha256: kinewright_media::sha256_bytes(&bytes),
            byte_length: bytes.len() as u64,
            record_kind: RecordKind::InputManifest,
        };
        let reservation = StorageReservation::reserve(
            &std::sync::Arc::default(),
            &crate::storage_budget::StorageLimits::default(),
            0,
            4096,
            0,
            false,
            u64::MAX,
        )
        .unwrap();
        Self::from_reserved(reference, reservation.hold(manifest)).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mixed_kinds_and_duplicate_content_keep_exact_membership() {
        // The fixture owns its decode reservation; the production constructor
        // consumes a store read with that same lifetime.
        let mut manifest = OwnedInputManifest {
            project_id: None,
            session_id: None,
            owner_epoch: 1,
            entries: vec![],
        };
        for (index, (kind, hash)) in [
            (RecordKind::OwnedFont, 'f'),
            (RecordKind::OwnedLut, '0'),
            (RecordKind::OwnedEvidence, '8'),
            (RecordKind::OwnedFont, '1'),
            (RecordKind::OwnedLut, '0'),
        ]
        .into_iter()
        .enumerate()
        {
            manifest.entries.push(OwnedInputEntry {
                entry_id: format!("input:{index}"),
                object: ObjectRef {
                    record_kind: kind,
                    sha256: hash.to_string().repeat(64),
                    byte_length: 7,
                },
                dependencies: vec![],
                selected_stream: None,
                time_base: None,
                origin: None,
                profile: None,
            });
        }
        let bytes = serde_json::to_vec(&manifest).unwrap();
        let reference = ObjectRef {
            sha256: kinewright_media::sha256_bytes(&bytes),
            byte_length: bytes.len() as u64,
            record_kind: RecordKind::InputManifest,
        };
        let usage = std::sync::Arc::default();
        let reservation = StorageReservation::reserve(
            &usage,
            &crate::storage_budget::StorageLimits::default(),
            0,
            16384,
            0,
            false,
            u64::MAX,
        )
        .unwrap();
        let certificate =
            ManualInputs::from_reserved(reference, reservation.hold(manifest)).unwrap();
        assert_eq!(
            certificate
                .entries(RecordKind::OwnedLut, &"0".repeat(64), 7)
                .len(),
            2,
            "INPUT_MEMBERSHIP_INDEX_USES_THE_LOOKUP_ORDER"
        );
        assert_eq!(
            certificate
                .entries(RecordKind::OwnedFont, &"f".repeat(64), 7)
                .len(),
            1
        );
        assert!(
            certificate
                .entries(RecordKind::OwnedLut, &"0".repeat(64), 8)
                .is_empty(),
            "INPUT_MEMBERSHIP_REQUIRES_EXACT_LENGTH"
        );
        drop(certificate);
        assert_eq!(
            usage.lock().unwrap().decoded_bytes,
            0,
            "INPUT_CERTIFICATE_RELEASES_ITS_ACTUAL_DECODE_RESERVATION"
        );
    }
}
