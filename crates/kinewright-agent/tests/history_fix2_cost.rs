//! On-demand startup and editing measurements; no provider or GUI is launched.
#![cfg(target_os = "linux")]
use kinewright_agent::writer_host::{
    HostClient, HostCommand, HostConfig, HostOrigin, HostSnapshot, HostSourceBinding, SessionRoots,
    SourceServer,
};
use kinewright_core::{
    Command, Core, Document, Event, Operation, TimeCode, Track, TrackId, TrackKind,
};
use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
use kinewright_project::{bundle::BundleWriteAdmission, editing_session::Ja1EditingSession};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn document() -> Document {
    Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    }
}
fn edit(core: &Core, index: usize) {
    assert!(matches!(
        core.request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: index.is_multiple_of(2)
        }))
        .unwrap(),
        Event::DocumentChanged { .. }
    ));
}
#[test]
#[ignore = "on-demand: alternating debug host startup measurement with KINEWRIGHT_BENCH_HOST and KINEWRIGHT_BENCH_EDITS"]
#[allow(
    clippy::too_many_lines,
    reason = "the measured real host startup and exact-history verification use one owned lifecycle"
)]
fn history_fix2_startup_sample() {
    let executable = PathBuf::from(
        std::env::var_os("KINEWRIGHT_BENCH_HOST").expect("host executable prerequisite"),
    );
    let edits: usize = std::env::var("KINEWRIGHT_BENCH_EDITS")
        .expect("edit count prerequisite")
        .parse()
        .unwrap();
    let temp = TempDirectory::new_in(&workspace_target_dir(), "fix2-startup");
    let path = temp.path("startup.kinewright");
    let core = Core::spawn(document()).unwrap();
    let mut owner =
        Ja1EditingSession::activate(&path, BundleWriteAdmission::new_document(), &core, None)
            .unwrap();
    owner.save_active().unwrap();
    for index in 0..edits {
        edit(&core, index);
    }
    let history = core.export_history().unwrap();
    assert_eq!(history.batches.len(), edits);
    println!(
        "STARTUP_INPUT={}",
        serde_json::json!({"edits":edits,"encoded":history.encoded_bytes().unwrap(),"retained":kinewright_core::history_size::restored_history_live_bytes(&history).unwrap(),"operation_bytes":std::mem::size_of::<Operation>(),"history_batch_bytes":std::mem::size_of::<kinewright_core::HistoryBatch>()})
    );
    let transfer = owner.prepare_host_attachment().unwrap();
    let live = Arc::new(AtomicBool::new(true));
    let context = Arc::downgrade(&live);
    let server = if let Some(service) = transfer.source_service {
        SourceServer::start(service).unwrap()
    } else {
        SourceServer::start_context(move || {
            if context
                .upgrade()
                .is_some_and(|live| live.load(Ordering::Acquire))
            {
                Ok(())
            } else {
                Err(kinewright_project::bundle::BundleError::Identity(
                    "benchmark GUI context ended".into(),
                ))
            }
        })
        .unwrap()
    };
    let config = HostConfig {
        project: "fix2-startup".into(),
        path: transfer.named_path,
        roots: SessionRoots {
            read: vec![temp.root().into()],
            write: vec![temp.root().into()],
        },
        scheduler_settings: Some(temp.path("scheduler")),
        origin: HostOrigin::Gui {
            retention: HostSourceBinding {
                inputs: transfer.source.as_ref().map(|_| transfer.inputs.clone()),
                source: transfer.source,
                callback: server.bootstrap(),
            },
            initial_snapshot: HostSnapshot::from_history(transfer.history).unwrap(),
            expected_root_sha256: transfer.expected_root_sha256,
            expected_project_id: transfer.project_id,
        },
    };
    let started = Instant::now();
    let result = HostClient::launch_with_issuer(&executable, config);
    let milliseconds = started.elapsed().as_secs_f64() * 1000.0;
    let error = result.as_ref().err().map(ToString::to_string);
    println!(
        "STARTUP_SAMPLE={}",
        serde_json::json!({"edits":edits,"milliseconds":milliseconds,"error":error,"host":executable})
    );
    if let Ok((client, mut child, _issuer)) = result {
        let received = client.request(HostCommand::ReadHistory).unwrap();
        assert!(received.error.is_none(), "history read refused");
        assert!(
            received.history.as_ref() == Some(&history),
            "STARTUP_RESTORES_ALL_HISTORY_EXACTLY"
        );
        let reply = client.request(HostCommand::Shutdown).unwrap();
        assert!(reply.error.is_none(), "{reply:?}");
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if child.try_wait().unwrap().is_none() {
            assert_eq!(
                std::fs::read_link(format!("/proc/{}/cwd", child.id())).unwrap(),
                std::env::current_dir().unwrap()
            );
            let command = std::fs::read(format!("/proc/{}/cmdline", child.id())).unwrap();
            let args: Vec<_> = command
                .split(|byte| *byte == 0)
                .filter(|arg| !arg.is_empty())
                .collect();
            assert_eq!(args[0], executable.as_os_str().as_encoded_bytes());
            assert_eq!(args[1], b"host");
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("benchmark host failed to stop gracefully");
        }
    }
    live.store(false, Ordering::Release);
    drop(server);
    assert_eq!(
        core.export_history().unwrap().document.duration,
        TimeCode::ZERO
    );
}

