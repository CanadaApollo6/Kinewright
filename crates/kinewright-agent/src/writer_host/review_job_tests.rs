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

/// Queue `key` with the default review settings at the document's own
/// raster, then resume it: for witnesses about the encode's fences, not the
/// delivery raster, whose debug 1080p encode only widens their windows.
fn s5_review_start_at_source_raster(
    host: &WriterHost,
    client: &HostClient,
    key: &kinewright_project::studio_records::SnapshotKey,
) -> ReviewToken {
    let settings = {
        let shared = host.owner.upgrade().unwrap();
        let owner = shared.lock().unwrap();
        let document = owner
            .owner()
            .unwrap()
            .read_snapshot(key)
            .unwrap()
            .open_review_inputs()
            .unwrap()
            .document;
        let mut settings = kinewright_core::output_review_default_settings(
            &document,
            kinewright_core::ExportCancellation::default(),
        );
        settings.resolution = document.resolution;
        settings
    };
    let queued = s5_review_call(
        client,
        ReviewControl::Queue {
            snapshot: key.clone(),
            settings,
            byte_limit: 64 << 20,
        },
    )
    .unwrap();
    assert_eq!(queued.phase, ReviewPhase::Queued);
    s5_review_call(client, ReviewControl::Resume(queued.token))
        .unwrap()
        .token
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

/// Every wait in these witnesses is on a real signal; the deadline only guards
/// against a hang. Debug encodes in loaded CI shards took over 20 s (R8).
const S5_REVIEW_HANG: Duration = Duration::from_secs(120);

fn s5_review_until(mut ready: impl FnMut() -> bool, marker: &str) {
    let started = std::time::Instant::now();
    while !ready() {
        assert!(
            started.elapsed() < S5_REVIEW_HANG,
            "{marker} after {:?}",
            started.elapsed()
        );
        std::thread::park_timeout(Duration::from_millis(2));
    }
    s5_review_note_slow(started, marker);
}

fn s5_review_note_slow(started: std::time::Instant, marker: &str) {
    if started.elapsed() > Duration::from_secs(20) {
        eprintln!("S5_REVIEW_SLOW_WAIT {marker} {:?}", started.elapsed());
    }
}

fn s5_review_wait(client: &HostClient, ready: impl Fn(&ReviewPhase) -> bool) -> ReviewStatus {
    let started = std::time::Instant::now();
    let until = started + S5_REVIEW_HANG;
    loop {
        let status = s5_review_call(client, ReviewControl::Status).unwrap();
        if ready(&status.phase) {
            s5_review_note_slow(started, "REVIEW_PHASE");
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
                let until = std::time::Instant::now() + S5_REVIEW_HANG;
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
        s5_review_until(|| arrived.load(Ordering::Acquire), "REVIEW_INSTALL_HELD");
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
        s5_review_until(
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
                let until = std::time::Instant::now() + S5_REVIEW_HANG;
                while release.load(Ordering::Acquire) < step {
                    assert!(
                        std::time::Instant::now() < until,
                        "race hold never released"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
            });
    }
    let token = s5_review_start_at_source_raster(&host, &client, &key);
    s5_review_until(
        || held.load(Ordering::Acquire) == 1,
        "REVIEW_INSTALL_INSIDE_GATE",
    );
    let shared = host.owner.upgrade().unwrap();
    let acknowledged = std::thread::scope(|scope| {
        let stop = scope.spawn(|| s5_review_call(&client, ReviewControl::Stop(token.clone())));
        std::thread::sleep(Duration::from_millis(300));
        assert!(!stop.is_finished(), "REVIEW_STOP_WAITS_FOR_THE_GATE");
        release.store(1, Ordering::Release);
        // The Stop cancels the export as soon as the gate lets it in. The
        // worker either reaches publication first, where the fence must
        // refuse it, or its hash or inspection sees the cancellation and it
        // ends before publication. Both publish nothing; the second is what a
        // loaded CI runner saw (fix1 decisions).
        s5_review_until(
            || {
                held.load(Ordering::Acquire) == 2
                    || shared
                        .lock()
                        .unwrap()
                        .review_fixture_worker()
                        .is_none_or(|(_, done)| done)
            },
            "REVIEW_INSTALLED_EXPORT_SETTLED",
        );
        if held.load(Ordering::Acquire) != 2 {
            eprintln!("S5_REVIEW_STOP_CANCELLED_BEFORE_PUBLICATION");
        }
        stop.join().unwrap().unwrap()
    });
    release.store(2, Ordering::Release);
    assert!(matches!(
        acknowledged.phase,
        ReviewPhase::Stopping | ReviewPhase::Stopped
    ));
    s5_review_until(
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
        s5_review_until(
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
    reason = "one filled store through three endings"
)]
fn s5_review_full_store_releases_the_stage_before_a_pending_stop() {
    use std::sync::atomic::Ordering;
    const STAGE: u64 = 64 << 20;
    // The pending Stop is retried by Status, or by the person pressing Stop again.
    for (publish, retry_stop) in [(false, false), (false, true), (true, false)] {
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
        s5_review_until(|| arrived.load(Ordering::Acquire), "REVIEW_FULL_HOLD");
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
            // No ordinary room past the 32 MiB revocation headroom: the
            // artifact publishes only through the finished stage's credit.
            probe(STAGE + (32 << 20));
            release.store(true, Ordering::Release);
            let checking =
                s5_review_wait(&client, |phase| matches!(phase, ReviewPhase::Checking(_)));
            assert_eq!(checking.token, token, "REVIEW_PUBLISHED_ON_A_FULL_STORE");
            assert_eq!(s5_review_artifact(&host, &token).output.video_frames, 6);
            // A Status that finds the thread already gone retires the worker,
            // so a retired worker has finished too.
            s5_review_until(
                || {
                    shared
                        .lock()
                        .unwrap()
                        .review_fixture_worker()
                        .is_none_or(|(_, done)| done)
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
            s5_review_until(
                || {
                    shared
                        .lock()
                        .unwrap()
                        .review_fixture_worker()
                        .is_some_and(|(_, done)| done)
                },
                "REVIEW_FULL_WORKER_FINISHED",
            );
            let retry = if retry_stop {
                ReviewControl::Stop(token.clone())
            } else {
                ReviewControl::Status
            };
            let _ = client.request(HostCommand::Review(retry));
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
                "REVIEW_RETIREMENT_BEFORE_PENDING_STOP retry_stop={retry_stop}"
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
    s5_review_until(|| arrived.load(Ordering::Acquire), "REVIEW_SHUTDOWN_HOLD");
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
    // Without any host: a clean shutdown left no stage for another process to
    // prove retired.
    assert_eq!(
        s5_review_stage_count(&temp),
        0,
        "REVIEW_CLEAN_SHUTDOWN_SWEEPS_ITS_STAGE"
    );
    {
        let pool = kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        );
        let session = Ja1EditingSession::open(&temp.path("project.kinewright"), &pool).unwrap();
        let job = session
            .review_bundle()
            .unwrap()
            .review_job()
            .unwrap()
            .unwrap();
        assert!(
            job.token.job == token.job && job.stage.is_none() && job.phase == ReviewPhase::Queued,
            "REVIEW_CLEAN_SHUTDOWN_RETIRES_ITS_STAGE {:?} {:?}",
            job.phase,
            job.stage
        );
    }
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
    s5_review_until(
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
    // The Stop never persisted, and shutdown cannot persist it either: the
    // job reopens in Checking with the attempt of a process that is gone.
    drop(shared);
    drop(client);
    kinewright_project::io_observer::fail_once("review_stop_before_admission", || drop(host));
    let (host, client) = s5_review_host(temp.root());
    // The reopen thread unbinds once native recovery has settled.
    s5_review_until(
        || s5_review_attempt(&host).is_none(),
        "REVIEW_REOPEN_UNBINDS_THE_DEAD_ATTEMPT",
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
    s5_review_until(
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
    s5_review_until(
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
    s5_review_until(
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

#[derive(Clone, Copy, Debug)]
enum S5StopCase {
    /// Ordinary metadata reads are refused; the terminal budget is not.
    Ceiling,
    /// The review's own root write has an unknown outcome, so the bundle and
    /// its ledger are unsettled until the Stop settles them.
    Unsettled,
    /// A manual save's outcome is unknown; only the save may settle it.
    ForeignSave,
    /// An orchestration publication is pending behind the review's own
    /// uncertain write; nothing may settle until it is.
    HooksPending,
    /// A recovery switch published a root with an unknown outcome; only the
    /// switch's own retry settles it.
    SwitchUnknown,
}

/// An orchestration publication with an unknown outcome, owned by the hooks.
fn s5_review_pending_publication(
    owner: &mut HostCommandOwner,
    project: &kinewright_project::bundle_records::ProjectId,
) {
    let id = |n: u8| serde_json::Value::String(format!("{n:x}").repeat(32));
    let project_value = serde_json::to_value(project).unwrap();
    let identity = serde_json::from_value(serde_json::json!({
        "version": 1, "project": project_value, "run": id(1), "task": id(2),
        "attempt": id(3), "nonce": id(4), "owner_instance": id(5),
    }))
    .unwrap();
    let record = serde_json::from_value(serde_json::json!({
        "version": 1, "project": project_value, "run": id(1), "revision": 0,
        "start_unix_ms": 0, "deadline_unix_ms": 0, "last_unix_ms": 0,
        "elapsed_ms": 0, "clock_uncertain": false, "durable_event_bytes": 0,
        "tasks": [],
    }))
    .unwrap();
    owner.hooks.pending = Some(super::super::orchestration::PendingPublication::Cumulative(
        Box::new(identity),
        Box::new(record),
    ));
}

fn s5_review_uncertain_annotation(
    owner: &mut HostCommandOwner,
    key: &kinewright_project::studio_records::SnapshotKey,
) {
    let descriptor = owner
        .owner()
        .unwrap()
        .read_snapshot(key)
        .unwrap()
        .descriptor;
    let annotate = kinewright_project::io_observer::fail_once("root_rename_io", || {
        owner.owner_mut().unwrap().set_review_annotations(
            key,
            kinewright_project::review_job::empty_annotations(&descriptor),
        )
    });
    assert!(
        matches!(
            annotate,
            Err(kinewright_project::bundle::BundleError::DurabilityUnknown)
        ),
        "{annotate:?}"
    );
}

fn s5_review_stop_case(case: S5StopCase) {
    use kinewright_project::bundle::BundleError;
    use kinewright_project::io_observer::fail_once;
    let temp = kinewright_media::test_support::TempDirectory::new("review-stop-case");
    s5_review_seed(temp.root());
    let (host, client) = s5_review_host(temp.root());
    // An earlier version whose annotations may change while the review of
    // `key` is active: the review's own version refuses that write.
    let other = matches!(case, S5StopCase::Unsettled | S5StopCase::HooksPending)
        .then(|| s5_review_version(&host));
    let key = s5_review_version(&host);
    let shared = host.owner.upgrade().unwrap();
    let checked = || {
        let token = s5_review_start(&client, &key);
        s5_review_wait(&client, |phase| matches!(phase, ReviewPhase::Checking(_)));
        s5_review_until(
            || {
                shared
                    .lock()
                    .unwrap()
                    .review_fixture_worker()
                    .is_none_or(|(_, done)| done)
            },
            "REVIEW_STOP_CASE_WORKER_FINISHED",
        );
        s5_review_call(&client, ReviewControl::Status).unwrap();
        token
    };
    if matches!(case, S5StopCase::SwitchUnknown) {
        // Checkpoints a recovery switch can select.
        ok(s5_review_save(&host, &client, 51), "REVIEW_STOP_CASE_SAVE");
        ok(s5_review_save(&host, &client, 52), "REVIEW_STOP_CASE_SAVE");
    }
    let mut token = checked();
    let stale = matches!(case, S5StopCase::Ceiling | S5StopCase::Unsettled).then(|| {
        // A Stop for a replaced job is answered by reading the live job.
        let stopped = s5_review_call(&client, ReviewControl::Stop(token.clone())).unwrap();
        assert_eq!(
            stopped.phase,
            ReviewPhase::Stopped,
            "REVIEW_STOP_CASE_STOPPED"
        );
        std::mem::replace(&mut token, checked())
    });
    let project = token.project.clone();
    {
        let mut owner = shared.lock().unwrap();
        assert!(owner.review_fixture_worker().is_none(), "worker retired");
        match case {
            S5StopCase::Ceiling => {
                // Over the revocation headroom, under a full decoded record.
                let limits = kinewright_project::storage_budget::StorageLimits {
                    metadata_bytes: 100 << 20,
                    ..Default::default()
                };
                let bundle = owner
                    .owner_mut()
                    .unwrap()
                    .review_source_bundle_mut(&project)
                    .unwrap();
                bundle.restrict_limits_probe(limits).unwrap();
                assert!(
                    bundle.review_status().is_err(),
                    "REVIEW_STOP_CASE_ORDINARY_READ_REFUSED"
                );
            }
            S5StopCase::Unsettled => {
                s5_review_uncertain_annotation(&mut owner, other.as_ref().unwrap());
                assert!(
                    owner
                        .owner()
                        .unwrap()
                        .store()
                        .unwrap()
                        .usage()
                        .unwrap()
                        .unsettled,
                    "REVIEW_STOP_CASE_LEDGER_UNSETTLED"
                );
            }
            S5StopCase::ForeignSave => {
                assert!(
                    owner
                        .core()
                        .request(kinewright_core::Command::Do(history_marker(41)))
                        .is_ok()
                );
                let save = fail_once("root_rename_io", || {
                    owner.owner_mut().unwrap().save_active()
                });
                assert!(
                    matches!(save, Err(BundleError::DurabilityUnknown)),
                    "{save:?}"
                );
                assert!(
                    owner
                        .owner()
                        .unwrap()
                        .review_source_bundle(&project)
                        .unwrap()
                        .pending_attempt()
                        .is_some()
                );
            }
            S5StopCase::HooksPending => {
                s5_review_uncertain_annotation(&mut owner, other.as_ref().unwrap());
                s5_review_pending_publication(&mut owner, &project);
            }
            S5StopCase::SwitchUnknown => {
                let session = owner.owner_mut().unwrap();
                let target = session.list_recovery_points().unwrap().pop().unwrap();
                session.prepare_recovery(target).unwrap();
                let selected = fail_once("root_parent_sync", || session.select_local());
                assert!(
                    matches!(
                        selected,
                        kinewright_project::bundle_switch::ProjectSelectionResult::Unknown
                    ),
                    "REVIEW_STOP_CASE_SWITCH_UNKNOWN {selected:?}"
                );
                assert!(session.switch_publication_unresolved());
            }
        }
    }
    if let Some(stale) = stale {
        let refused = client
            .request(HostCommand::Review(ReviewControl::Stop(stale)))
            .unwrap();
        assert!(
            refused
                .error
                .as_ref()
                .is_some_and(|error| error.message.contains("generation or version changed")),
            "REVIEW_STALE_STOP_READ_TERMINALLY {case:?} {refused:?}"
        );
    }
    let reply = s5_review_call(&client, ReviewControl::Stop(token.clone())).unwrap();
    assert_eq!(reply.token, token);
    match case {
        S5StopCase::Ceiling | S5StopCase::Unsettled => {
            assert!(
                !reply.stop_pending
                    && matches!(reply.phase, ReviewPhase::Stopping | ReviewPhase::Stopped),
                "REVIEW_STOP_TERMINAL_WRITE {case:?} {reply:?}"
            );
        }
        S5StopCase::ForeignSave => {
            assert!(reply.stop_pending, "REVIEW_STOP_WAITS_FOR_SAVE {reply:?}");
            assert!(
                shared
                    .lock()
                    .unwrap()
                    .owner()
                    .unwrap()
                    .review_source_bundle(&project)
                    .unwrap()
                    .pending_attempt()
                    .is_some(),
                "REVIEW_STOP_LEAVES_THE_SAVE_TO_ITS_OWNER"
            );
        }
        S5StopCase::HooksPending => {
            assert!(reply.stop_pending, "REVIEW_STOP_WAITS_FOR_HOOKS {reply:?}");
            let mut owner = shared.lock().unwrap();
            assert!(
                owner
                    .owner_mut()
                    .unwrap()
                    .review_source_bundle_mut(&project)
                    .unwrap()
                    .unsettled_probe(),
                "REVIEW_STOP_NEVER_SETTLES_BEHIND_A_PENDING_OWNER"
            );
            owner.hooks.pending = None;
            drop(owner);
            let stopped = s5_review_call(&client, ReviewControl::Stop(token.clone())).unwrap();
            assert!(
                !stopped.stop_pending && stopped.phase == ReviewPhase::Stopped,
                "REVIEW_STOP_SETTLES_ITS_OWN_WRITE_ONCE_FREE {stopped:?}"
            );
        }
        S5StopCase::SwitchUnknown => {
            assert!(reply.stop_pending, "REVIEW_STOP_WAITS_FOR_SWITCH {reply:?}");
            let mut owner = shared.lock().unwrap();
            let session = owner.owner_mut().unwrap();
            assert!(
                session.switch_publication_unresolved(),
                "REVIEW_STOP_LEAVES_THE_SWITCH_TO_ITS_OWNER"
            );
            // The switch's own retry proves its landed root.
            let selected = session.select_local();
            assert!(
                matches!(
                    selected,
                    kinewright_project::bundle_switch::ProjectSelectionResult::Activated(_)
                ),
                "REVIEW_STOP_CASE_SWITCH_SETTLES {selected:?}"
            );
            drop(owner);
            let stopped = s5_review_call(&client, ReviewControl::Stop(token.clone())).unwrap();
            assert!(
                !stopped.stop_pending && stopped.phase == ReviewPhase::Stopped,
                "REVIEW_STOP_AFTER_THE_SWITCH_SETTLES {stopped:?}"
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
fn s5_review_stop_reads_terminally_and_never_settles_another_owner() {
    for case in [
        S5StopCase::Ceiling,
        S5StopCase::Unsettled,
        S5StopCase::ForeignSave,
        S5StopCase::HooksPending,
        S5StopCase::SwitchUnknown,
    ] {
        eprintln!("S5_REVIEW_STOP_CASE {case:?}");
        s5_review_stop_case(case);
    }
}

/// After Save As the source's worker is still encoding: the copy cannot
/// queue a second worker until the first has retired.
#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s5_review_live_worker_refuses_a_second_queue() {
    use std::sync::atomic::Ordering;
    let temp = kinewright_media::test_support::TempDirectory::new("review-live-queue");
    s5_review_seed(temp.root());
    let (host, client) = s5_review_host(temp.root());
    let key = s5_review_version(&host);
    let (arrived, release, _) = s5_review_hold(&host, ReviewPoint::BeforeEncode);
    let source = s5_review_start(&client, &key);
    s5_review_until(|| arrived.load(Ordering::Acquire), "REVIEW_LIVE_HOLD");
    let shared = host.owner.upgrade().unwrap();
    shared
        .lock()
        .unwrap()
        .owner_mut()
        .unwrap()
        .save_as(&temp.path("copy.kinewright"), StorageLimits::default())
        .unwrap();
    let copy_key = s5_review_version(&host);
    let queued = client
        .request(HostCommand::Review(ReviewControl::QueueDefault(
            copy_key.clone(),
        )))
        .unwrap();
    release.store(true, Ordering::Release);
    assert!(
        queued
            .error
            .as_ref()
            .is_some_and(|error| error.message.contains("still encoding or retiring")),
        "REVIEW_SECOND_QUEUE_REFUSED {:?}",
        queued.error
    );
    s5_review_until(
        || {
            shared
                .lock()
                .unwrap()
                .review_fixture_worker()
                .is_some_and(|(_, done)| done)
        },
        "REVIEW_LIVE_WORKER_FINISHED",
    );
    let copy = s5_review_call(&client, ReviewControl::QueueDefault(copy_key)).unwrap();
    assert_ne!(
        copy.token.project, source.project,
        "REVIEW_QUEUE_AFTER_RETIREMENT"
    );
    assert!(
        shared.lock().unwrap().review_fixture_worker().is_none(),
        "the source worker retired before the copy queued"
    );
}

/// A retained source whose disk is full cannot record its worker's
/// retirement; the active copy's commands are not failed by it.
#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s5_review_retained_retirement_failure_stays_with_its_project() {
    let temp = kinewright_media::test_support::TempDirectory::new("review-retained-retire");
    s5_review_seed(temp.root());
    let (host, client) = s5_review_host(temp.root());
    let key = s5_review_version(&host);
    let source = s5_review_start(&client, &key);
    let shared = host.owner.upgrade().unwrap();
    // No Status: the finished source worker is still unretired at Save As.
    s5_review_until(
        || {
            shared
                .lock()
                .unwrap()
                .review_fixture_worker()
                .is_some_and(|(_, done)| done)
        },
        "REVIEW_RETAINED_WORKER_FINISHED",
    );
    {
        let mut owner = shared.lock().unwrap();
        owner
            .owner_mut()
            .unwrap()
            .save_as(&temp.path("copy.kinewright"), StorageLimits::default())
            .unwrap();
        owner
            .owner()
            .unwrap()
            .review_source_bundle(&source.project)
            .unwrap()
            .store()
            .free_space_probe(Some(0))
            .unwrap();
    }
    let status = client
        .request(HostCommand::Review(ReviewControl::Status))
        .unwrap();
    assert!(
        status.error.is_none(),
        "REVIEW_RETAINED_FAILURE_STAYS_WITH_ITS_PROJECT {:?}",
        status.error
    );
    let owner = shared.lock().unwrap();
    assert!(
        owner.review_fixture_worker().is_none(),
        "the worker retired"
    );
    let bundle = owner
        .owner()
        .unwrap()
        .review_source_bundle(&source.project)
        .unwrap();
    bundle.store().free_space_probe(None).unwrap();
    assert!(
        bundle
            .review_job()
            .unwrap()
            .is_some_and(|job| job.stage.is_some()),
        "the source records its stage for its own next command"
    );
}

/// A Stop for a replaced job, armed behind a pending owner, is dropped by the
/// next Stop through a terminal read even at the metadata ceiling.
#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s5_review_stale_pending_stop_is_dropped_at_the_ceiling() {
    let temp = kinewright_media::test_support::TempDirectory::new("review-stale-pending");
    s5_review_seed(temp.root());
    let (host, client) = s5_review_host(temp.root());
    let key = s5_review_version(&host);
    let shared = host.owner.upgrade().unwrap();
    let checked = || {
        let token = s5_review_start(&client, &key);
        s5_review_wait(&client, |phase| matches!(phase, ReviewPhase::Checking(_)));
        s5_review_until(
            || {
                shared
                    .lock()
                    .unwrap()
                    .review_fixture_worker()
                    .is_none_or(|(_, done)| done)
            },
            "REVIEW_STALE_PENDING_WORKER_FINISHED",
        );
        s5_review_call(&client, ReviewControl::Status).unwrap();
        token
    };
    let stale = checked();
    let stopped = s5_review_call(&client, ReviewControl::Stop(stale.clone())).unwrap();
    assert_eq!(stopped.phase, ReviewPhase::Stopped);
    let token = checked();
    s5_review_pending_publication(&mut shared.lock().unwrap(), &token.project);
    let armed = s5_review_call(&client, ReviewControl::Stop(stale)).unwrap();
    assert!(armed.stop_pending, "REVIEW_STALE_STOP_ARMED {armed:?}");
    {
        let mut owner = shared.lock().unwrap();
        owner.hooks.pending = None;
        // Over the revocation headroom, under a full decoded record.
        let bundle = owner
            .owner_mut()
            .unwrap()
            .review_source_bundle_mut(&token.project)
            .unwrap();
        bundle
            .restrict_limits_probe(kinewright_project::storage_budget::StorageLimits {
                metadata_bytes: 100 << 20,
                ..Default::default()
            })
            .unwrap();
        assert!(bundle.review_status().is_err(), "ordinary reads refused");
    }
    let reply = client
        .request(HostCommand::Review(ReviewControl::Stop(token.clone())))
        .unwrap();
    assert!(
        reply.error.is_none()
            && reply.review.as_ref().is_some_and(|status| {
                status.token == token
                    && !status.stop_pending
                    && matches!(status.phase, ReviewPhase::Stopping | ReviewPhase::Stopped)
            }),
        "REVIEW_STALE_PENDING_STOP_DROPPED_AT_CEILING {:?} {:?}",
        reply.error,
        reply.review
    );
}

/// A released copy never takes a live review worker or an unretired stage
/// with it; a finished worker retires before the copy is released.
#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s5_review_release_waits_for_the_review_worker() {
    use std::sync::atomic::Ordering;
    let temp = kinewright_media::test_support::TempDirectory::new("review-release");
    s5_review_seed(temp.root());
    let (host, client) = s5_review_host(temp.root());
    let key = s5_review_version(&host);
    let (arrived, release, _) = s5_review_hold(&host, ReviewPoint::BeforeEncode);
    let source = s5_review_start(&client, &key);
    s5_review_until(|| arrived.load(Ordering::Acquire), "REVIEW_RELEASE_HOLD");
    let shared = host.owner.upgrade().unwrap();
    let source_path = temp.path("project.kinewright");
    let prepared = (|| {
        let mut owner = shared.lock().unwrap();
        owner
            .owner_mut()
            .unwrap()
            .save_as(&temp.path("copy.kinewright"), StorageLimits::default())
            .map_err(|error| format!("save as: {error}"))?;
        owner
            .core()
            .request(kinewright_core::Command::Do(history_marker(61)))
            .map_err(|error| format!("edit: {error:?}"))?;
        let session = owner.owner_mut().unwrap();
        session
            .save_active()
            .map_err(|error| format!("save: {error}"))?;
        // Selecting the copy's predecessor seals the source's actor.
        let target = session
            .list_recovery_points()
            .map_err(|error| format!("points: {error}"))?
            .pop()
            .ok_or("no recovery point")?;
        session
            .prepare_recovery(target)
            .map_err(|error| format!("prepare: {error}"))?;
        match session.select_local() {
            kinewright_project::bundle_switch::ProjectSelectionResult::Activated(_) => {}
            other => return Err(format!("select: {other:?}")),
        }
        if !session.is_capture_destination(&source_path) {
            return Err("the source is not a completed capture".into());
        }
        Ok::<_, String>(())
    })();
    let live = prepared.as_ref().ok().map(|()| {
        let preview = client
            .request(HostCommand::PreviewRelease {
                destination: source_path.clone(),
            })
            .unwrap();
        assert!(preview.error.is_none(), "{:?}", preview.error);
        client.request(HostCommand::Release).unwrap()
    });
    release.store(true, Ordering::Release);
    prepared.unwrap_or_else(|error| panic!("REVIEW_RELEASE_FIXTURE {error}"));
    let live = live.unwrap();
    assert!(
        live.error
            .as_ref()
            .is_some_and(|error| error.message.contains("still encoding or retiring")),
        "REVIEW_LIVE_WORKER_KEEPS_ITS_COPY {:?}",
        live.error
    );
    s5_review_until(
        || {
            shared
                .lock()
                .unwrap()
                .review_fixture_worker()
                .is_some_and(|(_, done)| done)
        },
        "REVIEW_RELEASE_WORKER_FINISHED",
    );
    let preview = client
        .request(HostCommand::PreviewRelease {
            destination: source_path.clone(),
        })
        .unwrap();
    assert!(preview.error.is_none(), "{:?}", preview.error);
    let released = client.request(HostCommand::Release).unwrap();
    assert!(
        released.error.is_none(),
        "REVIEW_FINISHED_WORKER_RETIRES_BEFORE_RELEASE {:?}",
        released.error
    );
    let owner = shared.lock().unwrap();
    assert!(
        owner.review_fixture_worker().is_none()
            && owner
                .owner()
                .unwrap()
                .review_source_bundle(&source.project)
                .is_err(),
        "REVIEW_RELEASED_COPY_TOOK_NO_WORKER"
    );
    drop(owner);
    assert_eq!(
        s5_review_stage_count(&temp),
        0,
        "REVIEW_RELEASE_STAGE_SWEPT"
    );
}

/// A switch whose outcome is unknown is settled only by its own retry: review
/// results and review writes wait behind it, as they wait behind a hook.
#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s5_review_results_wait_for_an_unresolved_switch() {
    use kinewright_project::bundle_switch::ProjectSelectionResult;
    use kinewright_project::io_observer::fail_once;
    let temp = kinewright_media::test_support::TempDirectory::new("review-switch-results");
    s5_review_seed(temp.root());
    let (host, client) = s5_review_host(temp.root());
    let key = s5_review_version(&host);
    // Checkpoints a recovery switch can select.
    ok(
        s5_review_save(&host, &client, 71),
        "REVIEW_SWITCH_CASE_SAVE",
    );
    ok(
        s5_review_save(&host, &client, 72),
        "REVIEW_SWITCH_CASE_SAVE",
    );
    let shared = host.owner.upgrade().unwrap();
    let token = s5_review_start(&client, &key);
    s5_review_wait(&client, |phase| matches!(phase, ReviewPhase::Checking(_)));
    s5_review_until(
        || {
            shared
                .lock()
                .unwrap()
                .review_fixture_worker()
                .is_none_or(|(_, done)| done)
        },
        "REVIEW_SWITCH_CASE_WORKER_FINISHED",
    );
    s5_review_call(&client, ReviewControl::Status).unwrap();
    let binding = ReviewCheckBinding {
        configuration_sha256: [7; 32],
        reviewer_attempt: kinewright_project::bundle_records::SessionId::fresh().unwrap(),
    };
    let record = UpdateReviewJob::RecordCheck {
        binding: binding.clone(),
        row: kinewright_core::output_review::RequiredReviewRow::StructureAndAvIntegrity,
        result: kinewright_core::output_review::ReviewRowResult::Pass,
    };
    let mut owner = shared.lock().unwrap();
    owner
        .record_review_check(&token, UpdateReviewJob::BindChecks(binding))
        .expect("REVIEW_SWITCH_CASE_BOUND");
    let session = owner.owner_mut().unwrap();
    let target = session.list_recovery_points().unwrap().pop().unwrap();
    session.prepare_recovery(target).unwrap();
    let selected = fail_once("root_parent_sync", || session.select_local());
    assert!(
        matches!(selected, ProjectSelectionResult::Unknown),
        "REVIEW_SWITCH_CASE_UNKNOWN {selected:?}"
    );
    let fenced = owner
        .record_review_check(&token, record)
        .expect_err("REVIEW_RESULTS_WAIT_FOR_SWITCH");
    assert!(
        fenced.to_string().contains("authority was revoked"),
        "REVIEW_RESULTS_WAIT_FOR_SWITCH: {fenced}"
    );
    assert!(
        owner.owner().unwrap().switch_publication_unresolved(),
        "REVIEW_RESULT_LEAVES_THE_SWITCH_TO_ITS_OWNER"
    );
    drop(owner);
    let annotate = client
        .request(HostCommand::Review(ReviewControl::Annotate {
            snapshot: key.clone(),
            fields: kinewright_project::review_job::ReviewAnnotationFields::default(),
        }))
        .unwrap();
    assert!(
        annotate.error.as_ref().is_some_and(|error| error
            .message
            .contains("publication requires reconciliation")),
        "REVIEW_WRITES_WAIT_FOR_SWITCH {annotate:?}"
    );
    let mut owner = shared.lock().unwrap();
    let session = owner.owner_mut().unwrap();
    assert!(
        session.switch_publication_unresolved(),
        "REVIEW_WRITE_LEAVES_THE_SWITCH_TO_ITS_OWNER"
    );
    // The switch's own retry proves its landed root.
    let selected = session.select_local();
    assert!(
        matches!(selected, ProjectSelectionResult::Activated(_)),
        "REVIEW_SWITCH_CASE_SETTLES {selected:?}"
    );
}

/// A clean shutdown records a retained source's worker retirement on that
/// source, so no other process waits on a stage this process already swept.
#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s5_review_clean_shutdown_retires_a_retained_source_stage() {
    let temp = kinewright_media::test_support::TempDirectory::new("review-shutdown-source");
    s5_review_seed(temp.root());
    let (host, client) = s5_review_host(temp.root());
    let key = s5_review_version(&host);
    let source = s5_review_start(&client, &key);
    let shared = host.owner.upgrade().unwrap();
    // No Status: the finished source worker is still unretired at Save As.
    s5_review_until(
        || {
            shared
                .lock()
                .unwrap()
                .review_fixture_worker()
                .is_some_and(|(_, done)| done)
        },
        "REVIEW_SHUTDOWN_SOURCE_WORKER_FINISHED",
    );
    shared
        .lock()
        .unwrap()
        .owner_mut()
        .unwrap()
        .save_as(&temp.path("copy.kinewright"), StorageLimits::default())
        .unwrap();
    drop(shared);
    drop(client);
    drop(host);
    assert_eq!(
        s5_review_stage_count(&temp),
        0,
        "REVIEW_SHUTDOWN_SWEEPS_THE_SOURCE_STAGE"
    );
    let pool = kinewright_project::operation_history::HistoryPool::new(
        kinewright_project::operation_history::PoolLimits::default(),
    );
    let session = Ja1EditingSession::open(&temp.path("project.kinewright"), &pool).unwrap();
    let job = session
        .review_bundle()
        .unwrap()
        .review_job()
        .unwrap()
        .unwrap();
    assert!(
        job.token.job == source.job && job.stage.is_none(),
        "REVIEW_SHUTDOWN_RETIRES_THE_SOURCE_STAGE {:?} {:?}",
        job.phase,
        job.stage
    );
}
