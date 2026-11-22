use super::*;
use crate::{
    bundle::BundleWriteAdmission,
    operation_actor::Core,
    receipts::{CommandId, ReceiptLookup, ReceiptResult},
    studio_records::{
        MetadataChange, MetadataResult, MetadataScope, MetadataUpdate, PublishSnapshot, VersionId,
    },
};
use kinewright_core::{
    DeliveryEncodeDepth, DeliveryProfile, Document, ExportCancellation, TimeCode,
    output_review::{OverlayExpectations, ReviewInventory},
};

fn version(bundle: &mut ProjectBundle) -> SnapshotDescriptor {
    let issuer = bundle.issue_metadata_issuer().unwrap();
    let envelope = bundle.metadata_envelope(CommandId {
        issuer,
        sequence: 1,
    });
    let update = MetadataUpdate {
        expected_root_generation: bundle.generation,
        change: MetadataChange::PublishSnapshot(PublishSnapshot {
            version: VersionId::fresh().unwrap(),
            expected_head: bundle.manual_head().unwrap().clone(),
            checkpoint_ref: bundle
                .checkpoint_reference(bundle.manual_branch_id())
                .unwrap(),
            parent_version: None,
            run_provenance: None,
            revision_refs: vec![],
        }),
    };
    let ReceiptLookup::Committed(receipt) = bundle
        .metadata_command(&envelope, MetadataScope::Snapshot, &update)
        .unwrap()
    else {
        panic!("expected committed fixture version");
    };
    let ReceiptResult::Metadata {
        result: MetadataResult::Snapshot(key),
    } = receipt.result
    else {
        panic!("expected fixture version receipt");
    };
    bundle.read_snapshot(&key).unwrap().descriptor
}

fn digest(value: &impl Serialize) -> [u8; 32] {
    let hash = kinewright_media::sha256_bytes(&encode(value).unwrap());
    std::array::from_fn(|i| u8::from_str_radix(&hash[i * 2..i * 2 + 2], 16).unwrap())
}

fn missing<T>() -> ReviewInventory<T> {
    ReviewInventory::NotDetermined { known: vec![] }
}

fn queue(
    bundle: &mut ProjectBundle,
    descriptor: &SnapshotDescriptor,
    raster: (u32, u32),
) -> ReviewToken {
    queue_limited(bundle, descriptor, raster, 1 << 20)
}

fn queue_limited(
    bundle: &mut ProjectBundle,
    descriptor: &SnapshotDescriptor,
    raster: (u32, u32),
    byte_limit: u64,
) -> ReviewToken {
    let rulebook = bundle.approved_rulebook().cloned();
    queue_ruled(bundle, descriptor, raster, byte_limit, rulebook)
}

fn queue_ruled(
    bundle: &mut ProjectBundle,
    descriptor: &SnapshotDescriptor,
    raster: (u32, u32),
    byte_limit: u64,
    rulebook: Option<ObjectRef>,
) -> ReviewToken {
    let snapshot = bundle.read_snapshot(&descriptor.key).unwrap();
    let inputs = snapshot.open_review_inputs().unwrap();
    let mut settings = DeliveryProfile::SourceMaster.export_settings(
        &inputs.document,
        DeliveryEncodeDepth::Eight,
        ExportCancellation::default(),
    );
    settings.resolution = raster;
    let expectations = ReviewExpectations {
        identity: ReviewCutIdentity {
            snapshot: ReviewSnapshotId(descriptor.identity.digest()),
            input_manifest_sha256: digest(&*inputs.manifest),
            export_settings_sha256: digest(&settings),
        },
        project_fps: inputs.document.fps,
        output_fps: settings.fps,
        raster,
        output_frames: TimeCode(1),
        transcripts: vec![],
        segments: vec![],
        kept_words: missing(),
        speech_windows: missing(),
        uncovered_speech_segments: vec![],
        intended_pauses: missing(),
        av_landmarks: missing(),
        overlays: OverlayExpectations {
            annotated_off_output: vec![],
            captions: missing(),
            titles: missing(),
            cards: missing(),
        },
    };
    bundle
        .queue_review(descriptor, rulebook, settings, byte_limit, expectations)
        .unwrap()
}

fn fixture_timing() -> kinewright_media::ReviewOutputTiming {
    kinewright_media::ReviewOutputTiming {
        decoder_version: 1,
        video: kinewright_media::ReviewOutputStream {
            index: 0,
            decoder: "fixture".into(),
            time_base: (1, 30),
            start_pts: 0,
            duration_pts: 1,
        },
        audio: None,
        video_first_pts: 0,
        video_pts_step: None,
        video_frames: 1,
    }
}

fn publish_fixture_evidence(
    bundle: &mut ProjectBundle,
    temp: &kinewright_media::test_support::TempDirectory,
    raster: (u32, u32),
) -> (SnapshotKey, ObjectRef, ObjectRef) {
    let descriptor = version(bundle);
    let queued = queue(bundle, &descriptor, raster);
    let encoding = bundle
        .update_review_job(
            &queued,
            UpdateReviewJob::BeginEncoding(ReviewStage {
                id: SessionId::fresh().unwrap(),
                boot: "00000000-0000-0000-0000-000000000000".into(),
                pid: 1,
                process_start: 1,
                byte_limit: 1 << 20,
            }),
        )
        .unwrap();
    // Exercise the retention boundary with distinct bytes. Real encoding and
    // decoder timing are covered through the WriterHost.
    let bytes = descriptor.key.version.as_str().as_bytes();
    let path = temp.path("artifact-fixture");
    std::fs::write(&path, bytes).unwrap();
    let object = ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedMedia,
    };
    bundle
        .publish_review_artifact(
            &encoding.token,
            &mut std::fs::File::open(path).unwrap(),
            &object,
            kinewright_media::ReviewOutputTiming {
                decoder_version: 1,
                video: kinewright_media::ReviewOutputStream {
                    index: 0,
                    decoder: "fixture".into(),
                    time_base: (1, 30),
                    start_pts: 0,
                    duration_pts: 1,
                },
                audio: None,
                video_first_pts: 0,
                video_pts_step: None,
                video_frames: 1,
            },
        )
        .unwrap();
    let evidence = bundle
        .read_snapshot(&descriptor.key)
        .unwrap()
        .descriptor
        .review_evidence_ref
        .unwrap();
    bundle
        .update_review_job(&encoding.token, UpdateReviewJob::Stop)
        .unwrap();
    bundle
        .update_review_job(&encoding.token, UpdateReviewJob::RetireStopped)
        .unwrap();
    (descriptor.key, evidence, object)
}

fn collect(bundle: &mut ProjectBundle) {
    for _ in 0..3 {
        BundleCommit::publish_maintenance(bundle).unwrap();
    }
    bundle.collect_after_ack().unwrap();
}

#[test]
fn s5_review_collection_keeps_version_evidence_and_releases_retired_execution() {
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    let temp = TempDirectory::new_in(&workspace_target_dir(), "review-collection");
    let core = Core::for_test(Document::default()).unwrap();
    let mut bundle = ProjectBundle::activate(
        &temp.path("project.kinewright"),
        BundleWriteAdmission::new_document(),
        core.history_pool(),
    )
    .unwrap();
    let inputs = bundle.empty_inputs().unwrap();
    bundle.save(&core, &inputs.reference).unwrap();
    let (kept, kept_evidence, kept_media) = publish_fixture_evidence(&mut bundle, &temp, (64, 64));
    let (discarded, discarded_evidence, discarded_media) =
        publish_fixture_evidence(&mut bundle, &temp, (128, 64));
    let started = version(&mut bundle);
    let mut annotations = empty_annotations(&started);
    annotations.intended_pauses.push(TimeCode(1)..TimeCode(2));
    bundle
        .set_review_annotations(&started.key, annotations)
        .unwrap();
    let frozen = bundle.read_snapshot(&started.key).unwrap().descriptor;
    let token = queue(&mut bundle, &frozen, (128, 128));
    bundle
        .set_review_annotations_unfenced_for_test(&frozen.key, empty_annotations(&frozen))
        .unwrap();
    let active = bundle.review_job().unwrap().unwrap();
    let request = bundle
        .store
        .read_record::<ReviewRequest>(&active.request_ref)
        .unwrap();
    let transient = [
        active.request_ref.clone(),
        request.expectations_ref.clone(),
        request.annotations_ref.clone(),
    ];
    drop(request);
    drop(active);

    let mut index = bundle.version_index().unwrap().take();
    assert!(index.snapshots.remove(&discarded.version).is_some());
    let index = bundle.store.admit_record(&index).unwrap();
    bundle
        .publish_review_metadata(ValidatedMetadataUpdate {
            versions: Some(index.reference.clone()),
            seals: vec![index],
            ..Default::default()
        })
        .unwrap();
    collect(&mut bundle);
    for reference in &transient {
        bundle
            .store
            .verify(reference)
            .expect("REVIEW_ACTIVE_FROZEN_DEPENDENCY_RETAINED");
    }
    for reference in [&discarded_evidence, &discarded_media] {
        assert!(
            bundle.store.verify(reference).is_err(),
            "REVIEW_DISCARDED_EVIDENCE_COLLECTED"
        );
    }

    bundle
        .update_review_job(&token, UpdateReviewJob::Stop)
        .unwrap();
    bundle
        .update_review_job(&token, UpdateReviewJob::RetireStopped)
        .unwrap();
    collect(&mut bundle);
    for reference in &transient {
        assert!(
            bundle.store.verify(reference).is_err(),
            "REVIEW_RETIRED_DEPENDENCY_COLLECTED"
        );
    }
    assert_eq!(
        bundle
            .read_snapshot(&kept)
            .unwrap()
            .descriptor
            .review_evidence_ref
            .as_ref(),
        Some(&kept_evidence)
    );
    for reference in [&kept_evidence, &kept_media] {
        bundle
            .store
            .verify(reference)
            .expect("REVIEW_KEPT_EVIDENCE_RETAINED");
    }
}

