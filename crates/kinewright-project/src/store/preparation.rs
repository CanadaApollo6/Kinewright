//! A preparation owns only the names it newly seals, never a store-wide inventory copy.
use super::{ProjectStore, StoreInner, collect_scope};
use crate::{bundle::BundleError, namespace::ProjectNamespace};
use std::{collections::BTreeSet, sync::Arc};

pub(crate) struct PreparedObjects {
    // Retain the real store owner and its ledger until this preparation is released.
    inner: Arc<StoreInner>,
    recording: bool,
    names: BTreeSet<String>,
}
impl ProjectStore {
    pub(crate) fn prepare_objects(&self) -> Result<PreparedObjects, BundleError> {
        if self.inner.read_authority.is_some() {
            return Err(BundleError::Identity("read-only snapshot".into()));
        }
        let mut ledger = self
            .inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
        if ledger.preparation_objects.is_some() {
            return Err(BundleError::HistoryBarrierInvariant);
        }
        ledger.preparation_objects = Some(BTreeSet::new());
        Ok(PreparedObjects {
            inner: Arc::clone(&self.inner),
            recording: true,
            names: BTreeSet::new(),
        })
    }
    /// The caller restored the current recovery inventory, or deferred an uncertain root.
    pub(crate) fn collect_prepared(
        &mut self,
        mut prepared: PreparedObjects,
        settled: bool,
    ) -> Result<(), BundleError> {
        if self.inner.read_authority.is_some() {
            return Err(BundleError::Identity("read-only snapshot".into()));
        }
        if !Arc::ptr_eq(&self.inner, &prepared.inner) {
            return Err(BundleError::HistoryChanged);
        }
        prepared.finish()?;
        if !settled {
            // The root may already reference these objects. Keep them charged
            // until reconciliation restores the authoritative committed graph.
            return Ok(());
        }
        self.with_namespace(ProjectNamespace::finish_object_batch)?;
        let (acknowledged, unclassified, temporaries, can_collect) = {
            let mut ledger = self
                .inner
                .ledger
                .lock()
                .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
            if !ledger.pending.is_empty() || !ledger.pending_controls.is_empty() {
                return Ok(());
            }
            (
                std::mem::replace(
                    &mut ledger.acknowledged_objects,
                    std::mem::take(&mut prepared.names),
                ),
                std::mem::take(&mut ledger.unclassified),
                std::mem::take(&mut ledger.temporaries),
                std::mem::replace(&mut ledger.can_collect, true),
            )
        };
        // Like failed-copy retirement, this pass can touch only the names of
        // this preparation. Durable roots and other preparations' pins win.
        let result = collect_scope(&self.inner, true);
        let mut ledger = self
            .inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
        let remaining = std::mem::replace(&mut ledger.acknowledged_objects, acknowledged);
        for name in remaining {
            if ledger.objects.contains_key(&name) {
                ledger.acknowledged_objects.insert(name);
            }
        }
        ledger.unclassified = unclassified;
        ledger.temporaries = temporaries;
        ledger.can_collect = can_collect;
        result.map(|_| ())
    }
}
impl PreparedObjects {
    pub(crate) fn finish(&mut self) -> Result<(), BundleError> {
        if self.recording {
            self.names = self
                .inner
                .ledger
                .lock()
                .map_err(|_| BundleError::Capacity("ledger poisoned"))?
                .preparation_objects
                .take()
                .ok_or(BundleError::HistoryBarrierInvariant)?;
            self.recording = false;
        }
        Ok(())
    }
}
impl Drop for PreparedObjects {
    fn drop(&mut self) {
        // An interrupted/uncertain owner cannot collect yet. Preserve exact
        // eligibility for its next acknowledged maintenance, without I/O here.
        if self.finish().is_ok()
            && let Ok(mut ledger) = self.inner.ledger.lock()
        {
            ledger.acknowledged_objects.append(&mut self.names);
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::bundle_records::{ObjectRef, RecordKind};

    fn seal(store: &mut ProjectStore, mut bytes: &[u8]) -> super::super::SealedObject {
        let reference = ObjectRef {
            sha256: kinewright_media::sha256_bytes(bytes),
            byte_length: bytes.len() as u64,
            record_kind: RecordKind::OwnedEvidence,
        };
        store.admit_blob(&mut bytes, &reference, &[]).unwrap()
    }
    #[test]
    fn c4_preparation_retires_only_new_names_and_preserves_deduped_objects() {
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "c4-preparation-names",
        );
        let mut session = crate::bundle_copy::SessionStore::create(
            temp.root(),
            crate::storage_budget::StorageLimits::default(),
        )
        .unwrap();
        let store = session.store_mut();
        let older = seal(store, b"older unpublished data");
        let older_reference = older.reference.clone();
        drop(older);
        let anchor = seal(store, b"retained anchor");
        let _committed = store
            .mark_committed(std::slice::from_ref(&anchor.reference))
            .unwrap();
        drop(anchor);
        let before = store.usage().unwrap();
        let mut prepared = store.prepare_objects().unwrap();
        let mut seals = Vec::new();
        for index in 0..8 {
            seals.push(seal(store, &[index; 32]));
        }
        let duplicate = seal(store, b"older unpublished data");
        prepared.finish().unwrap();
        drop(seals);
        drop(duplicate);
        store.collect_prepared(prepared, true).unwrap();
        let after = store.usage().unwrap();
        assert_eq!(
            (after.metadata_bytes, after.objects, after.physical_bytes),
            (before.metadata_bytes, before.objects, before.physical_bytes),
            "C4_PREPARATION_EXACT_NAMES_UNCHARGED"
        );
        assert_eq!(
            store.read_evidence(&older_reference).unwrap().as_slice(),
            b"older unpublished data",
            "C4_PREPARATION_DEDUPED_AND_UNRELATED_NAME_SURVIVES"
        );
        assert!(!after.damaged_accounting && !after.unsettled);
    }
}
