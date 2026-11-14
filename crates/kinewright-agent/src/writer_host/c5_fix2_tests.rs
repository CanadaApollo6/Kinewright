// The second program deliberately uses only the public Project API. On the
// mutation it must execute an accepted write, not merely compile differently.
const C5_FIX2_PUBLIC_PROBE: &str = r#"
use kinewright_core::{Core, Document};
use kinewright_project::{bundle::{BundleError, LoadedBundle}, bundle_records::{ObjectRef, RecordKind, SessionId}, namespace::ProjectNamespace, storage_budget::StorageLimits, store::ProjectStore};
fn refused<T>(value: Result<T, BundleError>, api: &str) {
    match value {
        Err(BundleError::RunLimit) => {},
        Ok(_) => panic!("C5_FIX2_PUBLIC_GUARD_ACCEPTED_WRITE {api}"),
        Err(error) => panic!("C5_FIX2_WRONG_REFUSAL {api}: {error}"),
    }
}
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let path = std::path::Path::new(&args[1]);
    let doc: Document = serde_json::from_slice(&std::fs::read(&args[2]).unwrap()).unwrap();
    let core = Core::spawn(doc.clone()).unwrap();
    let history = core.export_history().unwrap();
    let fresh = path.with_file_name(format!("fresh-{}.kinewright",std::process::id()));
    let mut control = ProjectStore::new(ProjectNamespace::acquire(&fresh).unwrap(),None,Some(SessionId::fresh().unwrap()),StorageLimits::default()).unwrap();
    control.admit_document(&doc).unwrap();
    control.admit_history(&history).unwrap();
    drop(control);
    let loaded = LoadedBundle::read(path).unwrap();
    let manual_core = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&manual_core).unwrap();
    let mut bundle = loaded.into_writer().unwrap();
    let used = bundle.store().usage().unwrap().objects;
    bundle.restrict_limits_probe(StorageLimits { objects: used + 256, ..Default::default() }).unwrap();
    // PUBLIC_GUARD
    let written = bundle.store_mut().unwrap().admit_document(&doc);
    if written.is_ok() { eprintln!("C5_FIX2_ACCEPTED_OBJECT_DELTA {}",bundle.store().usage().unwrap().objects-used); }
    refused(written, "admit_document");
    refused(bundle.store_mut().unwrap().admit_history(&history), "admit_history");
    let inventory = bundle.store().manifest().unwrap();
    refused(bundle.store_mut().unwrap().admit_manifest_probe(&inventory), "admit_manifest_probe");
    refused(bundle.store().reserve_history_shadow(1), "reserve_history_shadow");
    let object = ObjectRef { sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(), byte_length: 0, record_kind: RecordKind::OwnedEvidence };
    refused(bundle.store_mut().unwrap().admit_blob(&mut &b""[..],&object,&[]), "admit_blob");
    let (project_id,session_id) = bundle.store().input_owner();
    let request = kinewright_project::owned_inputs::InputRequest { project_id: project_id.clone(),session_id:session_id.clone(),owner_epoch:bundle.store().owner_epoch(),job:"reserve-probe".into(),object:object.clone(),source_locator_hint:None,dependencies:vec![],selected_stream:None,time_base:None,origin:None,profile:None };
    refused(bundle.store_mut().unwrap().admit_input(&request,&mut &b""[..]), "admit_input");
    let table = kinewright_project::owned_inputs::InputChunkTable { version:1,algorithm:"sha256".into(),chunk_size:kinewright_project::owned_inputs::INPUT_CHUNK_BYTES,source_sha256:object.sha256.clone(),source_length:0,chunk_sha256:vec![] };
    refused(bundle.store_mut().unwrap().admit_chunk_table(&table), "admit_chunk_table");
    refused(bundle.store_mut().unwrap().import_media(&mut &b""[..],0,kinewright_core::AssetId(777),path,None), "import_media");
    let manifest = kinewright_project::owned_inputs::OwnedInputManifest {project_id,session_id,owner_epoch:bundle.store().owner_epoch(),entries:vec![]};
    refused(bundle.admit_inputs(&manifest), "admit_inputs");
    refused(bundle.empty_inputs(), "empty_inputs");
    refused(bundle.issue_metadata_issuer(),"issue_metadata_issuer");
    let branch = bundle.manual_branch_id().clone();
    let inputs = bundle.checkpoint(&branch).unwrap().input_manifest_ref.clone();
    refused(bundle.issue_issuer(&manual_core,&inputs,1),"issue_issuer");
    let usage = kinewright_project::studio_records::RunUsageRecord {version:1,run_id:kinewright_project::studio_records::RunId::fresh().unwrap(),event_id:"ordinary".into(),input_tokens:None,output_tokens:None,cache_read_tokens:None,cache_write_tokens:None,cost_microunits:None,coverage:"unavailable".into(),evidence_refs:vec![]};
    refused(bundle.admit_orchestration_record(&usage),"admit_orchestration_record");
    refused(bundle.save_branch(&branch,&manual_core),"save_branch");
    refused(bundle.fork_branch(&branch, kinewright_project::history_record::HistoryRole::Ordinary),"fork_branch");
    assert!(bundle.store().usage().unwrap().objects <= used,"C5_FIX2_ORDINARY_WRITES_CONSUMED_RESERVE");
    // Person publishers may use the raw reserve; ordinary run writes remain refused.
    for (kind,bytes) in [(kinewright_project::studio_records::FeedbackEvidenceKind::Text,&b"text"[..]),(kinewright_project::studio_records::FeedbackEvidenceKind::Strokes,&b"[]"[..]),(kinewright_project::studio_records::FeedbackEvidenceKind::FrameProof,&b"{}"[..])] {
        bundle.admit_feedback_evidence(kind,bytes).unwrap();
    }
    bundle.save(&manual_core,&inputs).unwrap();
    refused(bundle.store_mut().unwrap().admit_document(&doc), "run write after person Save");
    drop(bundle);
    let namespace = ProjectNamespace::acquire(path).unwrap();
    assert!(ProjectStore::new(namespace,Some(kinewright_project::bundle_records::ProjectId::fresh().unwrap()),None,StorageLimits::default()).is_err(),"C5_FIX2_LOW_LEVEL_REOPEN_BYPASS");
    println!("C5_FIX2_PUBLIC_APIS_REFUSED");
}
"#;