fn saved_bundle(
    temp: &kinewright_media::test_support::TempDirectory,
) -> (Core, ProjectBundle, ObjectRef) {
    let core = Core::for_test(Document::default()).unwrap();
    let mut bundle = ProjectBundle::activate(
        &temp.path("project.kinewright"),
        BundleWriteAdmission::new_document(),
        core.history_pool(),
    )
    .unwrap();
    let inputs = bundle.empty_inputs().unwrap().reference.clone();
    bundle.save(&core, &inputs).unwrap();
    (core, bundle, inputs)
}

// Bytes a compressing filesystem cannot shrink.
fn incompressible(length: usize) -> Vec<u8> {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut bytes = Vec::with_capacity(length + 8);
    while bytes.len() < length {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        bytes.extend_from_slice(&state.to_le_bytes());
    }
    bytes.truncate(length);
    bytes
}

/// Keep `bytes` as the published review artifact of an earlier version.
fn retain_owned_media(
    bundle: &mut ProjectBundle,
    temp: &kinewright_media::test_support::TempDirectory,
    bytes: &[u8],
) {
    let descriptor = version(bundle);
    let queued = queue_limited(bundle, &descriptor, (64, 64), bytes.len() as u64);
    let encoding = bundle
        .update_review_job(
            &queued,
            UpdateReviewJob::BeginEncoding(ReviewStage {
                id: SessionId::fresh().unwrap(),
                boot: "00000000-0000-0000-0000-000000000000".into(),
                pid: 1,
                process_start: 1,
                byte_limit: bytes.len() as u64,
            }),
        )
        .unwrap();
    let path = temp.path("owned-media");
    std::fs::write(&path, bytes).unwrap();
    let media = ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedMedia,
    };
    bundle
        .publish_review_artifact(
            &encoding.token,
            &mut std::fs::File::open(&path).unwrap(),
            &media,
            fixture_timing(),
        )
        .unwrap();
    std::fs::remove_file(path).unwrap();
    bundle
        .update_review_job(&encoding.token, UpdateReviewJob::Stop)
        .unwrap();
    bundle
        .update_review_job(&encoding.token, UpdateReviewJob::RetireStopped)
        .unwrap();
}

#[derive(Clone, Copy, Debug)]
enum StagedEnding {
    Stop,
    Pause,
    RunStop,
    Publish,
}

/// A project with 112 MiB of owned media runs a 128 MiB stage that holds 4 MiB
/// on a disk with little more room than the charge. The charge would overflow
/// the owned-media allowance. Ordinary writes reach their reduced ceiling;
/// terminal writes and publication are refused only for real lack of space.
#[test]
fn s5_review_store_filling_keeps_terminal_writes_and_publication() {
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    const OWNED_MEDIA: usize = 112 << 20;
    const STAGE: u64 = 128 << 20;
    const STAGED: usize = 4 << 20;
    let owned_media = incompressible(OWNED_MEDIA);
    for ending in [
        StagedEnding::Stop,
        StagedEnding::Pause,
        StagedEnding::RunStop,
        StagedEnding::Publish,
    ] {
        let temp = TempDirectory::new_in(&workspace_target_dir(), "review-filling");
        let (_core, mut bundle, _) = saved_bundle(&temp);
        retain_owned_media(&mut bundle, &temp, &owned_media);
        let descriptor = version(&mut bundle);
        let token = queue_limited(&mut bundle, &descriptor, (64, 64), STAGE);
        let stage = ReviewStage {
            id: SessionId::fresh().unwrap(),
            boot: "00000000-0000-0000-0000-000000000000".into(),
            pid: 1,
            process_start: 1,
            byte_limit: STAGE,
        };
        let token = bundle
            .update_review_job(&token, UpdateReviewJob::BeginEncoding(stage.clone()))
            .unwrap()
            .token;
        let charge = bundle.store.reserve_review_stage(&stage).unwrap();
        let staging = bundle.store.create_review_staging(&stage, &charge).unwrap();
        let bytes = incompressible(STAGED);
        std::fs::write(staging.destination(), &bytes).unwrap();
        staging.open_output().unwrap().sync_all().unwrap();

        let room = fill_to_ceiling(&mut bundle, ending, STAGE);
        bundle.store.free_space_probe(Some(STAGE + room)).unwrap();
        if !matches!(ending, StagedEnding::Publish) {
            assert_ordinary_at_ceiling(&mut bundle, ending);
        }
        let phase = match ending {
            StagedEnding::Stop => bundle.update_review_job(&token, UpdateReviewJob::Stop),
            StagedEnding::Pause => {
                bundle.update_review_job(&token, UpdateReviewJob::Pause("full".into()))
            }
            StagedEnding::RunStop => {
                bundle.store.set_run_budget(true);
                // A real lack of space stays a capacity refusal for the review.
                bundle.store.free_space_probe(Some(STAGE)).unwrap();
                let full = bundle.update_review_job(&token, UpdateReviewJob::Stop);
                assert!(
                    matches!(full, Err(BundleError::Capacity(_))),
                    "REVIEW_TERMINAL_CAPACITY_IS_NOT_A_RUN_LIMIT: {full:?}"
                );
                assert!(!bundle.store.run_limit_reached(), "REVIEW_RUN_LIMIT_UNSET");
                bundle.settle_review_root().unwrap();
                bundle.store.free_space_probe(Some(STAGE + room)).unwrap();
                bundle.update_review_job(&token, UpdateReviewJob::Stop)
            }
            StagedEnding::Publish => {
                charge.finish_writing();
                let media = ObjectRef {
                    sha256: kinewright_media::sha256_bytes(&bytes),
                    byte_length: bytes.len() as u64,
                    record_kind: RecordKind::OwnedMedia,
                };
                bundle.publish_review_artifact(
                    &token,
                    &mut staging.open_output().unwrap(),
                    &media,
                    fixture_timing(),
                )
            }
        }
        .unwrap_or_else(|error| panic!("REVIEW_STAGED_TERMINAL_WRITE {ending:?}: {error}"))
        .phase;
        assert!(
            matches!(
                (ending, &phase),
                (
                    StagedEnding::Stop | StagedEnding::RunStop,
                    ReviewPhase::Stopping
                ) | (StagedEnding::Pause, ReviewPhase::Paused { .. })
                    | (StagedEnding::Publish, ReviewPhase::Checking(_))
            ),
            "{ending:?}: {phase:?}"
        );
        assert!(!bundle.store.run_limit_reached(), "REVIEW_RUN_LIMIT_UNSET");
        drop(staging);
        retire_staged(&mut bundle, &token, &stage, charge, &phase, ending);
    }
}

/// The stage writes and finishes between the two samples of one ordinary
/// reservation. The credit is sampled first, so the finished stage's credit is
/// never paired with free space read before its bytes landed.
#[test]
fn s5_review_stage_credit_is_sampled_before_free_space() {
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    const STAGE: u64 = 64 << 20;
    const STAGED: usize = 8 << 20;
    const CAPACITY: u64 = STAGE + (64 << 20);
    let temp = TempDirectory::new_in(&workspace_target_dir(), "review-credit-order");
    let (_core, bundle, _) = saved_bundle(&temp);
    let stage = ReviewStage {
        id: SessionId::fresh().unwrap(),
        boot: "00000000-0000-0000-0000-000000000000".into(),
        pid: 1,
        process_start: 1,
        byte_limit: STAGE,
    };
    let charge = std::sync::Arc::new(bundle.store.reserve_review_stage(&stage).unwrap());
    let staging = bundle.store.create_review_staging(&stage, &charge).unwrap();
    bundle.store.free_space_probe(Some(CAPACITY)).unwrap();
    let destination = staging.destination();
    let writer = std::sync::Arc::clone(&charge);
    bundle
        .store
        .free_sample_probe(Box::new(move || {
            std::fs::write(&destination, incompressible(STAGED)).unwrap();
            writer.finish_writing();
        }))
        .unwrap();
    // Free space after the stage finished is CAPACITY - STAGED and the stage
    // needs no more; anything past it is space the disk does not have.
    let over = CAPACITY - STAGED as u64 + (1 << 20);
    assert!(
        matches!(
            bundle.store.reserve(over, 0, false),
            Err(BundleError::Capacity("disk reserve"))
        ),
        "T1_CREDIT_NEVER_COVERS_UNSAMPLED_BYTES"
    );
    let honest = CAPACITY - STAGED as u64 - (1 << 20);
    drop(
        bundle
            .store
            .reserve(honest, 0, false)
            .expect("T1_FINISHED_CREDIT_HONOURED"),
    );
    drop(staging);
    bundle.store.sweep_review_staging(&stage).unwrap();
    drop(charge);
}

