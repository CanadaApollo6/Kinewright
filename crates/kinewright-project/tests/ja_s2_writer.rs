//! C7 real legacy/Studio participants and deterministic writer identity races.
#![cfg(feature = "test-util")]
use kinewright_core::{Document, TimelineRevision};
use kinewright_media::test_support::TempDirectory;
use kinewright_project::{
    LockMode, LockfileError, SidecarMode, SidecarSession, acquire_project_lock_with_policy,
    bundle::BundleWriteAdmission, io_observer, load_document, save_headless,
    serialize_project_document,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

fn marked_document(mark: u64) -> Document {
    Document {
        tracks: vec![kinewright_core::Track {
            id: kinewright_core::TrackId(mark),
            kind: kinewright_core::TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    }
}

#[cfg(target_os = "linux")]
#[test]
fn g04_studio_associations_preserve_the_persons_path() {
    use kinewright_project::{editing_session::Ja1EditingSession, storage_budget::StorageLimits};
    let cwd = std::env::current_dir().unwrap();
    let base = cwd.join("target/c7-person-path-tests");
    fs::create_dir_all(&base).unwrap();
    let temp = TempDirectory::new_in(&base, "c7-studio-person-path");
    let relative = temp.root().strip_prefix(&cwd).unwrap();
    let first = relative.join("first.kinewright");
    let second = relative.join("second.kinewright");
    let core = kinewright_core::Core::spawn(Document::default()).unwrap();
    let mut owner = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
    owner.first_save(&first, StorageLimits::default()).unwrap();
    assert_eq!(
        owner.association().unwrap().path(),
        first,
        "first Save keeps the supplied spelling"
    );
    owner.save_as(&second, StorageLimits::default()).unwrap();
    assert_eq!(
        owner.association().unwrap().path(),
        second,
        "Save As keeps the supplied spelling"
    );
    drop(owner);
    let reopened = Ja1EditingSession::open(&second).unwrap();
    assert_eq!(
        reopened.association().unwrap().path(),
        second,
        "Studio open keeps the supplied spelling"
    );
    drop(reopened);
    let legacy = relative.join("legacy.kinewright");
    fs::write(
        &legacy,
        serialize_project_document(&Document::default()).unwrap(),
    )
    .unwrap();
    let core = kinewright_core::Core::spawn(Document::default()).unwrap();
    let admission = BundleWriteAdmission::read(&legacy).unwrap();
    let activated = Ja1EditingSession::activate(&legacy, admission, &core, None).unwrap();
    assert_eq!(
        activated.association().unwrap().path(),
        legacy,
        "activation keeps the supplied spelling"
    );
}
fn claim(path: &Path, recovery: &Path) -> Result<kinewright_project::AcquiredLock, LockfileError> {
    acquire_project_lock_with_policy(
        path,
        LockMode::Headless,
        "http://127.0.0.1:9/mcp",
        recovery,
        1,
        Duration::ZERO,
    )
}
fn sidecar(path: &Path) -> SidecarSession {
    SidecarSession::load(
        &SidecarMode::Load {
            project_digest: kinewright_project::digest_bytes(&fs::read(path).unwrap()),
        },
        Some(path),
        Arc::new(RwLock::new(kinewright_core::IncidentLog::with_start(
            Instant::now(),
            None,
        ))),
        TimelineRevision::default(),
        None,
        None,
    )
    .session
}
fn wait(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "participant did not reach {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
struct Participant {
    child: Child,
    signals: PathBuf,
}
impl Participant {
    fn start(path: &Path, recovery: &Path, signals: &Path, mode: &str, hook: Option<&str>) -> Self {
        fs::create_dir(signals).unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "g04_child_participant", "--nocapture"])
            .env("C7_CHILD_MODE", mode)
            .env("C7_PROJECT", path)
            .env("C7_RECOVERY", recovery)
            .env("C7_SIGNALS", signals)
            .env_remove("REV2_HOOK")
            .env_remove("REV2_EXIT")
            .stdout(Stdio::null())
            .stderr(fs::File::create(signals.join("stderr.log")).unwrap());
        if let Some(hook) = hook {
            command.env("REV2_HOOK", hook).env("REV2_SIGNALS", signals);
        }
        Self {
            child: command.spawn().unwrap(),
            signals: signals.into(),
        }
    }
    fn finish(mut self) {
        fs::write(self.signals.join("release"), b"").unwrap();
        let status = self.child.wait().unwrap();
        assert!(
            status.success(),
            "{status}: {}",
            fs::read_to_string(self.signals.join("stderr.log")).unwrap_or_default()
        );
    }
}
impl Drop for Participant {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Subprocess entry, also enumerated as a harness control in the parent binary.
#[test]
fn g04_child_participant() {
    let Some(mode) = std::env::var_os("C7_CHILD_MODE") else {
        return;
    };
    let path = PathBuf::from(std::env::var_os("C7_PROJECT").unwrap());
    let recovery = PathBuf::from(std::env::var_os("C7_RECOVERY").unwrap());
    let signals = PathBuf::from(std::env::var_os("C7_SIGNALS").unwrap());
    let owner = claim(&path, &recovery).unwrap();
    fs::write(signals.join("owned"), b"").unwrap();
    if mode == "save" {
        let mut sidecar = sidecar(owner.handle.resolved_identity());
        assert!(
            !sidecar.sidecar_suspended,
            "the child must be ready to save"
        );
        assert!(!sidecar.established.is_empty(), "the child loaded its stem");
        assert!(
            sidecar.established.contains(
                &kinewright_project::sidecar_path_for_project(Some(
                    owner.handle.resolved_identity()
                ))
                .unwrap()
            )
        );
        let result = save_headless(
            &Document::default(),
            &BundleWriteAdmission::new_document(),
            &path,
            None,
            &mut sidecar,
            "",
            TimelineRevision::default(),
            &recovery,
            Some(&owner.handle),
        );
        let disposition = match &result {
            Ok(_) => "saved",
            Err(kinewright_project::ProjectSaveError::LockLost { .. }) => "lock_lost",
            Err(_) => "other_refusal",
        };
        // Publish by rename: the parent waits for the name, so it must never see a
        // created-but-unwritten file.
        fs::write(
            signals.join("result.json.partial"),
            serde_json::to_vec(&serde_json::json!({
                "disposition": disposition,
                "result": result.map(|_| "saved").map_err(|e| e.to_string())
            }))
            .unwrap(),
        )
        .unwrap();
        fs::rename(
            signals.join("result.json.partial"),
            signals.join("result.json"),
        )
        .unwrap();
        // A refused alias change is legal; a successful write must still target the admitted namespace.
        drop(sidecar);
    }
    wait(&signals.join("release"));
    owner.handle.release().unwrap();
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one real-process witness with independent legacy and Studio controls"
)]
fn g04_two_process_lock_save_alias_and_discovery() {
    let temp = TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "c7-writer");
    let a = temp.path("a.kinewright");
    let b = temp.path("b.kinewright");
    let initial = serialize_project_document(&Document::default()).unwrap();
    fs::write(&a, &initial).unwrap();
    let b_bytes = serialize_project_document(&marked_document(321)).unwrap();
    fs::write(&b, &b_bytes).unwrap();
    let recovery = temp.path("recovery");
    fs::create_dir(&recovery).unwrap();
    let owner = Participant::start(&a, &recovery, &temp.path("held"), "hold", None);
    wait(&owner.signals.join("owned"));
    assert!(matches!(
        claim(&a, &recovery),
        Err(LockfileError::Contention { .. })
    ));
    let discovery = kinewright_project::discovery_path_for_project(Some(&a)).unwrap();
    fs::remove_file(&discovery).unwrap();
    assert!(
        matches!(claim(&a, &recovery), Err(LockfileError::Contention { .. })),
        "missing discovery never steals a held lock"
    );
    assert_eq!(fs::read(&a).unwrap(), initial.as_bytes());
    assert_eq!(fs::read(&b).unwrap(), b_bytes.as_bytes());
    owner.finish();
    let owner = claim(&a, &recovery).unwrap();
    let mut session = sidecar(&a);
    let report = save_headless(
        &Document::default(),
        &BundleWriteAdmission::new_document(),
        &a,
        None,
        &mut session,
        "",
        TimelineRevision::default(),
        &recovery,
        Some(&owner.handle),
    )
    .unwrap();
    assert_eq!(fs::read(&a).unwrap(), initial.as_bytes());
    assert_eq!(
        report.project.digest,
        kinewright_project::digest_bytes(initial.as_bytes())
    );
    #[cfg(unix)]
    let sidecar_bytes = fs::read(a.with_extension("kinewright-incidents")).unwrap();
    assert!(matches!(
        kinewright_project::load_sidecar(&a.with_extension("kinewright-incidents")),
        kinewright_project::SidecarLoad::Current(_)
    ));
    drop(session);
    owner.handle.release().unwrap();
    #[cfg(unix)]
    {
        let alias = temp.path("current.kinewright");
        std::os::unix::fs::symlink(&a, &alias).unwrap();
        let saver = Participant::start(
            &alias,
            &recovery,
            &temp.path("saver"),
            "save",
            Some("legacy_save_verified"),
        );
        wait(&saver.signals.join("paused"));
        assert!(matches!(
            claim(&a, &recovery),
            Err(LockfileError::Contention { .. })
        ));
        fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&b, &alias).unwrap();
        fs::write(saver.signals.join("resume"), b"").unwrap();
        wait(&saver.signals.join("result.json"));
        let observation: serde_json::Value =
            serde_json::from_slice(&fs::read(saver.signals.join("result.json")).unwrap()).unwrap();
        saver.finish();
        assert!(
            matches!(
                observation["disposition"].as_str(),
                Some("saved" | "lock_lost")
            ),
            "only an identity refusal or an admitted save is legal: {observation}"
        );
        assert_eq!(
            fs::read(&a).unwrap(),
            initial.as_bytes(),
            "original full project is preserved on refusal"
        );
        assert_eq!(
            fs::read(a.with_extension("kinewright-incidents")).unwrap(),
            sidecar_bytes
        );
        assert_eq!(
            fs::read(&b).unwrap(),
            b_bytes.as_bytes(),
            "alias target is never written"
        );
        assert!(!b.with_extension("kinewright-incidents").exists());
        assert!(!b.with_extension("kinewright-assets").exists());
    }
    assert_eq!(load_document(&a).unwrap().0, Document::default());
    #[cfg(target_os = "linux")]
    {
        use kinewright_project::bundle::{LoadedBundle, ProjectBundle};
        let studio_path = temp.path("studio.kinewright");
        let core = kinewright_core::Core::spawn(Document::default()).unwrap();
        let mut studio =
            ProjectBundle::activate(&studio_path, BundleWriteAdmission::new_document()).unwrap();
        let inputs = studio.empty_inputs().unwrap().reference.clone();
        studio.save(&core, &inputs).unwrap();
        let root = fs::read(&studio_path).unwrap();
        assert!(matches!(
            claim(&studio_path, &recovery),
            Err(LockfileError::Contention { .. })
        ));
        assert_eq!(fs::read(&studio_path).unwrap(), root);
        drop(studio);
        let loaded = LoadedBundle::read(&studio_path).unwrap();
        let reopened = kinewright_core::Core::spawn(Document::default()).unwrap();
        loaded.restore_manual(&reopened).unwrap();
        assert_eq!(
            reopened.export_history().unwrap(),
            core.export_history().unwrap()
        );
    }
    #[cfg(windows)]
    {
        assert!(
            kinewright_project::namespace::ProjectNamespace::acquire(
                &temp.path("studio.kinewright")
            )
            .is_err()
        );
    }
}

