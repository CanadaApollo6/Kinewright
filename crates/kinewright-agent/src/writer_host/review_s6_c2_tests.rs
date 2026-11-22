// JA-1 S6 C2: save a sealed cut through `/host`. The copy runs on its own
// worker: `/host` keeps answering while it runs, and a shutdown stops it with
// nothing under the final name. Linux only, like the C7 fixtures.

/// A per-chunk hook that stalls the first chunk until released or until the
/// save is cancelled, with no channel or timed receive (PITFALLS).
#[cfg(target_os = "linux")]
#[derive(Clone, Default)]
struct S6Stall {
    entered: std::sync::Arc<std::sync::atomic::AtomicBool>,
    release: std::sync::Arc<std::sync::atomic::AtomicBool>,
    cancel: std::sync::Arc<std::sync::Mutex<Option<kinewright_core::ExportCancellation>>>,
}

#[cfg(target_os = "linux")]
impl S6Stall {
    fn install(&self, f: &Fixture) {
        let stall = self.clone();
        let hook: super::super::review_job::sealed_save::SaveHook =
            std::sync::Arc::new(move |_, _| {
                use std::sync::atomic::Ordering;
                stall.entered.store(true, Ordering::SeqCst);
                let started = std::time::Instant::now();
                while !stall.release.load(Ordering::SeqCst)
                    && !stall
                        .cancel
                        .lock()
                        .unwrap()
                        .as_ref()
                        .is_some_and(kinewright_core::ExportCancellation::is_cancelled)
                {
                    assert!(
                        started.elapsed() < std::time::Duration::from_secs(120),
                        "S6_C2_STALL_NEVER_RELEASED"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Ok(())
            });
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .set_sealed_save_hook(Some(hook));
    }

    /// Wait until the worker is inside the copy, then hand the hook the
    /// save's cancellation.
    fn wait_entered(&self, f: &Fixture) {
        let started = std::time::Instant::now();
        while !self.entered.load(std::sync::atomic::Ordering::SeqCst) {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(60),
                "S6_C2_SAVE_NEVER_COPIED"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let cancel = f
            .host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .sealed_save_cancellation();
        *self.cancel.lock().unwrap() = cancel;
    }

    fn release(&self) {
        self.release
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Seal `text` through the C7 driver and return the job and its receipt.
#[cfg(target_os = "linux")]
fn s6_sealed_job(text: &str) -> (C7Job, kinewright_project::review_job::ReviewReceipt) {
    use kinewright_project::review_job::SealResult;
    let job = c7_job(c7_title_doc(text));
    let mut reviewer = C7Reviewer(|_: &ReviewToken| {
        c7_judge(&job.reviewer);
        Ok(())
    });
    let cancel = kinewright_core::ExportCancellation::default();
    let SealResult::Sealed(sealed) =
        c7_drive(&job, &c7_real_check, &mut reviewer, &cancel).expect("S6_C2_DRIVER_SEALS")
    else {
        panic!("S6_C2_SEALED: {:?}", c7_job_phase(&job));
    };
    (job, sealed.receipt)
}

#[cfg(target_os = "linux")]
fn s6_save_command(
    receipt: &kinewright_project::review_job::ReviewReceipt,
    folder: &std::path::Path,
) -> HostCommand {
    HostCommand::Review(ReviewControl::SaveSealed {
        snapshot: receipt.key.snapshot.clone(),
        coverage: receipt.key.profile,
        key_sha256: receipt.key_sha256.clone(),
        folder: folder.to_path_buf(),
    })
}

#[cfg(target_os = "linux")]
fn s6_refusal(reply: &HostReply) -> &str {
    &reply
        .error
        .as_ref()
        .unwrap_or_else(|| panic!("S6_C2_EXPECTED_REFUSAL: {reply:?}"))
        .message
}

#[cfg(target_os = "linux")]
fn s6_save_status(
    f: &Fixture,
) -> Option<kinewright_project::review_job::delivery::SealedSaveStatus> {
    ok(
        f.editor.request(HostCommand::Status).unwrap(),
        "S6_C2_STATUS",
    )
    .status
    .sealed_save
}

/// Poll Status until the save leaves `Saving`.
#[cfg(target_os = "linux")]
fn s6_finished(f: &Fixture) -> kinewright_project::review_job::delivery::SealedSaveStatus {
    use kinewright_project::review_job::delivery::SealedSaveStatus;
    let started = std::time::Instant::now();
    loop {
        match s6_save_status(f) {
            Some(SealedSaveStatus::Saving { .. }) => {}
            Some(done) => return done,
            None => panic!("S6_C2_STATUS_LOST"),
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(120),
            "S6_C2_SAVE_NEVER_FINISHED"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(target_os = "linux")]
fn s6_entries(folder: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s6_c2_host_saves_a_delivered_cut_while_host_requests_answer() {
    use kinewright_core::output_review::SealedReviewOutcome;
    use kinewright_project::review_job::delivery::{
        RECEIPT_FILE, SealedSaveStatus, sealed_cut_name, video_file,
    };
    let (job, receipt) = s6_sealed_job("Hello world");
    assert_eq!(receipt.outcome, SealedReviewOutcome::Delivered);
    let out = job.f.temp.path("out");
    std::fs::create_dir(&out).unwrap();
    let stall = S6Stall::default();
    stall.install(&job.f);

    let reply = ok(
        job.f
            .editor
            .request(s6_save_command(&receipt, &out))
            .unwrap(),
        "S6_C2_SAVE_STARTED",
    );
    drop(reply);
    stall.wait_entered(&job.f);
    let name = sealed_cut_name("project", &receipt.key.snapshot.version, &receipt.outcome);
    assert!(name.ends_with("-delivered"), "{name}");

    // The copy is stalled inside the worker: the writer lock is free, and
    // Status, an edit and a second save are all answered at once.
    assert!(
        job.f.host.owner.upgrade().unwrap().try_lock().is_ok(),
        "S6_C2_WORKER_HOLDS_NO_LOCK"
    );
    assert_eq!(
        s6_save_status(&job.f),
        Some(SealedSaveStatus::Saving { name: name.clone() }),
        "S6_C2_STATUS_WHILE_SAVING"
    );
    submitted_batch(&job.f);
    let second = job
        .f
        .editor
        .request(s6_save_command(&receipt, &out))
        .unwrap();
    assert_eq!(
        s6_refusal(&second),
        super::super::review_job::sealed_save::SAVE_RUNNING,
        "S6_C2_ONE_SAVE_AT_A_TIME"
    );
    assert!(
        s6_entries(&out).iter().all(|entry| entry != &name),
        "S6_C2_NO_FINAL_NAME_BEFORE_THE_HASH"
    );

    stall.release();
    let SealedSaveStatus::Saved { path } = s6_finished(&job.f) else {
        panic!("S6_C2_SAVED: {:?}", s6_save_status(&job.f));
    };
    assert_eq!(path, out.canonicalize().unwrap().join(&name));
    assert_eq!(s6_entries(&out), vec![name.clone()], "S6_C2_STAGING_GONE");
    let video = std::fs::read(path.join(video_file(&name))).unwrap();
    assert_eq!(
        kinewright_media::sha256_bytes(&video),
        receipt.key.media_ref.sha256,
        "S6_C2_SAVED_BYTES_ARE_SEALED"
    );
    let sheet = std::fs::read_to_string(path.join(RECEIPT_FILE)).unwrap();
    let target = receipt
        .key
        .settings
        .loudness_normalization
        .as_ref()
        .expect("S6_C2_DEFAULT_QUEUE_HAS_A_TARGET");
    for line in [
        "Label: Delivered".to_owned(),
        format!("Version: {}", receipt.key.snapshot.version.as_str()),
        "Not checked: nothing".to_owned(),
    ] {
        assert!(
            sheet.lines().any(|l| l == line),
            "S6_C2_SHEET {line}: {sheet}"
        );
    }
    let lufs = target.integrated_lufs_hundredths;
    assert!(
        sheet.lines().any(|l| l.starts_with(&format!(
            "  Loudness target: {}{}.{:02} LUFS integrated",
            if lufs < 0 { "-" } else { "" },
            lufs.unsigned_abs() / 100,
            lufs.unsigned_abs() % 100
        ))),
        "S6_C2_SHEET_NAMES_THE_TARGET: {sheet}"
    );
    // The working edit and the receipts are untouched.
    assert_eq!(c7_receipts(&job).len(), 1, "S6_C2_RECEIPTS_KEPT");

    // The same name again is refused before any worker starts.
    let again = job
        .f
        .editor
        .request(s6_save_command(&receipt, &out))
        .unwrap();
    assert_eq!(
        s6_refusal(&again),
        kinewright_project::review_job::delivery::NAME_TAKEN,
        "S6_C2_NAME_TAKEN_AT_ADMISSION"
    );
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s6_c2_host_names_a_facts_not_checked_cut_and_refuses_other_keys() {
    use kinewright_core::output_review::{CoverageProfileId, SealedReviewOutcome};
    use kinewright_project::review_job::delivery::{
        INSIDE_PROJECT, OTHER_RECEIPT, SealedSaveStatus, sealed_cut_name,
    };
    let (job, receipt) = s6_sealed_job("Step 1");
    assert!(matches!(
        receipt.outcome,
        SealedReviewOutcome::ReviewedFactsNotChecked { .. }
    ));
    // ReadEditorial selects the receipt by coverage profile.
    let read = ok(
        job.f
            .editor
            .request(HostCommand::ReadEditorial {
                version: Some(receipt.key.snapshot.clone()),
            })
            .unwrap(),
        "S6_C2_READ_EDITORIAL",
    )
    .review_receipt
    .expect("S6_C2_RECEIPT_READ");
    assert_eq!(read.key.profile, CoverageProfileId::OutputReviewV1);
    assert_eq!(read.key_sha256, receipt.key_sha256);

    let out = job.f.temp.path("out");
    std::fs::create_dir(&out).unwrap();
    // Another key digest, and a folder inside the project's store.
    let mut other = receipt.clone();
    other.key_sha256 = "0".repeat(64);
    let reply = job.f.editor.request(s6_save_command(&other, &out)).unwrap();
    assert_eq!(
        s6_refusal(&reply),
        format!("identity_refused: {OTHER_RECEIPT}"),
        "S6_C2_HOST_OTHER_KEY"
    );
    let assets = job.f.temp.path("project.kinewright-assets");
    assert!(assets.is_dir(), "S6_C2_FIXTURE_ASSETS");
    let reply = job
        .f
        .editor
        .request(s6_save_command(&receipt, &assets))
        .unwrap();
    assert_eq!(
        s6_refusal(&reply),
        format!("identity_refused: {INSIDE_PROJECT}"),
        "S6_C2_HOST_INSIDE_STORE"
    );
    assert!(
        s6_save_status(&job.f).is_none(),
        "S6_C2_REFUSED_STARTS_NOTHING"
    );

    ok(
        job.f
            .editor
            .request(s6_save_command(&receipt, &out))
            .unwrap(),
        "S6_C2_FACTS_SAVE",
    );
    let SealedSaveStatus::Saved { path } = s6_finished(&job.f) else {
        panic!("S6_C2_FACTS_SAVED: {:?}", s6_save_status(&job.f));
    };
    let name = sealed_cut_name("project", &receipt.key.snapshot.version, &receipt.outcome);
    assert!(name.ends_with("-reviewed-facts-not-checked"), "{name}");
    assert!(
        !name.contains("deliver"),
        "S6_C2_FACTS_NEVER_DELIVERED: {name}"
    );
    assert_eq!(path.file_name().unwrap().to_str(), Some(name.as_str()));
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s6_c2_host_shutdown_mid_save_leaves_no_final_name() {
    use kinewright_project::review_job::delivery::sealed_cut_name;
    let (job, receipt) = s6_sealed_job("Hello world");
    let out = job.f.temp.path("out");
    std::fs::create_dir(&out).unwrap();
    let stall = S6Stall::default();
    stall.install(&job.f);
    ok(
        job.f
            .editor
            .request(s6_save_command(&receipt, &out))
            .unwrap(),
        "S6_C2_SAVE_STARTED",
    );
    stall.wait_entered(&job.f);
    let name = sealed_cut_name("project", &receipt.key.snapshot.version, &receipt.outcome);
    // Dropping the host fences, cancels the save and joins its worker.
    let C7Job { f, .. } = job;
    let Fixture { host, temp, .. } = f;
    let started = std::time::Instant::now();
    host.shutdown().unwrap();
    // The shutdown cancelled the copy: it did not wait out the stall.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "S6_C2_SHUTDOWN_CANCELS_THE_SAVE"
    );
    let entries = s6_entries(&out);
    assert!(
        entries.is_empty(),
        "S6_C2_SHUTDOWN_LEAVES_NOTHING ({name}): {entries:?}"
    );
    drop(temp);
}