fn c5_fix2_dependency(
    deps: &std::path::Path,
    fingerprints: &std::path::Path,
    metadata: &std::path::Path,
    name: &str,
) -> std::path::PathBuf {
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(metadata).unwrap()).unwrap();
    let wanted = metadata["deps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d[1] == name)
        .unwrap()[3]
        .as_u64()
        .unwrap();
    let package = if name.starts_with("kinewright_") {
        name.replace('_', "-")
    } else {
        name.into()
    };
    let entry = std::fs::read_dir(fingerprints)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| {
            if !p
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&format!("{package}-"))
            {
                return false;
            }
            let Ok(hash) = std::fs::read_to_string(p.join(format!("lib-{name}"))) else {
                return false;
            };
            let Ok(hash) = u64::from_str_radix(hash.trim(), 16) else {
                return false;
            };
            hash.swap_bytes() == wanted
        })
        .unwrap();
    let hash = entry
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .rsplit('-')
        .next()
        .unwrap();
    deps.join(format!("lib{name}-{hash}.rlib"))
}
fn c5_fix2_native_paths(
    path: &std::path::Path,
    output: &mut std::collections::BTreeSet<std::path::PathBuf>,
) {
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.filter_map(Result::ok) {
            let p = entry.path();
            if p.is_dir() {
                c5_fix2_native_paths(&p, output);
            } else if p.extension().is_some_and(|e| e == "a" || e == "so") {
                output.insert(path.into());
            }
        }
    }
}
fn c5_fix2_compile_probe(
    source: &str,
    directory: &std::path::Path,
    name: &str,
) -> std::process::Output {
    let deps = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let src = directory.join(format!("{name}.rs"));
    std::fs::write(&src, source).unwrap();
    let mut command = std::process::Command::new("rustc");
    command.args([
        "--edition=2024",
        "-L",
        &format!("dependency={}", deps.display()),
    ]);
    // Cargo's fingerprint selects the exact feature/profile graph of this test.
    // A newer rlib from another gate is not necessarily type-compatible.
    let fingerprints = deps.parent().unwrap().join(".fingerprint");
    let executable = std::env::current_exe().unwrap();
    let test_hash = executable
        .file_stem()
        .unwrap()
        .to_str()
        .unwrap()
        .rsplit('-')
        .next()
        .unwrap();
    let test_metadata = fingerprints.join(format!(
        "kinewright-agent-{test_hash}/test-lib-kinewright_agent.json"
    ));
    let project = c5_fix2_dependency(&deps, &fingerprints, &test_metadata, "kinewright_project");
    let project_hash = project
        .file_stem()
        .unwrap()
        .to_str()
        .unwrap()
        .rsplit('-')
        .next()
        .unwrap();
    let project_metadata = fingerprints.join(format!(
        "kinewright-project-{project_hash}/lib-kinewright_project.json"
    ));
    command
        .arg("--extern")
        .arg(format!("kinewright_project={}", project.display()));
    for library in ["kinewright_core", "serde_json"] {
        let path = c5_fix2_dependency(&deps, &fingerprints, &project_metadata, library);
        command
            .arg("--extern")
            .arg(format!("{library}={}", path.display()));
    }
    for variable in ["FFMPEG_DIR", "KINEWRIGHT_NEMO_DIR"] {
        if let Some(root) = std::env::var_os(variable) {
            command.arg("-L").arg(format!(
                "native={}",
                std::path::PathBuf::from(root).join("lib").display()
            ));
        }
    }
    let mut paths = std::collections::BTreeSet::new();
    c5_fix2_native_paths(&deps.parent().unwrap().join("build"), &mut paths);
    for path in paths {
        command.arg("-L").arg(format!("native={}", path.display()));
    }
    command
        .arg(&src)
        .arg("-o")
        .arg(directory.join(name))
        .output()
        .unwrap()
}
fn c5_fix2_public_apis(path: &std::path::Path, doc: &Document) {
    let directory = path.parent().unwrap().join("public-api-probe");
    std::fs::create_dir_all(&directory).unwrap();
    let document = directory.join("document.json");
    std::fs::write(&document, serde_json::to_vec(doc).unwrap()).unwrap();
    let control = c5_fix2_compile_probe(C5_FIX2_PUBLIC_PROBE, &directory, "ordinary");
    assert!(
        control.status.success(),
        "C5_FIX2_CONTROL_COMPILE {}",
        String::from_utf8_lossy(&control.stderr)
    );
    let result = std::process::Command::new(directory.join("ordinary"))
        .arg(path)
        .arg(&document)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "C5_FIX2_ORDINARY_PUBLIC_API {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let guarded = C5_FIX2_PUBLIC_PROBE.replace(
        "// PUBLIC_GUARD",
        "let _terminal = bundle.store_mut().unwrap().terminal_admission();",
    );
    let exposed = c5_fix2_compile_probe(&guarded, &directory, "public_guard");
    if exposed.status.success() {
        let result = std::process::Command::new(directory.join("public_guard"))
            .arg(path)
            .arg(&document)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "C5_FIX2_PUBLIC_GUARD_BEHAVIOR {}",
            String::from_utf8_lossy(&result.stderr)
        );
        panic!("C5_FIX2_PUBLIC_GUARD_MUST_NOT_EXIST");
    }
    let error = String::from_utf8_lossy(&exposed.stderr);
    assert!(
        error.contains("E0599") && error.contains("terminal_admission"),
        "C5_FIX2_UNEXPECTED_PROBE_FAILURE {error}"
    );
}

