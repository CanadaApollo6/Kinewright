//! C4 record/reader/selection witnesses. Full rescue publication belongs to C5.
#![cfg(feature = "test-util")]
#![allow(clippy::too_many_lines)]
use kinewright_project::{
    bundle_records::{ProjectId, SessionId},
    rescue_record::{ProvenanceSummary, RescueCopyV1},
    storage_budget::StorageLimits,
    store::BundleRecord,
};
fn record(project: ProjectId) -> RescueCopyV1 {
    RescueCopyV1 {
        role: "rescue_copy_v1".into(),
        schema: 1,
        project_id: project,
        originating_session: SessionId::fresh().unwrap(),
        full_capture_sha256: "a".repeat(64),
        selected_capture_sha256: "b".repeat(64),
        old_floor: 0,
        new_floor: 0,
        removed_undo: 0,
        redo_preserved: true,
        gesture_preserved: true,
        acknowledged_root: None,
        acknowledged_absence: Some("unsaved source has no acknowledged root".into()),
        recovery_origin: None,
        omissions: vec![],
        attempts: vec![],
        summary: ProvenanceSummary::default(),
    }
}
#[test]
fn w11_warning_provenance_compaction_and_recopy() {
    use kinewright_project::rescue_record::RescueAttemptProvenance;
    let mut r = record(ProjectId::fresh().unwrap());
    let attempts = (0..40)
        .map(|_| RescueAttemptProvenance {
            session_id: SessionId::fresh().unwrap(),
            project_id: ProjectId::fresh().unwrap(),
            destination: std::path::PathBuf::from("historical.kinewright"),
            parent_device: 1,
            parent_inode: 2,
            attempted_root_sha256: "c".repeat(64),
            pending_command: None,
        })
        .collect::<Vec<_>>();
    let mut current = attempts[..4].to_vec();
    for attempt in &mut current {
        attempt.session_id = r.originating_session.clone();
    }
    r.attempts = current.clone();
    let omission = kinewright_project::rescue_record::RecoveryOmission {
        role: "acknowledged_root".into(),
        root: kinewright_project::bundle_records::ObjectRef {
            sha256: "e".repeat(64),
            byte_length: 1,
            record_kind: kinewright_project::bundle_records::RecordKind::RootCheckpoint,
        },
        operation: "read".into(),
        cause: "unavailable".into(),
    };
    r.omissions.push(omission.clone());
    r.compact(attempts).unwrap();
    assert_eq!(r.attempts.len(), 16);
    assert_eq!(r.attempts[..4], current);
    assert_eq!(r.omissions, vec![omission]);
    assert_eq!(r.summary.count, 28);
    assert!(r.summary.unresolved);
    let prior = r.attempts.clone();
    let mut long = prior[0].clone();
    long.destination = std::path::PathBuf::from("x".repeat(70_000));
    r.compact([long]).unwrap();
    assert_eq!(r.attempts, prior);
    assert_eq!(r.summary.count, 29);
    r.summary.count = u64::MAX;
    r.compact([prior[0].clone()]).unwrap();
    assert!(r.summary.count_overflow);
    assert!(serde_json::to_vec(&r).unwrap().len() <= 65_536);
    assert_eq!(
        serde_json::from_slice::<RescueCopyV1>(&serde_json::to_vec(&r).unwrap()).unwrap(),
        r
    );
    r.schema = 2;
    assert!(r.validate(&StorageLimits::default()).is_err());
}
#[cfg(target_os = "linux")]
mod durable {
    use super::*;
    use kinewright_core::{
        AttachmentState, Command, Core, Document, Event, Operation, Track, TrackId, TrackKind,
    };
    use kinewright_media::test_support::TempDirectory;
    use kinewright_project::{
        bundle::{BundleWriteAdmission, LoadedBundle, ProjectBundle},
        bundle_records::{ObjectRef, RecordKind},
        bundle_recovery::{RecoverySelection, without_api_probe},
        bundle_switch::ProjectSelectionResult,
        io_observer,
    };
    use std::{io::Read as _, path::Path};
    fn fixture() -> (TempDirectory, Core, ProjectBundle, ObjectRef) {
        let temp = TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "s2-c4");
        let core = Core::spawn(Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            }],
            ..Document::default()
        })
        .unwrap();
        let mut bundle = ProjectBundle::activate(
            &temp.path("b.kinewright"),
            BundleWriteAdmission::new_document(),
        )
        .unwrap();
        let input = bundle.empty_inputs().unwrap().reference.clone();
        edit(&core, false);
        bundle.save(&core, &input).unwrap();
        (temp, core, bundle, input)
    }
    fn edit(core: &Core, locked: bool) {
        assert!(matches!(
            core.request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked
            }))
            .unwrap(),
            Event::DocumentChanged { .. }
        ));
    }
    fn roots(bundle: &ProjectBundle) -> Vec<ObjectRef> {
        bundle
            .store()
            .read_record::<kinewright_project::store::StoreManifest>(
                &bundle.root().unwrap().store_manifest_ref,
            )
            .unwrap()
            .recovery
            .predecessors
            .clone()
    }
    fn assert_api_declarations(path: &Path, api: &str, zero_current_generation: bool) {
        let original = std::fs::read(path).unwrap();
        let mut root: serde_json::Value = serde_json::from_slice(&original).unwrap();
        if zero_current_generation {
            root["studio"]["manual_switch_generation"] = serde_json::json!(0);
            std::fs::write(path, serde_json::to_vec(&root).unwrap()).unwrap();
            assert!(
                LoadedBundle::read(path).is_ok(),
                "supported fresh-project wrapper with an archived switch"
            );
        }
        let objects = path
            .with_extension("kinewright-assets")
            .join("studio/objects");
        let bytes = std::fs::read(
            objects.join(
                root["studio"]["store_manifest_ref"]["sha256"]
                    .as_str()
                    .unwrap(),
            ),
        )
        .unwrap();
        for version in [None, Some(2)] {
            let mut manifest: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            if let Some(version) = version {
                manifest["required_apis"][api] = serde_json::json!(version);
            } else {
                manifest["required_apis"]
                    .as_object_mut()
                    .unwrap()
                    .remove(api);
            }
            let mutated = serde_json::to_vec(&manifest).unwrap();
            let hash = kinewright_media::sha256_bytes(&mutated);
            std::fs::write(objects.join(&hash), &mutated).unwrap();
            let mut changed = root.clone();
            changed["studio"]["store_manifest_ref"]["sha256"] = serde_json::json!(hash);
            changed["studio"]["store_manifest_ref"]["byte_length"] =
                serde_json::json!(mutated.len());
            std::fs::write(path, serde_json::to_vec(&changed).unwrap()).unwrap();
            let (result, trace) = io_observer::observe(|| LoadedBundle::read(path));
            assert!(result.is_err());
            assert!(trace.events.is_empty());
            std::fs::write(path, &original).unwrap();
            std::fs::remove_file(objects.join(hash)).unwrap();
        }
    }
    fn checkpoint_root(bundle: &mut ProjectBundle) -> ObjectRef {
        let bytes = bundle.serialized_root().unwrap();
        let reference = ObjectRef {
            sha256: kinewright_media::sha256_bytes(&bytes),
            byte_length: bytes.len() as u64,
            record_kind: RecordKind::RootCheckpoint,
        };
        // The next ordinary Save installs precisely these acknowledged bytes as a predecessor.
        reference
    }
    #[test]
    fn w13_local_selection_mapping_limits_and_retry() {
        for fault in [
            None,
            Some("root_before_publish"),
            Some("root_parent_sync"),
            Some("root_rename_io"),
            Some("selection_activation_prepare"),
        ] {
            let (temp, core, mut bundle, input) = fixture();
            let left = bundle.manual_branch_id().clone();
            let original = core.export_history().unwrap();
            let target = checkpoint_root(&mut bundle);
            edit(&core, true);
            bundle.save(&core, &input).unwrap();
            edit(&core, false);
            bundle.save(&core, &input).unwrap();
            assert_eq!(roots(&bundle)[1], target);
            let mut prepared = bundle
                .prepare_project_selection(
                    &core,
                    RecoverySelection::Checkpoint {
                        root: target.clone(),
                        branch: left.clone(),
                    },
                )
                .unwrap();
            let right = prepared.target_branch().clone();
            // The selected P2 really leaves the automatic list, with collection between prepare and publish.
            bundle.save(&core, &input).unwrap();
            bundle.save(&core, &input).unwrap();
            assert!(!roots(&bundle).contains(&target));
            bundle.collect_after_ack().unwrap();
            bundle
                .refresh_selection_source(&mut prepared, &core)
                .unwrap();
            bundle.collect_after_ack().unwrap();
            let result = if let Some(fault) = fault {
                io_observer::fail_once(fault, || {
                    bundle.publish_project_selection(&mut prepared, &core)
                })
            } else {
                bundle.publish_project_selection(&mut prepared, &core)
            };
            if fault.is_none() {
                assert!(
                    matches!(result, ProjectSelectionResult::Activated(_)),
                    "definite success must refresh and activate without reconciliation: {result:?}"
                );
            }
            if fault == Some("selection_activation_prepare") {
                assert!(
                    matches!(result, ProjectSelectionResult::CommittedWithError(_)),
                    "committed phase lost: {result:?}"
                );
            }
            match result {
                ProjectSelectionResult::Activated(_) => {}
                ProjectSelectionResult::CommittedWithError(_) => {
                    assert_eq!(core.attachment_state().unwrap(), AttachmentState::Frozen);
                    assert!(matches!(
                        bundle.publish_project_selection(&mut prepared, &core),
                        ProjectSelectionResult::Activated(_)
                    ));
                }
                ProjectSelectionResult::Unknown => {
                    assert_eq!(core.attachment_state().unwrap(), AttachmentState::Frozen);
                    assert!(bundle.save(&core, &input).is_err());
                    assert!(bundle.save_branch(&left, &core).is_err());
                    assert!(matches!(
                        core.clone().request(Command::Undo).unwrap(),
                        Event::MutationRefused {
                            reason: kinewright_core::CoreMutationRefusal::Frozen,
                            ..
                        }
                    ));
                    assert!(
                        bundle
                            .attach_core(&core)
                            .unwrap()
                            .install_history_transition(
                                kinewright_core::HistoryTransition::floor(
                                    core.export_history().unwrap(),
                                    1
                                )
                                .unwrap()
                            )
                            .is_err()
                    );
                    match bundle.publish_project_selection(&mut prepared, &core) {
                        ProjectSelectionResult::Activated(_) => {}
                        ProjectSelectionResult::Refused(_) => {
                            assert_eq!(core.attachment_state().unwrap(), AttachmentState::Live);
                            assert!(matches!(
                                bundle.publish_project_selection(&mut prepared, &core),
                                ProjectSelectionResult::Activated(_)
                            ));
                        }
                        ProjectSelectionResult::Unknown
                        | ProjectSelectionResult::CommittedWithError(_) => {
                            panic!("unresolved selection after fault removed")
                        }
                    }
                }
                ProjectSelectionResult::Refused(_) => {
                    assert_eq!(core.attachment_state().unwrap(), AttachmentState::Live);
                    bundle.save(&core, &input).unwrap();
                    bundle
                        .refresh_selection_source(&mut prepared, &core)
                        .unwrap();
                    assert!(matches!(
                        bundle.publish_project_selection(&mut prepared, &core),
                        ProjectSelectionResult::Activated(_)
                    ));
                }
            }
            assert_eq!(prepared.core().export_history().unwrap(), original);
            assert_eq!(bundle.manual_branch_id(), &right);
            assert_eq!(bundle.root().unwrap().manual_switch_generation, 1);
            assert_eq!(core.attachment_state().unwrap(), AttachmentState::Sealed);
            assert_eq!(
                bundle.branch_history(&right).unwrap().batches,
                original.batches
            );
            assert_eq!(
                bundle.checkpoint(&right).unwrap().batch_ids,
                bundle.checkpoint(&left).unwrap().batch_ids[..original.batches.len()]
            );
            assert!(matches!(
                bundle.publish_project_selection(&mut prepared, &core),
                ProjectSelectionResult::Activated(_)
            ));
            assert_eq!(bundle.branches_probe().len(), 2);
            let active = prepared.core();
            edit(&active, true);
            let inputs = bundle
                .checkpoint(&right)
                .unwrap()
                .input_manifest_ref
                .clone();
            bundle.save(&active, &inputs).unwrap();
            assert_eq!(bundle.root().unwrap().manual_switch_generation, 1);
            if fault.is_none() {
                let copied = bundle
                    .save_as(&temp.path("selected-copy.kinewright"))
                    .unwrap();
                assert!(matches!(
                    copied,
                    kinewright_project::bundle_copy::SaveAsOutcome::Copied(_)
                ));
                let read = LoadedBundle::read(&temp.path("selected-copy.kinewright")).unwrap();
                assert_eq!(read.root().unwrap().manual_switch_generation, 1);
                assert_eq!(read.root().unwrap().manual_branch_id, right);
                drop(read);
                assert_api_declarations(
                    &temp.path("selected-copy.kinewright"),
                    "manual-switch",
                    true,
                );
            }
            let loaded = LoadedBundle::read(&temp.path("b.kinewright")).unwrap();
            assert_eq!(loaded.root().unwrap().manual_branch_id, right);
            assert!(
                without_api_probe("manual-switch", || LoadedBundle::read(
                    &temp.path("b.kinewright")
                ))
                .is_err()
            );
            drop(loaded);
            assert_api_declarations(&temp.path("b.kinewright"), "manual-switch", false);
            drop(prepared);
            drop(bundle);
            let loaded = LoadedBundle::read(&temp.path("b.kinewright")).unwrap();
            let fresh = Core::spawn(Document::default()).unwrap();
            loaded.restore_manual(&fresh).unwrap();
            assert_eq!(
                fresh.export_history().unwrap(),
                active.export_history().unwrap()
            );
        }
    }
    #[test]
    fn w14_raw_role_schema_api_and_checkpoint_edges_are_required() {
        let (temp, core, mut bundle, input) = fixture();
        assert!(LoadedBundle::read(&temp.path("b.kinewright")).is_ok());
        assert!(
            without_api_probe("manual-switch", || LoadedBundle::read(
                &temp.path("b.kinewright")
            ))
            .is_ok()
        );
        let mut r = record(bundle.project_id().clone());
        let ack = checkpoint_root(&mut bundle);
        bundle.save(&core, &input).unwrap();
        r.acknowledged_root = Some(ack.clone());
        r.acknowledged_absence = None;
        bundle.retain_rescue_record(&r).unwrap();
        bundle.save(&core, &input).unwrap();
        assert!(LoadedBundle::read(&temp.path("b.kinewright")).is_ok());
        assert!(
            without_api_probe("rescue-copy", || LoadedBundle::read(
                &temp.path("b.kinewright")
            ))
            .is_err()
        );
        let restore = bundle
            .prepare_checkpoint_restore(&RecoverySelection::Checkpoint {
                root: ack,
                branch: bundle.manual_branch_id().clone(),
            })
            .unwrap();
        assert_ne!(restore.core().actor_identity(), core.actor_identity());
        assert_eq!(
            restore.core().export_history().unwrap(),
            core.export_history().unwrap()
        );
        let path = temp.path("b.kinewright");
        let original_root = std::fs::read(&path).unwrap();
        let root: serde_json::Value = serde_json::from_slice(&original_root).unwrap();
        let object_dir = path
            .with_extension("kinewright-assets")
            .join("studio/objects");
        let manifest_ref = root["studio"]["store_manifest_ref"].clone();
        let manifest_bytes =
            std::fs::read(object_dir.join(manifest_ref["sha256"].as_str().unwrap())).unwrap();
        for mutation in [
            "missing-api",
            "mismatched-api",
            "missing-role",
            "missing-edge",
            "third-predecessor",
            "extra-supported-api",
        ] {
            let mut raw: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
            match mutation {
                "missing-api" => {
                    raw["required_apis"]
                        .as_object_mut()
                        .unwrap()
                        .remove("rescue-copy");
                }
                "mismatched-api" => raw["required_apis"]["rescue-copy"] = serde_json::json!(2),
                "missing-role" => raw["evidence_roles"] = serde_json::json!({}),
                "missing-edge" => {
                    let key = raw["evidence_roles"]
                        .as_object()
                        .unwrap()
                        .keys()
                        .next()
                        .unwrap()
                        .clone();
                    raw["dependencies"][key] = serde_json::json!([]);
                }
                "extra-supported-api" => {
                    raw["required_apis"]["manual-switch"] = serde_json::json!(1);
                }
                _ => {
                    let extra = raw["recovery"]["predecessors"][0].clone();
                    raw["recovery"]["predecessors"]
                        .as_array_mut()
                        .unwrap()
                        .push(extra);
                }
            }
            let bytes = serde_json::to_vec(&raw).unwrap();
            let hash = kinewright_media::sha256_bytes(&bytes);
            std::fs::write(object_dir.join(&hash), &bytes).unwrap();
            let mut changed = root.clone();
            changed["studio"]["store_manifest_ref"]["sha256"] = serde_json::json!(hash);
            changed["studio"]["store_manifest_ref"]["byte_length"] = serde_json::json!(bytes.len());
            std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
            assert_eq!(
                LoadedBundle::read(&path).is_err(),
                mutation != "extra-supported-api",
                "{mutation}"
            );
            std::fs::write(&path, &original_root).unwrap();
            std::fs::remove_file(object_dir.join(hash)).unwrap();
        }
        let mut bad = r.clone();
        bad.schema = 2;
        assert!(bundle.retain_rescue_record(&bad).is_err());
        bad = r.clone();
        bad.acknowledged_root.as_mut().unwrap().record_kind = RecordKind::Document;
        assert!(bundle.retain_rescue_record(&bad).is_err());
    }
    #[test]
    fn w13_ack_and_two_predecessors_reopen_rotate_and_recover() {
        let (temp, core, mut bundle, input) = fixture();
        let ack = checkpoint_root(&mut bundle);
        bundle.save(&core, &input).unwrap();
        let mut r = record(bundle.project_id().clone());
        r.acknowledged_root = Some(ack.clone());
        r.acknowledged_absence = None;
        bundle.retain_rescue_record(&r).unwrap();
        bundle.save(&core, &input).unwrap();
        for n in 0..4 {
            edit(&core, n % 2 == 0);
            bundle.save(&core, &input).unwrap();
            bundle.collect_after_ack().unwrap();
        }
        assert_eq!(roots(&bundle).len(), 2);
        assert!(!roots(&bundle).contains(&ack));
        let left = bundle.manual_branch_id().clone();
        let restored = bundle
            .prepare_checkpoint_restore(&RecoverySelection::Checkpoint {
                root: ack,
                branch: left,
            })
            .unwrap();
        assert_eq!(restored.core().export_history().unwrap().batches.len(), 1);
        let loaded = LoadedBundle::read(&temp.path("b.kinewright")).unwrap();
        assert!(loaded.root().is_ok());
        drop(loaded);
        let copy = bundle.save_as(&temp.path("copy.kinewright")).unwrap();
        assert!(matches!(
            copy,
            kinewright_project::bundle_copy::SaveAsOutcome::Copied(_)
        ));
        assert!(LoadedBundle::read(&temp.path("copy.kinewright")).is_ok());
    }
    #[test]
    fn w11_independent_roots_survive_provenance_compaction() {
        let (temp, core, mut bundle, input) = fixture();
        let bytes = b"independent evidence";
        let reference = ObjectRef {
            sha256: kinewright_media::sha256_bytes(bytes),
            byte_length: bytes.len() as u64,
            record_kind: RecordKind::OwnedEvidence,
        };
        let _held = bundle
            .store_mut()
            .unwrap()
            .admit_blob(&mut &bytes[..], &reference, &[])
            .unwrap();
        bundle.retain_evidence(&reference).unwrap();
        let mut r = record(bundle.project_id().clone());
        r.compact((0..40).map(
            |_| kinewright_project::rescue_record::RescueAttemptProvenance {
                session_id: SessionId::fresh().unwrap(),
                project_id: bundle.project_id().clone(),
                destination: "older.kinewright".into(),
                parent_device: 1,
                parent_inode: 2,
                attempted_root_sha256: "d".repeat(64),
                pending_command: None,
            },
        ))
        .unwrap();
        assert!(r.summary.unresolved);
        bundle.retain_rescue_record(&r).unwrap();
        bundle.save(&core, &input).unwrap();
        assert!(bundle.evidence_refs().contains(&reference));
        assert!(LoadedBundle::read(&temp.path("b.kinewright")).is_ok());
    }
    #[test]
    fn w10_pending_command_exclusion_and_original_id_lookup() {
        use kinewright_project::{
            receipts::{CommandEnvelope, CommandId, ReceiptLookup},
            rescue_record::RescueAttemptProvenance,
        };
        let (_temp, core, mut bundle, input) = fixture();
        let issuer = bundle.issue_issuer(&core, &input, 1).unwrap();
        let envelope = CommandEnvelope {
            version: 1,
            action: "edit".into(),
            id: CommandId {
                issuer,
                sequence: 1,
            },
            project_id: bundle.project_id().clone(),
            target_branch: bundle.manual_branch_id().clone(),
            owner_epoch: bundle.store().owner_epoch(),
            lease: None,
            expected_head: bundle.manual_head().cloned(),
            expected_revision: kinewright_core::TimelineRevision(1),
            command: Some(kinewright_core::JournalCommand::Do(
                Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: true,
                },
            )),
            comment_text: None,
            comment_strokes: None,
            evidence: vec![],
            transition: None,
        };
        let mut r = record(bundle.project_id().clone());
        r.attempts.push(RescueAttemptProvenance {
            session_id: r.originating_session.clone(),
            project_id: bundle.project_id().clone(),
            destination: "a.kinewright".into(),
            parent_device: 1,
            parent_inode: 2,
            attempted_root_sha256: "c".repeat(64),
            pending_command: Some((envelope.id.clone(), envelope.digest().unwrap())),
        });
        bundle.retain_rescue_record(&r).unwrap();
        bundle.save(&core, &input).unwrap();
        assert_eq!(
            bundle.receipt_lookup(&envelope).unwrap(),
            ReceiptLookup::UnresolvedOrigin
        );
        let mut conflict = envelope.clone();
        conflict.expected_revision = kinewright_core::TimelineRevision(2);
        assert_eq!(
            bundle.receipt_lookup(&conflict).unwrap(),
            ReceiptLookup::IdPayloadConflict
        );
        for _ in 0..4 {
            bundle.save(&core, &input).unwrap();
        }
        assert_eq!(
            bundle.receipt_lookup(&envelope).unwrap(),
            ReceiptLookup::UnresolvedOrigin
        );
        let mut actual = envelope.clone();
        actual.id.issuer = bundle.issue_issuer(&core, &input, 1).unwrap();
        actual.expected_head = bundle.manual_head().cloned();
        let committed = bundle.command(&actual, &core, &input, &[]).unwrap();
        let ReceiptLookup::Committed(exact) = committed else {
            panic!("committed original receipt");
        };
        let ack = checkpoint_root(&mut bundle);
        bundle.save(&core, &input).unwrap();
        let mut retained = record(bundle.project_id().clone());
        retained.acknowledged_root = Some(ack);
        retained.acknowledged_absence = None;
        bundle.retain_rescue_record(&retained).unwrap();
        bundle.clear_current_receipt_details_probe();
        for _ in 0..4 {
            bundle.save(&core, &input).unwrap();
        }
        assert_eq!(
            bundle.receipt_lookup(&actual).unwrap(),
            ReceiptLookup::Retired(exact)
        );
    }
    #[test]
    fn w13_projection_preserves_every_non_linkage_payload() {
        use kinewright_project::bundle_recovery::verify_projection_payloads;
        for kind in ["document", "batch", "input"] {
            if std::env::var("JA_C4_L1_CASE").is_ok_and(|selected| selected != kind) {
                continue;
            }
            let (temp, core, mut bundle, input) = fixture();
            let first = checkpoint_root(&mut bundle);
            bundle.save(&core, &input).unwrap();
            let mut r = record(bundle.project_id().clone());
            r.acknowledged_root = Some(first);
            r.acknowledged_absence = None;
            let original_record = bundle.retain_rescue_record(&r).unwrap();
            let payload = b"independent rescue evidence";
            let independent = ObjectRef {
                sha256: kinewright_media::sha256_bytes(payload),
                byte_length: payload.len() as u64,
                record_kind: RecordKind::OwnedEvidence,
            };
            let _independent_hold = bundle
                .store_mut()
                .unwrap()
                .admit_blob(
                    &mut &payload[..],
                    &independent,
                    std::slice::from_ref(&original_record),
                )
                .unwrap();
            bundle.retain_evidence(&independent).unwrap();
            bundle.save(&core, &input).unwrap();
            let source = checkpoint_root(&mut bundle);
            bundle.save(&core, &input).unwrap();
            let _oracle_pin = bundle.store().pin(std::slice::from_ref(&source)).unwrap();
            let (projected, origin) = bundle.project_recovery_root(&source).unwrap();
            let closure = bundle
                .store()
                .closure(std::slice::from_ref(&projected))
                .unwrap();
            assert!(closure.contains(&independent));
            assert!(closure.contains(&original_record));
            assert!(origin.rebound);
            assert_eq!(origin.source_root, source);
            assert_eq!(origin.label(), "rebound recovery point");
            verify_projection_payloads(&bundle, &source, &projected).unwrap();
            let mut next = record(bundle.project_id().clone());
            next.acknowledged_root = Some(projected);
            next.acknowledged_absence = None;
            next.recovery_origin = Some(origin);
            bundle.retain_rescue_record(&next).unwrap();
            bundle.save(&core, &input).unwrap();
            assert!(LoadedBundle::read(&temp.path("b.kinewright")).is_ok());
            let changed_input = if kind == "input" {
                let mut inputs = (*bundle
                    .store()
                    .read_record::<kinewright_project::owned_inputs::OwnedInputManifest>(&input)
                    .unwrap())
                .clone();
                inputs.owner_epoch += 1;
                bundle.admit_inputs(&inputs).unwrap().reference.clone()
            } else {
                edit(&core, true);
                if kind == "batch" {
                    edit(&core, false);
                }
                input.clone()
            };
            bundle.save(&core, &changed_input).unwrap();
            let changed = checkpoint_root(&mut bundle);
            bundle.save(&core, &changed_input).unwrap();
            assert!(
                LoadedBundle::read(&temp.path("b.kinewright")).is_ok(),
                "mutant graph must be valid: {kind}"
            );
            assert!(
                verify_projection_payloads(&bundle, &source, &changed).is_err(),
                "accepted changed {kind}"
            );
        }
    }
    #[test]
    fn w13_selection_pin_capacity_refuses_before_publication() {
        let (temp, core, mut bundle, input) = fixture();
        let target = checkpoint_root(&mut bundle);
        bundle.save(&core, &input).unwrap();
        let before = std::fs::read(temp.path("b.kinewright")).unwrap();
        let history = core.export_history().unwrap();
        // Real decoded metadata readers are held through prepare. Their ordinary charges leave
        // room for every read without the target-pin charge, but not that charge as well.
        let root_manifest = bundle.root().unwrap().store_manifest_ref.clone();
        let retained = (0..8)
            .map(|_| {
                bundle
                    .store()
                    .read_record::<kinewright_project::store::StoreManifest>(&root_manifest)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let target_branch = bundle.manual_branch_id().clone();
        let baseline = bundle.store().usage().unwrap().decoded_bytes;
        let history_reader = bundle.branch_history(&target_branch).unwrap();
        let history_charge = bundle.store().usage().unwrap().decoded_bytes - baseline;
        drop(history_reader);
        let lineage_reader = bundle.branch_lineage(&target_branch).unwrap();
        let lineage_charge = bundle.store().usage().unwrap().decoded_bytes - baseline;
        drop(lineage_reader);
        let pin_metadata = bundle
            .store()
            .closure(std::slice::from_ref(&target))
            .unwrap()
            .len() as u64
            * 128;
        let before_charge = bundle.store().usage().unwrap().decoded_bytes;
        bundle.store().reset_decode_peaks_probe().unwrap();
        let control = bundle
            .prepare_project_selection(
                &core,
                RecoverySelection::Checkpoint {
                    root: target.clone(),
                    branch: target_branch.clone(),
                },
            )
            .unwrap();
        let peak = bundle.store().usage().unwrap();
        assert_eq!(
            peak.decoded_bytes - before_charge,
            history_charge + lineage_charge + pin_metadata,
            "the complete target pin has its own metadata charge"
        );
        let limit = peak.metadata_bytes + peak.peak_decoded_bytes - 1;
        drop(control);
        let limits = StorageLimits {
            metadata_bytes: limit,
            ..StorageLimits::default()
        };
        assert!(
            LoadedBundle::read_with_limits_probe(&temp.path("b.kinewright"), limits.clone())
                .is_ok()
        );
        bundle.restrict_limits_probe(limits).unwrap();
        let (result, trace) = io_observer::observe(|| {
            bundle.prepare_project_selection(
                &core,
                RecoverySelection::Checkpoint {
                    root: target,
                    branch: target_branch,
                },
            )
        });
        assert!(
            matches!(
                result,
                Err(kinewright_project::bundle::BundleError::Capacity(_))
            ),
            "pin admission unexpectedly fit"
        );
        assert_eq!(std::fs::read(temp.path("b.kinewright")).unwrap(), before);
        assert!(trace.events.is_empty());
        assert_eq!(core.export_history().unwrap(), history);
        drop(retained);
    }
    #[test]
    fn w14_oversized_unclaimed_raw_role_refuses() {
        for role in [false, true] {
            let (_temp, core, mut bundle, input) = fixture();
            let mut raw = serde_json::json!({"padding":"x".repeat(70_000)});
            if role {
                raw["role"] = serde_json::json!("rescue_copy_v1");
            }
            let bytes = serde_json::to_vec(&raw).unwrap();
            let reference = ObjectRef {
                sha256: kinewright_media::sha256_bytes(&bytes),
                byte_length: bytes.len() as u64,
                record_kind: RecordKind::OwnedEvidence,
            };
            let _hold = bundle
                .store_mut()
                .unwrap()
                .admit_blob(&mut bytes.as_slice(), &reference, &[])
                .unwrap();
            bundle.retain_evidence(&reference).unwrap();
            let result = bundle.save(&core, &input);
            assert_eq!(result.is_err(), role);
        }
    }
    fn copy_tree(source: &Path, destination: &Path) {
        std::fs::create_dir_all(destination).unwrap();
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let target = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    #[test]
    fn w13_selection_fresh_open_exact_and_one_byte_over() {
        let (temp, core, mut initial, input) = fixture();
        let target = checkpoint_root(&mut initial);
        for n in 0..8 {
            edit(&core, n % 2 == 0);
        }
        initial.save(&core, &input).unwrap();
        let branch = initial.manual_branch_id().clone();
        drop(initial);
        drop(core);
        // Same exact source IDs and bytes: random BranchId order otherwise changes reader decode peaks.
        let mut exact = 0;
        let target_id: kinewright_project::bundle_records::BranchId =
            serde_json::from_value(serde_json::json!("00000000000000000000000000000000")).unwrap();
        assert!(target_id < branch);
        for lane in 0..3 {
            let folder = temp.path(&format!("lane-{lane}"));
            std::fs::create_dir(&folder).unwrap();
            let path = folder.join("b.kinewright");
            std::fs::copy(temp.path("b.kinewright"), &path).unwrap();
            copy_tree(
                &temp.path("b.kinewright-assets"),
                &folder.join("b.kinewright-assets"),
            );
            let limit = if lane == 0 {
                StorageLimits::default()
            } else {
                StorageLimits {
                    live_history_bytes: exact - u64::from(lane == 2),
                    ..StorageLimits::default()
                }
            };
            let loaded = LoadedBundle::read_with_limits_probe(&path, limit.clone()).unwrap();
            let core = Core::spawn(Document::default()).unwrap();
            loaded.restore_manual(&core).unwrap();
            let mut bundle = loaded.into_writer().unwrap();
            let before = std::fs::read(&path).unwrap();
            let mut prepared = bundle
                .prepare_project_selection(
                    &core,
                    RecoverySelection::Checkpoint {
                        root: target.clone(),
                        branch: branch.clone(),
                    },
                )
                .unwrap();
            prepared.reuse_new_target_id_probe(target_id.clone());
            let result = bundle.publish_project_selection(&mut prepared, &core);
            eprintln!(
                "lane={lane} limit={} publication={} writer={} result={result:?}",
                limit.live_history_bytes,
                io_observer::publication_usage().peak_live_history_bytes,
                bundle.store().usage().unwrap().peak_live_history_bytes
            );
            if lane == 2 {
                assert!(
                    matches!(
                        result,
                        ProjectSelectionResult::Refused(
                            kinewright_project::bundle::BundleError::Capacity(_)
                        )
                    ),
                    "exact={exact}, result={result:?}"
                );
                assert_eq!(std::fs::read(&path).unwrap(), before);
                assert_eq!(core.attachment_state().unwrap(), AttachmentState::Live);
            } else {
                assert!(
                    matches!(result, ProjectSelectionResult::Activated(_)),
                    "{result:?}"
                );
                let loaded = LoadedBundle::read_with_limits_probe(&path, limit.clone()).unwrap();
                if lane == 0 {
                    exact = loaded.usage_probe().unwrap().peak_live_history_bytes;
                    eprintln!("reader={exact}");
                } else {
                    assert_eq!(loaded.usage_probe().unwrap().peak_live_history_bytes, exact);
                }
            }
            assert!(LoadedBundle::read_with_limits_probe(&path, limit).is_ok());
        }
    }
    #[test]
    fn w13_checkpoint_restore_uses_b_identity_without_rewriting_inputs() {
        let (temp, a_core, mut a, input) = fixture();
        let data = b"B-owned resource bytes";
        let object = ObjectRef {
            sha256: kinewright_media::sha256_bytes(data),
            byte_length: data.len() as u64,
            record_kind: RecordKind::OwnedData,
        };
        let held = a
            .store_mut()
            .unwrap()
            .admit_blob(&mut &data[..], &object, &[])
            .unwrap();
        let mut manifest = (*a.store().read_input_manifest(&input).unwrap()).clone();
        manifest
            .entries
            .push(kinewright_project::owned_inputs::OwnedInputEntry {
                entry_id: "data:recovery".into(),
                object,
                dependencies: vec![],
                selected_stream: None,
                time_base: None,
                origin: None,
                profile: None,
            });
        let input = a.admit_inputs(&manifest).unwrap().reference.clone();
        a.save(&a_core, &input).unwrap();
        let original = a_core.export_history().unwrap();
        let a_id = a.project_id().clone();
        let branch = a.manual_branch_id().clone();
        let source_root = checkpoint_root(&mut a);
        let source_input = a.checkpoint(&branch).unwrap().input_manifest_ref.clone();
        let kinewright_project::bundle_copy::SaveAsOutcome::Copied(copy) =
            a.save_as(&temp.path("fresh-b.kinewright")).unwrap()
        else {
            panic!("fresh copy");
        };
        let Ok(mut b) = copy.into_destination() else {
            panic!("definite copy");
        };
        assert_ne!(b.project_id(), &a_id);
        let live = Core::spawn(Document::default()).unwrap();
        b.attach_core(&live)
            .unwrap()
            .restore_history((*b.branch_history(b.manual_branch_id()).unwrap()).clone())
            .unwrap();
        let mut prepared = b
            .prepare_project_selection(
                &live,
                RecoverySelection::Checkpoint {
                    root: source_root,
                    branch,
                },
            )
            .unwrap();
        assert_ne!(prepared.core().actor_identity(), a_core.actor_identity());
        let lease = prepared.input_leases_mut().first_mut().unwrap();
        assert_eq!(lease.owner_epoch(), b.store().owner_epoch());
        let mut bytes = Vec::new();
        lease.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, data);

        assert!(matches!(
            b.publish_project_selection(&mut prepared, &live),
            ProjectSelectionResult::Activated(_)
        ));
        assert_eq!(
            b.checkpoint(b.manual_branch_id())
                .unwrap()
                .input_manifest_ref,
            source_input
        );
        assert_eq!(prepared.core().export_history().unwrap(), original);
        edit(&prepared.core(), true);
        b.save(&prepared.core(), &source_input).unwrap();
        let loaded = LoadedBundle::read(&temp.path("fresh-b.kinewright")).unwrap();
        assert_ne!(loaded.root().unwrap().project_id, a_id);
        assert_eq!(loaded.root().unwrap().input_manifest_ref, source_input);
        let b_id = b.project_id().clone();
        drop(loaded);
        drop(prepared);
        drop(b);
        drop(a);
        drop(held);
        drop(a_core);
        std::fs::remove_file(temp.path("b.kinewright")).unwrap();
        std::fs::remove_dir_all(temp.path("b.kinewright-assets")).unwrap();
        let loaded = LoadedBundle::read(&temp.path("fresh-b.kinewright")).unwrap();
        assert_eq!(loaded.root().unwrap().project_id, b_id);
        let fresh = Core::spawn(Document::default()).unwrap();
        loaded.restore_manual(&fresh).unwrap();
        assert_eq!(
            fresh.export_history().unwrap().batches.len(),
            original.batches.len() + 1
        );
        let writer = loaded.into_writer().unwrap();
        let mut restored = writer
            .prepare_checkpoint_restore(&RecoverySelection::WorkingBranch(
                writer.manual_branch_id().clone(),
            ))
            .unwrap();
        let mut owned = Vec::new();
        restored
            .input_leases_mut()
            .first_mut()
            .unwrap()
            .read_to_end(&mut owned)
            .unwrap();
        assert_eq!(owned, data);
        let _ = input;
    }
}

#[test]
fn w14_raw_switch_structure_and_ordinary_v3_backward_control() {
    use kinewright_project::bundle::{BundleWriteAdmission, StudioRoot};
    use kinewright_project::bundle_records::{BranchId, ObjectRef, RecordKind};
    let reference = |kind| ObjectRef {
        sha256: "c".repeat(64),
        byte_length: 1,
        record_kind: kind,
    };
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
        "c4-raw-portable",
    );
    let mut root = StudioRoot {
        project_id: ProjectId::fresh().unwrap(),
        origin_project_ids: vec![],
        origin_session_ids: vec![],
        generation: 3,
        creation_brief_ref: None,
        manual_branch_id: BranchId::fresh().unwrap(),
        manual_switch_generation: 0,
        candidate_branch_ids: vec![],
        branch_index_ref: reference(RecordKind::BranchIndex),
        version_index_ref: None,
        feedback_index_ref: None,
        run_index_ref: None,
        claim_index_ref: None,
        input_manifest_ref: reference(RecordKind::InputManifest),
        receipt_index_ref: None,
        issuer_state_ref: None,
        store_manifest_ref: reference(RecordKind::StoreManifest),
    };
    for generation in [0, 1, 3] {
        root.manual_switch_generation = generation;
        let mut raw = serde_json::to_value(kinewright_core::Document::default()).unwrap();
        raw["format_version"] = serde_json::json!(3);
        raw["studio"] = serde_json::to_value(&root).unwrap();
        let path = temp.path("raw.kinewright");
        std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        assert_eq!(
            BundleWriteAdmission::read(&path)
                .unwrap()
                .require_supported()
                .is_ok(),
            generation < 3
        );
    }
    #[cfg(not(target_os = "linux"))]
    {
        let path = temp.path("refused.kinewright");
        let (result, trace) = kinewright_project::io_observer::observe(|| {
            kinewright_project::bundle::ProjectBundle::activate(
                &path,
                BundleWriteAdmission::new_document(),
            )
        });
        assert!(matches!(
            result,
            Err(kinewright_project::bundle::BundleError::DurabilityUnsupported)
        ));
        assert!(trace.events.is_empty());
        assert!(!path.exists());
    }
}