/// A Save As copy whose first publication is uncertain is settled only by its
/// `BundleCopy` owner: a review read on the copy, such as a Stop naming the
/// copy's project, never reconciles it first.
#[test]
fn s5_review_settle_leaves_an_uncertain_copy_to_its_owner() {
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    let temp = TempDirectory::new_in(&workspace_target_dir(), "review-copy-owner");
    let (_core, mut bundle, _) = saved_bundle(&temp);
    let outcome = crate::io_observer::fail_once("root_parent_sync", || {
        bundle.save_as_with_limits(&temp.path("copy.kinewright"), StorageLimits::default())
    })
    .unwrap();
    let crate::bundle_copy::SaveAsOutcome::Copied(mut copy) = outcome else {
        panic!("expected a copy");
    };
    assert!(
        matches!(
            copy.status(),
            crate::bundle_copy::CopyStatus::DurabilityUnknown
                | crate::bundle_copy::CopyStatus::CommittedWithError { .. }
        ),
        "U2_COPY_UNCERTAIN_STATUS {:?}",
        copy.status()
    );
    let target = copy.destination_owner_mut().unwrap();
    assert!(
        target.copy_publication.is_some() && target.unsettled,
        "U2_COPY_UNCERTAIN"
    );
    assert!(
        matches!(target.review_status_for_stop(), Err(BundleError::Unsettled)),
        "U2_REVIEW_SETTLE_WAITS_FOR_THE_COPY_OWNER"
    );
    assert!(
        target.copy_publication.is_some() && target.unsettled,
        "U2_COPY_LEFT_TO_ITS_OWNER"
    );
    // Its own owner reconciles it, after which review reads settle again.
    copy.reconcile().expect("U2_COPY_OWNER_RECONCILES");
    let target = copy.destination_owner_mut().unwrap();
    target
        .review_status_for_stop()
        .expect("U2_REVIEW_READ_AFTER_THE_COPY");
}

/// Limits at the reduced ceiling for the run's ordinary writes. Returns the
/// disk room left past the stage charge.
fn fill_to_ceiling(bundle: &mut ProjectBundle, ending: StagedEnding, stage: u64) -> u64 {
    let usage = bundle.store.usage().unwrap();
    let mut limits = StorageLimits {
        owned_bytes: usage.owned_bytes + (16 << 20),
        // Over the review headroom and under the staged charge. Ordinary
        // publication needs room for a full decoded record.
        metadata_bytes: if matches!(ending, StagedEnding::Publish) {
            StorageLimits::default().metadata_bytes
        } else {
            100 << 20
        },
        ..StorageLimits::default()
    };
    assert!(
        matches!(ending, StagedEnding::Publish)
            || usage.physical_bytes + stage > limits.owned_bytes + limits.metadata_bytes,
        "the charge would overflow the owned-media allowance"
    );
    let room = if matches!(ending, StagedEnding::Publish) {
        // 2 MiB of ordinary room after the disk headroom: the 4 MiB artifact
        // fits only through the finished stage's unused charge.
        DISK_HEADROOM + (2 << 20)
    } else {
        // Objects and disk at the reduced ceiling; 2 MiB is under the
        // staged bytes, so counting them twice leaves no room at all.
        limits.objects = usage.objects + OBJECT_HEADROOM;
        2 << 20
    };
    bundle.restrict_limits_probe(limits).unwrap();
    room
}

fn assert_ordinary_at_ceiling(bundle: &mut ProjectBundle, ending: StagedEnding) {
    let ordinary = bundle.store.admit_blob(
        &mut &b"ordinary"[..],
        &ObjectRef {
            sha256: kinewright_media::sha256_bytes(b"ordinary"),
            byte_length: 8,
            record_kind: RecordKind::OwnedEvidence,
        },
        &[],
    );
    assert!(
        matches!(ordinary, Err(BundleError::Capacity("disk reserve"))),
        "REVIEW_ORDINARY_WRITE_AT_CEILING {ending:?}: {:?}",
        ordinary.map(|_| ())
    );
}

/// Sweep, release the charge and retire; the released charge frees ordinary
/// space again.
fn retire_staged(
    bundle: &mut ProjectBundle,
    token: &ReviewToken,
    stage: &ReviewStage,
    charge: ReviewStageCharge,
    phase: &ReviewPhase,
    ending: StagedEnding,
) {
    charge.finish_writing();
    bundle.store.sweep_review_staging(stage).unwrap();
    drop(charge);
    bundle.store.set_run_budget(false);
    let retire = if matches!(phase, ReviewPhase::Stopping) {
        UpdateReviewJob::RetireStopped
    } else {
        UpdateReviewJob::RetireStage
    };
    bundle
        .update_review_job(token, retire)
        .unwrap_or_else(|error| panic!("REVIEW_STAGED_RETIREMENT {ending:?}: {error}"));
    let usage = bundle.store.usage().unwrap();
    assert_eq!(
        (usage.reserved_copy_bytes, usage.reserved_stage_bytes),
        (0, 0),
        "REVIEW_STAGE_CHARGE_RELEASED {ending:?}"
    );
    assert!(
        bundle.store.reserve(64 << 10, 0, false).is_ok(),
        "REVIEW_RELEASED_CHARGE_FREES_ORDINARY_SPACE {ending:?}"
    );
}

#[test]
fn s5_review_uncertain_publication_never_rides_on_a_later_save() {
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    let temp = TempDirectory::new_in(&workspace_target_dir(), "review-uncertain");
    let (core, mut bundle, inputs) = saved_bundle(&temp);
    let descriptor = version(&mut bundle);
    let token = queue(&mut bundle, &descriptor, (64, 64));
    let uncertain = crate::io_observer::fail_once("root_rename_io", || {
        bundle.update_review_job(&token, UpdateReviewJob::Stop)
    });
    assert!(
        matches!(uncertain, Err(BundleError::DurabilityUnknown)),
        "{uncertain:?}"
    );
    bundle.reconcile().unwrap();
    assert_eq!(
        bundle.review_job().unwrap().unwrap().phase,
        ReviewPhase::Queued,
        "the root was never renamed"
    );
    assert!(
        core.request(kinewright_core::Command::Do(
            kinewright_core::Operation::AddMarker {
                marker: kinewright_core::Marker {
                    id: kinewright_core::MarkerId(7),
                    position: TimeCode(1),
                    label: "after uncertainty".into(),
                    color_token: 0,
                },
            }
        ))
        .is_ok()
    );
    bundle.save(&core, &inputs).unwrap();
    assert_eq!(
        bundle.review_job().unwrap().unwrap().phase,
        ReviewPhase::Queued,
        "REVIEW_ABANDONED_UPDATE_NEVER_RIDES_ON_SAVE"
    );
    let stopped = bundle
        .update_review_job(&token, UpdateReviewJob::Stop)
        .expect("REVIEW_METADATA_COMMANDS_NOT_WEDGED");
    assert_eq!(stopped.phase, ReviewPhase::Stopping);
}

/// A sweep never deletes an entry it did not create; it refuses instead.
#[test]
fn s5_review_sweep_refuses_an_unexpected_stage_entry() {
    let temp = kinewright_media::test_support::TempDirectory::new("review-sweep-foreign");
    let (_core, bundle, _) = saved_bundle(&temp);
    let stage = ReviewStage {
        id: SessionId::fresh().unwrap(),
        boot: "00000000-0000-0000-0000-000000000000".into(),
        pid: 1,
        process_start: 1,
        byte_limit: 1 << 20,
    };
    let charge = bundle.store.reserve_review_stage(&stage).unwrap();
    let staging = bundle.store.create_review_staging(&stage, &charge).unwrap();
    let foreign = staging.destination().with_file_name("notes.txt");
    std::fs::write(&foreign, b"kept").unwrap();
    let refused = bundle
        .store
        .sweep_review_staging(&stage)
        .expect_err("REVIEW_SWEEP_REFUSES_FOREIGN_ENTRY");
    assert!(
        matches!(&refused, BundleError::Identity(message) if message == "unexpected review stage entry"),
        "REVIEW_SWEEP_REFUSES_FOREIGN_ENTRY: {refused:?}"
    );
    assert_eq!(
        std::fs::read(&foreign).unwrap(),
        b"kept",
        "REVIEW_SWEEP_KEEPS_FOREIGN_ENTRY"
    );
}

