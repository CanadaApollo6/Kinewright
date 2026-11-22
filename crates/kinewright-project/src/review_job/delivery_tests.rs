// JA-1 S6 C2: save a sealed cut. Included from `review_job/tests.rs`, so the
// seal fixtures above are in scope. Linux only, like the module.

use super::delivery::{
    HASH_MISMATCH, INSIDE_PROJECT, NAME_TAKEN, NO_SEALED_RECEIPT, OTHER_RECEIPT, RECEIPT_FILE,
    SAVE_STOPPED, SaveDestination, SealedCut, admit_save_folder, outcome_label,
    receipt_for_profile, save_action_label, sealed_cut_name, video_file,
};

/// A sealed (facts-not-checked) version in a saved bundle, and an empty
/// destination folder outside it.
struct S6Sealed {
    temp: kinewright_media::test_support::TempDirectory,
    bundle: ProjectBundle,
    receipt: ReviewReceipt,
    out: std::path::PathBuf,
}

fn s6_sealed(tag: &str) -> S6Sealed {
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    let temp = TempDirectory::new_in(&workspace_target_dir(), tag);
    let (_core, mut bundle, _) = saved_bundle(&temp);
    let ready = c7_checked(&mut bundle, &temp, false, None);
    let SealResult::Sealed(sealed) = c7_seal(&mut bundle, &ready).unwrap() else {
        panic!("S6_C2_FIXTURE_SEALED");
    };
    let out = temp.path("out");
    std::fs::create_dir(&out).unwrap();
    S6Sealed {
        temp,
        bundle,
        receipt: sealed.receipt,
        out,
    }
}

impl S6Sealed {
    /// The bundle file and its `.kinewright-assets` store.
    fn refused_roots(&self) -> (std::path::PathBuf, std::path::PathBuf) {
        let roots = self.bundle.owned_roots().unwrap();
        assert_eq!(roots[0], self.temp.path("project.kinewright"));
        (roots[0].clone(), roots[1].clone())
    }

    fn destination(&self, folder: &std::path::Path) -> Result<SaveDestination, BundleError> {
        let roots = self.bundle.owned_roots().unwrap();
        let roots: Vec<&std::path::Path> = roots.iter().map(std::path::PathBuf::as_path).collect();
        admit_save_folder(folder, &roots)
    }

    fn pin(&self) -> SealedCut {
        self.bundle
            .pin_sealed_cut(
                &self.receipt.key.snapshot,
                CoverageProfileId::OutputReviewV1,
                &self.receipt.key_sha256,
            )
            .unwrap()
    }

    fn name(&self) -> String {
        sealed_cut_name(
            "project",
            &self.receipt.key.snapshot.version,
            &self.receipt.outcome,
        )
    }

    fn save_with(
        &self,
        cancel: &ExportCancellation,
        observe: &mut dyn FnMut(u64, &mut [u8]) -> std::io::Result<()>,
    ) -> Result<std::path::PathBuf, BundleError> {
        let destination = self.destination(&self.out).unwrap();
        self.pin().save(&destination, &self.name(), cancel, observe)
    }

    fn entries(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.out)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }
}

fn s6_identity(error: &BundleError) -> &str {
    match error {
        BundleError::Identity(message) => message,
        other => panic!("S6_C2_EXPECTED_REFUSAL: {other:?}"),
    }
}

/// A save observer that changes nothing.
fn s6_no_op() -> impl FnMut(u64, &mut [u8]) -> std::io::Result<()> {
    |_, _| Ok(())
}

/// The artifact bytes the seal checked, read through the store.
fn s6_sealed_bytes(fixture: &S6Sealed) -> Vec<u8> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    fixture
        .bundle
        .store
        .open_verified(&fixture.receipt.key.media_ref)
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}

