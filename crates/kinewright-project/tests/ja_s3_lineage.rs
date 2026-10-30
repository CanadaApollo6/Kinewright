#![cfg(all(feature = "test-util", target_os = "linux"))]
use kinewright_core::{Command, Core, Event, Operation, TimeCode, TrackId};
use kinewright_media::test_support::TempDirectory;
use kinewright_project::{
    bundle::{LoadedBundle, ProjectBundle},
    bundle_records::{BranchId, ObjectRef, RecordKind},
    c0_fixtures::build_c0_fixture_pair,
    feedback_mapping::{FeedbackBeatOwner, MappedFrame},
    receipts::{CommandId, ReceiptLookup, ReceiptResult},
    store::SealedObject,
    studio_records::*,
};
use std::path::Path;

fn metadata(b: &mut ProjectBundle, scope: MetadataScope, change: MetadataChange) -> MetadataResult {
    let id = CommandId {
        issuer: b.issue_metadata_issuer().unwrap(),
        sequence: 1,
    };
    let envelope = MetadataEnvelope {
        id,
        project_id: b.project_id().clone(),
        target_branch: b.manual_branch_id().clone(),
        owner_epoch: b.store().owner_epoch(),
        expected_head: b.manual_head().cloned(),
    };
    let update = MetadataUpdate {
        expected_root_generation: b.root().unwrap().generation,
        change,
    };
    let ReceiptLookup::Committed(receipt) = b.metadata_command(&envelope, scope, &update).unwrap()
    else {
        panic!("committed metadata")
    };
    let ReceiptResult::Metadata { result } = receipt.result else {
        panic!("metadata result")
    };
    result
}
fn blob(b: &mut ProjectBundle, bytes: &[u8]) -> SealedObject {
    let reference = ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedEvidence,
    };
    b.store_mut()
        .unwrap()
        .admit_blob(&mut &*bytes, &reference, &[])
        .unwrap()
}
fn publish(b: &mut ProjectBundle, branch: &BranchId) -> SnapshotKey {
    let publication = PublishSnapshot {
        version: VersionId::fresh().unwrap(),
        expected_head: b.checkpoint(branch).unwrap().head_id.clone(),
        checkpoint_ref: b.checkpoint_reference(branch).unwrap(),
        parent_version: None,
        run_provenance: None,
        revision_refs: vec![],
    };
    let MetadataResult::Snapshot(key) = metadata(
        b,
        MetadataScope::Snapshot,
        MetadataChange::PublishSnapshot(publication),
    ) else {
        panic!("snapshot")
    };
    key
}
fn feedback(b: &mut ProjectBundle, key: SnapshotKey, clip: kinewright_core::ClipId) -> ObjectRef {
    let snapshot = b.read_snapshot(&key).unwrap();
    let lineage = b
        .store()
        .read_record::<kinewright_project::bundle_records::LineageRecord>(
            &snapshot.descriptor.lineage_ref,
        )
        .unwrap();
    let text = blob(b, b"move this chart");
    let strokes = blob(b, b"[[0.25,0.75]]");
    let proof = blob(b, b"original frame six");
    let revision = FeedbackRevision {
        comment_id: CommentId::fresh().unwrap(),
        revision: 1,
        predecessor: None,
        anchor: FeedbackAnchor {
            snapshot: key,
            frame: 8,
            fps: FrameRate {
                numerator: 30,
                denominator: 1,
            },
            raster_size: [1920, 1080],
            orientation: Orientation::Upright,
            target: Some(FeedbackTarget {
                local_clip_id: clip,
                occurrence_id: Some(lineage.current[&clip].clone()),
                local_frame: Some(6),
            }),
            source_ref: None,
            script_beat_id: Some("chart".into()),
        },
        original_text_ref: text.reference.clone(),
        strokes_ref: strokes.reference.clone(),
        frame_proof_ref: proof.reference.clone(),
        voice_ref: None,
        transcript_revision: None,
    };
    let MetadataResult::Feedback(reference) = metadata(
        b,
        MetadataScope::Feedback,
        MetadataChange::SaveFeedback(revision),
    ) else {
        panic!("feedback")
    };
    reference
}
fn apply(core: &Core, operation: Operation) {
    assert!(matches!(
        core.request(Command::Do(operation)).unwrap(),
        Event::DocumentChanged { .. }
    ));
}
fn request(b: &ProjectBundle, original: &ObjectRef, branch: &BranchId) -> ResolveFeedback {
    ResolveFeedback {
        comment_revision_ref: original.clone(),
        candidate_checkpoint_ref: b.checkpoint_reference(branch).unwrap(),
        beat_map_ref: None,
    }
}
struct Beats(Vec<MappedFrame>);
impl FeedbackBeatOwner for Beats {
    fn candidates(
        &self,
        _: &ResolveFeedback,
        _: &FeedbackRevision,
        _: &kinewright_project::bundle_records::BranchCheckpoint,
    ) -> Result<Vec<MappedFrame>, kinewright_project::bundle::BundleError> {
        Ok(self.0.clone())
    }
}

