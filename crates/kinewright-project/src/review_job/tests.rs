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
        .queue_review(descriptor, None, settings, byte_limit, expectations)
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
        .set_review_annotations(&frozen.key, empty_annotations(&frozen))
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