#[test]
fn ja1_s6_c2_saved_video_hashes_to_the_receipt_and_the_sheet_names_it() {
    let fixture = s6_sealed("s6c2-save");
    let path = fixture
        .save_with(&ExportCancellation::default(), &mut s6_no_op())
        .unwrap();
    let name = fixture.name();
    assert_eq!(path, fixture.out.canonicalize().unwrap().join(&name));
    assert_eq!(
        fixture.entries(),
        vec![name.clone()],
        "S6_C2_ONLY_THE_FOLDER"
    );
    let video = std::fs::read(path.join(video_file(&name))).unwrap();
    let media = &fixture.receipt.key.media_ref;
    assert_eq!(video, s6_sealed_bytes(&fixture), "S6_C2_EXACT_SEALED_BYTES");
    assert_eq!(kinewright_media::sha256_bytes(&video), media.sha256);
    assert_eq!(video.len() as u64, media.byte_length);
    let sheet = std::fs::read_to_string(path.join(RECEIPT_FILE)).unwrap();
    for line in [
        "Label: Reviewed, facts not checked".to_owned(),
        format!("Version: {}", fixture.receipt.key.snapshot.version.as_str()),
        "Coverage profile: output_review/1".to_owned(),
        format!("Video SHA-256: {}", media.sha256),
        format!("Receipt key SHA-256: {}", fixture.receipt.key_sha256),
        "  Loudness target: none".to_owned(),
        "Not checked:".to_owned(),
    ] {
        assert!(
            sheet.lines().any(|l| l == line),
            "S6_C2_SHEET {line}: {sheet}"
        );
    }
    assert!(
        !sheet.to_lowercase().contains("delivered"),
        "S6_C2_FACTS_NOT_CHECKED_NEVER_DELIVERED: {sheet}"
    );
    let mut dirs = std::fs::read_dir(&path)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    dirs.sort();
    assert_eq!(dirs, vec![video_file(&name), RECEIPT_FILE.to_owned()]);
}

#[test]
fn ja1_s6_c2_both_labels_name_the_folder_and_the_action() {
    use kinewright_core::output_review::{DeliveryException, OverlayKind, SealedReviewOutcome};
    let version = crate::studio_records::VersionId::fresh().unwrap();
    let delivered = SealedReviewOutcome::Delivered;
    let facts = SealedReviewOutcome::ReviewedFactsNotChecked {
        exceptions: vec![DeliveryException::OverlayInventoryNotDetermined(
            OverlayKind::Title,
        )],
    };
    assert_eq!(
        sealed_cut_name("My talk", &version, &delivered),
        format!("My-talk-{}-delivered", version.as_str())
    );
    let facts_name = sealed_cut_name("My talk", &version, &facts);
    assert_eq!(
        facts_name,
        format!("My-talk-{}-reviewed-facts-not-checked", version.as_str())
    );
    for words in [
        facts_name.as_str(),
        save_action_label(&facts),
        outcome_label(&facts),
        video_file(&facts_name).as_str(),
    ] {
        assert!(
            !words.to_lowercase().contains("deliver"),
            "S6_C2_FACTS_NEVER_DELIVERED: {words}"
        );
    }
    assert_eq!(save_action_label(&delivered), "Save delivered cut…");
    assert_eq!(
        save_action_label(&facts),
        "Save reviewed cut (facts not checked)…"
    );
    // A hidden or path-shaped project name never escapes one component.
    let hidden = sealed_cut_name("../.secret/x", &version, &delivered);
    assert!(
        !hidden.starts_with('.') && !hidden.contains('/'),
        "{hidden}"
    );
}