#[test]
fn g04_suffixed_journal_direct_probe_and_release_identity() {
    let temp = TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "c7-probe");
    let path = temp.path("project.kinewright");
    fs::write(
        &path,
        serialize_project_document(&Document::default()).unwrap(),
    )
    .unwrap();
    let recovery = temp.path("recovery");
    fs::create_dir(&recovery).unwrap();
    let base = kinewright_project::journal_file_name(&path);
    let suffix = recovery.join(format!(
        "{}-4096.journal",
        base.trim_end_matches(".journal")
    ));
    let late = suffix.clone();
    let result = io_observer::at_barrier(
        "journal_after_scan",
        move || {
            fs::write(&late, b"unreplayed exact bytes").unwrap();
        },
        || claim(&path, &recovery),
    );
    assert!(matches!(result, Err(LockfileError::PendingRecovery { journal }) if journal == suffix));
    assert_eq!(fs::read(&suffix).unwrap(), b"unreplayed exact bytes");
    fs::remove_file(&suffix).unwrap();
    let owner = claim(&path, &recovery).unwrap();
    #[cfg(unix)]
    {
        let lock = owner.handle.path.clone();
        let target = path.clone();
        let dir = recovery.clone();
        let successor = Arc::new(std::sync::Mutex::new(None));
        let captured = Arc::clone(&successor);
        io_observer::at_barrier(
            "release_after_read",
            move || {
                fs::remove_file(&lock).unwrap();
                *captured.lock().unwrap() = Some(
                    std::thread::spawn({
                        let target = target.clone();
                        let dir = dir.clone();
                        move || claim(&target, &dir).unwrap()
                    })
                    .join()
                    .unwrap(),
                );
            },
            || owner.handle.release(),
        )
        .unwrap();
        let successor = successor.lock().unwrap().take().unwrap();
        assert_eq!(
            serde_json::from_slice::<kinewright_project::LockfileClaim>(
                &fs::read(&successor.handle.discovery).unwrap()
            )
            .unwrap()
            .claim_id,
            successor.handle.claim.claim_id,
            "release preserves the successor discovery after lock-fd replacement"
        );
        successor.handle.release().unwrap();
    }
    #[cfg(not(unix))]
    {
        let discovery = owner.handle.discovery.clone();
        fs::remove_file(&discovery).unwrap();
        assert!(matches!(
            claim(&path, &recovery),
            Err(LockfileError::Contention { .. })
        ));
        owner.handle.release().unwrap();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn g04_parent_and_asset_directories_remain_pinned() {
    use kinewright_project::project::write_project_bytes_owned;
    let temp = TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "c7-native-pins");
    let parent = temp.path("admitted");
    fs::create_dir(&parent).unwrap();
    let path = parent.join("project.kinewright");
    let old = serialize_project_document(&Document::default()).unwrap();
    fs::write(&path, &old).unwrap();
    let recovery = temp.path("recovery");
    fs::create_dir(&recovery).unwrap();
    let owner = claim(&path, &recovery).unwrap();
    let moved = temp.path("original-parent");
    let planted = serialize_project_document(&marked_document(55)).unwrap();
    let from = parent.clone();
    let to = moved.clone();
    let replacement = planted.clone();
    let document = marked_document(77);
    let bytes = serialize_project_document(&document).unwrap();
    let report = io_observer::at_barrier(
        "legacy_writer_verified",
        move || {
            fs::rename(&from, &to).unwrap();
            fs::create_dir(&from).unwrap();
            fs::write(from.join("project.kinewright"), &replacement).unwrap();
        },
        || write_project_bytes_owned(&bytes, &document, &owner.handle, None),
    )
    .unwrap();
    assert_eq!(report.path, path);
    assert_eq!(
        fs::read(moved.join("project.kinewright")).unwrap(),
        bytes.as_bytes()
    );
    assert_eq!(
        fs::read(&path).unwrap(),
        planted.as_bytes(),
        "replaced caller parent must stay byte-exact"
    );
    assert!(owner.handle.verify().is_err());
    drop(owner);

    let source_path = temp.path("source.kinewright");
    let source = kinewright_media::LutStore::for_project(&source_path).unwrap();
    let lut = temp.path("identity.cube");
    fs::write(
        &lut,
        b"LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n",
    )
    .unwrap();
    let asset = source
        .import_lut_asset(&lut)
        .unwrap()
        .into_lut_asset(kinewright_core::LutAssetId(1));
    let source_bytes = fs::read(source.path_for(&asset.sha256).unwrap()).unwrap();
    let destination = temp.path("copied.kinewright");
    let assets = destination.with_extension("kinewright-assets");
    fs::create_dir_all(assets.join("luts")).unwrap();
    let owner = claim(&destination, &recovery).unwrap();
    let from = assets.join("luts");
    let held = assets.join("admitted-luts");
    let kept = held.clone();
    let target = from.clone();
    let doc = Document {
        lut_assets: vec![asset.clone()],
        ..Document::default()
    };
    let json = serialize_project_document(&doc).unwrap();
    let report = io_observer::at_barrier(
        "legacy_assets_pinned",
        move || {
            fs::rename(&from, &kept).unwrap();
            fs::create_dir(&from).unwrap();
            fs::write(from.join("sentinel"), b"unrelated directory").unwrap();
        },
        || write_project_bytes_owned(&json, &doc, &owner.handle, Some(&source)),
    )
    .unwrap();
    assert!(
        report.lut_store_copy_failed.is_empty(),
        "{:?}",
        report.lut_store_copy_failed
    );
    assert_eq!(
        fs::read(held.join(format!("{}.cube", asset.sha256))).unwrap(),
        source_bytes
    );
    assert_eq!(
        fs::read(target.join("sentinel")).unwrap(),
        b"unrelated directory"
    );
    assert!(!target.join(format!("{}.cube", asset.sha256)).exists());
    assert_eq!(load_document(&destination).unwrap().0, doc);
}