/// JA-1 S5a C5 rework (Neon F19): a checking job with its published artifact.
fn s5c5_checking(
    bundle: &mut ProjectBundle,
    temp: &kinewright_media::test_support::TempDirectory,
) -> ReviewStatus {
    let descriptor = version(bundle);
    let queued = queue(bundle, &descriptor, (64, 64));
    let encoding = bundle
        .update_review_job(
            &queued,
            UpdateReviewJob::BeginEncoding(ReviewStage {
                id: SessionId::fresh().unwrap(),
                boot: "00000000-0000-0000-0000-000000000000".into(),
                pid: 1,
                process_start: 1,
                byte_limit: 1 << 20,
            }),
        )
        .unwrap();
    let bytes = b"s5c5 review check artifact";
    let path = temp.path("s5c5-artifact");
    std::fs::write(&path, bytes).unwrap();
    let object = ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedMedia,
    };
    bundle
        .publish_review_artifact(
            &encoding.token,
            &mut std::fs::File::open(path).unwrap(),
            &object,
            fixture_timing(),
        )
        .unwrap()
}

#[test]
fn ja1_s5a_check_inputs_bind_every_record_to_the_token() {
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    let temp = TempDirectory::new_in(&workspace_target_dir(), "s5c5-check-binding");
    let core = Core::for_test(Document::default()).unwrap();
    let mut bundle = ProjectBundle::activate(
        &temp.path("project.kinewright"),
        BundleWriteAdmission::new_document(),
        core.history_pool(),
    )
    .unwrap();
    let inputs = bundle.empty_inputs().unwrap();
    bundle.save(&core, &inputs.reference).unwrap();
    let status = s5c5_checking(&mut bundle, &temp);
    assert!(matches!(status.phase, ReviewPhase::Checking(_)));
    let token = status.token.clone();

    // The current token opens verified bytes and the pinned inputs.
    let opened = bundle.prepare_review_check(&token).unwrap().open().unwrap();
    assert_eq!(opened.artifact.generation, token.generation);
    assert_eq!(opened.context.snapshot.descriptor.key, token.snapshot);
    drop(opened);

    // A token of another generation or job is refused under the lock.
    let mut stale = token.clone();
    stale.generation += 1;
    let refused = bundle.prepare_review_check(&stale).err().unwrap();
    assert!(
        refused.to_string().contains("token is not current"),
        "{refused}"
    );
    let mut other = token.clone();
    other.job = SessionId::fresh().unwrap();
    let refused = bundle.prepare_review_check(&other).err().unwrap();
    assert!(
        refused.to_string().contains("token is not current"),
        "{refused}"
    );

    // Every record must name the token, whatever the job record says.
    let job = bundle.review_job().unwrap().unwrap();
    let artifact = bundle
        .store
        .read_record::<ReviewArtifact>(job.artifact_ref.as_ref().unwrap())
        .unwrap();
    let request = bundle
        .store
        .read_record::<ReviewRequest>(&job.request_ref)
        .unwrap();
    check_inputs::check_review_check_binding(&token, &job, &artifact, &request).unwrap();
    let other_version = version(&mut bundle).key;
    let refused_as = |artifact: &ReviewArtifact, request: &ReviewRequest, wanted: &str| {
        let refusal = check_inputs::check_review_check_binding(&token, &job, artifact, request)
            .err()
            .map_or_else(|| "accepted".to_owned(), |error| error.to_string());
        assert!(
            refusal.contains(wanted),
            "REVIEW_C5_REFUSED {wanted}: {refusal}"
        );
    };
    let mut changed = (*artifact).clone();
    changed.job = SessionId::fresh().unwrap();
    refused_as(&changed, &request, "artifact belongs to another job");
    let mut changed = (*artifact).clone();
    changed.generation += 1;
    refused_as(&changed, &request, "artifact belongs to another generation");
    let mut changed = (*artifact).clone();
    changed.snapshot = other_version.clone();
    refused_as(&changed, &request, "artifact belongs to another version");
    let mut changed = (*artifact).clone();
    changed.request_ref = request.expectations_ref.clone();
    refused_as(&changed, &request, "artifact belongs to another request");
    let mut changed = (*request).clone();
    changed.snapshot = other_version;
    refused_as(&artifact, &changed, "request belongs to another version");
    let mut stopped = (*job).clone();
    stopped.phase = ReviewPhase::Stopping;
    let refused = check_inputs::check_review_check_binding(&token, &stopped, &artifact, &request)
        .err()
        .unwrap();
    assert!(
        refused.to_string().contains("job is not checking"),
        "{refused}"
    );
}

// JA-1 S5a commit 7: the seal, its receipt and its transaction.

const C7_DIGEST: [u8; 32] = [7; 32];

struct C7Ready {
    token: ReviewToken,
    binding: ReviewCheckBinding,
}

fn c7_attempt(bundle: &ProjectBundle) -> crate::orchestration_records::ExecutionIdentity {
    let fresh = || SessionId::fresh().unwrap();
    crate::orchestration_records::ExecutionIdentity {
        version: 1,
        project: bundle.project_id().clone(),
        run: crate::orchestration_records::RunId(fresh()),
        task: crate::orchestration_records::TaskId(fresh()),
        attempt: crate::orchestration_records::AttemptId(fresh()),
        nonce: fresh(),
        owner_instance: fresh(),
    }
}

/// The Reviewer's complete audit; `blocking` adds a missing required beat.
fn c7_audit(bundle: &mut ProjectBundle, key: &SnapshotKey, blocking: bool) {
    use crate::editorial::{AuditFinding, AuditKind, AuditObservation, AuditSubmission};
    let entry = |kind, finding, start_micros, text: &str| AuditObservation {
        kind,
        line_or_beat: "Opening".into(),
        output: crate::studio_records::AudioRange {
            start_micros,
            end_micros: start_micros + 1000,
        },
        text: text.into(),
        finding,
    };
    let mut entries = vec![
        entry(AuditKind::Retention, AuditFinding::Resolved, 0, "It holds."),
        entry(
            AuditKind::Hook,
            AuditFinding::Resolved,
            0,
            "The opening works.",
        ),
        entry(AuditKind::Story, AuditFinding::Taste, 0, "A longer pause."),
    ];
    if blocking {
        entries.push(entry(
            AuditKind::Story,
            AuditFinding::MissingRequiredBeat,
            5000,
            "The promised beat is missing.",
        ));
    }
    let (target, part) = AuditSubmission {
        version: key.version.clone(),
        snapshot: key.clone(),
        rulebook_ref: None,
        entries,
    }
    .attribute(c7_attempt(bundle));
    let change = MetadataChange::RetainEditorial {
        target,
        role: crate::orchestration_records::Role::Reviewer,
        frozen: crate::editorial::FrozenRulebook { revision: None },
        part,
    };
    let issuer = bundle.issue_metadata_issuer().unwrap();
    let envelope = bundle.metadata_envelope(crate::receipts::CommandId {
        issuer,
        sequence: 1,
    });
    let update = crate::studio_records::MetadataUpdate {
        expected_root_generation: bundle.generation,
        change,
    };
    let lookup = bundle
        .metadata_command(
            &envelope,
            crate::studio_records::MetadataScope::Editorial,
            &update,
        )
        .unwrap();
    assert!(
        matches!(lookup, crate::receipts::ReceiptLookup::Committed(_)),
        "C7_AUDIT_COMMITTED"
    );
}

/// Approve a house rule, so the project's rulebook moves on.
fn c7_approve_rule(bundle: &mut ProjectBundle, rule: &str) -> ObjectRef {
    let publish = |bundle: &mut ProjectBundle, change| {
        let issuer = bundle.issue_metadata_issuer().unwrap();
        let envelope = bundle.metadata_envelope(CommandId {
            issuer,
            sequence: 1,
        });
        let update = MetadataUpdate {
            expected_root_generation: bundle.generation,
            change,
        };
        let lookup = bundle
            .metadata_command(&envelope, MetadataScope::Editorial, &update)
            .unwrap();
        assert!(matches!(lookup, ReceiptLookup::Committed(_)), "C7_RULE");
    };
    let proposal = crate::editorial::RulebookProposal {
        proposed_by: c7_attempt(bundle),
        parent: bundle.approved_rulebook().cloned(),
        rule: rule.into(),
    };
    publish(bundle, MetadataChange::ProposeRulebook(proposal));
    let proposal = bundle
        .rulebook_proposal()
        .unwrap()
        .map(|(reference, _)| reference)
        .unwrap();
    publish(
        bundle,
        MetadataChange::DecideRulebook(crate::editorial::RulebookDecision {
            proposal,
            approve: true,
        }),
    );
    bundle.approved_rulebook().unwrap().clone()
}

