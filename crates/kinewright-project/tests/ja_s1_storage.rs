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
    let temp = TempDirectory::new_in(
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
        "ja-s1-namespace-alias",
    );
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
    let temp = TempDirectory::new_in(
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
        "ja-s1-replaced-lock",
    );
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
    let temp = TempDirectory::new_in(
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
        "ja-s1-strict-refusal",
    );
    let root = temp.path("edit.kinewright");
    fs::write(&root, b"prior A").unwrap();
    let mut owner = ProjectNamespace::acquire(&root).unwrap();
    owner.replace_root_probe(b"normal changed B").unwrap();
    assert_eq!(owner.read_root().unwrap().unwrap(), b"normal changed B");
    let attempt = b"independent attempted C\n";
    let (result, trace) = io_observer::observe(|| owner.replace_root_refusing_rename(attempt));
    assert!(
        matches!(result, Err(kinewright_project::bundle::BundleError::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied),
        "the production rename refusal must be definitely unpublished"
    );
    assert!(
        trace
            .events
            .iter()
            .any(|event| event.operation == "root_rename_refused")
    );
    assert!(
        !fs::read_dir(temp.root()).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".studio-")),
        "definitely unpublished root temporary was not retired"
    );
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
    let temp = TempDirectory::new_in(
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
        "ja-s1-ancestor-sync",
    );
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
    let temp = TempDirectory::new_in(
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
        "ja-s1-windows-refusal",
    );
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
    let temp = TempDirectory::new_in(
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
        "ja-s1-windows-identity",
    );
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
    let temp = TempDirectory::new_in(
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
        "ja-s1-windows-parents",
    );
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
        "", ".", "a/./b", "a//b", "a/../b", "../x", "/x", "a:b", "NUL.txt", "COM¹", "LPT2.x",
        "space ", "dot.", "a?b",
    ] {
        assert!(
            PortablePath::new(Path::new(spelling)).is_err(),
            "{spelling}"
        );
    }
    assert!(PortablePath::new(Path::new("studio/objects/abcd")).is_ok());
    #[cfg(windows)]
    assert_eq!(
        PortablePath::new(Path::new(r"a\b")).unwrap(),
        PortablePath::new(Path::new("a/b")).unwrap()
    );
    #[cfg(not(windows))]
    assert!(PortablePath::new(Path::new(r"a\b")).is_err());
    let path = PortablePath::new(Path::new("a/b")).unwrap();
    assert_eq!(
        serde_json::from_slice::<PortablePath>(&serde_json::to_vec(&path).unwrap()).unwrap(),
        path
    );
    for bad in [r#"["a/b"]"#, r#"["a\\b"]"#] {
        assert!(serde_json::from_str::<PortablePath>(bad).is_err());
    }
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
            record_kind: RecordKind::OwnedEvidence,
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
        let temp = TempDirectory::new_in(
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
            "ja-s1-object-ingress",
        );
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
        let temp = TempDirectory::new_in(
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
            "ja-s1-reservations",
        );
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
        let temp = TempDirectory::new_in(
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
            "ja-s1-last-reader",
        );
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
        let temp = TempDirectory::new_in(
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
            "ja-s1-ingress-caps",
        );
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
        let temp = TempDirectory::new_in(
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
            "ja-s1-owned-tamper",
        );
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

mod durable_bundle {
    use super::*;
    use kinewright_core::Document;
    #[cfg(target_os = "linux")]
    use kinewright_core::{Command, Core, Operation, Track, TrackId, TrackKind};
    #[cfg(target_os = "linux")]
    use kinewright_project::bundle::{BundleWriteAdmission, LoadedBundle, ProjectBundle};
    use std::path::{Path, PathBuf};