#[test]
fn ja1_s6_c2_an_existing_name_is_refused_and_left_byte_identical() {
    let fixture = s6_sealed("s6c2-exists");
    let name = fixture.name();
    let existing = fixture.out.join(&name);
    std::fs::create_dir(&existing).unwrap();
    std::fs::write(existing.join("mine.txt"), b"the person's own file").unwrap();
    let error = fixture
        .save_with(&ExportCancellation::default(), &mut s6_no_op())
        .unwrap_err();
    assert_eq!(s6_identity(&error), NAME_TAKEN, "S6_C2_NAME_TAKEN");
    assert_eq!(fixture.entries(), vec![name.clone()]);
    assert_eq!(
        std::fs::read(existing.join("mine.txt")).unwrap(),
        b"the person's own file"
    );
    assert_eq!(std::fs::read_dir(&existing).unwrap().count(), 1);

    // The name appears while the copy runs: the no-replace rename refuses,
    // the person's folder stays as it was, and the staging folder goes.
    std::fs::remove_dir_all(&existing).unwrap();
    let racing = existing.clone();
    let error = fixture
        .save_with(&ExportCancellation::default(), &mut |_, _| {
            if !racing.exists() {
                std::fs::create_dir(&racing).unwrap();
                std::fs::write(racing.join("late.txt"), b"late").unwrap();
            }
            Ok(())
        })
        .unwrap_err();
    assert_eq!(s6_identity(&error), NAME_TAKEN, "S6_C2_RENAME_NO_REPLACE");
    assert_eq!(fixture.entries(), vec![name], "S6_C2_STAGING_REMOVED");
    assert_eq!(std::fs::read(existing.join("late.txt")).unwrap(), b"late");
    assert_eq!(std::fs::read_dir(&existing).unwrap().count(), 1);
}

#[test]
fn ja1_s6_c2_a_folder_inside_the_bundle_or_the_store_is_refused() {
    let fixture = s6_sealed("s6c2-inside");
    let (bundle_file, store) = fixture.refused_roots();
    let nested = store.join("studio").join("objects");
    assert!(
        nested.is_dir(),
        "S6_C2_FIXTURE_OBJECTS: {}",
        nested.display()
    );
    let link = fixture.temp.path("link-to-store");
    std::os::unix::fs::symlink(&nested, &link).unwrap();
    for folder in [store.clone(), nested.clone(), link] {
        let Err(error) = fixture.destination(&folder) else {
            panic!("S6_C2_INSIDE_ADMITTED: {}", folder.display());
        };
        assert_eq!(
            s6_identity(&error),
            INSIDE_PROJECT,
            "S6_C2_INSIDE {}",
            folder.display()
        );
    }
    // The bundle is one file: it is never a folder to save into.
    assert!(matches!(
        fixture.destination(&bundle_file),
        Err(BundleError::Io(io)) if io.kind() == std::io::ErrorKind::NotADirectory
    ));
    // Beside the bundle is fine.
    assert!(fixture.destination(&fixture.out).is_ok());
}

