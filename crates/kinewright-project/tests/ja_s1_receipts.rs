#![cfg(all(feature = "test-util", target_os = "linux"))]
use kinewright_core::{
    Command, Core, Document, Event, JournalCommand, Operation, Query, QueryResult,
    TimelineRevision, Track, TrackId, TrackKind,
};
use kinewright_media::test_support::TempDirectory;
use kinewright_project::{
    bundle::{BundleError, BundleWriteAdmission, LoadedBundle, ProjectBundle},
    bundle_records::{ObjectRef, RecordKind},
    io_observer,
    receipts::{CommandEnvelope, CommandId, CommandReceipt, ReceiptLookup, ReceiptResult},
};
use std::{
    fs,
    path::{Path, PathBuf},
};
fn setup(path: &Path) -> (Core, ProjectBundle, ObjectRef) {
    let mut document = Document::default();
    document.tracks.push(Track {
        id: TrackId(83),
        kind: TrackKind::Video,
        sync_lock: true,
        clips: vec![],
    });
    let core = Core::spawn(document).unwrap();
    let mut bundle = ProjectBundle::activate(path, BundleWriteAdmission::new_document()).unwrap();
    let inputs = bundle.empty_inputs().unwrap();
    let reference = inputs.reference.clone();
    bundle
        .save(&core.export_history().unwrap(), &reference)
        .unwrap();
    drop(inputs);
    (core, bundle, reference)
}
fn revision(core: &Core) -> TimelineRevision {
    let Event::QueryResult(QueryResult::Snapshot { revision, .. }) =
        core.request(Command::Query(Query::Snapshot)).unwrap()
    else {
        panic!("snapshot")
    };
    revision
}
fn envelope(bundle: &mut ProjectBundle, core: &Core, inputs: &ObjectRef) -> CommandEnvelope {
    let issuer = bundle.issue_issuer(core, inputs, 1).unwrap();
    CommandEnvelope {
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
        expected_revision: revision(core),
        command: Some(JournalCommand::Do(Operation::SetTrackSyncLock {
            track: TrackId(83),
            locked: false,
        })),
        comment_text: None,
        comment_strokes: None,
        evidence: vec![],
    }
}
fn committed(lookup: ReceiptLookup) -> CommandReceipt {
    match lookup {
        ReceiptLookup::Committed(r) | ReceiptLookup::Retired(r) => r,
        other => panic!("expected committed result, got {other:?}"),
    }
}
fn reference(bytes: &[u8]) -> ObjectRef {
    ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedEvidence,
    }
}
fn comment(mut request: CommandEnvelope, text: &[u8], strokes: &[u8]) -> CommandEnvelope {
    request.action = "comment".into();
    request.command = None;
    request.comment_text = Some(reference(text));
    request.comment_strokes = Some(reference(strokes));
    request
}
fn execute_comment(
    bundle: &mut ProjectBundle,
    core: &Core,
    inputs: &ObjectRef,
    request: &CommandEnvelope,
    text: &[u8],
    strokes: &[u8],
) -> ReceiptLookup {
    bundle
        .command(
            request,
            core,
            inputs,
            &[
                (request.comment_text.as_ref().unwrap(), text),
                (request.comment_strokes.as_ref().unwrap(), strokes),
            ],
        )
        .unwrap()
}
fn restart(path: &Path) -> (Core, ProjectBundle, ObjectRef) {
    let loaded = LoadedBundle::read(path).unwrap();
    let core = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&core).unwrap();
    let inputs = loaded.root().unwrap().input_manifest_ref.clone();
    (core, loaded.into_writer().unwrap(), inputs)
}
fn inventory(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, path: &Path, out: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                out.insert(path.strip_prefix(root).unwrap().to_path_buf(), vec![]);
                walk(root, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(&path).unwrap(),
                );
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(root, root, &mut out);
    out
}
#[test]
fn edit_and_comment_lost_replies_reconcile_before_stale_admission() {
    let temp = TempDirectory::new("ja-s1-receipts");
    let path = temp.path("edit.kinewright");
    let (core, mut bundle, _) = setup(&path);
    let source = b"archived owned font bytes survive a new owner epoch";
    let input_request = kinewright_project::owned_inputs::InputRequest {
        project_id: Some(bundle.project_id().clone()),
        session_id: None,
        owner_epoch: bundle.store().owner_epoch(),
        job: "font-admission".into(),
        object: reference(source),
        source_locator_hint: None,
        dependencies: vec![],
        selected_stream: None,
        time_base: None,
        origin: None,
        profile: None,
    };
    let (owned, sealed) = bundle
        .store_mut()
        .unwrap()
        .admit_input(&input_request, &mut std::io::Cursor::new(source))
        .unwrap();
    let inputs = sealed.reference.clone();
    bundle
        .save(&core.export_history().unwrap(), &inputs)
        .unwrap();
    drop(sealed);
    let edit = envelope(&mut bundle, &core, &inputs);
    let original = committed(bundle.command(&edit, &core, &inputs, &[]).unwrap());
    assert_eq!(core.export_history().unwrap().cursor.undo.len(), 1);
    let text = "  Café\u{a0}🧭  \n\tsecond e\u{301}\r\n".as_bytes();
    let strokes = b" [ [3,5], [13,21] ]\r\n";
    let request = comment(envelope(&mut bundle, &core, &inputs), text, strokes);
    let comment_result = committed(execute_comment(
        &mut bundle,
        &core,
        &inputs,
        &request,
        text,
        strokes,
    ));
    let ReceiptResult::Comment {
        record_id,
        text: text_ref,
        strokes: stroke_ref,
        ..
    } = &comment_result.result
    else {
        panic!("comment")
    };
    assert!(!record_id.is_empty());
    assert_eq!(&*bundle.store().read_evidence(text_ref).unwrap(), text);
    assert_eq!(&*bundle.store().read_evidence(stroke_ref).unwrap(), strokes);
    drop(bundle);
    drop(core);
    let (core, mut bundle, inputs) = restart(&path);
    let mut reader = bundle.store().lease_input(&owned).unwrap();
    assert_eq!(reader.owner_epoch(), bundle.store().owner_epoch());
    assert_ne!(reader.owner_epoch(), request.owner_epoch);
    let mut actual = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut actual).unwrap();
    assert_eq!(actual, source);
    assert!(
        bundle
            .store_mut()
            .unwrap()
            .admit_input(&input_request, &mut std::io::Cursor::new(source))
            .is_err()
    );
    let before = core.export_history().unwrap();
    let disk = inventory(temp.root());
    assert_ne!(edit.owner_epoch, bundle.store().owner_epoch());
    let (reply, trace) = io_observer::observe(|| bundle.command(&edit, &core, &inputs, &[]));
    assert_eq!(committed(reply.unwrap()), original);
    assert!(trace.events.is_empty());
    let (reply, trace) = io_observer::observe(|| {
        execute_comment(&mut bundle, &core, &inputs, &request, text, strokes)
    });
    assert_eq!(committed(reply), comment_result);
    assert!(trace.events.is_empty());
    assert_eq!(core.export_history().unwrap(), before);
    assert_eq!(inventory(temp.root()), disk);
}
#[test]
fn same_id_changed_digest_refuses_without_mutation() {
    let temp = TempDirectory::new("ja-s1-digest");
    let path = temp.path("edit.kinewright");
    let (core, mut bundle, inputs) = setup(&path);
    let request = envelope(&mut bundle, &core, &inputs);
    committed(bundle.command(&request, &core, &inputs, &[]).unwrap());
    let mut changed = request.clone();
    changed.command = Some(JournalCommand::Undo);
    let before = inventory(temp.root());
    let history = core.export_history().unwrap();
    let (result, trace) = io_observer::observe(|| bundle.command(&changed, &core, &inputs, &[]));
    assert_eq!(result.unwrap(), ReceiptLookup::IdPayloadConflict);
    assert!(trace.events.is_empty());
    assert_eq!(inventory(temp.root()), before);
    assert_eq!(core.export_history().unwrap(), history);
}
#[test]
fn retired_exact_evidence_resolves_original_edit_and_comment_results() {
    let temp = TempDirectory::new("ja-s1-retired-exact");
    let path = temp.path("edit.kinewright");
    let (core, mut bundle, inputs) = setup(&path);
    let edit = envelope(&mut bundle, &core, &inputs);
    let result = committed(bundle.command(&edit, &core, &inputs, &[]).unwrap());
    bundle
        .acknowledge_issuer(&edit.id.issuer, edit.id.sequence, &core, &inputs)
        .unwrap();
    bundle
        .retire_issuer(&edit.id.issuer, false, &core, &inputs)
        .unwrap();
    for _ in 0..3 {
        bundle
            .save(&core.export_history().unwrap(), &inputs)
            .unwrap();
    }
    assert_eq!(
        committed(bundle.command(&edit, &core, &inputs, &[]).unwrap()),
        result
    );
    let text = b"not normalized  \r\n";
    let strokes = b"[[1,2],[3,4]]";
    let request = comment(envelope(&mut bundle, &core, &inputs), text, strokes);
    let result = committed(execute_comment(
        &mut bundle,
        &core,
        &inputs,
        &request,
        text,
        strokes,
    ));
    bundle
        .acknowledge_issuer(&request.id.issuer, request.id.sequence, &core, &inputs)
        .unwrap();
    bundle
        .retire_issuer(&request.id.issuer, false, &core, &inputs)
        .unwrap();
    let before = core.export_history().unwrap();
    drop(bundle);
    let (core, mut bundle, inputs) = restart(&path);
    let (result2, trace) = io_observer::observe(|| bundle.command(&request, &core, &inputs, &[]));
    assert_eq!(committed(result2.unwrap()), result);
    assert!(trace.events.is_empty());
    assert_eq!(core.export_history().unwrap(), before);
}
#[test]
fn retired_unresolved_never_reapplies_or_acknowledges() {
    let temp = TempDirectory::new("ja-s1-retired-unresolved");
    let path = temp.path("edit.kinewright");
    let (core, mut bundle, inputs) = setup(&path);
    let request = envelope(&mut bundle, &core, &inputs);
    committed(bundle.command(&request, &core, &inputs, &[]).unwrap());
    bundle
        .acknowledge_issuer(&request.id.issuer, request.id.sequence, &core, &inputs)
        .unwrap();
    bundle
        .retire_issuer(&request.id.issuer, true, &core, &inputs)
        .unwrap();
    for _ in 0..3 {
        bundle
            .save(&core.export_history().unwrap(), &inputs)
            .unwrap();
    }
    let history = core.export_history().unwrap();
    let disk = inventory(temp.root());
    let (result, trace) = io_observer::observe(|| bundle.command(&request, &core, &inputs, &[]));
    assert_eq!(result.unwrap(), ReceiptLookup::RetiredUnresolved);
    assert!(trace.events.is_empty());
    assert_eq!(core.export_history().unwrap(), history);
    assert_eq!(inventory(temp.root()), disk);
    drop(bundle);
    let (core, mut bundle, inputs) = restart(&path);
    assert_eq!(
        bundle.command(&request, &core, &inputs, &[]).unwrap(),
        ReceiptLookup::RetiredUnresolved
    );
}
#[test]
fn issuer_exhaustion_preserves_unresolved_evidence() {
    let temp = TempDirectory::new("ja-s1-issuer-cap");
    let path = temp.path("edit.kinewright");
    let (core, mut bundle, inputs) = setup(&path);
    let request = envelope(&mut bundle, &core, &inputs);
    let result = committed(bundle.command(&request, &core, &inputs, &[]).unwrap());
    assert!(
        bundle
            .retire_issuer(&request.id.issuer, true, &core, &inputs)
            .is_err()
    );
    let mut issued = vec![request.id.issuer.clone()];
    for _ in 1..128 {
        issued.push(bundle.issue_issuer(&core, &inputs, 1).unwrap());
    }
    let disk = inventory(temp.root());
    let (failed, trace) = io_observer::observe(|| bundle.issue_issuer(&core, &inputs, 1));
    assert!(matches!(
        failed,
        Err(BundleError::Capacity("issuer entries"))
    ));
    assert!(trace.events.is_empty());
    assert_eq!(inventory(temp.root()), disk);
    assert_eq!(
        committed(bundle.command(&request, &core, &inputs, &[]).unwrap()),
        result
    );
    assert_eq!(issued.len(), 128);
}
#[test]
fn ambiguous_commit_keeps_pending_until_receipt_reconciliation() {
    let temp = TempDirectory::new("ja-s1-receipt-unknown");
    let path = temp.path("edit.kinewright");
    let (core, mut bundle, inputs) = setup(&path);
    let control = envelope(&mut bundle, &core, &inputs);
    committed(bundle.command(&control, &core, &inputs, &[]).unwrap());
    assert_eq!(
        core.export_history().unwrap().cursor.undo.len(),
        1,
        "retained-reply positive control"
    );
    let request = envelope(&mut bundle, &core, &inputs);
    let before = core.export_history().unwrap();
    let (result, trace) = io_observer::observe(|| {
        io_observer::fail_once("root_parent_sync", || {
            bundle.command(&request, &core, &inputs, &[])
        })
    });
    assert!(matches!(result, Err(BundleError::DurabilityUnknown)));
    assert!(trace.events.iter().any(|e| e.operation == "root_rename"));
    assert!(
        trace
            .events
            .iter()
            .any(|e| e.operation == "injected_failure")
    );
    assert_eq!(core.export_history().unwrap(), before);
    assert_eq!(
        bundle.receipt_lookup(&request).unwrap(),
        ReceiptLookup::Pending
    );
    assert_eq!(
        bundle.command(&request, &core, &inputs, &[]).unwrap(),
        ReceiptLookup::Pending
    );
    bundle.reconcile_command(&core).unwrap();
    let result = committed(bundle.command(&request, &core, &inputs, &[]).unwrap());
    assert!(matches!(result.result, ReceiptResult::Edit { .. }));
    assert_eq!(core.export_history().unwrap().cursor.undo.len(), 2);
}
