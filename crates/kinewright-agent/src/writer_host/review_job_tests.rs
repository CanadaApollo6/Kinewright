use super::super::review_job::ReviewPoint;
use kinewright_project::review_job::{
    ReviewArtifact, ReviewCheckBinding, ReviewControl, ReviewEvidence, ReviewPhase, ReviewStage,
    ReviewStatus, ReviewToken, UpdateReviewJob,
};

fn s5_review_seed(root: &std::path::Path) {
    let mut document = Document {
        resolution: (64, 64),
        ..Document::default()
    };
    kinewright_core::apply_batch(
        &mut document,
        &[
            kinewright_core::Operation::AddTrack {
                track: kinewright_core::Track {
                    name: None,
                    id: kinewright_core::TrackId(81),
                    kind: kinewright_core::TrackKind::Video,
                    sync_lock: false,
                    clips: vec![],
                },
            },
            kinewright_core::Operation::AddSolidClip {
                track: kinewright_core::TrackId(81),
                timeline_start: kinewright_core::TimeCode::ZERO,
                duration: kinewright_core::TimeCode(6),
                color: kinewright_core::SolidColor::default(),
            },
        ],
    )
    .unwrap();
    let core = Core::for_test(document).unwrap();
    let mut bundle = ProjectBundle::activate(
        &root.join("project.kinewright"),
        BundleWriteAdmission::new_document(),
        core.history_pool(),
    )
    .unwrap();
    let inputs = bundle.empty_inputs().unwrap();
    bundle.save(&core, &inputs.reference).unwrap();
}

fn s5_review_host(root: &std::path::Path) -> (WriterHost, HostClient) {
    let host = crash_host(root);
    ok(
        host.client().request(HostCommand::Detach).unwrap(),
        "REVIEW_PERSON_HANDOFF",
    );
    let owner = host.owner.upgrade().unwrap();
    let auth = owner
        .lock()
        .unwrap()
        .issue_person_facing_editor("output-review-person".into(), host.roots.clone())
        .unwrap();
    let client = HostClient::connect(host.endpoint.clone(), auth, host.roots.clone()).unwrap();
    (host, client)
}

fn s5_review_version(host: &WriterHost) -> kinewright_project::studio_records::SnapshotKey {
    let owner = host.owner.upgrade().unwrap();
    let mut host = owner.lock().unwrap();
    let owner = host.owner_mut().unwrap();
    let issuer = owner.reuse_metadata_issuer_for("review-version").unwrap();
    let envelope = owner
        .metadata_envelope(CommandId {
            sequence: owner.metadata_issuer_sequence(&issuer).unwrap(),
            issuer,
        })
        .unwrap();
    let update = MetadataUpdate {
        expected_root_generation: owner.metadata_generation().unwrap(),
        change: MetadataChange::PublishSnapshot(PublishSnapshot {
            version: VersionId::fresh().unwrap(),
            expected_head: envelope.expected_head.clone().unwrap(),
            checkpoint_ref: owner.manual_checkpoint_ref().unwrap(),
            parent_version: None,
            run_provenance: None,
            revision_refs: vec![],
        }),
    };
    let kinewright_project::receipts::ReceiptLookup::Committed(receipt) = owner
        .metadata_command(
            &envelope,
            kinewright_project::studio_records::MetadataScope::Snapshot,
            &update,
        )
        .unwrap()
    else {
        panic!("expected committed version");
    };
    let ReceiptResult::Metadata {
        result: MetadataResult::Snapshot(key),
    } = receipt.result
    else {
        panic!("expected version receipt");
    };
    key
}

fn s5_review_call(client: &HostClient, control: ReviewControl) -> Option<ReviewStatus> {
    ok(
        client.request(HostCommand::Review(control)).unwrap(),
        "S5_REVIEW_CONTROL",
    )
    .review
}

fn s5_review_start(
    client: &HostClient,
    key: &kinewright_project::studio_records::SnapshotKey,
) -> ReviewToken {
    let queued = s5_review_call(client, ReviewControl::QueueDefault(key.clone())).unwrap();
    assert_eq!(queued.phase, ReviewPhase::Queued);
    s5_review_call(client, ReviewControl::Resume(queued.token))
        .unwrap()
        .token
}