    #[cfg(target_os = "linux")]
    fn edit(core: &Core) {
        core.send(Command::Do(Operation::AddTrack {
            track: Track {
                id: TrackId(71),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            },
        }))
        .unwrap();
    }
    #[cfg(target_os = "linux")]
    fn initial_bundle(path: &Path) -> (Core, ProjectBundle) {
        let core = Core::spawn(Document::default()).unwrap();
        edit(&core);
        let mut bundle =
            ProjectBundle::activate(path, BundleWriteAdmission::new_document()).unwrap();
        let inputs = bundle.empty_inputs().unwrap();
        bundle.save(&core, &inputs.reference).unwrap();
        drop(inputs);
        (core, bundle)
    }
    #[cfg(target_os = "linux")]
    fn inventory(path: &Path) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
        fn walk(
            root: &Path,
            path: &Path,
            out: &mut std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
        ) {
            let mut entries: Vec<_> = fs::read_dir(path)
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            entries.sort();
            for entry in entries {
                let metadata = fs::symlink_metadata(&entry).unwrap();
                if metadata.is_dir() {
                    out.insert(entry.strip_prefix(root).unwrap().to_path_buf(), vec![]);
                    walk(root, &entry, out);
                } else {
                    assert!(metadata.is_file());
                    out.insert(
                        entry.strip_prefix(root).unwrap().to_path_buf(),
                        fs::read(entry).unwrap(),
                    );
                }
            }
        }
        let mut out = std::collections::BTreeMap::new();
        walk(path, path, &mut out);
        out
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn legacy_writer_corpus_remains_byte_exact() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../kinewright-agent/tests/fixtures/ja");
        let pins: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.join("writer-corpus-pins.json")).unwrap())
                .unwrap();
        assert_eq!(pins["files"].as_array().unwrap().len(), 4);
        for pin in pins["files"].as_array().unwrap() {
            let bytes = fs::read(directory.join(pin["path"].as_str().unwrap())).unwrap();
            assert_eq!(kinewright_media::sha256_bytes(&bytes), pin["sha256"]);
            let file: kinewright_project::ProjectFile = serde_json::from_slice(&bytes).unwrap();
            assert!(file.studio.is_none());
            assert_eq!(
                kinewright_project::serialize_project_document(&file.document)
                    .unwrap()
                    .as_bytes(),
                bytes
            );
        }
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn document_only_paths_refuse_loaded_studio_bundle() {
        let temp = TempDirectory::new_in(
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
            "ja-s1-no-projection",
        );
        let path = temp.path("studio.kinewright");
        let (_core, bundle) = initial_bundle(&path);
        drop(bundle);
        if let Some(destination) = std::env::var_os("JA_S1_EXPORT_BUNDLE") {
            let destination = std::path::PathBuf::from(destination);
            fs::create_dir(&destination).unwrap();
            for (relative, bytes) in inventory(temp.root()) {
                if relative == Path::new("studio.kinewright")
                    || relative.starts_with("studio.kinewright-assets")
                {
                    let target = destination.join(&relative);
                    if temp.root().join(&relative).is_dir() {
                        fs::create_dir_all(target).unwrap();
                    } else {
                        fs::write(target, bytes).unwrap();
                    }
                }
            }
        }
        let before = inventory(temp.root());
        let bytes = fs::read(&path).unwrap();
        let fresh = temp.path("fresh.kinewright");
        let (result, trace) = io_observer::observe(|| {
            assert!(kinewright_project::load_document(&path).is_err());
            assert!(
                kinewright_project::write_project_document(&Document::default(), &path, None)
                    .is_err()
            );
            kinewright_project::write_project_bytes(
                std::str::from_utf8(&bytes).unwrap(),
                &Document::default(),
                &fresh,
                None,
            )
        });
        assert!(result.is_err());
        assert!(trace.events.is_empty());
        assert_eq!(inventory(temp.root()), before);
        assert!(!fresh.exists());
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn v3_support_inventory_refuses_hidden_and_future_kinds_without_writes() {
        let temp = TempDirectory::new_in(
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
            "ja-s1-raw-support",
        );
        let path = temp.path("studio.kinewright");
        let (_core, bundle) = initial_bundle(&path);
        drop(bundle);
        let original = fs::read(&path).unwrap();
        let root: serde_json::Value = serde_json::from_slice(&original).unwrap();
        for case in [
            "future", "disabled", "pipeline", "hdr", "kind", "api", "fields",
        ] {
            let mut raw = root.clone();
            if case == "future" {
                raw["format_version"] = serde_json::json!(4);
            } else if case == "disabled" {
                raw["parked_code"] = serde_json::json!({"enabled":false,"source":"owned"});
            } else if case == "pipeline" {
                raw["color_context"]["pipeline_state"] =
                    serde_json::json!("future_disabled_pipeline");
            } else if case == "hdr" {
                raw["color_context"]["delivery"]["transfer"] = serde_json::json!("smpte2084");
            } else {
                let sha = raw["studio"]["store_manifest_ref"]["sha256"]
                    .as_str()
                    .unwrap();
                let object = temp.path(&format!("studio.kinewright-assets/studio/objects/{sha}"));
                let mut manifest: serde_json::Value =
                    serde_json::from_slice(&fs::read(&object).unwrap()).unwrap();
                match case {
                    "api" => {
                        manifest["required_apis"]["future-disabled-producer"] =
                            serde_json::json!(9);
                    }
                    "fields" => {
                        manifest["required_fields"]
                            .as_array_mut()
                            .unwrap()
                            .push(serde_json::json!("document.parked_future"));
                    }
                    _ => {
                        manifest["required_kinds"]
                            .as_array_mut()
                            .unwrap()
                            .push(serde_json::json!("code"));
                    }
                }
                let bytes = serde_json::to_vec(&manifest).unwrap();
                let next = kinewright_media::sha256_bytes(&bytes);
                fs::write(
                    temp.path(&format!("studio.kinewright-assets/studio/objects/{next}")),
                    &bytes,
                )
                .unwrap();
                raw["studio"]["store_manifest_ref"]["sha256"] = serde_json::json!(next);
                raw["studio"]["store_manifest_ref"]["byte_length"] = serde_json::json!(bytes.len());
            }
            fs::write(&path, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
            let before = inventory(temp.root());
            let (result, trace) = io_observer::observe(|| LoadedBundle::read(&path));
            assert!(result.is_err(), "unsupported {case} admitted");
            assert!(trace.events.is_empty(), "{case} mutated before refusal");
            assert_eq!(inventory(temp.root()), before, "{case}");
        }
        fs::write(&path, original).unwrap();
        assert!(LoadedBundle::read(&path).is_ok());
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn corrupt_history_quarantines_without_rewriting_usable_versions() {
        let temp = TempDirectory::new_in(
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
            "ja-s1-quarantine",
        );
        let path = temp.path("studio.kinewright");
        let (_core, bundle) = initial_bundle(&path);
        drop(bundle);
        let loaded = LoadedBundle::read(&path).unwrap();
        let checkpoint = loaded.manual_checkpoint().unwrap().clone();
        drop(loaded);
        let document = temp.path(&format!(
            "studio.kinewright-assets/studio/objects/{}",
            checkpoint.document_ref.sha256
        ));
        let usable = fs::read(&document).unwrap();
        let history = temp.path(&format!(
            "studio.kinewright-assets/studio/objects/{}",
            checkpoint.history_ref.sha256
        ));
        let original = fs::read(&history).unwrap();
        let damaged =
            serde_json::to_vec(&serde_json::from_slice::<serde_json::Value>(&original).unwrap())
                .unwrap();
        assert_eq!(original.len(), damaged.len());
        assert_ne!(original, damaged);
        fs::write(&history, &damaged).unwrap();
        let before = inventory(temp.root());
        let (result, trace) = io_observer::observe(|| LoadedBundle::read(&path));
        assert!(result.is_err());
        assert!(trace.events.is_empty());
        assert_eq!(inventory(temp.root()), before);
        let (assessment, trace) =
            io_observer::observe(|| kinewright_project::bundle_recovery::assess(&path));
        let kinewright_project::bundle_recovery::RecoveryAssessment::Quarantined {
            admission,
            branches,
            verified_document_bytes,
            ..
        } = assessment.unwrap()
        else {
            panic!("damaged history resumed")
        };
        assert_eq!(branches.len(), 1);
        assert_eq!(branches[0].branch_id, checkpoint.branch_id);
        assert_eq!(branches[0].history_ref, checkpoint.history_ref);
        assert!(verified_document_bytes.contains(&checkpoint.document_ref));
        assert_eq!(admission.raw_bytes(), fs::read(&path).unwrap());
        assert!(trace.events.is_empty());
        assert_eq!(inventory(temp.root()), before);
        assert_eq!(fs::read(history).unwrap(), damaged);
        assert_eq!(fs::read(document).unwrap(), usable);
        assert_eq!(
            kinewright_media::sha256_bytes(&usable),
            checkpoint.document_ref.sha256
        );
    }
    #[derive(Clone)]
    pub(super) struct CrashFs {
        root: PathBuf,
        assets: PathBuf,
        next: u64,
        volatile_names: std::collections::BTreeMap<PathBuf, u64>,
        durable_names: std::collections::BTreeMap<PathBuf, u64>,
        volatile_dirs: std::collections::BTreeSet<PathBuf>,
        durable_dirs: std::collections::BTreeSet<PathBuf>,
        data: std::collections::BTreeMap<u64, (String, usize)>,
        durable_data: std::collections::BTreeSet<u64>,
        staged: std::collections::BTreeMap<PathBuf, u64>,
        root_stage: Option<PathBuf>,
    }
    impl CrashFs {
        pub(super) fn new(root: &Path, original: &[u8]) -> Self {
            let parent = root.parent().unwrap().to_path_buf();
            Self {
                root: root.to_path_buf(),
                assets: root.with_extension("kinewright-assets"),
                next: 1,
                volatile_names: std::collections::BTreeMap::from([(root.to_path_buf(), 0)]),
                durable_names: std::collections::BTreeMap::from([(root.to_path_buf(), 0)]),
                volatile_dirs: std::collections::BTreeSet::from([parent.clone()]),
                durable_dirs: std::collections::BTreeSet::from([parent]),
                data: std::collections::BTreeMap::from([(
                    0,
                    (kinewright_media::sha256_bytes(original), original.len()),
                )]),
                durable_data: std::collections::BTreeSet::from([0]),
                staged: std::collections::BTreeMap::new(),
                root_stage: None,
            }
        }
        fn path(&self, path: &Path) -> PathBuf {
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                self.assets.join(path)
            }
        }
        pub(super) fn apply(&mut self, event: &kinewright_project::io_observer::IoObservation) {
            let path = self.path(&event.path);
            match event.operation {
                "directory_create" => {
                    self.volatile_dirs.insert(path);
                }
                "object_stage" | "root_stage" | "inventory_stage" => {
                    let node = self.next;
                    self.next += 1;
                    self.data.insert(
                        node,
                        (
                            event
                                .attempted_sha256
                                .clone()
                                .expect("stage has actual digest"),
                            event.attempted_length.expect("stage has actual length"),
                        ),
                    );
                    self.staged.insert(path.clone(), node);
                    if event.operation == "root_stage" {
                        self.root_stage = Some(path);
                    }
                }
                "object_file_sync" | "root_file_sync" | "inventory_file_sync" => {
                    self.durable_data.insert(
                        *self
                            .staged
                            .get(&path)
                            .expect("file sync follows its actual write"),
                    );
                }
                "object_rename" | "inventory_rename" => {
                    self.volatile_names.insert(
                        path.clone(),
                        *self.staged.get(&path).expect("published file was staged"),
                    );
                }
                "root_rename" => {
                    let staged = self.root_stage.as_ref().expect("root was staged");
                    self.volatile_names
                        .insert(self.root.clone(), *self.staged.get(staged).unwrap());
                }
                "object_unlink" => {
                    self.volatile_names.remove(&path);
                }
                "directory_sync" => {
                    self.durable_names
                        .retain(|name, _| name.parent() != Some(path.as_path()));
                    for (name, node) in &self.volatile_names {
                        if name.parent() == Some(path.as_path()) {
                            self.durable_names.insert(name.clone(), *node);
                        }
                    }
                    self.durable_dirs
                        .retain(|name| name.parent() != Some(path.as_path()));
                    for name in &self.volatile_dirs {
                        if name.parent() == Some(path.as_path()) {
                            self.durable_dirs.insert(name.clone());
                        }
                    }
                }
                _ => {}
            }
        }
        pub(super) fn read<'a>(
            &self,
            path: &Path,
            bytes: &'a std::collections::BTreeMap<String, Vec<u8>>,
        ) -> Result<&'a [u8], String> {
            let mut parent = path.parent();
            while let Some(directory) = parent {
                if !self.durable_dirs.contains(directory) {
                    return Err(format!("directory name lost: {}", directory.display()));
                }
                if directory == self.root.parent().unwrap() {
                    break;
                }
                parent = directory.parent();
            }
            let node = self
                .durable_names
                .get(path)
                .ok_or_else(|| format!("name lost: {}", path.display()))?;
            if !self.durable_data.contains(node) {
                return Err(format!("file data lost at inode {node}"));
            }
            let (digest, length) = self.data.get(node).ok_or("unknown inode")?;
            let bytes = bytes
                .get(digest)
                .ok_or_else(|| format!("missing independent observed bytes {digest}"))?;
            if bytes.len() != *length || kinewright_media::sha256_bytes(bytes) != *digest {
                return Err("observed bytes differ".into());
            }
            Ok(bytes)
        }
        fn graph(
            &self,
            bytes: &std::collections::BTreeMap<String, Vec<u8>>,
        ) -> Result<String, String> {
            let root = self.read(&self.root, bytes)?;
            let raw: serde_json::Value = serde_json::from_slice(root).map_err(|e| e.to_string())?;
            if let Some(studio) = raw.get("studio") {
                let reference = &studio["store_manifest_ref"];
                let hash = reference["sha256"].as_str().ok_or("manifest hash")?;
                let path = self.assets.join("studio/objects").join(hash);
                let encoded = self.read(&path, bytes)?;
                if kinewright_media::sha256_bytes(encoded) != hash
                    || encoded.len() as u64
                        != reference["byte_length"].as_u64().ok_or("manifest length")?
                {
                    return Err("manifest identity lost".into());
                }
                let manifest: serde_json::Value =
                    serde_json::from_slice(encoded).map_err(|e| e.to_string())?;
                for object in manifest["objects"].as_array().ok_or("manifest objects")? {
                    let hash = object["sha256"].as_str().ok_or("object hash")?;
                    let encoded =
                        self.read(&self.assets.join("studio/objects").join(hash), bytes)?;
                    if kinewright_media::sha256_bytes(encoded) != hash
                        || encoded.len() as u64
                            != object["byte_length"].as_u64().ok_or("object length")?
                    {
                        return Err("reachable object identity lost".into());
                    }
                }
            } else {
                let document: Document = serde_json::from_value(raw).map_err(|e| e.to_string())?;
                document.validate().map_err(|e| e.to_string())?;
            }
            Ok(kinewright_media::sha256_bytes(root))
        }
    }
    #[cfg(target_os = "linux")]
    #[allow(
        clippy::too_many_lines,
        reason = "fresh production inventory/GC trace replayed at every dropped-write crash cut"
    )]
    pub(super) fn model_inventory_retirement_cuts() {
        use kinewright_project::{
            bundle_records::{ObjectRef, RecordKind},
            store::RecoveryInventory,
        };
        let temp = TempDirectory::new_in(
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
            "ja-s1-retirement-model",
        );
        let path = temp.path("edit.kinewright");
        let (core, mut bundle) = initial_bundle(&path);
        let original = fs::read(&path).unwrap();
        let mut model = CrashFs::new(&path, &original);
        let mut bytes = std::collections::BTreeMap::new();
        for (relative, value) in inventory(temp.root()) {
            let absolute = temp.path(relative.to_str().unwrap());
            if !absolute.is_file() {
                continue;
            }
            let node = model.next;
            model.next += 1;
            model
                .data
                .insert(node, (kinewright_media::sha256_bytes(&value), value.len()));
            model.durable_data.insert(node);
            model.volatile_names.insert(absolute.clone(), node);
            model.durable_names.insert(absolute.clone(), node);
            bytes.insert(kinewright_media::sha256_bytes(&value), value);
            let mut parent = absolute.parent();
            while let Some(directory) = parent {
                model.volatile_dirs.insert(directory.to_path_buf());
                model.durable_dirs.insert(directory.to_path_buf());
                if directory == path.parent().unwrap() {
                    break;
                }
                parent = directory.parent();
            }
        }
        let mut events = vec![];
        let mut roots =
            std::collections::BTreeSet::from([kinewright_media::sha256_bytes(&original)]);
        let inputs = bundle.root().unwrap().input_manifest_ref.clone();
        for iteration in 0..6 {
            let old = fs::read(&path).unwrap();
            let committed = bundle
                .store()
                .read_record::<kinewright_project::store::StoreManifest>(
                    &bundle.root().unwrap().store_manifest_ref,
                )
                .unwrap();
            let mut staged = committed.recovery.clone();
            let settled = serde_json::to_vec(&staged).unwrap();
            bytes.insert(kinewright_media::sha256_bytes(&settled), settled);
            staged.staging = Some(ObjectRef {
                sha256: kinewright_media::sha256_bytes(&old),
                byte_length: old.len() as u64,
                record_kind: RecordKind::RootCheckpoint,
            });
            let encoded = serde_json::to_vec(&staged).unwrap();
            bytes.insert(kinewright_media::sha256_bytes(&encoded), encoded);
            core.send(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(71),
                locked: iteration % 2 == 0,
            }))
            .unwrap();
            let (_, trace) = io_observer::observe(|| bundle.save(&core, &inputs).unwrap());
            assert!(!trace.overflowed);
            events.extend(trace.events);
            roots.insert(kinewright_media::sha256_file(&path).unwrap());
            for (relative, value) in inventory(temp.root()) {
                if temp.path(relative.to_str().unwrap()).is_file() {
                    bytes.insert(kinewright_media::sha256_bytes(&value), value);
                }
            }
        }
        assert!(events.iter().any(|e| e.operation == "object_unlink"));
        assert!(
            events
                .iter()
                .filter(|e| e.operation == "inventory_rename")
                .count()
                >= 12
        );
        let mut acknowledged = kinewright_media::sha256_bytes(&original);
        let mut cuts = 0;
        for event in &events {
            model.apply(event);
            let selected = model
                .graph(&bytes)
                .unwrap_or_else(|e| panic!("retirement cut after {}: {e}", event.operation));
            assert!(roots.contains(&selected));
            if event.operation == "bundle_B5" {
                acknowledged.clone_from(&selected);
            }
            if event.operation == "object_unlink" {
                assert_eq!(
                    selected, acknowledged,
                    "GC preceded durable root acknowledgment"
                );
            }
            let journal = model.assets.join("studio/recovery/inventory");
            let encoded = model.read(&journal, &bytes).unwrap();
            let inventory: RecoveryInventory = serde_json::from_slice(encoded).unwrap();
            if matches!(event.operation, "bundle_B5" | "object_unlink") {
                let root: serde_json::Value =
                    serde_json::from_slice(model.read(&model.root, &bytes).unwrap()).unwrap();
                let manifest_hash = root["studio"]["store_manifest_ref"]["sha256"]
                    .as_str()
                    .unwrap();
                let manifest: serde_json::Value = serde_json::from_slice(
                    model
                        .read(
                            &model.assets.join("studio/objects").join(manifest_hash),
                            &bytes,
                        )
                        .unwrap(),
                )
                .unwrap();
                if event.operation == "object_unlink" {
                    assert_eq!(
                        serde_json::to_value(&inventory).unwrap(),
                        manifest["recovery"],
                        "collection preceded reconciled retirement inventory"
                    );
                } else {
                    let selected: RecoveryInventory =
                        serde_json::from_value(manifest["recovery"].clone()).unwrap();
                    for reference in selected
                        .predecessors
                        .iter()
                        .chain(selected.pre_migration.iter())
                    {
                        assert!(
                            inventory.predecessors.contains(reference)
                                || inventory.staging.as_ref() == Some(reference)
                                || inventory.pre_migration.as_ref() == Some(reference),
                            "B3 inventory lost the acknowledged recovery union"
                        );
                    }
                }
            }
            for reference in inventory
                .predecessors
                .iter()
                .chain(inventory.staging.iter())
                .chain(inventory.pre_migration.iter())
            {
                let actual = model
                    .read(
                        &model.assets.join("studio/objects").join(&reference.sha256),
                        &bytes,
                    )
                    .unwrap();
                assert_eq!(actual.len() as u64, reference.byte_length);
                assert_eq!(kinewright_media::sha256_bytes(actual), reference.sha256);
            }
            cuts += 1;
        }
        assert!(
            cuts > 100,
            "the production retirement trace was not exercised"
        );
        assert_eq!(
            model.graph(&bytes).unwrap(),
            kinewright_media::sha256_file(&path).unwrap()
        );
    }
    #[allow(clippy::type_complexity, clippy::too_many_lines)]
    #[cfg(target_os = "linux")]
    fn modeled_publication() -> (
        TempDirectory,
        PathBuf,
        Vec<u8>,
        kinewright_project::io_observer::IoTrace,
        std::collections::BTreeMap<String, Vec<u8>>,
        String,
    ) {
        use kinewright_project::{
            bundle_records::{ObjectRef, RecordKind},
            owned_inputs::{OwnedInputEntry, OwnedInputManifest},
        };
        let temp = TempDirectory::new_in(
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
            "ja-s1-crash-model",
        );
        let path = temp.path("studio.kinewright");
        let original = include_bytes!(
            "../../kinewright-agent/tests/fixtures/ja/s1-migration-input.kinewright"
        )
        .to_vec();
        fs::write(&path, &original).unwrap();
        let admission = BundleWriteAdmission::read(&path).unwrap();
        let core = Core::spawn(admission.legacy_document().unwrap()).unwrap();
        edit(&core);
        let (writer, trace) = io_observer::observe(|| {
            let mut bundle = ProjectBundle::activate(&path, admission).unwrap();
            let mut seals = vec![];
            let mut entries = vec![];
            for (entry_id, bytes) in [
                (
                    "opaque-comment",
                    b"  Exact caf\xc3\xa9\n\tcomment  ".as_slice(),
                ),
                ("opaque-voice", b"\x00voice\xffowned\n".as_slice()),
                (
                    "opaque-proof",
                    b"proof bytes independently specified\r\n".as_slice(),
                ),
            ] {
                let reference = ObjectRef {
                    sha256: kinewright_media::sha256_bytes(bytes),
                    byte_length: bytes.len() as u64,
                    record_kind: RecordKind::OwnedEvidence,
                };
                seals.push(
                    bundle
                        .store_mut()
                        .unwrap()
                        .admit_blob(&mut &*bytes, &reference, &[])
                        .unwrap(),
                );
                entries.push(OwnedInputEntry {
                    entry_id: entry_id.into(),
                    object: reference,
                    dependencies: vec![],
                    selected_stream: None,
                    time_base: None,
                    origin: None,
                    profile: None,
                });
            }
            let inputs = bundle
                .admit_inputs(&OwnedInputManifest {
                    project_id: Some(bundle.project_id().clone()),
                    session_id: None,
                    owner_epoch: bundle.store().owner_epoch(),
                    entries,
                })
                .unwrap();
            let report = bundle.save(&core, &inputs.reference).unwrap();
            assert_eq!(
                fs::read(&path).unwrap(),
                *bundle.serialized_root().unwrap(),
                "complete production serializer control"
            );
            drop(inputs);
            drop(seals);
            drop(bundle);
            report.root_sha256
        });
        assert!(!trace.overflowed);
        let loaded = LoadedBundle::read(&path).unwrap();
        let restored = Core::spawn(Document::default()).unwrap();
        loaded.restore_manual(&restored).unwrap();
        assert_eq!(
            restored.export_history().unwrap(),
            core.export_history().unwrap(),
            "native changed history control"
        );
        drop(loaded);
        let mut bytes = std::collections::BTreeMap::from([(
            kinewright_media::sha256_bytes(&original),
            original.clone(),
        )]);
        for (_, value) in inventory(temp.root()) {
            bytes.insert(kinewright_media::sha256_bytes(&value), value);
        }
        if let Some(output) = std::env::var_os("JA_S1_EXPORT_MODEL") {
            let events: Vec<_> = trace.events.iter().map(|event| {
                let (absolute, relative) = if event.path.is_absolute() {
                    (true,event.path.strip_prefix(temp.root()).unwrap())
                } else { (false,event.path.as_path()) };
                serde_json::json!({"operation":event.operation,"absolute":absolute,
                    "components":relative.components().map(|c|c.as_os_str().to_str().unwrap()).collect::<Vec<_>>(),
                    "attempted_sha256":event.attempted_sha256,"attempted_length":event.attempted_length})
            }).collect();
            let binary = std::env::current_exe().unwrap();
            let record = serde_json::json!({"backend":"recorded-linux-production-io-for-portable-crash-model", "events":events,
                "original":original,"bytes":bytes,"writer":writer,
                "binary_sha256":kinewright_media::sha256_file(&binary).unwrap()});
            fs::write(output, serde_json::to_vec(&record).unwrap()).unwrap();
        }
        (temp, path, original, trace, bytes, writer)
    }
    #[cfg(not(target_os = "linux"))]
    #[allow(clippy::type_complexity)]
    fn modeled_publication() -> (
        TempDirectory,
        PathBuf,
        Vec<u8>,
        kinewright_project::io_observer::IoTrace,
        std::collections::BTreeMap<String, Vec<u8>>,
        String,
    ) {
        let frozen = include_bytes!("../../kinewright-agent/tests/fixtures/ja/s1-crash-model.json");
        let record: serde_json::Value = serde_json::from_slice(frozen).unwrap();
        let temp = TempDirectory::new_in(
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
            "ja-s1-portable-model",
        );
        let events = record["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| {
                let operation = match event["operation"].as_str().unwrap() {
                    "directory_create" => "directory_create",
                    "directory_sync" => "directory_sync",
                    "object_stage" => "object_stage",
                    "object_file_sync" => "object_file_sync",
                    "object_rename" => "object_rename",
                    "root_stage" => "root_stage",
                    "root_file_sync" => "root_file_sync",
                    "root_rename" => "root_rename",
                    "inventory_stage" => "inventory_stage",
                    "inventory_file_sync" => "inventory_file_sync",
                    "inventory_rename" => "inventory_rename",
                    "bundle_B0" => "bundle_B0",
                    "bundle_B1" => "bundle_B1",
                    "bundle_B2" => "bundle_B2",
                    "bundle_B3" => "bundle_B3",
                    "bundle_B4" => "bundle_B4",
                    "bundle_B5" => "bundle_B5",
                    value => panic!("unknown frozen event {value}"),
                };
                let relative: PathBuf = event["components"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| c.as_str().unwrap())
                    .collect();
                let path = if event["absolute"] == true {
                    temp.root().join(relative)
                } else {
                    relative
                };
                kinewright_project::io_observer::IoObservation {
                    operation,
                    path,
                    attempted_sha256: event["attempted_sha256"].as_str().map(str::to_owned),
                    attempted_length: event["attempted_length"]
                        .as_u64()
                        .map(|n| usize::try_from(n).unwrap()),
                }
            })
            .collect();
        let original = serde_json::from_value(record["original"].clone()).unwrap();
        let bytes = serde_json::from_value(record["bytes"].clone()).unwrap();
        let writer = record["writer"].as_str().unwrap().to_owned();
        let path = temp.path("studio.kinewright");
        (
            temp,
            path,
            original,
            kinewright_project::io_observer::IoTrace {
                events,
                overflowed: false,
            },
            bytes,
            writer,
        )
    }

    #[test]
    fn bundle_crash_cuts_preserve_acknowledged_graph() {
        let (_temp, path, original, trace, bytes, next) = modeled_publication();
        let old = kinewright_media::sha256_bytes(&original);
        let mut model = CrashFs::new(&path, &original);
        let mut acknowledged = false;
        let mut seen_old = false;
        let mut seen_new = false;
        assert_eq!(model.graph(&bytes).unwrap(), old);
        for (cut, event) in trace.events.iter().enumerate() {
            model.apply(event);
            acknowledged |= event.operation == "bundle_B5";
            let recovered = model
                .graph(&bytes)
                .unwrap_or_else(|e| panic!("cut {cut} after {}: {e}", event.operation));
            assert!(
                recovered == old || recovered == next,
                "cut {cut} selected unrelated state"
            );
            if acknowledged {
                assert_eq!(recovered, next, "acknowledged graph lost at {cut}");
            }
            seen_old |= recovered == old;
            seen_new |= recovered == next;
        }
        assert!(acknowledged && seen_old && seen_new);
        assert!(trace.events.iter().any(|e| e.operation == "root_rename"));
        assert!(
            trace
                .events
                .iter()
                .any(|e| e.operation == "object_file_sync")
        );
        assert!(
            trace
                .events
                .iter()
                .any(|e| e.operation == "inventory_rename")
        );
        assert!(
            !trace
                .events
                .iter()
                .any(|e| e.operation == "in_place_fallback")
        );
    }
    #[test]
    fn each_missing_barrier_breaks_storage_crash_model() {
        let (_temp, path, original, trace, bytes, next) = modeled_publication();
        let assets = path.with_extension("kinewright-assets");
        let original_hash = kinewright_media::sha256_bytes(&original);
        let first_metadata = trace
            .events
            .iter()
            .find(|e| {
                e.operation == "object_file_sync"
                    && e.path.file_name().unwrap().to_str().unwrap() != original_hash
            })
            .unwrap()
            .path
            .clone();
        for missing in ["B1", "B2", "B3", "B4", "B5", "ancestor"] {
            let mut model = CrashFs::new(&path, &original);
            let mut after_rename = false;
            let mut removed = 0;
            for event in &trace.events {
                let normalized = model.path(&event.path);
                let omit = match missing {
                    "B1" => event.operation == "object_file_sync" && event.path == first_metadata,
                    "B2" => {
                        event.operation == "directory_sync"
                            && normalized == assets.join("studio/objects")
                    }
                    "B3" => {
                        event.operation == "object_file_sync"
                            && event.path.file_name().unwrap().to_str().unwrap() == original_hash
                    }
                    "B4" => event.operation == "root_file_sync",
                    "B5" => {
                        event.operation == "directory_sync"
                            && normalized == path.parent().unwrap()
                            && after_rename
                    }
                    _ => event.operation == "directory_sync" && normalized == assets,
                };
                after_rename |= event.operation == "root_rename";
                if omit {
                    removed += 1;
                } else {
                    model.apply(event);
                }
            }
            assert!(removed > 0, "{missing} never reached an actual barrier");
            assert_ne!(
                model.graph(&bytes).ok().as_deref(),
                Some(next.as_str()),
                "{missing} omission did not lose acknowledged graph"
            );
        }
    }

    #[test]
    #[allow(clippy::too_many_lines, clippy::items_after_statements)]
    #[cfg(target_os = "linux")]
    fn linux_bundle_process_death_preserves_acknowledged_graph() {
        use std::{
            process::{Command as Process, Stdio},
            time::{Duration, Instant},
        };
        const WORKER: &str = "JA_S1_BUNDLE_CRASH_WORKER";
        if let Some(path) = std::env::var_os(WORKER) {
            let path = PathBuf::from(path);
            let loaded = LoadedBundle::read(&path).unwrap();
            let core = Core::spawn(Document::default()).unwrap();
            loaded.restore_manual(&core).unwrap();
            let mut bundle = loaded.into_writer().unwrap();
            core.send(Command::Do(Operation::AddTrack {
                track: Track {
                    id: TrackId(72),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![],
                },
            }))
            .unwrap();
            let inputs = bundle.empty_inputs().unwrap();
            bundle.save(&core, &inputs.reference).unwrap();
            panic!("recorded publication hook was never reached");
        }
        for point in [
            "bundle_B0",
            "object_file_sync",
            "object_rename",
            "bundle_B1",
            "bundle_B2",
            "bundle_B3",
            "root_file_sync",
            "root_rename",
            "bundle_B5",
        ] {
            let temp = TempDirectory::new_in(
                std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
                "ja-s1-owned-process-crash",
            );
            let path = temp.path("studio.kinewright");
            let signals = temp.path("signals");
            fs::create_dir(&signals).unwrap();
            let (core, bundle) = initial_bundle(&path);
            let original = core.export_history().unwrap();
            drop(bundle);
            core.send(Command::Do(Operation::AddTrack {
                track: Track {
                    id: TrackId(72),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![],
                },
            }))
            .unwrap();
            let next = core.export_history().unwrap();
            let mut child = Process::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "durable_bundle::linux_bundle_process_death_preserves_acknowledged_graph",
                    "--test-threads=1",
                    "--nocapture",
                ])
                .env(WORKER, &path)
                .env("REV2_HOOK", point)
                .env("REV2_SIGNALS", &signals)
                .env_remove("REV2_EXIT")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let pid = child.id();
            println!("owned fixture worker PID {pid}, publication cut {point}");
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                if signals.join("paused").exists() {
                    break;
                }
                if child.try_wait().unwrap().is_some() {
                    let result = child.wait_with_output().unwrap();
                    panic!(
                        "worker {pid} never reached {point}: {} {}",
                        String::from_utf8_lossy(&result.stdout),
                        String::from_utf8_lossy(&result.stderr)
                    );
                }
                if Instant::now() > deadline {
                    child.kill().unwrap();
                    let result = child.wait_with_output().unwrap();
                    panic!(
                        "worker {pid} timed out before {point}: {} {}",
                        String::from_utf8_lossy(&result.stdout),
                        String::from_utf8_lossy(&result.stderr)
                    );
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            assert_eq!(fs::read_to_string(signals.join("paused")).unwrap(), point);
            child.kill().unwrap();
            let result = child.wait_with_output().unwrap();
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(
                result.status.signal(),
                Some(9),
                "only the recorded fixture worker was killed"
            );
            let loaded = LoadedBundle::read(&path).unwrap();
            let generation = loaded.root().unwrap().generation;
            assert!(
                generation == 1 || generation == 2,
                "{point} selected an unrelated generation"
            );
            let restored = Core::spawn(Document::default()).unwrap();
            loaded.restore_manual(&restored).unwrap();
            assert_eq!(
                restored.export_history().unwrap(),
                if generation == 1 {
                    original.clone()
                } else {
                    next.clone()
                },
                "{point}"
            );
            if point == "bundle_B5" {
                assert_eq!(
                    generation, 2,
                    "acknowledged publication survives process death"
                );
            }
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn migration_keeps_exact_backup_and_is_idempotent() {
        let temp = TempDirectory::new_in(
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
            "ja-s1-migration",
        );
        let path = temp.path("edit.kinewright");
        let bytes=b"{\"tracks\":[],\"media_pool\":[],\"fps\":{\"numerator\":30,\"denominator\":1},\"duration\":0,\"resolution\":[1920,1080]}";
        // The source deliberately has independent compact whitespace and omitted default fields.
        fs::write(&path, bytes).unwrap();
        let admission = BundleWriteAdmission::read(&path).unwrap();
        let core = Core::spawn(admission.legacy_document().unwrap()).unwrap();
        edit(&core);
        let mut bundle = ProjectBundle::activate(&path, admission).unwrap();
        let id = bundle.project_id().clone();
        let branch = bundle.manual_branch_id().clone();
        let inputs = bundle.empty_inputs().unwrap();
        let report = bundle.save(&core, &inputs.reference).unwrap();
        assert_eq!(report.generation, 1);
        drop(inputs);
        drop(bundle);
        let loaded = LoadedBundle::read(&path).unwrap();
        assert_eq!(loaded.root().unwrap().project_id, id);
        assert_eq!(loaded.root().unwrap().manual_branch_id, branch);
        let restored = Core::spawn(Document::default()).unwrap();
        loaded.restore_manual(&restored).unwrap();
        assert_eq!(
            restored.export_history().unwrap(),
            core.export_history().unwrap()
        );
        let mut bundle = loaded.into_writer().unwrap();
        let manifest_path = temp.path("edit.kinewright-assets/studio/recovery/inventory");
        let recovery: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        let backup = recovery["pre_migration"]["sha256"].as_str().unwrap();
        let backup_path = temp.path(&format!("edit.kinewright-assets/studio/objects/{backup}"));
        assert_eq!(fs::read(&backup_path).unwrap(), bytes);
        let inputs = bundle.empty_inputs().unwrap();
        bundle.save(&core, &inputs.reference).unwrap();
        drop(inputs);
        drop(bundle);
        assert_eq!(fs::read(&backup_path).unwrap(), bytes);
        let admission = BundleWriteAdmission::read(&path).unwrap();
        let bundle = ProjectBundle::activate(&path, admission).unwrap();
        assert_eq!(bundle.project_id(), &id);
        assert_eq!(bundle.manual_branch_id(), &branch);
    }
}

