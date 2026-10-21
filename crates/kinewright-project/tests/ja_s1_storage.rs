#![cfg(feature = "test-util")]
use kinewright_media::test_support::TempDirectory;
#[cfg(windows)]
use kinewright_project::bundle::BundleError;
#[cfg(target_os = "linux")]
use kinewright_project::namespace::PortablePath;
use kinewright_project::{io_observer, namespace::ProjectNamespace};
use std::fs;
#[cfg(target_os = "linux")]
use std::path::Path;

#[cfg(target_os = "linux")]
#[test]
fn namespace_remains_bound_after_alias_swap() {
    use std::os::unix::fs::symlink;
    let temp = TempDirectory::new("ja-s1-namespace-alias");
    let original = temp.path("original.kinewright");
    let other = temp.path("other.kinewright");
    let alias = temp.path("alias.kinewright");
    fs::write(&original, b"original root A").unwrap();
    fs::write(&other, b"foreign root B").unwrap();
    fs::create_dir(temp.path("other.kinewright-assets")).unwrap();
    fs::write(temp.path("other.kinewright-assets/LUT"), b"foreign LUT").unwrap();
    symlink(&original, &alias).unwrap();
    let mut owner = ProjectNamespace::acquire(&alias).unwrap();
    fs::remove_file(&alias).unwrap();
    symlink(&other, &alias).unwrap();
    owner
        .publish_immutable_probe(
            &PortablePath::new(Path::new("studio/objects/proof")).unwrap(),
            b"owned proof",
        )
        .unwrap();
    owner
        .publish_immutable_probe(
            &PortablePath::new(Path::new("luts/owned.cube")).unwrap(),
            b"owned LUT bytes",
        )
        .unwrap();
    owner.replace_root_probe(b"changed original A").unwrap();
    assert_eq!(fs::read(&original).unwrap(), b"changed original A");
    assert_eq!(fs::read(&other).unwrap(), b"foreign root B");
    assert_eq!(
        fs::read(temp.path("other.kinewright-assets/LUT")).unwrap(),
        b"foreign LUT"
    );
    assert_eq!(
        fs::read(temp.path("original.kinewright-assets/studio/objects/proof")).unwrap(),
        b"owned proof"
    );
    owner.release().unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn namespace_release_preserves_replaced_lock() {
    let temp = TempDirectory::new("ja-s1-replaced-lock");
    let root = temp.path("edit.kinewright");
    let mut owner = ProjectNamespace::acquire(&root).unwrap();
    let lock = kinewright_project::lockfile_path_for_project(Some(&root)).unwrap();
    let discovery = kinewright_project::discovery_path_for_project(Some(&root)).unwrap();
    let discovery_bytes = fs::read(&discovery).unwrap();
    fs::rename(&lock, temp.path("held-object")).unwrap();
    fs::write(&lock, b"new lock object").unwrap();
    owner.release().unwrap();
    assert_eq!(fs::read(&discovery).ok().as_ref(), Some(&discovery_bytes));
    assert_eq!(fs::read(&lock).unwrap(), b"new lock object");
}

#[cfg(target_os = "linux")]
#[test]
fn strict_replace_refusal_preserves_exact_root() {
    let temp = TempDirectory::new("ja-s1-strict-refusal");
    let root = temp.path("edit.kinewright");
    fs::write(&root, b"prior A").unwrap();
    let mut owner = ProjectNamespace::acquire(&root).unwrap();
    owner.replace_root_probe(b"normal changed B").unwrap();
    assert_eq!(owner.read_root().unwrap().unwrap(), b"normal changed B");
    let attempt = b"independent attempted C\n";
    let (result, trace) = io_observer::observe(|| owner.replace_root_refusing_rename(attempt));
    assert!(result.is_err());
    let hook = trace
        .events
        .iter()
        .find(|e| e.operation == "rename_refusal_hook")
        .expect("armed rename hook was reached");
    assert_eq!(
        hook.attempted_sha256.as_deref(),
        Some(kinewright_media::sha256_bytes(attempt).as_str())
    );
    assert_eq!(hook.attempted_length, Some(attempt.len()));
    assert!(
        trace
            .events
            .iter()
            .any(|e| e.operation == "rename_refusal_injected")
    );
    assert!(
        !trace
            .events
            .iter()
            .any(|e| e.operation == "in_place_fallback")
    );
    assert_eq!(owner.read_root().unwrap().unwrap(), b"normal changed B");
    owner.release().unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn new_ancestor_syncs_precede_root_publication() {
    let temp = TempDirectory::new("ja-s1-ancestor-sync");
    let root = temp.path("edit.kinewright");
    let mut owner = ProjectNamespace::acquire(&root).unwrap();
    let ((), trace) = io_observer::observe(|| {
        owner
            .publish_immutable_probe(
                &PortablePath::new(Path::new("studio/objects/exact")).unwrap(),
                b"exact object",
            )
            .unwrap();
        owner.replace_root_probe(b"complete root").unwrap();
    });
    let root_rename = trace
        .events
        .iter()
        .position(|e| e.operation == "root_rename")
        .unwrap();
    let object_sync = trace
        .events
        .iter()
        .position(|e| e.operation == "object_file_sync")
        .unwrap();
    let object_rename = trace
        .events
        .iter()
        .position(|e| e.operation == "object_rename")
        .unwrap();
    let object_directory_sync = trace
        .events
        .iter()
        .position(|e| e.operation == "directory_sync" && e.path == Path::new("studio/objects"))
        .unwrap();
    let root_sync = trace
        .events
        .iter()
        .position(|e| e.operation == "root_file_sync")
        .unwrap();
    assert!(
        object_sync < object_rename
            && object_rename < object_directory_sync
            && object_directory_sync < root_sync
            && root_sync < root_rename
    );
    for relative in [
        "edit.kinewright-assets",
        "edit.kinewright-assets/studio",
        "edit.kinewright-assets/studio/objects",
    ] {
        assert!(
            trace.events[..root_rename]
                .iter()
                .any(|e| e.operation == "directory_sync" && e.path == temp.path(relative)),
            "missing durable ancestor {relative}"
        );
    }
    assert!(
        trace.events[root_rename + 1..]
            .iter()
            .any(|e| e.operation == "directory_sync" && e.path == root.parent().unwrap())
    );
    assert_eq!(owner.read_root().unwrap().unwrap(), b"complete root");
    owner.release().unwrap();
}

#[cfg(windows)]
#[test]
fn unsupported_profile_refuses_before_store_mutation() {
    let temp = TempDirectory::new("ja-s1-windows-refusal");
    let root = temp.path("new.kinewright");
    let (result, trace) = io_observer::observe(|| ProjectNamespace::acquire(&root));
    assert!(matches!(result, Err(BundleError::DurabilityUnsupported)));
    assert!(trace.events.is_empty());
    assert!(!root.exists());
    assert!(
        !kinewright_project::lockfile_path_for_project(Some(&root))
            .unwrap()
            .exists()
    );
}

#[cfg(windows)]
#[test]
fn windows_unprovable_identity_refuses_before_claim_or_mutation() {
    let temp = TempDirectory::new("ja-s1-windows-identity");
    let root = temp.path("existing.kinewright");
    fs::write(&root, b"exact existing bytes").unwrap();
    let (result, trace) = io_observer::observe(|| ProjectNamespace::acquire(&root));
    assert!(matches!(result, Err(BundleError::Identity(_))));
    assert!(trace.events.is_empty());
    assert_eq!(fs::read(&root).unwrap(), b"exact existing bytes");
    assert!(
        !kinewright_project::lockfile_path_for_project(Some(&root))
            .unwrap()
            .exists()
    );
}

#[cfg(windows)]
#[test]
fn windows_ancestor_pins_deny_delete_and_reparse_components() {
    let temp = TempDirectory::new("ja-s1-windows-parents");
    let parent = temp.path("parent");
    fs::create_dir(&parent).unwrap();
    let pins = ProjectNamespace::inspect_windows_parents(&parent.join("new.kinewright")).unwrap();
    assert!(fs::rename(&parent, temp.path("renamed")).is_err());
    drop(pins);
    fs::rename(&parent, temp.path("renamed")).unwrap();
    std::os::windows::fs::symlink_dir(temp.path("renamed"), &parent)
        .expect("Windows symlink prerequisite unavailable: NOT RUN");
    assert!(ProjectNamespace::inspect_windows_parents(&parent.join("new.kinewright")).is_err());
}

#[test]
fn portable_owned_paths_refuse_nonportable_spellings() {
    use kinewright_project::namespace::PortablePath;
    use std::path::Path;
    for spelling in [
        "", ".", "a/./b", "a//b", "a/../b", "../x", "/x", "a\\b", "a:b", "NUL.txt", "COM¹",
        "LPT2.x", "space ", "dot.", "a?b",
    ] {
        assert!(
            PortablePath::new(Path::new(spelling)).is_err(),
            "{spelling}"
        );
    }
    assert!(PortablePath::new(Path::new("studio/objects/abcd")).is_ok());
}

#[cfg(target_os = "linux")]
mod bounded_store {
    use super::*;
    use kinewright_project::{
        bundle::BundleError,
        bundle_copy::SessionStore,
        bundle_records::{ObjectRef, ProjectId, RecordKind},
        owned_inputs::InputRequest,
        storage_budget::StorageLimits,
        store::ProjectStore,
    };
    use std::io::{Cursor, Read};
    fn reference(bytes: &[u8]) -> ObjectRef {
        ObjectRef {
            sha256: kinewright_media::sha256_bytes(bytes),
            byte_length: bytes.len() as u64,
            record_kind: RecordKind::OwnedMedia,
        }
    }
    fn named(root: &Path, limits: StorageLimits) -> ProjectStore {
        ProjectStore::new(
            ProjectNamespace::acquire(root).unwrap(),
            Some(ProjectId::fresh().unwrap()),
            None,
            limits,
        )
        .unwrap()
    }
    fn input(store: &ProjectStore, bytes: &[u8]) -> InputRequest {
        InputRequest {
            project_id: store.input_owner().0,
            session_id: store.input_owner().1,
            owner_epoch: store.owner_epoch(),
            job: "fixture-import".into(),
            object: reference(bytes),
            source_locator_hint: Some("untrusted-and-never-opened".into()),
            dependencies: vec![],
            selected_stream: None,
            time_base: None,
            origin: None,
            profile: None,
        }
    }
    #[test]
    fn object_admission_checks_exact_bytes_and_dependency_closure() {
        let temp = TempDirectory::new("ja-s1-object-ingress");
        let mut store = named(&temp.path("edit.kinewright"), StorageLimits::default());
        let bytes = b"actual independently specified camera bytes";
        let r = reference(bytes);
        let sealed = store.admit_blob(&mut Cursor::new(bytes), &r, &[]).unwrap();
        assert_eq!(sealed.reference, r);
        store.verify(&r).unwrap();
        assert_eq!(
            store.closure(std::slice::from_ref(&r)).unwrap(),
            vec![r.clone()]
        );
        let child = reference(b"dependent bytes");
        let child_pin = store
            .admit_blob(
                &mut Cursor::new(b"dependent bytes"),
                &child,
                std::slice::from_ref(&r),
            )
            .unwrap();
        assert_eq!(
            store.closure(std::slice::from_ref(&child)).unwrap().len(),
            2
        );
        let before = store.usage().unwrap();
        let missing = reference(b"unadmitted dependency");
        assert!(
            store
                .admit_blob(
                    &mut Cursor::new(b"new child"),
                    &reference(b"new child"),
                    &[missing]
                )
                .is_err()
        );
        let mut wrong = reference(b"expected but absent");
        wrong.byte_length = bytes.len() as u64;
        assert!(
            store
                .admit_blob(&mut Cursor::new(bytes), &wrong, &[])
                .is_err()
        );
        assert_eq!(store.usage().unwrap().physical_bytes, before.physical_bytes);
        assert_eq!(store.usage().unwrap().reserved_bytes, 0);
        let request = input(&store, bytes);
        let (input_ref, record) = store
            .admit_input(&request, &mut Cursor::new(bytes))
            .unwrap();
        let decoded = store.read_input_manifest(&record.reference).unwrap();
        assert_eq!(decoded.entries, vec![input_ref.entry]);
        assert!(store.usage().unwrap().decoded_bytes > 0);
        drop(decoded);
        assert_eq!(store.usage().unwrap().decoded_bytes, 0);
        let manifest_bytes = record.reference.byte_length;
        drop(record);
        drop(child_pin);
        drop(sealed);
        assert_eq!(
            store.collect_unpublished().unwrap(),
            (bytes.len() + b"dependent bytes".len()) as u64 + manifest_bytes
        );
        assert_eq!(store.usage().unwrap().physical_bytes, 0);
    }
    #[test]
    fn reservation_counts_unique_blobs_and_simultaneous_copies() {
        let temp = TempDirectory::new("ja-s1-reservations");
        let limits = StorageLimits {
            owned_bytes: 16,
            copy_overlap_bytes: 16,
            metadata_bytes: 4 << 20,
            ..StorageLimits::default()
        };
        let mut store = named(&temp.path("edit.kinewright"), limits);
        let bytes = b"1234567890abcdef";
        let r = reference(bytes);
        let first = store.admit_blob(&mut Cursor::new(bytes), &r, &[]).unwrap();
        let duplicate = store.admit_blob(&mut Cursor::new(bytes), &r, &[]).unwrap();
        assert_eq!(store.usage().unwrap().physical_bytes, 16);
        assert_eq!(store.usage().unwrap().objects, 1);
        let copy = store.reserve_copy(16).unwrap();
        assert_eq!(store.usage().unwrap().reserved_copy_bytes, 16);
        assert_eq!(
            store.usage().unwrap().physical_bytes + store.usage().unwrap().reserved_copy_bytes,
            32
        );
        assert!(store.reserve_copy((4 << 20) + 17).is_err());
        assert!(
            store
                .admit_blob(&mut Cursor::new(b"x"), &reference(b"x"), &[])
                .is_err()
        );
        drop(first);
        assert_eq!(store.collect_unpublished().unwrap(), 0);
        drop(copy);
        assert_eq!(store.usage().unwrap().reserved_copy_bytes, 0);
        drop(duplicate);
        assert_eq!(store.collect_unpublished().unwrap(), 16);
        assert!(!store.usage().unwrap().damaged_accounting);
    }
    #[test]
    fn failed_seal_and_last_reader_retire_charges_once() {
        let temp = TempDirectory::new("ja-s1-last-reader");
        let mut session = SessionStore::create(&temp.path(""), StorageLimits::default()).unwrap();
        let root = session.store().resolved_root().unwrap();
        let directory = root.parent().unwrap().to_path_buf();
        let bytes = b"live reader exact source bytes";
        let request = input(session.store(), bytes);
        let (reference, sealed) = session
            .store_mut()
            .admit_input(&request, &mut Cursor::new(bytes))
            .unwrap();
        let mut reader = session.store().lease_input(&reference).unwrap();
        let monitor = session.store().usage_handle();
        let physical = monitor().unwrap().physical_bytes;
        assert!(physical > bytes.len() as u64);
        let mut bad = request.clone();
        bad.object = reference_for_bad_input();
        assert!(
            session
                .store_mut()
                .admit_input(&bad, &mut Cursor::new(b"wrong".as_slice()))
                .is_err()
        );
        assert_eq!(monitor().unwrap().physical_bytes, physical);
        assert_eq!(monitor().unwrap().reserved_bytes, 0);
        drop(sealed);
        drop(session);
        assert!(directory.is_dir());
        assert_eq!(monitor().unwrap().physical_bytes, physical);
        let mut actual = Vec::new();
        reader.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, bytes);
        assert!(reader.certify_decoder_open().is_err());
        drop(reader);
        assert_eq!(monitor().unwrap().physical_bytes, 0);
        assert_eq!(monitor().unwrap().objects, 0);
        assert!(!monitor().unwrap().damaged_accounting);
        assert!(!directory.exists());
    }
    fn reference_for_bad_input() -> ObjectRef {
        reference(b"not the supplied bytes")
    }
    #[test]
    fn storage_ingress_refuses_decoded_and_transient_overflow() {
        let temp = TempDirectory::new("ja-s1-ingress-caps");
        let limits = StorageLimits {
            decoded_record_bytes: 4096,
            branch_history_bytes: 4096,
            live_history_bytes: 6000,
            ..StorageLimits::default()
        };
        let mut store = named(&temp.path("edit.kinewright"), limits);
        let first = store.reserve_history_shadow(4000).unwrap();
        assert!(store.reserve_history_shadow(4000).is_err());
        assert!(store.reserve_copy(u64::MAX).is_err());
        drop(first);
        assert_eq!(store.usage().unwrap().live_history_bytes, 0);
        // The native Document contains many values even though its encoded envelope is small.
        assert!(matches!(
            store.admit_document(&kinewright_core::Document::default()),
            Err(BundleError::Capacity(_))
        ));
        assert_eq!(store.usage().unwrap().physical_bytes, 0);
        assert_eq!(store.usage().unwrap().decoded_bytes, 0);
        assert!(!store.usage().unwrap().damaged_accounting);
        let limits = StorageLimits {
            branch_history_bytes: 16_384,
            decoded_record_bytes: 4096,
            ..StorageLimits::default()
        };
        let decoded_store = named(&temp.path("decode-limit.kinewright"), limits);
        assert!(matches!(
            decoded_store.reserve_history_shadow(4097),
            Err(BundleError::Capacity(_))
        ));
        let limits = StorageLimits {
            branch_history_bytes: 4096,
            decoded_record_bytes: 16_384,
            ..StorageLimits::default()
        };
        let branch_store = named(&temp.path("branch-limit.kinewright"), limits);
        assert!(matches!(
            branch_store.reserve_history_shadow(4097),
            Err(BundleError::Capacity(_))
        ));
    }
    #[test]
    fn owned_input_tamper_invalidates_verified_identity() {
        let temp = TempDirectory::new("ja-s1-owned-tamper");
        let root = temp.path("edit.kinewright");
        let mut store = named(&root, StorageLimits::default());
        let bytes = b"owned font exact bytes A";
        let request = input(&store, bytes);
        let (reference, _sealed) = store
            .admit_input(&request, &mut Cursor::new(bytes))
            .unwrap();
        let mut lease = store.lease_input(&reference).unwrap();
        let mut actual = Vec::new();
        lease.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, bytes);
        drop(lease);
        let path = temp
            .path("edit.kinewright-assets/studio/objects")
            .join(&request.object.sha256);
        let changed = b"owned font exact bytes B";
        assert_eq!(bytes.len(), changed.len());
        fs::write(&path, changed).unwrap();
        assert!(store.lease_input(&reference).is_err());
        assert!(
            store
                .closure(std::slice::from_ref(&reference.manifest_ref))
                .is_err()
        );
        assert_eq!(fs::read(path).unwrap(), changed);
    }
}