struct C5Fix2History {
    f: Fixture,
    run: RunId,
    parent: ExecutionIdentity,
    attempt: ExecutionIdentity,
    child: TaskId,
    expected: kinewright_core::HistorySnapshot,
    manual: kinewright_core::HistorySnapshot,
}
fn c5_fix2_editor(mut f: Fixture) -> C5Fix2History {
    {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        let owner = owner.owner_mut().unwrap();
        if !owner
            .durable_sessions()
            .iter()
            .any(|(_, guard)| guard.branch_id() == &f.brief.scope.base.branch)
        {
            owner.save_active().unwrap();
            let mut candidate = owner
                .fork_durable_branch(
                    "pressure-candidate",
                    kinewright_project::editing_session::DurableBranchRole::RunCandidate,
                )
                .unwrap();
            owner.persist_durable_branch(&mut candidate).unwrap();
            f.brief.scope.base = owner.candidate_binding(candidate.branch_id()).unwrap();
        }
    }
    let mut scope = Document::default();
    for id in [1, 2] {
        if let kinewright_core::Operation::AddMarker { marker } = history_marker(id) {
            scope.markers.push(marker);
        }
    }
    f.brief.scope =
        crate::orchestration::policy::director_scope(&scope, f.brief.scope.base.clone()).unwrap();
    {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        for id in [50, 51] {
            owner
                .core()
                .request(kinewright_core::Command::Do(history_marker(id)))
                .unwrap();
        }
        owner
            .core()
            .request(kinewright_core::Command::Undo)
            .unwrap();
        owner.owner_mut().unwrap().save_active().unwrap();
    }
    let (run, parent, director) = c4_director(&f);
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    director.accepted(
        "assign_task",
        OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: run.clone(),
            parent: parent.task.clone(),
            brief,
        })),
    );
    let child = f.projection(&run).tasks[1].id.clone();
    let (attempt, client, endpoint, core) = c4_reserve_task(&f, &run, &child);
    history_turn(&f, &attempt, &client, &endpoint, 1, true);
    let specialist = C4Mcp::new(&f, &run, &child);
    let expected = c4_mcp_edit_redo(&specialist, &core);
    let manual = {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        let producer = owner.run_producer(&run).unwrap();
        owner
            .checkpoint_task_history(&producer, &run, &child)
            .unwrap();
        owner.core().export_history().unwrap()
    };
    history_turn(&f, &attempt, &client, &endpoint, 1, false);
    drop(specialist);
    drop(director);
    drop(core);
    drop(client);
    C5Fix2History {
        f,
        run,
        parent,
        attempt,
        child,
        expected,
        manual,
    }
}
#[test]
fn s4b_c5_fix2_public_writes_cannot_spend_terminal_reserve() {
    let C5Fix2History {
        f,
        run,
        child,
        expected,
        manual,
        ..
    } = c5_fix2_editor(Fixture::new());
    let mut doc = (*expected.document).clone();
    if let kinewright_core::Operation::AddMarker { marker } = history_marker(999) {
        doc.markers.push(marker);
    }
    doc.validate().unwrap();
    let claim_root = f.temp.path("public-api-machine");
    let f = c4_restart_with(f, &claim_root, |path| c5_fix2_public_apis(path, &doc));
    assert_eq!(
        c5e_history(&f, &run, &child),
        expected,
        "C5_FIX2_TASK_EXACT_AFTER_PUBLIC_WRITES"
    );
    let stop = f.control(&f.editor, &run, RunAction::Stop);
    ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(stop),
            })
            .unwrap(),
        "C5_FIX2_STOP_AFTER_PUBLIC_WRITES",
    );
    let state = f.projection(&run);
    assert_eq!(
        state.cleanup,
        RunCleanup::Settled,
        "C5_FIX2_CLEANUP_AFTER_PUBLIC_WRITES"
    );
    let f = c4_restart(f, &claim_root);
    assert_eq!(f.projection(&run), state, "C5_FIX2_REOPEN_EXACT");
    assert_eq!(
        c5e_history(&f, &run, &child),
        expected,
        "C5_FIX2_TASK_HISTORY_EXACT"
    );
    assert_eq!(
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .core()
            .export_history()
            .unwrap(),
        manual,
        "C5_FIX2_MANUAL_HISTORY_EXACT"
    );
}