#[cfg(windows)]
#[test]
fn windows_portable_fixtures_refuse_durability_without_mutation() {
    use kinewright_core::{Command, Core, Document, JournalCommand, TimelineRevision};
    use kinewright_project::{bundle::BundleWriteAdmission, headless::save_headless_bundle};
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../kinewright-agent/tests/fixtures/ja");
    for name in ["history-loss", "reply-retry"] {
        let temp = TempDirectory::new_in(
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
            "ja-s1-windows-headless-refusal",
        );
        let original = fs::read(fixtures.join(format!("{name}.json"))).unwrap();
        let fixture: serde_json::Value = serde_json::from_slice(&original).unwrap();
        let action = fixture["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["action"] == "core_send")
            .unwrap();
        let role = action["args"]["input"].as_str().unwrap();
        let command_bytes =
            fs::read(fixtures.join(fixture["inputs"][role]["relative_file"].as_str().unwrap()))
                .unwrap();
        let command: JournalCommand = serde_json::from_slice(&command_bytes).unwrap();
        let document =
            fixture["initial"]["document"]
                .as_str()
                .map_or_else(Document::default, |role| {
                    let bytes = fs::read(
                        fixtures.join(fixture["inputs"][role]["relative_file"].as_str().unwrap()),
                    )
                    .unwrap();
                    serde_json::from_slice::<kinewright_project::ProjectFile>(&bytes)
                        .unwrap()
                        .document
                });
        let core = Core::spawn_at(
            document,
            TimelineRevision(fixture["initial"]["revision"].as_u64().unwrap()),
        )
        .unwrap();
        core.request(Command::from(command)).unwrap();
        let frozen = core.export_history().unwrap();
        assert_eq!(
            frozen.cursor.undo.len(),
            1,
            "the unchanged edit fixture reached Core"
        );
        let path = temp.path("valid-new-root.kinewright");
        let sidecar = kinewright_project::sidecar_path_for_project(Some(&path)).unwrap();
        fs::write(&sidecar, b"unchanged sidecar bytes").unwrap();
        let (result, trace) = io_observer::observe(|| {
            save_headless_bundle(&path, BundleWriteAdmission::new_document(), &core, None)
        });
        assert!(matches!(result, Err(BundleError::DurabilityUnsupported)));
        assert!(trace.events.is_empty());
        assert!(!path.exists());
        assert_eq!(fs::read_dir(temp.root()).unwrap().count(), 1);
        assert_eq!(fs::read(&sidecar).unwrap(), b"unchanged sidecar bytes");
        assert_eq!(core.export_history().unwrap(), frozen);
        assert_eq!(
            fs::read(fixtures.join(format!("{name}.json"))).unwrap(),
            original
        );
    }
}

#[cfg(target_os = "linux")]
#[path = "support/ja_s1_fix_r1.rs"]
mod final_head;
#[cfg(target_os = "linux")]
#[path = "support/ja_s1_retention.rs"]
mod retention;
#[cfg(target_os = "linux")]
#[path = "support/ja_s1_fix_r3.rs"]
mod round_three;
#[cfg(target_os = "linux")]
#[path = "support/ja_s1_fix_r2.rs"]
mod round_two;

#[path = "support/ja_s1_fix_r4.rs"]
mod round_four;

#[cfg(target_os = "linux")]
#[path = "support/ja_s1_fix_r5.rs"]
mod round_five;

#[cfg(target_os = "linux")]
#[path = "support/ja_s1_fix_r6.rs"]
mod round_six;