#[test]
#[ignore = "on-demand: debug manual edit and changed publication at 1,000 and 12,000 accepted edits"]
fn history_fix2_edit_and_save_sample() {
    use kinewright_project::storage_budget::StorageLimits;
    for edits in [1000, 12000] {
        let temp = TempDirectory::new_in(&workspace_target_dir(), "fix2-edit-cost");
        let path = temp.path("cost.kinewright");
        let core = Core::spawn(document()).unwrap();
        let mut owner =
            Ja1EditingSession::activate(&path, BundleWriteAdmission::new_document(), &core, None)
                .unwrap();
        owner.save_active().unwrap();
        for index in 0..edits {
            edit(&core, index);
        }
        let first_save = Instant::now();
        owner.save_active().unwrap();
        println!(
            "INITIAL_SAVE_SAMPLE={}",
            serde_json::json!({"edits":edits,"milliseconds":first_save.elapsed().as_secs_f64()*1000.0})
        );
        let lease = owner
            .issue_manual_lease("cost-person".into(), "cost-local".into())
            .unwrap();
        let gui = Core::spawn(Document::default()).unwrap();
        gui.restore_history(core.export_history().unwrap()).unwrap();
        let replica = Ja1EditingSession::new(temp.root(), &gui, StorageLimits::default()).unwrap();
        let permit = replica
            .bind_manual_producer(lease.clone(), owner.manual_input_context().unwrap())
            .unwrap();
        for sample in 0_usize..10 {
            let before = gui.export_history().unwrap();
            let start = Instant::now();
            let event = permit
                .request(Command::Do(Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: sample.is_multiple_of(2),
                }))
                .unwrap();
            let edit_ms = start.elapsed().as_secs_f64() * 1000.0;
            if let Event::MutationRefused { reason, .. } = &event {
                assert_eq!(
                    *reason,
                    kinewright_core::CoreMutationRefusal::ManualHistoryCapacity
                );
                assert_eq!(gui.export_history().unwrap(), before);
                assert!(permit.pending().unwrap().is_empty());
                let started = Instant::now();
                owner.save_active().unwrap();
                println!(
                    "EDIT_SAVE_REFUSAL={}",
                    serde_json::json!({"edits":edits,"retained_batches":before.batches.len(),"floor_offset":before.floor_offset,"sample":sample,"hand_edit_ms":edit_ms,"unchanged_save_ms":started.elapsed().as_secs_f64()*1000.0,"reason":reason.to_string()})
                );
                continue;
            }
            assert!(
                matches!(event, Event::DocumentChanged { .. }),
                "manual edit must return an exact outcome"
            );
            let pending = permit.pending().unwrap();
            let start = Instant::now();
            owner
                .commit_manual_history(&lease, pending.last().unwrap())
                .unwrap();
            let save_ms = start.elapsed().as_secs_f64() * 1000.0;
            replica.acknowledge_manual(&permit, pending).unwrap();
            println!(
                "EDIT_SAVE_SAMPLE={}",
                serde_json::json!({"edits":edits,"retained_batches":before.batches.len(),"floor_offset":before.floor_offset,"sample":sample,"hand_edit_ms":edit_ms,"save_ms":save_ms})
            );
        }
        let expected = gui.export_history().unwrap();
        drop(permit);
        drop(replica);
        drop(owner);
        let loaded = kinewright_project::bundle::LoadedBundle::read(&path).unwrap();
        let restored = Core::spawn(Document::default()).unwrap();
        loaded.restore_manual(&restored).unwrap();
        assert_eq!(
            restored.export_history().unwrap(),
            expected,
            "cost run must retain every accepted boundary"
        );
    }
}