#[derive(Clone, Copy, Default)]
struct C7Setup {
    blocking: bool,
    skip: Option<RequiredReviewRow>,
    keep_stage: bool,
    fault: Option<C7Fault>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum C7Fault {
    /// A house rule approved after the audit, before the queue.
    NewRulebook,
    /// The bound attempt is not a Reviewer task attempt on the cut.
    NotAReviewer,
}

/// A checked job: published artifact, staging retired unless kept, one
/// binding and every row but `skip` recorded Pass by its attempt.
fn c7_checked_staged(
    bundle: &mut ProjectBundle,
    temp: &kinewright_media::test_support::TempDirectory,
    blocking: bool,
    skip: Option<RequiredReviewRow>,
    retire_stage: bool,
) -> C7Ready {
    c7_checked_with(
        bundle,
        temp,
        C7Setup {
            blocking,
            skip,
            keep_stage: !retire_stage,
            ..C7Setup::default()
        },
    )
}

fn c7_checked_with(
    bundle: &mut ProjectBundle,
    temp: &kinewright_media::test_support::TempDirectory,
    setup: C7Setup,
) -> C7Ready {
    let C7Setup {
        blocking,
        skip,
        keep_stage,
        fault,
    } = setup;
    let retire_stage = !keep_stage;
    let descriptor = version(bundle);
    c7_audit(bundle, &descriptor.key, blocking);
    if fault == Some(C7Fault::NewRulebook) {
        c7_approve_rule(bundle, "Keep the promise.");
    }
    let queued = queue(bundle, &descriptor, (64, 64));
    let encoding = bundle
        .update_review_job(
            &queued,
            UpdateReviewJob::BeginEncoding(ReviewStage {
                id: SessionId::fresh().unwrap(),
                boot: "00000000-0000-0000-0000-000000000000".into(),
                pid: 1,
                process_start: 1,
                byte_limit: 1 << 20,
            }),
        )
        .unwrap();
    let bytes = descriptor.key.version.as_str().as_bytes();
    let path = temp.path(&format!("c7-artifact-{}", descriptor.key.version.as_str()));
    std::fs::write(&path, bytes).unwrap();
    let object = ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedMedia,
    };
    let token = bundle
        .publish_review_artifact(
            &encoding.token,
            &mut std::fs::File::open(path).unwrap(),
            &object,
            fixture_timing(),
        )
        .unwrap()
        .token;
    if retire_stage {
        bundle
            .update_review_job(&token, UpdateReviewJob::RetireStage)
            .unwrap();
    }
    let binding = ReviewCheckBinding {
        configuration_sha256: C7_DIGEST,
        reviewer_attempt: SessionId::fresh().unwrap(),
    };
    if fault != Some(C7Fault::NotAReviewer) {
        super::seal::test_reviewers::admit(&descriptor.checkpoint_ref, &binding.reviewer_attempt);
    }
    bundle
        .update_review_job(&token, UpdateReviewJob::BindChecks(binding.clone()))
        .unwrap();
    for &row in CoverageProfileId::OutputReviewV1.required_rows() {
        if Some(row) != skip {
            c7_record(bundle, &token, &binding, row);
        }
    }
    C7Ready { token, binding }
}

fn c7_checked(
    bundle: &mut ProjectBundle,
    temp: &kinewright_media::test_support::TempDirectory,
    blocking: bool,
    skip: Option<RequiredReviewRow>,
) -> C7Ready {
    c7_checked_staged(bundle, temp, blocking, skip, true)
}

fn c7_record(
    bundle: &mut ProjectBundle,
    token: &ReviewToken,
    binding: &ReviewCheckBinding,
    row: RequiredReviewRow,
) {
    bundle
        .update_review_job(
            token,
            UpdateReviewJob::RecordCheck {
                binding: binding.clone(),
                row,
                result: ReviewRowResult::Pass,
            },
        )
        .unwrap();
}

fn c7_verified(bundle: &ProjectBundle, token: &ReviewToken) -> VerifiedReviewArtifact {
    bundle
        .prepare_review_artifact(token)
        .unwrap()
        .verify_for_seal(&ExportCancellation::default())
        .unwrap()
}

fn c7_seal(bundle: &mut ProjectBundle, ready: &C7Ready) -> Result<SealResult, BundleError> {
    let verified = c7_verified(bundle, &ready.token);
    bundle.seal_review(
        &ready.token,
        &SessionId::fresh().unwrap(),
        C7_DIGEST,
        &verified,
    )
}

fn c7_receipts(bundle: &ProjectBundle, key: &SnapshotKey) -> Vec<ReviewReceipt> {
    let descriptor = bundle.read_snapshot(key).unwrap().descriptor;
    bundle
        .review_receipts(&descriptor)
        .unwrap()
        .into_iter()
        .map(|(_, receipt)| receipt.take())
        .collect()
}

/// The findings of a seal that was refused; the job is Paused with them.
fn c7_refused(bundle: &mut ProjectBundle, result: SealResult) -> Vec<ReviewFinding> {
    let SealResult::Paused(status) = result else {
        panic!("C7_SEAL_EXPECTED_REFUSAL: {result:?}");
    };
    let ReviewPhase::Paused { findings, .. } = &status.phase else {
        panic!("C7_SEAL_EXPECTED_PAUSED: {:?}", status.phase);
    };
    assert_eq!(
        bundle.review_job().unwrap().unwrap().phase,
        status.phase,
        "C7_PAUSE_DURABLE"
    );
    assert!(
        c7_receipts(bundle, &status.token.snapshot).is_empty(),
        "C7_REFUSED_SEAL_HAS_NO_RECEIPT"
    );
    for finding in findings {
        assert_eq!(finding.version, status.token.snapshot.version);
    }
    findings.clone()
}

/// Receipts as they persist; settings equality includes a live handle.
fn c7_same(left: &ReviewReceipt, right: &ReviewReceipt) -> bool {
    serde_json::to_value(left).unwrap() == serde_json::to_value(right).unwrap()
}

fn c7_has(findings: &[ReviewFinding], row: Option<RequiredReviewRow>, note: &str) -> bool {
    findings
        .iter()
        .any(|finding| finding.row == row && finding.note == note)
}

#[test]
fn ja1_s5a_c7_seal_publishes_one_receipt_and_retires_the_job() {
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    let temp = TempDirectory::new_in(&workspace_target_dir(), "c7-seal");
    let (core, mut bundle, _) = saved_bundle(&temp);
    let ready = c7_checked(&mut bundle, &temp, false, None);
    let key = ready.token.snapshot.clone();
    assert!(matches!(
        bundle.review_job().unwrap().unwrap().phase,
        ReviewPhase::ReadyToSeal { .. }
    ));
    let command = SessionId::fresh().unwrap();
    let verified = c7_verified(&bundle, &ready.token);
    let SealResult::Sealed(sealed) = bundle
        .seal_review(&ready.token, &command, C7_DIGEST, &verified)
        .unwrap()
    else {
        panic!("C7_SEALED");
    };
    assert!(!sealed.replayed);
    assert_eq!(sealed.receipt.token, ready.token);
    assert_eq!(sealed.receipt.command, command);
    assert_eq!(
        sealed.receipt.key.reviewer_attempt,
        ready.binding.reviewer_attempt
    );
    assert_eq!(sealed.receipt.key.configuration_sha256, C7_DIGEST);
    assert!(
        sealed
            .receipt
            .rows
            .values()
            .all(|result| *result == ReviewRowResult::Pass)
    );
    // The fixture's overlay inventory is not determined: never Delivered.
    let kinewright_core::output_review::SealedReviewOutcome::ReviewedFactsNotChecked { exceptions } =
        &sealed.receipt.outcome
    else {
        panic!("C7_NOT_DELIVERED: {:?}", sealed.receipt.outcome);
    };
    assert!(exceptions.contains(
        &kinewright_core::output_review::DeliveryException::OverlayInventoryNotDetermined(
            kinewright_core::output_review::OverlayKind::Caption
        )
    ));
    assert!(bundle.review_job().unwrap().is_none(), "C7_JOB_RETIRED");
    let receipts = c7_receipts(&bundle, &key);
    assert!(receipts.len() == 1 && c7_same(&receipts[0], &sealed.receipt));

    // A lost reply: the same token returns the same receipt; nothing is sealed twice.
    let replay = bundle.sealed_review(&ready.token).unwrap().unwrap();
    assert!(replay.replayed);
    assert!(c7_same(&replay.receipt, &sealed.receipt));
    let SealResult::Sealed(again) = bundle
        .seal_review(
            &ready.token,
            &SessionId::fresh().unwrap(),
            C7_DIGEST,
            &verified,
        )
        .unwrap()
    else {
        panic!("C7_REPLAYED");
    };
    assert!(again.replayed && c7_same(&again.receipt, &sealed.receipt));
    assert_eq!(c7_receipts(&bundle, &key).len(), 1, "C7_ONE_RECEIPT");

    // A sealed version and profile is never reviewed or sealed again.
    let descriptor = bundle.read_snapshot(&key).unwrap().descriptor;
    let refused = bundle
        .queue_review(
            &descriptor,
            None,
            bundle
                .store
                .read_record::<ReviewRequest>(&sealed.receipt.key.request_ref)
                .unwrap()
                .settings
                .clone(),
            1 << 20,
            bundle
                .store
                .read_record::<StoredReviewExpectations>(&sealed.receipt.key.expectations_ref)
                .unwrap()
                .0
                .clone(),
        )
        .err()
        .unwrap();
    assert!(
        refused
            .to_string()
            .contains("already has a sealed output review"),
        "{refused}"
    );

    // Reopen: one immutable receipt, no job.
    drop(verified);
    let path = temp.path("project.kinewright");
    drop(bundle);
    let reopened = ProjectBundle::activate(
        &path,
        BundleWriteAdmission::read(&path).unwrap(),
        core.history_pool(),
    )
    .unwrap();
    assert!(reopened.review_job().unwrap().is_none());
    let receipts = c7_receipts(&reopened, &key);
    assert!(receipts.len() == 1 && c7_same(&receipts[0], &sealed.receipt));
}

