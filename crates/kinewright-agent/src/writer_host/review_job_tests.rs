use super::super::review_job::ReviewPoint;
use kinewright_project::review_job::{
    ReviewArtifact, ReviewCheckBinding, ReviewControl, ReviewEvidence, ReviewPhase, ReviewStatus,
    ReviewToken, UpdateReviewJob,
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
    let point = match std::env::var("KINEWRIGHT_REVIEW_CRASH_CUT")
        .unwrap()
        .as_str()
    {
        "unpublished-stage" => ReviewPoint::BeforeInstall,
        "installed-stage" => ReviewPoint::BeforePublication,
        "published-artifact" => ReviewPoint::AfterPublication,
        other => panic!("unknown review crash cut {other}"),
    };
    let (host, client) = s5_review_host(&root);
    let snapshot = s5_review_version(&host);
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
        let (host, client) = s5_review_host(temp.root());
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
    wrong.snapshot = prior;
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
}