#[test]
fn ja1_s6_c2_a_forced_hash_mismatch_leaves_nothing() {
    let fixture = s6_sealed("s6c2-mismatch");
    let error = fixture
        .save_with(&ExportCancellation::default(), &mut |_, chunk| {
            chunk[0] ^= 0xff;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(s6_identity(&error), HASH_MISMATCH, "S6_C2_MISMATCH");
    assert!(
        fixture.entries().is_empty(),
        "S6_C2_MISMATCH_LEAVES_NOTHING"
    );
}

#[test]
fn ja1_s6_c2_storage_exhausted_fails_cleanly_and_keeps_the_project() {
    let fixture = s6_sealed("s6c2-full");
    let key = fixture.receipt.key.snapshot.clone();
    let before = c7_receipts(&fixture.bundle, &key);
    let versions_before = fixture.bundle.version_index().unwrap().snapshots.len();
    let error = fixture
        .save_with(&ExportCancellation::default(), &mut |_, _| {
            Err(std::io::ErrorKind::StorageFull.into())
        })
        .unwrap_err();
    assert!(
        matches!(&error, BundleError::Io(io) if io.kind() == std::io::ErrorKind::StorageFull),
        "S6_C2_FULL: {error:?}"
    );
    assert!(fixture.entries().is_empty(), "S6_C2_FULL_LEAVES_NOTHING");
    // The project, its versions and receipts are untouched, before and
    // after a reopen.
    assert!(!fixture.bundle.review_root_unsettled());
    assert_eq!(
        fixture.bundle.version_index().unwrap().snapshots.len(),
        versions_before
    );
    let after = c7_receipts(&fixture.bundle, &key);
    assert!(after.len() == 1 && c7_same(&after[0], &before[0]));
    let S6Sealed {
        temp,
        bundle,
        receipt,
        out,
    } = fixture;
    let path = temp.path("project.kinewright");
    drop(bundle);
    let core = Core::for_test(Document::default()).unwrap();
    let bundle = ProjectBundle::activate(
        &path,
        BundleWriteAdmission::read(&path).unwrap(),
        core.history_pool(),
    )
    .unwrap();
    let reopened = c7_receipts(&bundle, &key);
    assert!(reopened.len() == 1 && c7_same(&reopened[0], &before[0]));
    // And a later save of the same cut succeeds.
    let fixture = S6Sealed {
        temp,
        bundle,
        receipt,
        out,
    };
    fixture
        .save_with(&ExportCancellation::default(), &mut s6_no_op())
        .unwrap();
}

#[test]
fn ja1_s6_c2_a_stopped_save_leaves_nothing() {
    let fixture = s6_sealed("s6c2-stop");
    let cancel = ExportCancellation::default();
    let error = fixture
        .save_with(&cancel, &mut |_, _| {
            cancel.cancel();
            Ok(())
        })
        .unwrap_err();
    assert_eq!(s6_identity(&error), SAVE_STOPPED, "S6_C2_STOPPED");
    assert!(fixture.entries().is_empty(), "S6_C2_STOP_LEAVES_NOTHING");
}

#[test]
fn ja1_s6_c2_no_receipt_or_another_key_is_refused() {
    let mut fixture = s6_sealed("s6c2-refuse");
    let unsealed = version(&mut fixture.bundle);
    let Err(error) = fixture.bundle.pin_sealed_cut(
        &unsealed.key,
        CoverageProfileId::OutputReviewV1,
        &fixture.receipt.key_sha256,
    ) else {
        panic!("S6_C2_UNSEALED_PINNED");
    };
    assert_eq!(s6_identity(&error), NO_SEALED_RECEIPT, "S6_C2_NO_RECEIPT");
    let Err(error) = fixture.bundle.pin_sealed_cut(
        &fixture.receipt.key.snapshot,
        CoverageProfileId::OutputReviewV1,
        &"0".repeat(64),
    ) else {
        panic!("S6_C2_OTHER_KEY_PINNED");
    };
    assert_eq!(s6_identity(&error), OTHER_RECEIPT, "S6_C2_OTHER_KEY");
    assert!(fixture.entries().is_empty());
}

#[test]
fn ja1_s6_c2_the_receipt_is_selected_by_coverage_profile() {
    let fixture = s6_sealed("s6c2-profile");
    let receipts = c7_receipts(&fixture.bundle, &fixture.receipt.key.snapshot);
    let chosen = receipt_for_profile(receipts, CoverageProfileId::OutputReviewV1).unwrap();
    assert!(c7_same(&chosen, &fixture.receipt));
    assert!(receipt_for_profile(Vec::new(), CoverageProfileId::OutputReviewV1).is_none());
    assert_eq!(
        fixture.pin().receipt.key.profile,
        CoverageProfileId::OutputReviewV1
    );
}

const S6_KILL_ROOT: &str = "KINEWRIGHT_S6C2_KILL_ROOT";

/// The kill child: seal, then stall inside the copy until killed.
fn s6_kill_child(root: &std::path::Path) {
    let fixture = s6_sealed("s6c2-kill-child");
    std::fs::write(
        root.join("bundle"),
        fixture.temp.path("project.kinewright").to_str().unwrap(),
    )
    .unwrap();
    std::fs::write(root.join("out"), fixture.out.to_str().unwrap()).unwrap();
    let marker = root.join("copying");
    let _ = fixture.save_with(&ExportCancellation::default(), &mut |_, _| {
        std::fs::write(&marker, b"").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(600));
        Ok(())
    });
    // The child keeps its temp folder: it is killed before this returns.
    std::mem::forget(fixture);
}

struct S6Reaped(std::process::Child);
impl Drop for S6Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// After the kill: the project reopens with its receipt, and a new save of
/// it succeeds beside the stale partial folder.
fn s6_kill_reopen(bundle: &std::path::Path, out: &std::path::Path) {
    let core = Core::for_test(Document::default()).unwrap();
    let reopened = ProjectBundle::activate(
        bundle,
        BundleWriteAdmission::read(bundle).unwrap(),
        core.history_pool(),
    )
    .unwrap();
    let descriptor_receipts = reopened
        .version_index()
        .unwrap()
        .snapshots
        .values()
        .flat_map(|descriptor| reopened.review_receipts(descriptor).unwrap())
        .map(|(_, receipt)| receipt.take())
        .collect::<Vec<_>>();
    assert_eq!(descriptor_receipts.len(), 1, "S6_C2_RECEIPT_KEPT");
    let receipt = &descriptor_receipts[0];
    let cut = reopened
        .pin_sealed_cut(
            &receipt.key.snapshot,
            CoverageProfileId::OutputReviewV1,
            &receipt.key_sha256,
        )
        .unwrap();
    let roots = reopened.owned_roots().unwrap();
    let roots: Vec<&std::path::Path> = roots.iter().map(std::path::PathBuf::as_path).collect();
    let destination = admit_save_folder(out, &roots).unwrap();
    let name = sealed_cut_name("project", &receipt.key.snapshot.version, &receipt.outcome);
    cut.save(
        &destination,
        &name,
        &ExportCancellation::default(),
        &mut s6_no_op(),
    )
    .unwrap();
    assert!(out.join(&name).join(video_file(&name)).is_file());
    // The child's own temp folder holds the bundle; remove it with ours.
    drop(cut);
    drop(reopened);
    let _ = std::fs::remove_dir_all(out.parent().unwrap());
}

/// Kill the saving process mid-copy: only a `.partial` name remains, never
/// the final name, and the project reopens with its receipt.
#[test]
fn ja1_s6_c2_a_save_killed_mid_copy_leaves_only_a_partial_name() {
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    if let Some(root) = std::env::var_os(S6_KILL_ROOT) {
        s6_kill_child(std::path::Path::new(&root));
        return;
    }
    let temp = TempDirectory::new_in(&workspace_target_dir(), "s6c2-kill");
    let mut child = S6Reaped(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "review_job::tests::ja1_s6_c2_a_save_killed_mid_copy_leaves_only_a_partial_name",
                "--test-threads=1",
            ])
            .env(S6_KILL_ROOT, temp.path(""))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let marker = temp.path("copying");
    let started = std::time::Instant::now();
    while !marker.exists() {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(120),
            "S6_C2_CHILD_NEVER_COPIED"
        );
        assert!(child.0.try_wait().unwrap().is_none(), "S6_C2_CHILD_EXITED");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let out = std::path::PathBuf::from(std::fs::read_to_string(temp.path("out")).unwrap());
    let bundle = std::path::PathBuf::from(std::fs::read_to_string(temp.path("bundle")).unwrap());
    let entries: Vec<String> = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(entries.len(), 1, "S6_C2_ONE_STAGING_FOLDER: {entries:?}");
    assert!(
        entries[0].starts_with('.') && entries[0].contains(".partial-"),
        "S6_C2_ONLY_PARTIAL: {entries:?}"
    );
    s6_kill_reopen(&bundle, &out);
}