#[test]
fn ja1_s5a_c7_seal_refuses_missing_unknown_and_stale_rows() {
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    let temp = TempDirectory::new_in(&workspace_target_dir(), "c7-refusals");
    let (_core, mut bundle, _) = saved_bundle(&temp);

    // A missing row.
    let ready = c7_checked(
        &mut bundle,
        &temp,
        false,
        Some(RequiredReviewRow::OverlayVerification),
    );
    let result = c7_seal(&mut bundle, &ready).unwrap();
    let findings = c7_refused(&mut bundle, result);
    assert!(
        c7_has(
            &findings,
            Some(RequiredReviewRow::OverlayVerification),
            "This check has no recorded result."
        ),
        "C7_MISSING_ROW: {findings:?}"
    );
    bundle
        .update_review_job(&ready.token, UpdateReviewJob::Stop)
        .unwrap();
    bundle
        .update_review_job(&ready.token, UpdateReviewJob::RetireStopped)
        .unwrap();

    // An Unknown row keeps its reason.
    let ready = c7_checked(
        &mut bundle,
        &temp,
        false,
        Some(RequiredReviewRow::LoudnessAndPeak),
    );
    bundle
        .update_review_job(
            &ready.token,
            UpdateReviewJob::RecordUnknown {
                binding: ready.binding.clone(),
                row: RequiredReviewRow::LoudnessAndPeak,
                reason: "The decoder stalled.".into(),
            },
        )
        .unwrap();
    let result = c7_seal(&mut bundle, &ready).unwrap();
    let findings = c7_refused(&mut bundle, result);
    assert!(
        c7_has(
            &findings,
            Some(RequiredReviewRow::LoudnessAndPeak),
            "The decoder stalled."
        ),
        "C7_UNKNOWN_ROW: {findings:?}"
    );
    c7_retire(&mut bundle, &ready.token);
}

#[test]
fn ja1_s5a_c7_seal_refuses_changed_inputs() {
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    let temp = TempDirectory::new_in(&workspace_target_dir(), "c7-inputs");
    let (_core, mut bundle, _) = saved_bundle(&temp);

    // A changed configuration digest.
    let ready = c7_checked(&mut bundle, &temp, false, None);
    let verified = c7_verified(&bundle, &ready.token);
    let result = bundle
        .seal_review(
            &ready.token,
            &SessionId::fresh().unwrap(),
            [9; 32],
            &verified,
        )
        .unwrap();
    let findings = c7_refused(&mut bundle, result);
    assert!(
        c7_has(
            &findings,
            None,
            "The checker configuration differs from the bound configuration."
        ),
        "C7_CONFIG: {findings:?}"
    );
    c7_retire(&mut bundle, &ready.token);

    // A changed artifact: the verified bytes of another job.
    let other = c7_checked(&mut bundle, &temp, false, None);
    let foreign = c7_verified(&bundle, &other.token);
    c7_retire(&mut bundle, &other.token);
    let ready = c7_checked(&mut bundle, &temp, false, None);
    let result = bundle
        .seal_review(
            &ready.token,
            &SessionId::fresh().unwrap(),
            C7_DIGEST,
            &foreign,
        )
        .unwrap();
    let findings = c7_refused(&mut bundle, result);
    assert!(
        c7_has(
            &findings,
            None,
            "The verified bytes are not the reviewed artifact."
        ),
        "C7_ARTIFACT: {findings:?}"
    );
    c7_retire(&mut bundle, &ready.token);

    // A stale reviewer attempt: after a rebind, the earlier attempt's Pass
    // rows never count, so one new row cannot reach ReadyToSeal.
    let ready = c7_checked(
        &mut bundle,
        &temp,
        false,
        Some(RequiredReviewRow::IndependentVisualReview),
    );
    bundle
        .update_review_job(&ready.token, UpdateReviewJob::UnbindChecks)
        .unwrap();
    let rebound = ReviewCheckBinding {
        configuration_sha256: C7_DIGEST,
        reviewer_attempt: SessionId::fresh().unwrap(),
    };
    let checkpoint = bundle
        .read_snapshot(&ready.token.snapshot)
        .unwrap()
        .descriptor
        .checkpoint_ref;
    super::seal::test_reviewers::admit(&checkpoint, &rebound.reviewer_attempt);
    bundle
        .update_review_job(&ready.token, UpdateReviewJob::BindChecks(rebound.clone()))
        .unwrap();
    c7_record(
        &mut bundle,
        &ready.token,
        &rebound,
        RequiredReviewRow::IndependentVisualReview,
    );
    assert!(
        matches!(
            bundle.review_job().unwrap().unwrap().phase,
            ReviewPhase::Checking(_)
        ),
        "C7_STALE_ROWS_NEVER_READY"
    );
    let result = c7_seal(&mut bundle, &ready).unwrap();
    let findings = c7_refused(&mut bundle, result);
    assert!(
        c7_has(
            &findings,
            Some(RequiredReviewRow::StructureAndAvIntegrity),
            "This check has no recorded result."
        ),
        "C7_STALE_ATTEMPT: {findings:?}"
    );
    c7_retire(&mut bundle, &ready.token);
}

#[test]
fn ja1_s5a_c7_seal_refuses_blocking_findings_and_changed_annotations() {
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    let temp = TempDirectory::new_in(&workspace_target_dir(), "c7-audit");
    let (_core, mut bundle, _) = saved_bundle(&temp);

    // An unresolved blocking finding, linked to its output time.
    let ready = c7_checked(&mut bundle, &temp, true, None);
    let result = c7_seal(&mut bundle, &ready).unwrap();
    let findings = c7_refused(&mut bundle, result);
    assert!(
        findings.iter().any(|finding| {
            finding.row == Some(RequiredReviewRow::EditorialAuditPresent)
                && finding.note == "The promised beat is missing."
                && finding.output
                    == Some(crate::studio_records::AudioRange {
                        start_micros: 5000,
                        end_micros: 6000,
                    })
        }),
        "C7_BLOCKING: {findings:?}"
    );
    c7_retire(&mut bundle, &ready.token);

    // Annotations changed after the freeze; Annotate itself is refused.
    let ready = c7_checked(&mut bundle, &temp, false, None);
    let descriptor = bundle
        .read_snapshot(&ready.token.snapshot)
        .unwrap()
        .descriptor;
    let mut changed = empty_annotations(&descriptor);
    changed.intended_pauses.push(TimeCode(0)..TimeCode(1));
    let refused = bundle
        .set_review_annotations(&ready.token.snapshot, changed.clone())
        .err()
        .unwrap();
    assert!(
        refused
            .to_string()
            .contains("must finish or stop before its annotations change"),
        "C7_ANNOTATE_REFUSED: {refused}"
    );
    bundle
        .set_review_annotations_unfenced_for_test(&ready.token.snapshot, changed)
        .unwrap();
    let result = c7_seal(&mut bundle, &ready).unwrap();
    let findings = c7_refused(&mut bundle, result);
    assert!(
        c7_has(
            &findings,
            None,
            "The version's annotations changed after the review froze them."
        ),
        "C7_ANNOTATIONS: {findings:?}"
    );
}

fn c7_retire(bundle: &mut ProjectBundle, token: &ReviewToken) {
    bundle
        .update_review_job(token, UpdateReviewJob::Stop)
        .unwrap();
    bundle
        .update_review_job(token, UpdateReviewJob::RetireStopped)
        .unwrap();
}

#[test]
fn ja1_s5a_c7_storage_exhausted_during_seal_keeps_work_and_retries() {
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    let temp = TempDirectory::new_in(&workspace_target_dir(), "c7-exhausted");
    let (_core, mut bundle, _) = saved_bundle(&temp);
    let ready = c7_checked(&mut bundle, &temp, false, None);
    let before = bundle.review_job().unwrap().unwrap().take();
    let verified = c7_verified(&bundle, &ready.token);
    let command = SessionId::fresh().unwrap();
    let exhausted =
        crate::io_observer::fail_native_once("review_seal_before_admission", 28, || {
            bundle.seal_review(&ready.token, &command, C7_DIGEST, &verified)
        });
    assert!(exhausted.is_err(), "C7_EXHAUSTED_REPORTED: {exhausted:?}");
    let after = bundle.review_job().unwrap().unwrap().take();
    assert_eq!(after, before, "C7_JOB_STAYS_READY_TO_SEAL");
    assert!(c7_receipts(&bundle, &ready.token.snapshot).is_empty());
    let SealResult::Sealed(sealed) = bundle
        .seal_review(&ready.token, &command, C7_DIGEST, &verified)
        .unwrap()
    else {
        panic!("C7_RETRY_SEALS");
    };
    assert!(!sealed.replayed);
    assert_eq!(c7_receipts(&bundle, &ready.token.snapshot).len(), 1);
}