#[test]
fn s3_old_feedback_reverse_split_ancestry() {
    let temp = TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "s3-reverse-split");
    let [fixture, _] = build_c0_fixture_pair(temp.root()).unwrap();
    let mut owner = LoadedBundle::read(&fixture.path)
        .unwrap()
        .into_writer()
        .unwrap();
    let branch = owner.manual_branch_id().clone();
    let baseline = owner.branch_history(&branch).unwrap();
    let parent = baseline.document.tracks[0].clips[0].id;
    let core = Core::spawn((*baseline.document).clone()).unwrap();
    core.restore_history((*baseline).clone()).unwrap();
    apply(
        &core,
        Operation::SplitClip {
            clip: parent,
            at: TimeCode(6),
        },
    );
    owner.save_branch(&branch, &core).unwrap();
    let split = owner.branch_history(&branch).unwrap();
    let child = split.document.tracks[0]
        .clips
        .iter()
        .find(|clip| clip.timeline_start == TimeCode(6))
        .unwrap()
        .id;
    assert_ne!(parent, child);
    let key = publish(&mut owner, &branch);
    let original = feedback(&mut owner, key, child);
    core.request(Command::Undo).unwrap();
    owner.save_branch(&branch, &core).unwrap();
    let restored = owner
        .resolve_feedback(&request(&owner, &original, &branch), None)
        .unwrap();
    assert_eq!(
        restored.resolution.temporal,
        TemporalStatus::Exact,
        "S3_REVERSE_SPLIT_IS_NOT_REMOVAL"
    );
    assert_eq!(
        restored.candidates,
        vec![MappedFrame {
            clip: parent,
            frame: 8
        }]
    );
    core.request(Command::Redo).unwrap();
    owner.save_branch(&branch, &core).unwrap();
    let redone = owner
        .resolve_feedback(&request(&owner, &original, &branch), None)
        .unwrap();
    assert_eq!(redone.resolution.temporal, TemporalStatus::Exact);
    assert_eq!(
        redone.candidates,
        vec![MappedFrame {
            clip: child,
            frame: 8
        }]
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "One saved origin is checked against each real Core operation and checkpoint"
)]
fn s3_old_feedback_mapping() {
    let temp = TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "s3-lineage");
    let [fixture, _other] = build_c0_fixture_pair(temp.root()).unwrap();
    let mut b = LoadedBundle::read(&fixture.path)
        .unwrap()
        .into_writer()
        .unwrap();
    let manual = b.manual_branch_id().clone();
    let baseline = b.branch_history(&manual).unwrap();
    let clip = baseline.document.tracks[0].clips[0].id;
    let asset = baseline.document.tracks[0].clips[0].asset;
    let key = publish(&mut b, &manual);
    let original = feedback(&mut b, key, clip);
    let original_bytes = b
        .store()
        .read_evidence(&b.read_feedback(&original).unwrap().revision.strokes_ref)
        .unwrap()
        .to_vec();
    let original_revision = b.read_feedback(&original).unwrap().revision;
    let branch = b.fork_branch(&manual).unwrap();
    let core = Core::spawn((*baseline.document).clone()).unwrap();
    let exact = b
        .resolve_feedback(&request(&b, &original, &branch), None)
        .unwrap();
    assert_eq!(
        (exact.resolution.temporal, exact.resolution.spatial),
        (TemporalStatus::Exact, SpatialStatus::Exact)
    );
    apply(
        &core,
        Operation::MoveClip {
            clip,
            to_track: TrackId(1),
            to: TimeCode(30),
        },
    );
    b.save_branch(&branch, &core).unwrap();
    let moved = b
        .resolve_feedback(&request(&b, &original, &branch), None)
        .unwrap();
    assert_eq!(moved.candidates[0].frame, 38);
    assert_eq!(
        (moved.resolution.temporal, moved.resolution.spatial),
        (TemporalStatus::Mapped, SpatialStatus::Exact)
    );
    apply(
        &core,
        Operation::TrimClip {
            clip,
            new_source: TimeCode(2)..TimeCode(10),
        },
    );
    b.save_branch(&branch, &core).unwrap();
    let trimmed = b
        .resolve_feedback(&request(&b, &original, &branch), None)
        .unwrap();
    assert_eq!(trimmed.candidates[0].frame, 38);
    assert_eq!(trimmed.resolution.temporal, TemporalStatus::Mapped);
    apply(
        &core,
        Operation::SplitClip {
            clip,
            at: TimeCode(36),
        },
    );
    b.save_branch(&branch, &core).unwrap();
    let split = b
        .resolve_feedback(&request(&b, &original, &branch), None)
        .unwrap();
    assert_eq!(split.candidates[0].frame, 38);
    assert_eq!(split.resolution.temporal, TemporalStatus::Mapped);
    core.request(Command::Undo).unwrap();
    b.save_branch(&branch, &core).unwrap();
    assert_eq!(
        b.resolve_feedback(&request(&b, &original, &branch), None)
            .unwrap()
            .candidates[0]
            .clip,
        clip
    );
    core.request(Command::Redo).unwrap();
    b.save_branch(&branch, &core).unwrap();
    assert_eq!(
        b.resolve_feedback(&request(&b, &original, &branch), None)
            .unwrap()
            .candidates,
        split.candidates
    );
    let geometry_branch = b.fork_branch(&manual).unwrap();
    let geometry_core = Core::spawn((*baseline.document).clone()).unwrap();
    let transform: kinewright_core::Effect = serde_json::from_value(serde_json::json!({
        "id": 100, "name": "transform", "parameters": {},
        "keyframes": { "scale_percent": { "keyframes": [
            { "at": 0, "value": 50 }, { "at": 10, "value": 100 }
        ] } }
    }))
    .unwrap();
    apply(
        &geometry_core,
        Operation::AddEffect {
            clip,
            effect: transform,
        },
    );
    b.save_branch(&geometry_branch, &geometry_core).unwrap();
    let transformed = b
        .resolve_feedback(&request(&b, &original, &geometry_branch), None)
        .unwrap();
    assert_eq!(
        (
            transformed.resolution.temporal,
            transformed.resolution.spatial
        ),
        (TemporalStatus::Exact, SpatialStatus::Transformed)
    );
    let points = transformed.proposed_points.unwrap();
    assert!((points[0][0] - 0.275).abs() < 1e-6);
    assert!((points[0][1] - 0.725).abs() < 1e-6);
    let crop: kinewright_core::Effect = serde_json::from_value(serde_json::json!({
        "id": 101, "name": "crop", "parameters": { "left_percent": 25 }
    }))
    .unwrap();
    apply(&geometry_core, Operation::AddEffect { clip, effect: crop });
    b.save_branch(&geometry_branch, &geometry_core).unwrap();
    let cropped = b
        .resolve_feedback(&request(&b, &original, &geometry_branch), None)
        .unwrap();
    assert_eq!(
        (cropped.resolution.temporal, cropped.resolution.spatial),
        (TemporalStatus::Exact, SpatialStatus::Unresolved)
    );
    geometry_core.request(Command::Undo).unwrap();
    let reframe: kinewright_core::Effect = serde_json::from_value(serde_json::json!({
        "id": 102, "name": "reframe", "parameters": { "target_aspect_basis_points": 5625 }
    }))
    .unwrap();
    apply(
        &geometry_core,
        Operation::AddEffect {
            clip,
            effect: reframe,
        },
    );
    b.save_branch(&geometry_branch, &geometry_core).unwrap();
    let vertical = b
        .resolve_feedback(&request(&b, &original, &geometry_branch), None)
        .unwrap();
    assert_eq!(
        (vertical.resolution.temporal, vertical.resolution.spatial),
        (TemporalStatus::Exact, SpatialStatus::Unresolved)
    );

    let sibling = b.fork_branch(&manual).unwrap();
    let recreated = Core::spawn((*baseline.document).clone()).unwrap();
    for track in &baseline.document.tracks {
        for old in &track.clips {
            apply(&recreated, Operation::DeleteClip { clip: old.id });
        }
    }
    apply(
        &recreated,
        Operation::AddClip {
            track: TrackId(1),
            asset,
            at: TimeCode::ZERO,
            source: TimeCode::ZERO..TimeCode(12),
        },
    );
    b.save_branch(&sibling, &recreated).unwrap();
    assert_eq!(
        recreated.export_history().unwrap().document.tracks[0].clips[0].id,
        clip
    );
    let removed = b
        .resolve_feedback(&request(&b, &original, &sibling), None)
        .unwrap();
    assert_eq!(
        (removed.resolution.temporal, removed.resolution.spatial),
        (TemporalStatus::Removed, SpatialStatus::Removed)
    );
    let sibling_key = publish(&mut b, &sibling);
    let sibling_feedback = feedback(&mut b, sibling_key, clip);
    let other_sibling = b.fork_branch(&manual).unwrap();
    b.save_branch(&other_sibling, &recreated).unwrap();
    let collision = b
        .resolve_feedback(&request(&b, &sibling_feedback, &other_sibling), None)
        .unwrap();
    assert_eq!(
        (collision.resolution.temporal, collision.resolution.spatial),
        (TemporalStatus::Removed, SpatialStatus::Removed)
    );
    let mut beat_request = request(&b, &original, &sibling);
    beat_request.beat_map_ref = Some(original_revision.original_text_ref.clone());
    let replacement = b
        .resolve_feedback(
            &beat_request,
            Some(&Beats(vec![MappedFrame { clip, frame: 6 }])),
        )
        .unwrap();
    assert_eq!(
        (
            replacement.resolution.temporal,
            replacement.resolution.spatial
        ),
        (TemporalStatus::Mapped, SpatialStatus::Unresolved)
    );
    let repeated = b
        .resolve_feedback(
            &beat_request,
            Some(&Beats(vec![
                MappedFrame { clip, frame: 6 },
                MappedFrame { clip, frame: 7 },
            ])),
        )
        .unwrap();
    assert_eq!(
        (repeated.resolution.temporal, repeated.resolution.spatial),
        (TemporalStatus::Ambiguous, SpatialStatus::Unresolved)
    );
    assert_eq!(
        b.resolve_feedback(&beat_request, None)
            .unwrap()
            .resolution
            .temporal,
        TemporalStatus::Unresolved
    );
    // Abandoned sibling edits have no route into the accepted branch's head map.
    assert_eq!(
        b.resolve_feedback(&request(&b, &original, &branch), None)
            .unwrap()
            .candidates,
        split.candidates
    );
    assert_eq!(
        b.read_feedback(&original).unwrap().revision,
        original_revision
    );
    assert_eq!(
        &*b.store()
            .read_evidence(&original_revision.strokes_ref)
            .unwrap(),
        &original_bytes
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "The import, replacement and removal share one durable fixture owner"
)]
fn s3_old_feedback_mapping_take_replacement_and_removal() {
    use kinewright_media::test_support::GeneratedMedia;
    use kinewright_project::editing_session::Ja1EditingSession;
    let temp = TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "s3-take-mapping");
    let [fixture, _other] = build_c0_fixture_pair(temp.root()).unwrap();
    let mut b = LoadedBundle::read(&fixture.path)
        .unwrap()
        .into_writer()
        .unwrap();
    let manual = b.manual_branch_id().clone();
    let core = Core::spawn(kinewright_core::Document::default()).unwrap();
    core.restore_history(fixture.history).unwrap();
    let clip = core.export_history().unwrap().document.tracks[0].clips[0].id;
    let key = publish(&mut b, &manual);
    let original = feedback(&mut b, key, clip);
    let inputs = b.checkpoint(&manual).unwrap().input_manifest_ref.clone();
    let mut session = Ja1EditingSession::from_bundle(b, &core, inputs).unwrap();
    let green = GeneratedMedia::ffmpeg(
        "replacement-take",
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=green:s=32x24:r=24:d=0.5",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=990:sample_rate=48000:duration=0.5",
            "-c:v",
            "libx264",
            "-qp",
            "0",
            "-x264-params",
            "colorprim=bt709:transfer=bt709:colormatrix=bt709",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
            "-c:a",
            "pcm_s16le",
            "-shortest",
        ],
        "mkv",
    );
    let (take, _) = session.import_owned_media(green.path()).unwrap();
    apply(
        &core,
        Operation::ReplaceClip {
            clip,
            asset: take.id,
            source: TimeCode::ZERO..take.duration,
        },
    );
    session.save_active().unwrap();
    drop(session);
    drop(core);
    let mut b = LoadedBundle::read(&fixture.path)
        .unwrap()
        .into_writer()
        .unwrap();
    let manual = b.manual_branch_id().clone();
    let mut target = request(&b, &original, &manual);
    assert_eq!(
        b.resolve_feedback(&target, None)
            .unwrap()
            .resolution
            .temporal,
        TemporalStatus::Removed
    );
    target.beat_map_ref = Some(
        b.read_feedback(&original)
            .unwrap()
            .revision
            .original_text_ref,
    );
    let meaning = b
        .resolve_feedback(&target, Some(&Beats(vec![MappedFrame { clip, frame: 8 }])))
        .unwrap();
    assert_eq!(
        (meaning.resolution.temporal, meaning.resolution.spatial),
        (TemporalStatus::Mapped, SpatialStatus::Unresolved)
    );
    let branch = b.fork_branch(&manual).unwrap();
    let document = b.branch_history(&manual).unwrap().document.clone();
    let removed = Core::spawn((*document).clone()).unwrap();
    apply(&removed, Operation::DeleteClip { clip });
    b.save_branch(&branch, &removed).unwrap();
    let resolution = b
        .resolve_feedback(&request(&b, &original, &branch), None)
        .unwrap();
    assert_eq!(
        (
            resolution.resolution.temporal,
            resolution.resolution.spatial
        ),
        (TemporalStatus::Removed, SpatialStatus::Removed)
    );
}