fn s5_review_wait(client: &HostClient, ready: impl Fn(&ReviewPhase) -> bool) -> ReviewStatus {
    let until = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let status = s5_review_call(client, ReviewControl::Status).unwrap();
        if ready(&status.phase) {
            return status;
        }
        assert!(
            std::time::Instant::now() < until,
            "review settled as {:?}",
            status.phase
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn s5_review_artifact(host: &WriterHost, token: &ReviewToken) -> ReviewArtifact {
    let (evidence, bytes) = {
        let owner = host.owner.upgrade().unwrap();
        let owner = owner.lock().unwrap();
        let bundle = owner
            .owner()
            .unwrap()
            .review_source_bundle(&token.project)
            .unwrap();
        bundle.open_review_artifact(token).unwrap()
    };
    let measured = kinewright_media::inspect_review_output(
        bytes,
        &kinewright_core::ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        measured, evidence.output,
        "REVIEW_EVIDENCE_DESCRIBES_RETAINED_BYTES"
    );
    (*evidence).clone()
}

fn s5_review_kept_versions(
    host: &WriterHost,
) -> Vec<(
    kinewright_project::studio_records::SnapshotKey,
    kinewright_project::bundle_records::ObjectRef,
)> {
    let owner = host.owner.upgrade().unwrap();
    let owner = owner.lock().unwrap();
    let bundle = owner.owner().unwrap().review_bundle().unwrap();
    let index = bundle.version_index().unwrap();
    index
        .snapshots
        .values()
        .map(|descriptor| {
            bundle.store().verify(&descriptor.checkpoint_ref).unwrap();
            (descriptor.key.clone(), descriptor.checkpoint_ref.clone())
        })
        .collect()
}

fn s5_review_hold(
    host: &WriterHost,
    point: ReviewPoint,
) -> (
    Arc<std::sync::atomic::AtomicBool>,
    Arc<std::sync::atomic::AtomicBool>,
    Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let arrived = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let installed = Arc::new(AtomicBool::new(false));
    let witness = (arrived.clone(), release.clone(), installed.clone());
    host.owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .review_fixture_probe(move |at| {
            if at == ReviewPoint::AfterInstall {
                installed.store(true, Ordering::Release);
            }
            if at == point {
                arrived.store(true, Ordering::Release);
                let until = std::time::Instant::now() + Duration::from_secs(30);
                while !release.load(Ordering::Acquire) {
                    assert!(
                        std::time::Instant::now() < until,
                        "worker hold was never released"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        });
    witness
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s5_review_pending_and_durable_stop_fence_the_real_install_gate() {
    use std::sync::atomic::Ordering;
    for fault in [
        Some("review_stop_before_admission"),
        Some("root_rename_io"),
        None,
    ] {
        let temp = kinewright_media::test_support::TempDirectory::new("review-stop");
        s5_review_seed(temp.root());
        let (host, client) = s5_review_host(temp.root());
        let key = s5_review_version(&host);
        let (arrived, release, installed) = s5_review_hold(&host, ReviewPoint::BeforeInstall);
        let token = s5_review_start(&client, &key);
        c4_wait(|| arrived.load(Ordering::Acquire), "REVIEW_INSTALL_HELD");
        let shared = host.owner.upgrade().unwrap();
        if let Some(fault) = fault {
            let mut owner = shared.lock().unwrap();
            let status = owner.review_fixture_fail_stop(&token, fault);
            assert!(status.stop_pending, "REVIEW_STOP_PENDING");
            assert_eq!(
                owner.review_fixture_worker(),
                Some((false, false)),
                "REVIEW_SIGNAL_REQUIRES_DURABLE_STOP"
            );
        } else {
            let stopped = s5_review_call(&client, ReviewControl::Stop(token.clone())).unwrap();
            assert_eq!(stopped.phase, ReviewPhase::Stopping);
            assert_eq!(
                shared.lock().unwrap().review_fixture_worker(),
                Some((true, false))
            );
        }
        let reserved = shared
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .store()
            .unwrap()
            .usage()
            .unwrap()
            .reserved_copy_bytes;
        assert!(reserved > 0, "REVIEW_LIVE_STAGE_CHARGE");
        release.store(true, Ordering::Release);
        c4_wait(
            || {
                shared
                    .lock()
                    .unwrap()
                    .review_fixture_worker()
                    .is_some_and(|(_, done)| done)
            },
            "REVIEW_WORKER_RETIRED",
        );
        assert!(
            !installed.load(Ordering::Acquire),
            "REVIEW_LATE_INSTALL_WAS_FENCED"
        );
        let stopped = s5_review_call(&client, ReviewControl::Stop(token)).unwrap();
        assert_eq!(stopped.phase, ReviewPhase::Stopped);
        let owner = shared.lock().unwrap();
        assert_eq!(
            owner
                .owner()
                .unwrap()
                .store()
                .unwrap()
                .usage()
                .unwrap()
                .reserved_copy_bytes,
            0
        );
        assert!(
            owner
                .owner()
                .unwrap()
                .read_snapshot(&key)
                .unwrap()
                .descriptor
                .review_evidence_ref
                .is_none()
        );
    }
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s5_review_stop_after_installation_never_publishes_after_acknowledgement() {
    use std::sync::atomic::Ordering;
    let temp = kinewright_media::test_support::TempDirectory::new("review-race");
    s5_review_seed(temp.root());
    let (host, client) = s5_review_host(temp.root());
    let key = s5_review_version(&host);
    // Hold once inside the real gate (after installation, owner lock held)
    // and once before publication takes the owner lock again.
    let held = Arc::new(std::sync::atomic::AtomicU8::new(0));
    let release = Arc::new(std::sync::atomic::AtomicU8::new(0));
    {
        let (held, release) = (held.clone(), release.clone());
        host.owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .review_fixture_probe(move |at| {
                let step = match at {
                    ReviewPoint::AfterInstall => 1,
                    ReviewPoint::BeforePublication => 2,
                    _ => return,
                };
                held.store(step, Ordering::Release);
                let until = std::time::Instant::now() + Duration::from_secs(30);
                while release.load(Ordering::Acquire) < step {
                    assert!(
                        std::time::Instant::now() < until,
                        "race hold never released"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
            });
    }
    let token = s5_review_start(&client, &key);
    c4_wait(
        || held.load(Ordering::Acquire) == 1,
        "REVIEW_INSTALL_INSIDE_GATE",
    );
    let acknowledged = std::thread::scope(|scope| {
        let stop = scope.spawn(|| s5_review_call(&client, ReviewControl::Stop(token.clone())));
        std::thread::sleep(Duration::from_millis(300));
        assert!(!stop.is_finished(), "REVIEW_STOP_WAITS_FOR_THE_GATE");
        release.store(1, Ordering::Release);
        c4_wait(
            || held.load(Ordering::Acquire) == 2,
            "REVIEW_INSTALLED_EXPORT_SUCCEEDED",
        );
        stop.join().unwrap().unwrap()
    });
    release.store(2, Ordering::Release);
    assert!(matches!(
        acknowledged.phase,
        ReviewPhase::Stopping | ReviewPhase::Stopped
    ));
    let shared = host.owner.upgrade().unwrap();
    c4_wait(
        || {
            shared
                .lock()
                .unwrap()
                .review_fixture_worker()
                .is_none_or(|(_, done)| done)
        },
        "REVIEW_WORKER_RETIRED",
    );
    let stopped = s5_review_call(&client, ReviewControl::Stop(token)).unwrap();
    assert_eq!(stopped.phase, ReviewPhase::Stopped, "REVIEW_STOP_TERMINAL");
    let owner = shared.lock().unwrap();
    assert!(
        owner
            .owner()
            .unwrap()
            .read_snapshot(&key)
            .unwrap()
            .descriptor
            .review_evidence_ref
            .is_none(),
        "REVIEW_NO_PUBLICATION_AFTER_STOP"
    );
    assert_eq!(
        owner
            .owner()
            .unwrap()
            .store()
            .unwrap()
            .usage()
            .unwrap()
            .reserved_copy_bytes,
        0,
        "REVIEW_RETIRED_STAGE_RELEASED"
    );
    drop(owner);
    assert_eq!(
        std::fs::read_dir(temp.path("project.kinewright-assets/studio/review-staging"))
            .unwrap()
            .count(),
        0,
        "REVIEW_INSTALLED_STAGE_RECLAIMED"
    );
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s5_review_capacity_failures_keep_saved_work_and_allow_retry_or_stop() {
    for artifact_failure in [false, true] {
        let temp = kinewright_media::test_support::TempDirectory::new("review-capacity");
        s5_review_seed(temp.root());
        let (host, client) = s5_review_host(temp.root());
        let key = s5_review_version(&host);
        let shared = host.owner.upgrade().unwrap();
        let (settings, history) = {
            let mut owner = shared.lock().unwrap();
            owner.review_fixture_exhaust_artifact(artifact_failure);
            assert!(
                owner
                    .core()
                    .request(kinewright_core::Command::Do(history_marker(1)))
                    .is_ok()
            );
            let history = owner.core().snapshot_identity().unwrap();
            let document = owner
                .owner()
                .unwrap()
                .read_snapshot(&key)
                .unwrap()
                .open_review_inputs()
                .unwrap()
                .document;
            let settings = kinewright_core::DeliveryProfile::SourceMaster.export_settings(
                &document,
                kinewright_core::DeliveryEncodeDepth::Eight,
                kinewright_core::ExportCancellation::default(),
            );
            (settings, history)
        };
        let kept = s5_review_kept_versions(&host);
        let queued = s5_review_call(
            &client,
            ReviewControl::Queue {
                snapshot: key.clone(),
                settings,
                byte_limit: if artifact_failure { 1 << 20 } else { 1 },
            },
        )
        .unwrap();
        s5_review_call(&client, ReviewControl::Resume(queued.token)).unwrap();
        let paused = s5_review_wait(&client, |phase| matches!(phase, ReviewPhase::Paused { .. }));
        let ReviewPhase::Paused { reason, .. } = &paused.phase else {
            unreachable!()
        };
        if artifact_failure {
            assert!(reason.contains("No space left"), "{reason}");
        } else {
            assert!(
                reason.contains("staging limit") || reason.contains("File too large"),
                "{reason}"
            );
        }
        assert_eq!(
            s5_review_kept_versions(&host),
            kept,
            "REVIEW_CAPACITY_KEEPS_VERSIONS"
        );
        {
            let owner = shared.lock().unwrap();
            assert_eq!(owner.core().snapshot_identity().unwrap(), history);
            assert!(
                owner
                    .owner()
                    .unwrap()
                    .read_snapshot(&key)
                    .unwrap()
                    .descriptor
                    .review_evidence_ref
                    .is_none()
            );
        }
        if artifact_failure {
            shared
                .lock()
                .unwrap()
                .review_fixture_exhaust_artifact(false);
            let resumed = s5_review_call(&client, ReviewControl::Resume(paused.token)).unwrap();
            let checking =
                s5_review_wait(&client, |phase| matches!(phase, ReviewPhase::Checking(_)));
            assert_eq!(checking.token, resumed.token);
            assert_eq!(
                s5_review_artifact(&host, &checking.token)
                    .output
                    .video_frames,
                6
            );
        } else {
            assert_eq!(
                s5_review_call(&client, ReviewControl::Stop(paused.token))
                    .unwrap()
                    .phase,
                ReviewPhase::Stopped
            );
        }
    }
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s5_review_crash_child() {
    let Some(root) = std::env::var_os("KINEWRIGHT_REVIEW_CRASH_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let cut = std::env::var("KINEWRIGHT_REVIEW_CRASH_CUT").unwrap();
    let (host, client) = s5_review_host(&root);
    let snapshot = s5_review_version(&host);
    if cut == "stopped-before-retirement" {
        // Stop is durable while the worker still holds its stage; the process
        // ends before the worker retires.
        let (arrived, _release, _) = s5_review_hold(&host, ReviewPoint::BeforeInstall);
        let token = s5_review_start(&client, &snapshot);
        c4_wait(
            || arrived.load(std::sync::atomic::Ordering::Acquire),
            "REVIEW_CRASH_HOLD",
        );
        let stopped = s5_review_call(&client, ReviewControl::Stop(token)).unwrap();
        assert_eq!(stopped.phase, ReviewPhase::Stopping);
        assert!(!stopped.stop_pending);
        std::process::exit(86);
    }
    let point = match cut.as_str() {
        "unpublished-stage" => ReviewPoint::BeforeInstall,
        "installed-stage" => ReviewPoint::BeforePublication,
        "published-artifact" => ReviewPoint::AfterPublication,
        other => panic!("unknown review crash cut {other}"),
    };
    host.owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .review_fixture_probe(move |at| {
            if at == point {
                std::process::exit(86);
            }
        });
    let _ = s5_review_start(&client, &snapshot);
    let _ = s5_review_wait(&client, |_| false);
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s5_review_reopens_the_root_selected_by_the_artifact_publication() {
    for cut in ["unpublished-stage", "installed-stage", "published-artifact"] {
        let temp = kinewright_media::test_support::TempDirectory::new("review-crash");
        s5_review_seed(temp.root());
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "writer_host::hooks_tests::tests::s5_review_crash_child",
                "--nocapture",
            ])
            .env("KINEWRIGHT_REVIEW_CRASH_ROOT", temp.root())
            .env("KINEWRIGHT_REVIEW_CRASH_CUT", cut);
        let output = command.output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(86),
            "REVIEW_CRASH_CUT: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let staging = temp.path("project.kinewright-assets/studio/review-staging");
        assert_eq!(
            std::fs::read_dir(&staging).unwrap().count(),
            1,
            "crash left a stage"
        );
        let (mut host, mut client) = s5_review_host(temp.root());
        if cut == "published-artifact" {
            s5_review_refuses_changed_reopened_bytes(&host, &client, &temp);
            // A refused hash invalidates the cached store graph. Reopen after
            // restoring the fixture bytes so the new owner reads that graph.
            drop(client);
            drop(host);
            (host, client) = s5_review_host(temp.root());
        }
        let status = s5_review_call(&client, ReviewControl::Status).unwrap();
        assert_eq!(status.token.generation, 1);
        assert_eq!(
            std::fs::read_dir(staging).unwrap().count(),
            0,
            "REVIEW_COLD_SWEEP"
        );
        if cut == "published-artifact" {
            assert!(
                matches!(status.phase, ReviewPhase::Checking(_)),
                "REVIEW_COMMITTED_ARTIFACT_WINS"
            );
            let output = s5_review_artifact(&host, &status.token);
            let resumed =
                s5_review_call(&client, ReviewControl::Resume(status.token.clone())).unwrap();
            assert_eq!(resumed.token, status.token, "REVIEW_NO_SECOND_ENCODE");
            assert_eq!(s5_review_artifact(&host, &resumed.token), output);
        } else {
            assert_eq!(
                status.phase,
                ReviewPhase::Queued,
                "REVIEW_REQUEUE_UNPUBLISHED"
            );
            let owner = host.owner.upgrade().unwrap();
            assert!(
                owner
                    .lock()
                    .unwrap()
                    .owner()
                    .unwrap()
                    .read_snapshot(&status.token.snapshot)
                    .unwrap()
                    .descriptor
                    .review_evidence_ref
                    .is_none()
            );
        }
    }
}

fn s5_review_crash(temp: &kinewright_media::test_support::TempDirectory, cut: &str) {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "writer_host::hooks_tests::tests::s5_review_crash_child",
            "--nocapture",
        ])
        .env("KINEWRIGHT_REVIEW_CRASH_ROOT", temp.root())
        .env("KINEWRIGHT_REVIEW_CRASH_CUT", cut)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(86),
        "REVIEW_CRASH_CUT: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn s5_review_stage_count(temp: &kinewright_media::test_support::TempDirectory) -> usize {
    std::fs::read_dir(temp.path("project.kinewright-assets/studio/review-staging"))
        .unwrap()
        .count()
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s5_review_reopen_drains_a_stop_persisted_before_retirement() {
    let temp = kinewright_media::test_support::TempDirectory::new("review-stopped-crash");
    s5_review_seed(temp.root());
    s5_review_crash(&temp, "stopped-before-retirement");
    assert_eq!(s5_review_stage_count(&temp), 1, "crash left a stage");
    let (host, client) = s5_review_host(temp.root());
    let status = s5_review_call(&client, ReviewControl::Status).unwrap();
    assert_eq!(
        status.phase,
        ReviewPhase::Stopped,
        "REVIEW_REOPEN_DRAINS_STOPPING"
    );
    assert_eq!(
        s5_review_stage_count(&temp),
        0,
        "REVIEW_STOPPED_STAGE_SWEPT"
    );
    let owner = host.owner.upgrade().unwrap();
    let owner = owner.lock().unwrap();
    assert!(
        owner
            .owner()
            .unwrap()
            .review_bundle()
            .unwrap()
            .review_job()
            .unwrap()
            .is_none_or(|job| job.stage.is_none()),
        "REVIEW_STOPPED_STAGE_RETIRED"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn s5_review_prove_retired_refuses_a_live_owner_of_another_process() {
    use super::super::review_job::{process_start, prove_retired};
    struct Reaped(std::process::Child);
    impl Drop for Reaped {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = Reaped(
        std::process::Command::new("sleep")
            .arg("30")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let pid = child.0.id();
    let start = process_start(pid).unwrap().unwrap();
    let stage = |process_start| ReviewStage {
        id: kinewright_project::bundle_records::SessionId::fresh().unwrap(),
        boot: crate::guardian::boot_id().unwrap(),
        pid,
        process_start,
        byte_limit: 1,
    };
    let live = stage(start);
    let refused = prove_retired(&live).unwrap_err();
    assert!(
        refused.to_string().contains("has not retired"),
        "REVIEW_FOREIGN_LIVE_OWNER_REFUSED: {refused}"
    );
    assert!(
        prove_retired(&stage(start + 1)).is_ok(),
        "a reused process id is another process"
    );
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    assert!(prove_retired(&live).is_ok(), "REVIEW_REAPED_OWNER_RETIRED");
}

fn s5_review_save(host: &WriterHost, client: &HostClient, marker: u64) -> HostReply {
    {
        let owner = host.owner.upgrade().unwrap();
        let owner = owner.lock().unwrap();
        assert!(
            owner
                .core()
                .request(kinewright_core::Command::Do(history_marker(marker)))
                .is_ok()
        );
    }
    client
        .request(HostCommand::Save {
            snapshot: host_snapshot_for_test(host),
        })
        .unwrap()
}

/// A live stage on a disk with room for its charge only: the Stop cannot
/// persist and the person's Save is refused for real lack of space. The
/// finished worker releases the charge before the Stop is retried.
#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
#[allow(
    clippy::too_many_lines,
    reason = "one filled store through two endings"
)]
fn s5_review_full_store_releases_the_stage_before_a_pending_stop() {
    use std::sync::atomic::Ordering;
    const STAGE: u64 = 64 << 20;
    for publish in [false, true] {
        let temp = kinewright_media::test_support::TempDirectory::new("review-full");
        s5_review_seed(temp.root());
        let (host, client) = s5_review_host(temp.root());
        let key = s5_review_version(&host);
        let shared = host.owner.upgrade().unwrap();
        let settings = {
            let owner = shared.lock().unwrap();
            let document = owner
                .owner()
                .unwrap()
                .read_snapshot(&key)
                .unwrap()
                .open_review_inputs()
                .unwrap()
                .document;
            kinewright_core::DeliveryProfile::SourceMaster.export_settings(
                &document,
                kinewright_core::DeliveryEncodeDepth::Eight,
                kinewright_core::ExportCancellation::default(),
            )
        };
        let (arrived, release, _) = s5_review_hold(&host, ReviewPoint::BeforeInstall);
        let queued = s5_review_call(
            &client,
            ReviewControl::Queue {
                snapshot: key.clone(),
                settings,
                byte_limit: STAGE,
            },
        )
        .unwrap();
        let token = s5_review_call(&client, ReviewControl::Resume(queued.token))
            .unwrap()
            .token;
        c4_wait(|| arrived.load(Ordering::Acquire), "REVIEW_FULL_HOLD");
        let probe = |capacity| {
            shared
                .lock()
                .unwrap()
                .owner()
                .unwrap()
                .store()
                .unwrap()
                .free_space_probe(Some(capacity))
                .unwrap();
        };
        if publish {
            // 8 MiB of ordinary room past the revocation headroom.
            probe(STAGE + (40 << 20));
            ok(
                s5_review_save(&host, &client, 31),
                "REVIEW_SAVE_WHILE_ENCODING",
            );
            release.store(true, Ordering::Release);
            let checking =
                s5_review_wait(&client, |phase| matches!(phase, ReviewPhase::Checking(_)));
            assert_eq!(checking.token, token, "REVIEW_PUBLISHED_ON_A_FULL_STORE");
            assert_eq!(s5_review_artifact(&host, &token).output.video_frames, 6);
            c4_wait(
                || {
                    shared
                        .lock()
                        .unwrap()
                        .review_fixture_worker()
                        .is_some_and(|(_, done)| done)
                },
                "REVIEW_FULL_WORKER_FINISHED",
            );
            s5_review_call(&client, ReviewControl::Status).unwrap();
        } else {
            probe(STAGE);
            let pending = s5_review_call(&client, ReviewControl::Stop(token.clone())).unwrap();
            assert!(
                pending.stop_pending,
                "REVIEW_STOP_NEEDS_SPACE_HELD_BY_STAGE"
            );
            let refused = s5_review_save(&host, &client, 32);
            assert!(
                refused
                    .error
                    .as_ref()
                    .is_some_and(|error| error.message.contains("disk reserve")),
                "REVIEW_FULL_SAVE_REFUSED: {:?}",
                refused.error
            );
            // No room at all: the Stop still cannot persist once the worker ends.
            probe(0);
            release.store(true, Ordering::Release);
            c4_wait(
                || {
                    shared
                        .lock()
                        .unwrap()
                        .review_fixture_worker()
                        .is_some_and(|(_, done)| done)
                },
                "REVIEW_FULL_WORKER_FINISHED",
            );
            let _ = client.request(HostCommand::Review(ReviewControl::Status));
            let held = shared
                .lock()
                .unwrap()
                .owner()
                .unwrap()
                .store()
                .unwrap()
                .usage()
                .unwrap()
                .reserved_copy_bytes;
            assert_eq!(
                (held, s5_review_stage_count(&temp)),
                (0, 0),
                "REVIEW_RETIREMENT_BEFORE_PENDING_STOP"
            );
            probe(STAGE);
            let status = s5_review_call(&client, ReviewControl::Status).unwrap();
            assert_eq!(
                (status.phase, status.stop_pending),
                (ReviewPhase::Stopped, false),
                "REVIEW_PENDING_STOP_PERSISTS_AFTER_RELEASE"
            );
            assert!(
                shared
                    .lock()
                    .unwrap()
                    .owner()
                    .unwrap()
                    .read_snapshot(&key)
                    .unwrap()
                    .descriptor
                    .review_evidence_ref
                    .is_none(),
                "REVIEW_NO_PUBLICATION_AFTER_PENDING_STOP"
            );
        }
        assert_eq!(s5_review_stage_count(&temp), 0, "REVIEW_FULL_STAGE_SWEPT");
        assert_eq!(
            shared
                .lock()
                .unwrap()
                .owner()
                .unwrap()
                .store()
                .unwrap()
                .usage()
                .unwrap()
                .reserved_copy_bytes,
            0,
            "REVIEW_FULL_CHARGE_RELEASED"
        );
        ok(
            s5_review_save(&host, &client, 33),
            "REVIEW_SAVE_AFTER_RELEASE",
        );
    }
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s5_review_shutdown_joins_a_held_encode_before_reopen() {
    use std::sync::atomic::Ordering;
    let temp = kinewright_media::test_support::TempDirectory::new("review-shutdown");
    s5_review_seed(temp.root());
    let (host, client) = s5_review_host(temp.root());
    let key = s5_review_version(&host);
    let (arrived, release, _) = s5_review_hold(&host, ReviewPoint::BeforeEncode);
    let token = s5_review_start(&client, &key);
    c4_wait(|| arrived.load(Ordering::Acquire), "REVIEW_SHUTDOWN_HOLD");
    std::thread::scope(|scope| {
        let shutdown = scope.spawn(|| client.request(HostCommand::Shutdown).unwrap());
        std::thread::sleep(Duration::from_millis(300));
        let waited = !shutdown.is_finished();
        release.store(true, Ordering::Release);
        ok(shutdown.join().unwrap(), "REVIEW_SHUTDOWN");
        assert!(waited, "REVIEW_SHUTDOWN_JOINS_THE_WORKER");
    });
    drop(client);
    drop(host);
    let (_host, client) = s5_review_host(temp.root());
    let status = s5_review_call(&client, ReviewControl::Status).unwrap();
    assert_eq!(
        (status.token.job, status.phase),
        (token.job, ReviewPhase::Queued),
        "REVIEW_SHUTDOWN_REOPENS_QUEUED"
    );
    assert_eq!(
        s5_review_stage_count(&temp),
        0,
        "REVIEW_SHUTDOWN_STAGE_SWEPT"
    );
}

fn s5_review_attempt(host: &WriterHost) -> Option<kinewright_project::bundle_records::SessionId> {
    let owner = host.owner.upgrade().unwrap();
    let owner = owner.lock().unwrap();
    let job = owner
        .owner()
        .unwrap()
        .review_bundle()
        .unwrap()
        .review_job()
        .unwrap()
        .unwrap();
    let ReviewPhase::Checking(checks) = &job.phase else {
        panic!("expected a checking job, got {:?}", job.phase);
    };
    checks.reviewer_attempt.clone()
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s5_review_check_attempt_survives_status_and_dies_with_its_process() {
    let temp = kinewright_media::test_support::TempDirectory::new("review-attempt");
    s5_review_seed(temp.root());
    let (host, client) = s5_review_host(temp.root());
    let key = s5_review_version(&host);
    let token = s5_review_start(&client, &key);
    // No Status before the first bind: Status verification would mask a
    // publishing host that forgot its own artifact.
    c4_wait(
        || {
            let owner = host.owner.upgrade().unwrap();
            let owner = owner.lock().unwrap();
            let bundle = owner.owner().unwrap().review_bundle().unwrap();
            matches!(
                bundle.review_job().unwrap().map(|job| job.phase.clone()),
                Some(ReviewPhase::Checking(_))
            )
        },
        "REVIEW_ATTEMPT_CHECKING",
    );
    let binding = ReviewCheckBinding {
        configuration_sha256: [7; 32],
        reviewer_attempt: kinewright_project::bundle_records::SessionId::fresh().unwrap(),
    };
    let record = |binding| UpdateReviewJob::RecordCheck {
        binding,
        row: kinewright_core::output_review::RequiredReviewRow::StructureAndAvIntegrity,
        result: kinewright_core::output_review::ReviewRowResult::Pass,
    };
    let shared = host.owner.upgrade().unwrap();
    shared
        .lock()
        .unwrap()
        .record_review_check(&token, UpdateReviewJob::BindChecks(binding.clone()))
        .expect("REVIEW_PUBLISHING_HOST_VERIFIED");
    s5_review_call(&client, ReviewControl::Status).unwrap();
    assert_eq!(
        s5_review_attempt(&host),
        Some(binding.reviewer_attempt.clone()),
        "REVIEW_LIVE_ATTEMPT_SURVIVES_STATUS"
    );
    {
        let mut owner = shared.lock().unwrap();
        owner
            .record_review_check(&token, record(binding.clone()))
            .unwrap();
        let other = owner
            .record_review_check(&token, UpdateReviewJob::Stop)
            .unwrap_err();
        assert!(other.to_string().contains("Only check bindings"), "{other}");
        let pending = owner.review_fixture_fail_stop(&token, "review_stop_before_admission");
        assert!(pending.stop_pending);
        let fenced = owner
            .record_review_check(&token, record(binding.clone()))
            .expect_err("REVIEW_PENDING_STOP_FENCES_RESULTS");
        assert!(
            fenced.to_string().contains("authority was revoked"),
            "REVIEW_PENDING_STOP_FENCES_RESULTS: {fenced}"
        );
    }
    // The Stop never persisted: the job reopens in Checking with the attempt
    // of a process that is gone.
    drop(shared);
    drop(client);
    drop(host);
    let (host, client) = s5_review_host(temp.root());
    assert_eq!(
        s5_review_attempt(&host),
        None,
        "REVIEW_REOPEN_UNBINDS_THE_DEAD_ATTEMPT"
    );
    let shared = host.owner.upgrade().unwrap();
    let unverified = shared
        .lock()
        .unwrap()
        .record_review_check(&token, UpdateReviewJob::BindChecks(binding))
        .expect_err("REVIEW_RESULTS_NEED_A_VERIFIED_ARTIFACT");
    assert!(
        unverified.to_string().contains("has not been verified"),
        "REVIEW_RESULTS_NEED_A_VERIFIED_ARTIFACT: {unverified}"
    );
    s5_review_call(&client, ReviewControl::Status).unwrap();
    let fresh = ReviewCheckBinding {
        configuration_sha256: [7; 32],
        reviewer_attempt: kinewright_project::bundle_records::SessionId::fresh().unwrap(),
    };
    shared
        .lock()
        .unwrap()
        .record_review_check(&token, UpdateReviewJob::BindChecks(fresh.clone()))
        .unwrap();
    assert_eq!(s5_review_attempt(&host), Some(fresh.reviewer_attempt));
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s5_review_pending_stop_stays_with_its_project_across_save_as() {
    let temp = kinewright_media::test_support::TempDirectory::new("review-save-as-stop");
    s5_review_seed(temp.root());
    let (host, client) = s5_review_host(temp.root());
    let key = s5_review_version(&host);
    let source = s5_review_start(&client, &key);
    let shared = host.owner.upgrade().unwrap();
    // No Status: the finished source worker is still unretired at Save As.
    c4_wait(
        || {
            shared
                .lock()
                .unwrap()
                .review_fixture_worker()
                .is_some_and(|(_, done)| done)
        },
        "REVIEW_SOURCE_WORKER_FINISHED",
    );
    {
        let mut owner = shared.lock().unwrap();
        owner
            .owner_mut()
            .unwrap()
            .save_as(&temp.path("copy.kinewright"), StorageLimits::default())
            .unwrap();
        // The source has no room left: its Stop stays pending in memory.
        owner
            .owner()
            .unwrap()
            .review_source_bundle(&source.project)
            .unwrap()
            .store()
            .free_space_probe(Some(0))
            .unwrap();
    }
    let pending = s5_review_call(&client, ReviewControl::Stop(source.clone())).unwrap();
    assert!(pending.stop_pending, "the source Stop could not persist");
    // Every copy-side step fails if the source's pending Stop leaks into it.
    let copy_side = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let copy_key = s5_review_version(&host);
        let copy = s5_review_start(&client, &copy_key);
        assert_ne!(copy.project, source.project);
        s5_review_wait(&client, |phase| matches!(phase, ReviewPhase::Checking(_)));
        let stopped = client
            .request(HostCommand::Review(ReviewControl::Stop(copy)))
            .unwrap();
        assert!(stopped.error.is_none(), "{:?}", stopped.error);
        stopped.review.unwrap().phase
    }));
    let phase = copy_side.unwrap_or_else(|_| panic!("REVIEW_PENDING_STOP_STAYS_WITH_ITS_PROJECT"));
    assert!(matches!(
        phase,
        ReviewPhase::Stopping | ReviewPhase::Stopped
    ));
}

fn s5_review_refuses_changed_reopened_bytes(
    host: &WriterHost,
    client: &HostClient,
    temp: &kinewright_media::test_support::TempDirectory,
) {
    let hash = {
        let owner = host.owner.upgrade().unwrap();
        let owner = owner.lock().unwrap();
        let bundle = owner.owner().unwrap().review_bundle().unwrap();
        let job = bundle.review_job().unwrap().unwrap();
        let reference = job
            .artifact_ref
            .as_ref()
            .expect("REVIEW_COMMITTED_ARTIFACT_WINS");
        bundle
            .store()
            .read_record::<ReviewArtifact>(reference)
            .unwrap()
            .media_ref
            .sha256
            .clone()
    };
    let path = temp
        .path("project.kinewright-assets/studio/objects")
        .join(hash);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[0] ^= 1;
    std::fs::write(&path, &bytes).unwrap();
    let reply = client
        .request(HostCommand::Review(ReviewControl::Status))
        .unwrap();
    bytes[0] ^= 1;
    std::fs::write(&path, &bytes).unwrap();
    assert!(
        reply
            .error
            .is_some_and(|error| error.message.contains("owned identity changed")),
        "REVIEW_FIRST_REQUEST_VERIFIES_REOPENED_ARTIFACT"
    );
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s5_review_edits_undo_copy_and_checker_attempt_stay_bound_to_the_frozen_version() {
    use std::sync::atomic::Ordering;
    let temp = kinewright_media::test_support::TempDirectory::new("review-copy");
    s5_review_seed(temp.root());
    let (host, client) = s5_review_host(temp.root());
    let prior = s5_review_version(&host);
    let prior_token = s5_review_start(&client, &prior);
    s5_review_wait(&client, |phase| matches!(phase, ReviewPhase::Checking(_)));
    let first = s5_review_artifact(&host, &prior_token);
    s5_review_call(&client, ReviewControl::Stop(prior_token)).unwrap();
    let key = s5_review_version(&host);
    let (arrived, release, _) = s5_review_hold(&host, ReviewPoint::BeforeEncode);
    let token = s5_review_start(&client, &key);
    c4_wait(
        || arrived.load(Ordering::Acquire),
        "REVIEW_COPY_HOLDS_ENCODE",
    );
    let duplicate = client
        .request(HostCommand::Review(ReviewControl::QueueDefault(
            prior.clone(),
        )))
        .unwrap();
    assert!(
        duplicate
            .error
            .is_some_and(|error| error.message.contains("already active")),
        "explicit duplicate review refusal"
    );
    let shared = host.owner.upgrade().unwrap();
    let working;
    let document_before_copy;
    {
        let mut owner = shared.lock().unwrap();
        let core = owner.core();
        assert!(
            core.request(kinewright_core::Command::Do(
                kinewright_core::Operation::AddSolidClip {
                    track: kinewright_core::TrackId(81),
                    timeline_start: kinewright_core::TimeCode(6),
                    duration: kinewright_core::TimeCode(6),
                    color: kinewright_core::SolidColor::default(),
                }
            ))
            .is_ok()
        );
        assert!(
            core.request(kinewright_core::Command::Do(history_marker(1)))
                .is_ok()
        );
        assert!(core.request(kinewright_core::Command::Undo).is_ok());
        working = core.snapshot_identity().unwrap();
        document_before_copy = core
            .request(kinewright_core::Command::Query(
                kinewright_core::Query::Document,
            ))
            .unwrap();
        assert_ne!(
            working,
            owner
                .owner()
                .unwrap()
                .read_snapshot(&key)
                .unwrap()
                .descriptor
                .identity
        );
        let bundle = owner.owner().unwrap().review_bundle().unwrap();
        let job = bundle.review_job().unwrap().unwrap();
        assert_eq!(
            (&job.token, &job.phase),
            (&token, &ReviewPhase::Encoding),
            "REVIEW_PROGRESS_NAMES_FROZEN_VERSION"
        );
        let request = bundle
            .store()
            .read_record::<kinewright_project::review_job::ReviewRequest>(&job.request_ref)
            .unwrap();
        assert_eq!(
            request.document_ref,
            bundle.read_snapshot(&key).unwrap().descriptor.document_ref,
            "REVIEW_REQUEST_NAMES_FROZEN_DOCUMENT"
        );
        drop(request);
        drop(job);
        owner
            .owner_mut()
            .unwrap()
            .save_as(&temp.path("copy.kinewright"), StorageLimits::default())
            .unwrap();
        let copy = owner.owner().unwrap().review_bundle().unwrap();
        assert!(
            copy.review_job().unwrap().is_none(),
            "REVIEW_COPY_OMITS_AUTHORITY"
        );
        let evidence = copy
            .read_snapshot(&prior)
            .unwrap()
            .descriptor
            .review_evidence_ref
            .unwrap();
        let evidence = copy
            .store()
            .read_record::<ReviewEvidence>(&evidence)
            .unwrap();
        assert_eq!(
            *copy
                .store()
                .read_record::<ReviewArtifact>(evidence.artifacts().next_back().unwrap())
                .unwrap(),
            first
        );
    }
    release.store(true, Ordering::Release);
    c4_wait(
        || {
            shared
                .lock()
                .unwrap()
                .review_fixture_worker()
                .is_some_and(|(_, done)| done)
        },
        "REVIEW_SOURCE_WORKER_COMPLETED",
    );
    assert!(s5_review_call(&client, ReviewControl::Status).is_none());
    let resume = client
        .request(HostCommand::Review(ReviewControl::Resume(token.clone())))
        .unwrap();
    assert!(
        resume
            .error
            .is_some_and(|error| error.message.contains("generation or version changed")),
        "REVIEW_COPY_REFUSES_SOURCE_RESUME"
    );
    let artifact = s5_review_artifact(&host, &token);
    assert_eq!(
        artifact.output.video_frames, 6,
        "REVIEW_ARTIFACT_MATCHES_FROZEN_DURATION"
    );
    let mut owner = shared.lock().unwrap();
    let source = owner
        .owner_mut()
        .unwrap()
        .review_source_bundle_mut(&token.project)
        .unwrap();
    let binding = ReviewCheckBinding {
        configuration_sha256: [91; 32],
        reviewer_attempt: kinewright_project::bundle_records::SessionId::fresh().unwrap(),
    };
    source
        .update_review_job(&token, UpdateReviewJob::BindChecks(binding.clone()))
        .unwrap();
    let row = kinewright_core::output_review::RequiredReviewRow::StructureAndAvIntegrity;
    let result = kinewright_core::output_review::ReviewRowResult::Pass;
    let update = |binding| UpdateReviewJob::RecordCheck {
        binding,
        row,
        result,
    };
    let mut wrong = token.clone();
    wrong.generation += 1;
    assert!(
        source
            .update_review_job(&wrong, update(binding.clone()))
            .unwrap_err()
            .to_string()
            .contains("generation or version")
    );
    wrong = token.clone();
    wrong.snapshot = prior.clone();
    assert!(
        source
            .update_review_job(&wrong, update(binding.clone()))
            .unwrap_err()
            .to_string()
            .contains("generation or version")
    );
    let mut old_binding = binding.clone();
    old_binding.reviewer_attempt = kinewright_project::bundle_records::SessionId::fresh().unwrap();
    assert!(
        source
            .update_review_job(&token, update(old_binding))
            .unwrap_err()
            .to_string()
            .contains("check binding")
    );
    source.update_review_job(&token, update(binding)).unwrap();
    let core = owner.core();
    assert_eq!(
        core.request(kinewright_core::Command::Query(
            kinewright_core::Query::Document
        ))
        .unwrap(),
        document_before_copy
    );
    assert!(
        core.request(kinewright_core::Command::Redo).is_ok(),
        "REVIEW_EDIT_UNDO_SURVIVES"
    );
    let pool = core.history_pool().clone();
    drop(owner);
    drop(shared);
    drop(host);
    let copy = Ja1EditingSession::open(&temp.path("copy.kinewright"), &pool).unwrap();
    let copy = copy.review_bundle().unwrap();
    assert!(
        copy.review_job().unwrap().is_none(),
        "REVIEW_COLD_COPY_OMITS_AUTHORITY"
    );
    let descriptor = copy.read_snapshot(&prior).unwrap().descriptor;
    let evidence = copy
        .store()
        .read_record::<ReviewEvidence>(&descriptor.review_evidence_ref.unwrap())
        .unwrap();
    let artifact = copy
        .store()
        .read_record::<ReviewArtifact>(evidence.artifacts().next_back().unwrap())
        .unwrap();
    assert_eq!(
        *artifact, first,
        "REVIEW_COLD_COPY_RETAINS_PUBLISHED_EVIDENCE"
    );
    copy.store().verify(&artifact.media_ref).unwrap();
}