#[test]
fn ja1_s5a_c7_kill_between_publication_and_ack_reopens_sealed() {
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    let temp = TempDirectory::new_in(&workspace_target_dir(), "c7-kill");
    let (core, mut bundle, _) = saved_bundle(&temp);
    let ready = c7_checked(&mut bundle, &temp, false, None);
    let verified = c7_verified(&bundle, &ready.token);
    let command = SessionId::fresh().unwrap();
    // The root is renamed but its parent sync fails: published, never acknowledged.
    let uncertain = crate::io_observer::fail_once("root_parent_sync", || {
        bundle.seal_review(&ready.token, &command, C7_DIGEST, &verified)
    });
    assert!(uncertain.is_err(), "C7_ACK_LOST: {uncertain:?}");
    drop(verified);
    let path = temp.path("project.kinewright");
    drop(bundle);
    let mut reopened = ProjectBundle::activate(
        &path,
        BundleWriteAdmission::read(&path).unwrap(),
        core.history_pool(),
    )
    .unwrap();
    assert!(reopened.review_job().unwrap().is_none(), "C7_JOB_RETIRED");
    let receipts = c7_receipts(&reopened, &ready.token.snapshot);
    assert_eq!(receipts.len(), 1, "C7_RECEIPT_FOUND");
    assert_eq!(receipts[0].command, command);
    let replay = reopened.sealed_review(&ready.token).unwrap().unwrap();
    assert!(
        c7_same(&replay.receipt, &receipts[0]),
        "C7_RETRY_SAME_RECEIPT"
    );
}

#[test]
fn ja1_s5a_c7_seal_leaves_the_working_edit_and_save_as_copy_alone() {
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    let temp = TempDirectory::new_in(&workspace_target_dir(), "c7-working");
    let (core, mut bundle, inputs) = saved_bundle(&temp);
    // A prior sealed version the copy keeps.
    let prior = c7_checked(&mut bundle, &temp, false, None);
    assert!(matches!(
        c7_seal(&mut bundle, &prior).unwrap(),
        SealResult::Sealed(_)
    ));
    let ready = c7_checked(&mut bundle, &temp, false, None);
    // The working edit advances while the job is ReadyToSeal.
    core.request(kinewright_core::Command::Do(
        kinewright_core::Operation::AddMarker {
            marker: kinewright_core::Marker {
                id: kinewright_core::MarkerId(11),
                position: TimeCode(1),
                label: "after freeze".into(),
                color_token: 0,
            },
        },
    ))
    .unwrap();
    bundle.save(&core, &inputs).unwrap();
    let head = bundle.manual_head().unwrap().clone();
    // Save As during ReadyToSeal: the copy has the prior receipt and no job.
    let crate::bundle_copy::SaveAsOutcome::Copied(copy) =
        bundle.save_as(&temp.path("copy.kinewright")).unwrap()
    else {
        panic!("C7_COPY");
    };
    let copied = copy
        .into_destination()
        .unwrap_or_else(|_| panic!("C7_COPY_COMMITTED"));
    assert!(copied.review_job().unwrap().is_none(), "C7_COPY_HAS_NO_JOB");
    assert_eq!(c7_receipts(&copied, &prior.token.snapshot).len(), 1);
    assert!(c7_receipts(&copied, &ready.token.snapshot).is_empty());
    drop(copied);
    // The copy reopened from disk: its receipts pass recovery, and no job.
    let copy_path = temp.path("copy.kinewright");
    let reopened = ProjectBundle::activate(
        &copy_path,
        BundleWriteAdmission::read(&copy_path).unwrap(),
        core.history_pool(),
    )
    .unwrap();
    assert!(
        reopened.review_job().unwrap().is_none(),
        "C7_FIX1_REOPENED_COPY_HAS_NO_JOB"
    );
    assert_eq!(
        c7_receipts(&reopened, &prior.token.snapshot).len(),
        1,
        "C7_FIX1_REOPENED_COPY_KEEPS_PRIOR_RECEIPT"
    );
    assert!(c7_receipts(&reopened, &ready.token.snapshot).is_empty());
    drop(reopened);

    assert!(matches!(
        c7_seal(&mut bundle, &ready).unwrap(),
        SealResult::Sealed(_)
    ));
    assert_eq!(
        bundle.manual_head().unwrap(),
        &head,
        "C7_WORKING_EDIT_NOT_ROLLED_BACK"
    );
    assert_eq!(c7_receipts(&bundle, &ready.token.snapshot).len(), 1);
    assert_eq!(c7_receipts(&bundle, &prior.token.snapshot).len(), 1);
    let versions = bundle.version_index().unwrap();
    let with_receipts = versions
        .snapshots
        .values()
        .filter(|descriptor| !c7_receipts(&bundle, &descriptor.key).is_empty())
        .count();
    assert_eq!(with_receipts, 2, "C7_ONLY_FROZEN_VERSIONS_SEALED");
    drop(versions);
    // Undo across the seal: the edit is undone and saved; the receipts stay
    // on the frozen versions only.
    core.request(kinewright_core::Command::Undo).unwrap();
    bundle.save(&core, &inputs).unwrap();
    assert_ne!(bundle.manual_head().unwrap(), &head, "C7_FIX1_UNDO_SAVED");
    assert_eq!(c7_receipts(&bundle, &ready.token.snapshot).len(), 1);
    assert_eq!(c7_receipts(&bundle, &prior.token.snapshot).len(), 1);
    let versions = bundle.version_index().unwrap();
    let with_receipts = versions
        .snapshots
        .values()
        .filter(|descriptor| !c7_receipts(&bundle, &descriptor.key).is_empty())
        .count();
    assert_eq!(with_receipts, 2, "C7_FIX1_UNDO_SEALS_NO_OTHER_VERSION");
}

#[test]
fn ja1_s5a_c7_seal_refuses_while_staging_is_held() {
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    let temp = TempDirectory::new_in(&workspace_target_dir(), "c7-staging");
    let (_core, mut bundle, _) = saved_bundle(&temp);
    let ready = c7_checked_staged(&mut bundle, &temp, false, None, false);
    let result = c7_seal(&mut bundle, &ready).unwrap();
    let findings = c7_refused(&mut bundle, result);
    assert!(
        c7_has(
            &findings,
            None,
            "Review staging is still held by the encode."
        ),
        "C7_STAGING: {findings:?}"
    );
}

#[test]
fn ja1_s5a_c7_fix1_seal_refuses_another_rulebook_and_a_non_reviewer() {
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    let temp = TempDirectory::new_in(&workspace_target_dir(), "c7-fix1-rulebook");
    let (_core, mut bundle, _) = saved_bundle(&temp);
    // The audit was made under no rulebook; the review froze a new one.
    let ready = c7_checked_with(
        &mut bundle,
        &temp,
        C7Setup {
            fault: Some(C7Fault::NewRulebook),
            ..C7Setup::default()
        },
    );
    let result = c7_seal(&mut bundle, &ready).unwrap();
    let findings = c7_refused(&mut bundle, result);
    assert!(
        c7_has(
            &findings,
            Some(RequiredReviewRow::EditorialAuditPresent),
            "The editorial audit was made under another rulebook."
        ),
        "C7_FIX1_OTHER_RULEBOOK: {findings:?}"
    );
    c7_retire(&mut bundle, &ready.token);
    // The audit row agrees: the driver records it Unknown.
    let audited = bundle.review_audit_complete(&ready.token);
    assert!(audited.is_err(), "C7_FIX1_RETIRED_JOB_HAS_NO_AUDIT_ROW");

    // No editor rationale, and the bound attempt is no Reviewer of the cut.
    let ready = c7_checked_with(
        &mut bundle,
        &temp,
        C7Setup {
            fault: Some(C7Fault::NotAReviewer),
            ..C7Setup::default()
        },
    );
    let result = c7_seal(&mut bundle, &ready).unwrap();
    let findings = c7_refused(&mut bundle, result);
    assert!(
        c7_has(
            &findings,
            Some(RequiredReviewRow::IndependentVisualReview),
            "The visual reviewer is not a Reviewer task attempt on this cut."
        ),
        "C7_FIX1_NOT_A_REVIEWER: {findings:?}"
    );
}

#[test]
fn ja1_s5a_c7_fix1_audit_row_follows_the_frozen_rulebook() {
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    let temp = TempDirectory::new_in(&workspace_target_dir(), "c7-fix1-audit-row");
    let (_core, mut bundle, _) = saved_bundle(&temp);
    let ready = c7_checked_with(
        &mut bundle,
        &temp,
        C7Setup {
            fault: Some(C7Fault::NewRulebook),
            skip: Some(RequiredReviewRow::EditorialAuditPresent),
            ..C7Setup::default()
        },
    );
    assert_eq!(
        bundle.review_audit_complete(&ready.token).unwrap(),
        Err("The editorial audit was made under another rulebook."),
        "C7_FIX1_AUDIT_ROW_RULEBOOK"
    );
}

