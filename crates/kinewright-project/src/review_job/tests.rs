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
        .queue_review(descriptor, None, settings, 1 << 20, expectations)
        .unwrap()
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