#[test]
fn s4b_c5_fix2_task_actor_cleanup_preserves_acknowledged_redo_at_reserve() {
    let C5Fix2History {
        f,
        run,
        parent,
        attempt,
        child,
        expected,
        manual,
    } = c5_fix2_editor(c5_fix1_fixture_limits(
        kinewright_project::storage_budget::StorageLimits {
            objects: 512,
            ..Default::default()
        },
    ));
    let producer = f.producer(&run);
    let host = f.host.owner.upgrade().unwrap();
    let mut closed = false;
    for _ in 0..128 {
        let mut owner = host.lock().unwrap();
        let state = owner.spending_scope(&producer, &run).unwrap();
        match owner.publish_native(&producer, state, &parent, vec![]) {
            Ok(()) => {}
            Err(error) => {
                assert!(
                    error.to_string().contains("Run limit"),
                    "C5_FIX2_PRESSURE {error}"
                );
                closed = true;
                break;
            }
        }
    }
    assert!(closed, "C5_FIX2_PRESSURE_PRECONDITION");
    // A new attachment needs a fresh issuer and grant while ordinary writes
    // are already closed. Their admission belongs to this exact Stop cut.
    ok(
        f.editor.request(HostCommand::Detach).unwrap(),
        "C5_FIX2_REPLACE_EDITOR",
    );
    let editor = f.actor(AttachmentRole::Editor, "reserve-stop");
    let stop = f.control(&editor, &run, RunAction::Stop);
    ok(
        editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(stop),
            })
            .unwrap(),
        "C5_FIX2_TASK_ACTOR_STOP",
    );
    let claim_root = f.temp.path("task-actor-pressure");
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &claim_root,
        parent.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    for identity in [&parent, &attempt] {
        crate::effect_pump::reconcile_attempt(&host, &mut claim, &producer, identity).unwrap();
    }
    claim.finish().unwrap();
    assert_eq!(
        f.projection(&run).cleanup,
        RunCleanup::Settled,
        "C5_FIX2_TASK_ACTOR_CLEANUP"
    );
    assert_eq!(
        c5e_history(&f, &run, &child),
        expected,
        "C5_FIX2_TASK_ACTOR_HISTORY"
    );
    let projection = f.projection(&run);
    let accounting = spending_record(&f, &run);
    drop(host);
    let f = c4_restart(f, &claim_root);
    assert_eq!(f.projection(&run), projection, "C5_FIX2_TASK_ACTOR_REOPEN");
    assert_eq!(
        spending_record(&f, &run),
        accounting,
        "C5_FIX2_TASK_ACTOR_ACCOUNTING"
    );
    assert_eq!(
        c5e_history(&f, &run, &child),
        expected,
        "C5_FIX2_TASK_ACTOR_REDO"
    );
    assert_eq!(
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .core()
            .export_history()
            .unwrap(),
        manual,
        "C5_FIX2_TASK_ACTOR_MANUAL"
    );
}
