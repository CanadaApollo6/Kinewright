//! Resolve immutable resource certificates at publication boundaries. Warm
//! commands retain references; they do not inspect historical manifests.
use crate::{
    bundle::BundleError,
    bundle_records::{ObjectRef, RecordKind},
    operation_history::{Resource, StoredOwnerRecord},
    owned_inputs::OwnedInputManifest,
    storage_budget::StorageReservation,
    store::ProjectStore,
};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn bindings(
    store: &ProjectStore,
    sources: &BTreeSet<ObjectRef>,
    current: &ObjectRef,
) -> Result<StorageReservation<BTreeMap<Resource, ObjectRef>>, BundleError> {
    bindings_inner(store, sources, current, false)
}
pub(crate) fn retained_bindings(
    store: &ProjectStore,
    sources: &BTreeSet<ObjectRef>,
    current: &ObjectRef,
) -> Result<StorageReservation<BTreeMap<Resource, ObjectRef>>, BundleError> {
    bindings_inner(store, sources, current, true)
}
fn bindings_inner(
    store: &ProjectStore,
    sources: &BTreeSet<ObjectRef>,
    current: &ObjectRef,
    retained: bool,
) -> Result<StorageReservation<BTreeMap<Resource, ObjectRef>>, BundleError> {
    let mut total = 0_u64;
    for reference in sources {
        let count = match reference.record_kind {
            RecordKind::InputManifest => store
                .read_copy_record::<OwnedInputManifest>(reference, retained)?
                .entries
                .len() as u64,
            RecordKind::OperationHistory => store
                .read_copy_record::<StoredOwnerRecord>(reference, retained)?
                .binding_count()?,
            _ => return Err(BundleError::Corrupt("resource context kind".into())),
        };
        total = total
            .checked_add(count)
            .ok_or(BundleError::Capacity("resource certificates"))?;
    }
    let charge = store.reserve_copy_read(
        0,
        total
            .checked_mul(1024)
            .and_then(|n| n.checked_add(4096))
            .ok_or(BundleError::Capacity("resource certificates"))?,
        false,
        retained,
    )?;
    let mut result = BTreeMap::new();
    // Older contexts supply missing resources; the current certificate wins.
    for reference in sources
        .iter()
        .filter(|r| r.record_kind == RecordKind::OperationHistory)
        .chain(
            sources
                .iter()
                .filter(|r| r.record_kind == RecordKind::InputManifest && *r != current),
        )
        .chain(std::iter::once(current))
    {
        match reference.record_kind {
            RecordKind::OperationHistory => {
                let record = store.read_copy_record::<StoredOwnerRecord>(reference, retained)?;
                result.extend(record.binding_refs()?);
            }
            RecordKind::InputManifest => {
                let manifest = store.read_copy_record::<OwnedInputManifest>(reference, retained)?;
                include_manifest(&mut result, reference, &manifest)?;
            }
            _ => unreachable!("resource context kinds checked above"),
        }
    }
    Ok(charge.hold(result))
}
fn include_manifest(
    result: &mut BTreeMap<Resource, ObjectRef>,
    reference: &ObjectRef,
    manifest: &OwnedInputManifest,
) -> Result<(), BundleError> {
    for entry in &manifest.entries {
        let resource = match entry.object.record_kind {
            RecordKind::OwnedMedia => {
                let asset = entry
                    .entry_id
                    .strip_prefix("media:")
                    .and_then(|id| id.split(':').next())
                    .and_then(|id| id.parse().ok())
                    .ok_or_else(|| BundleError::Corrupt("media input identity".into()))?;
                Resource::Media {
                    asset,
                    sha256: Some(entry.object.sha256.clone()),
                    byte_len: Some(entry.object.byte_length),
                }
            }
            RecordKind::OwnedLut => Resource::Lut {
                sha256: entry.object.sha256.clone(),
                byte_len: entry.object.byte_length,
            },
            RecordKind::OwnedFont
                if entry.object.sha256
                    == kinewright_media::sha256_bytes(kinewright_core::title_font_bytes())
                    && entry.object.byte_length
                        == kinewright_core::title_font_bytes().len() as u64 =>
            {
                Resource::TitleFont
            }
            _ => continue,
        };
        result.insert(resource, reference.clone());
    }
    Ok(())
}