#[test]
fn ja1_s5a_c7_fix1_retry_after_a_pause_binds_a_fresh_attempt() {
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    let temp = TempDirectory::new_in(&workspace_target_dir(), "c7-fix1-retry");
    let (_core, mut bundle, _) = saved_bundle(&temp);
    let ready = c7_checked(
        &mut bundle,
        &temp,
        false,
        Some(RequiredReviewRow::IndependentVisualReview),
    );
    // The visual row is missing: the seal pauses.
    let result = c7_seal(&mut bundle, &ready).unwrap();
    c7_refused(&mut bundle, result);
    let retried = bundle
        .update_review_job(&ready.token, UpdateReviewJob::Retry)
        .unwrap();
    assert_eq!(
        retried.phase,
        ReviewPhase::Checking(ReviewChecks::default()),
        "C7_FIX1_RETRY_CLEARS_THE_BINDING"
    );
    // A fresh attempt with its own digest binds and records every row.
    let fresh = ReviewCheckBinding {
        configuration_sha256: [8; 32],
        reviewer_attempt: SessionId::fresh().unwrap(),
    };
    let checkpoint = bundle
        .read_snapshot(&ready.token.snapshot)
        .unwrap()
        .descriptor
        .checkpoint_ref;
    super::seal::test_reviewers::admit(&checkpoint, &fresh.reviewer_attempt);
    bundle
        .update_review_job(&ready.token, UpdateReviewJob::BindChecks(fresh.clone()))
        .unwrap();
    let rows = CoverageProfileId::OutputReviewV1.required_rows();
    for (index, &row) in rows.iter().enumerate() {
        c7_record(&mut bundle, &ready.token, &fresh, row);
        let ready_to_seal = matches!(
            bundle.review_job().unwrap().unwrap().phase,
            ReviewPhase::ReadyToSeal { .. }
        );
        assert_eq!(
            ready_to_seal,
            index + 1 == rows.len(),
            "C7_FIX1_READY_ONLY_FROM_THE_FRESH_ATTEMPT"
        );
    }
    let verified = c7_verified(&bundle, &ready.token);
    let sealed = bundle
        .seal_review(
            &ready.token,
            &SessionId::fresh().unwrap(),
            [8; 32],
            &verified,
        )
        .unwrap();
    let SealResult::Sealed(sealed) = sealed else {
        panic!("C7_FIX1_RETRY_SEALS: {sealed:?}");
    };
    assert_eq!(sealed.receipt.key.reviewer_attempt, fresh.reviewer_attempt);
}

#[test]
fn ja1_s5a_c7_fix1_disk_full_admitting_the_receipt_keeps_work_and_retries() {
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    let temp = TempDirectory::new_in(&workspace_target_dir(), "c7-fix1-enospc");
    let (_core, mut bundle, _) = saved_bundle(&temp);
    let prior = c7_checked(&mut bundle, &temp, false, None);
    assert!(matches!(
        c7_seal(&mut bundle, &prior).unwrap(),
        SealResult::Sealed(_)
    ));
    let prior_receipts = c7_receipts(&bundle, &prior.token.snapshot);
    let ready = c7_checked(&mut bundle, &temp, false, None);
    let before = bundle.review_job().unwrap().unwrap().take();
    let head = bundle.manual_head().unwrap().clone();
    let generation = bundle.generation;
    let verified = c7_verified(&bundle, &ready.token);
    let command = SessionId::fresh().unwrap();
    // The receipt's own object write runs out of space.
    let exhausted = crate::io_observer::fail_native_once("object_file_sync", 28, || {
        bundle.seal_review(&ready.token, &command, C7_DIGEST, &verified)
    });
    assert!(exhausted.is_err(), "C7_FIX1_ENOSPC_REPORTED: {exhausted:?}");
    assert_eq!(bundle.generation, generation, "C7_FIX1_ROOT_UNCHANGED");
    assert_eq!(
        bundle.review_job().unwrap().unwrap().take(),
        before,
        "C7_FIX1_JOB_KEPT"
    );
    assert_eq!(bundle.manual_head().unwrap(), &head, "C7_FIX1_EDITS_KEPT");
    assert!(c7_receipts(&bundle, &ready.token.snapshot).is_empty());
    let kept = c7_receipts(&bundle, &prior.token.snapshot);
    assert!(
        kept.len() == 1 && c7_same(&kept[0], &prior_receipts[0]),
        "C7_FIX1_PRIOR_RECEIPT_KEPT"
    );
    let SealResult::Sealed(sealed) = bundle
        .seal_review(&ready.token, &command, C7_DIGEST, &verified)
        .unwrap()
    else {
        panic!("C7_FIX1_RETRY_SEALS");
    };
    assert!(!sealed.replayed);
}

#[test]
fn ja1_s5a_c7_fix1_reopen_checks_the_sealed_audit_and_expectations() {
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    let temp = TempDirectory::new_in(&workspace_target_dir(), "c7-fix1-receipt-audit");
    let (_core, mut bundle, _) = saved_bundle(&temp);
    let ready = c7_checked(&mut bundle, &temp, false, None);
    let SealResult::Sealed(sealed) = c7_seal(&mut bundle, &ready).unwrap() else {
        panic!("C7_FIX1_SEALED");
    };
    let receipt = sealed.receipt;
    let descriptor = bundle
        .read_snapshot(&receipt.key.snapshot)
        .unwrap()
        .descriptor;
    let store = &bundle.store;
    let evidence = store
        .read_record::<ReviewEvidence>(descriptor.review_evidence_ref.as_ref().unwrap())
        .unwrap();
    let artifact = store
        .read_record::<ReviewArtifact>(&receipt.key.artifact_ref)
        .unwrap();
    let request = store
        .read_record::<ReviewRequest>(&receipt.key.request_ref)
        .unwrap();
    let expectations = store
        .read_record::<StoredReviewExpectations>(&request.expectations_ref)
        .unwrap();
    let audit = store
        .read_record::<crate::editorial::EditorialAudit>(receipt.key.audit_ref.as_ref().unwrap())
        .unwrap();
    let validate =
        |audit: &crate::editorial::EditorialAudit,
         expectations: &kinewright_core::output_review::ReviewExpectations| {
            validate_version_receipt(
                &descriptor,
                &evidence,
                &receipt,
                &artifact,
                &request,
                expectations,
                audit,
                &mut Vec::new(),
            )
        };
    validate(&audit, &expectations.0).unwrap();
    let mut ruled = (*audit).clone();
    ruled.review.rulebook_ref = Some(receipt.key.request_ref.clone());
    assert!(
        validate(&ruled, &expectations.0).is_err(),
        "C7_FIX1_RECEIPT_AUDIT_RULEBOOK"
    );
    let mut blocking = (*audit).clone();
    blocking.review.entries[0].disposition = crate::editorial::AuditDisposition::Blocking;
    assert!(
        validate(&blocking, &expectations.0).is_err(),
        "C7_FIX1_RECEIPT_AUDIT_BLOCKING"
    );
    let mut other = expectations.0.clone();
    other.identity.export_settings_sha256 = [9; 32];
    assert!(
        validate(&audit, &other).is_err(),
        "C7_FIX1_RECEIPT_EXPECTATIONS_IDENTITY"
    );
}

#[test]
fn ja1_s5a_c7_fix2_a_failed_seal_pauses_ready_to_seal_with_its_reason() {
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    let temp = TempDirectory::new_in(&workspace_target_dir(), "c7-fix2-seal-failure");
    let (_core, mut bundle, _) = saved_bundle(&temp);
    let ready = c7_checked(&mut bundle, &temp, false, None);
    assert!(
        matches!(
            bundle.review_status().unwrap().unwrap().phase,
            ReviewPhase::ReadyToSeal { .. }
        ),
        "C7_FIX2_READY_TO_SEAL"
    );
    // The host records why a seal it started failed, where Status shows it.
    let paused = bundle
        .update_review_job(
            &ready.token,
            UpdateReviewJob::Pause("C7_FIX2_SEAL_FAILED".into()),
        )
        .unwrap();
    let ReviewPhase::Paused {
        checkpoint: ReviewCheckpoint::Checking(checks),
        reason,
        ..
    } = &paused.phase
    else {
        panic!("C7_FIX2_PAUSED_WITH_CHECKS: {:?}", paused.phase);
    };
    assert_eq!(reason, "C7_FIX2_SEAL_FAILED");
    assert_eq!(
        checks.reviewer_attempt,
        Some(ready.binding.reviewer_attempt)
    );
    assert_eq!(
        bundle.review_status().unwrap().unwrap().phase,
        paused.phase,
        "C7_FIX2_FAILURE_IS_DURABLE"
    );
    // Only an explicit Retry continues, and it binds afresh.
    let retried = bundle
        .update_review_job(&ready.token, UpdateReviewJob::Retry)
        .unwrap();
    assert_eq!(
        retried.phase,
        ReviewPhase::Checking(ReviewChecks::default())
    );
}

#[test]
fn ja1_s5a_c7_fix2_persisted_checks_cannot_name_a_reviewer_task() {
    // The seal derives the Reviewer task from the orchestration records; a
    // bundle that names one in its checks is refused when it is read.
    let mut value = serde_json::to_value(ReviewChecks::default()).unwrap();
    value["reviewer_task"] = serde_json::json!("task-forged");
    assert!(
        serde_json::from_value::<ReviewChecks>(value).is_err(),
        "C7_FIX2_REVIEWER_TASK_REFUSED"
    );
}

// JA-1 S6 C2: save a sealed cut.
include!("delivery_tests.rs");

// JA-1 S6 C2 fix 1: admission, staging identity and the remaining witnesses.
include!("delivery_fix1_tests.rs");
