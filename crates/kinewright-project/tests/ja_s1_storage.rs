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