#[test]
fn g04_sidecar_refuses_lost_authority_and_preserves_pending_journal() {
    let temp = TempDirectory::new_in(
        Path::new(env!("CARGO_TARGET_TMPDIR")),
        "c7-sidecar-authority",
    );
    let path = temp.path("project.kinewright");
    fs::write(
        &path,
        serialize_project_document(&Document::default()).unwrap(),
    )
    .unwrap();
    let recovery = temp.path("recovery");
    fs::create_dir(&recovery).unwrap();
    let owner = claim(&path, &recovery).unwrap();
    let journal = recovery.join(kinewright_project::journal_file_name(&path));
    fs::write(&journal, b"unreplayed journal bytes").unwrap();
    let mut session = sidecar(&path);
    save_headless(
        &Document::default(),
        &BundleWriteAdmission::new_document(),
        &path,
        None,
        &mut session,
        "",
        TimelineRevision::default(),
        &recovery,
        Some(&owner.handle),
    )
    .unwrap();
    assert_eq!(fs::read(&journal).unwrap(), b"unreplayed journal bytes");
    #[cfg(unix)]
    {
        let before = fs::read(path.with_extension("kinewright-incidents")).unwrap();
        fs::remove_file(&owner.handle.path).unwrap();
        let successor =
            kinewright_project::lockfile::acquire_gui_project_lock(&path, &recovery).unwrap();
        let discovery = fs::read(&successor.handle.discovery).unwrap();
        assert!(
            session
                .flush_incidents(Some(&path), "new-pair", "old-pair", |_| None)
                .is_err()
        );
        assert_eq!(
            fs::read(path.with_extension("kinewright-incidents")).unwrap(),
            before
        );
        assert_eq!(fs::read(&journal).unwrap(), b"unreplayed journal bytes");
        assert_eq!(fs::read(&successor.handle.discovery).unwrap(), discovery);
        drop(session);
        owner.handle.release().unwrap();
        assert_eq!(fs::read(&successor.handle.discovery).unwrap(), discovery);
        successor.handle.release().unwrap();
    }
    #[cfg(not(unix))]
    {
        fs::remove_file(&owner.handle.discovery).unwrap();
        assert!(matches!(
            claim(&path, &recovery),
            Err(LockfileError::Contention { .. })
        ));
        drop(session);
        owner.handle.release().unwrap();
    }
}

#[test]
fn g04_sidecar_load_cannot_rename_under_another_writer() {
    let temp = TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "c7-sidecar-load");
    let path = temp.path("project.kinewright");
    fs::write(
        &path,
        serialize_project_document(&Document::default()).unwrap(),
    )
    .unwrap();
    let recovery = temp.path("recovery");
    fs::create_dir(&recovery).unwrap();
    let owner = claim(&path, &recovery).unwrap();
    let stem = path.with_extension("kinewright-incidents");
    fs::write(&stem, b"foreign corrupt sidecar").unwrap();
    let loaded = sidecar(&path);
    assert!(loaded.sidecar_suspended);
    assert_eq!(
        fs::read(&stem).unwrap(),
        b"foreign corrupt sidecar",
        "open-time refusal must not rename without authority"
    );
    assert!(!stem.with_extension("kinewright-incidents.bak").exists());
    owner.handle.release().unwrap();
}
