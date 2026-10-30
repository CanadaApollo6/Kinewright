//! Real child-process owners use the shipping serialized command dispatcher.
//! The private pipe adds only deterministic IO fault scheduling and observations.
#![allow(clippy::too_many_lines)]
use super::*;

#[derive(serde::Serialize, serde::Deserialize)]
struct Request {
    command: HostCommand,
    fault: Option<String>,
}
fn participant() {
    let input = std::io::stdin();
    let mut lines = input.lock().lines();
    let configuration: HostConfig = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    let roots = configuration.roots.clone();
    let mut owner = HostCommandOwner::open(configuration).unwrap();
    for line in lines {
        let request: Request = serde_json::from_str(&line.unwrap()).unwrap();
        let shutdown = matches!(request.command, HostCommand::Shutdown);
        let execute = || owner.execute(request.command, roots.clone());
        let reply = match request.fault.as_deref() {
            None => execute(),
            Some("root_parent_sync") => {
                kinewright_project::io_observer::fail_once("root_parent_sync", execute)
            }
            Some("root_before_publish") => {
                kinewright_project::io_observer::fail_once("root_before_publish", execute)
            }
            Some(other) => panic!("unknown composed fault {other}"),
        };
        println!("C12_REPLY {}", serde_json::to_string(&reply).unwrap());
        std::io::stdout().flush().unwrap();
        if shutdown {
            break;
        }
    }
}
struct OwningProcess {
    child: Child,
    input: std::process::ChildStdin,
    replies: std::sync::mpsc::Receiver<kinewright_agent::writer_host::HostReply>,
    reader: Option<std::thread::JoinHandle<()>>,
}
impl OwningProcess {
    fn start(configuration: &HostConfig) -> Self {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "rescue_and_switch_share_one_owner_after_restart",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("JA_C12_OWNER_CHILD", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        writeln!(input, "{}", serde_json::to_string(configuration).unwrap()).unwrap();
        input.flush().unwrap();
        let output = child.stdout.take().unwrap();
        let (sender, replies) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in std::io::BufReader::new(output).lines() {
                let line = line.unwrap();
                if let Some((_, json)) = line.split_once("C12_REPLY ")
                    && sender.send(serde_json::from_str(json).unwrap()).is_err()
                {
                    break;
                }
            }
        });
        Self {
            child,
            input,
            replies,
            reader: Some(reader),
        }
    }
    fn request(
        &mut self,
        command: HostCommand,
        fault: Option<&str>,
    ) -> kinewright_agent::writer_host::HostReply {
        writeln!(
            self.input,
            "{}",
            serde_json::to_string(&Request {
                command,
                fault: fault.map(str::to_owned)
            })
            .unwrap()
        )
        .unwrap();
        self.input.flush().unwrap();
        self.replies
            .recv_timeout(Duration::from_secs(30))
            .expect("real host owner did not reply")
    }
    fn ok(&mut self, command: HostCommand) -> kinewright_agent::writer_host::HostReply {
        let reply = self.request(command, None);
        assert!(reply.error.is_none(), "{:?}", reply.error);
        reply
    }
    fn kill(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
    fn close(&mut self) {
        self.ok(HostCommand::Shutdown);
        assert!(wait_child(&mut self.child).success());
    }
}
impl Drop for OwningProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
    }
}
pub(super) fn run() {
    if std::env::var_os("JA_C12_OWNER_CHILD").is_some() {
        participant();
        return;
    }
    #[cfg(target_os = "linux")]
    composed();
    #[cfg(not(target_os = "linux"))]
    {
        let root = temp("c12-native-refusal");
        let configuration = config(root.root(), None);
        let before = admission::tree(root.root());
        let mut host = OwningProcess::start(&configuration);
        assert!(
            host.request(
                HostCommand::Save {
                    snapshot: snapshot()
                },
                None
            )
            .error
            .is_some()
        );
        host.kill();
        let mut successor = OwningProcess::start(&configuration);
        assert!(
            successor
                .request(
                    HostCommand::RescueTo {
                        destination: root.path("b.kinewright")
                    },
                    None
                )
                .error
                .is_some()
        );
        successor.close();
        assert_eq!(admission::tree(root.root()), before);
    }
}

#[cfg(target_os = "linux")]
fn composed() {
    use kinewright_agent::writer_host::{HostSourceBinding, SourceServer};
    use kinewright_core::{AttachmentState, Command as Edit, Event, Operation, TrackId};
    use kinewright_project::{
        bundle::LoadedBundle,
        editing_session::HostActivationKind,
        receipts::{ReceiptLookup, ReceiptResult},
    };
    use kinewright_project::{
        editing_session::Ja1EditingSession, owned_inputs::OwnedInputManifest,
        storage_budget::StorageLimits,
    };
    let started = Instant::now();
    let root = temp("c12-composed");
    let path = root.path("drive/a.kinewright");
    let core = Core::spawn(Document {
        tracks: vec![kinewright_core::Track {
            id: TrackId(1),
            kind: kinewright_core::TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    })
    .unwrap();
    core.request(Edit::Do(Operation::AddSolidClip {
        track: TrackId(1),
        timeline_start: kinewright_core::TimeCode(0),
        duration: kinewright_core::TimeCode(30),
        color: kinewright_core::SolidColor::default(),
    }))
    .unwrap();
    let mut gui = Ja1EditingSession::new(root.root(), &core, StorageLimits::default()).unwrap();
    let store = gui.source_mut().unwrap();
    let (asset, entry, media, expected_bytes) =
        import_known_wav(store.store_mut(), 700, &root.path("source.wav"), 660.0);
    let inputs = store
        .admit_inputs(&OwnedInputManifest {
            project_id: None,
            session_id: Some(store.session_id().clone()),
            owner_epoch: store.store().owner_epoch(),
            entries: vec![entry],
        })
        .unwrap()
        .reference
        .clone();
    gui.set_unsaved_inputs(inputs.clone()).unwrap();
    core.request(Edit::Do(Operation::AddAsset { asset }))
        .unwrap();
    drop(media);
    let prepared = gui.prepare_host_attachment().unwrap();
    let descriptor = prepared.source.unwrap();
    let service = prepared.source_service.unwrap();
    let mut callback = SourceServer::start(service.clone()).unwrap();
    let mut configuration = config(root.root(), None);
    configuration.origin = HostOrigin::Gui {
        initial_snapshot: HostSnapshot::from_history(prepared.history.clone()).unwrap(),
        retention: HostSourceBinding {
            source: Some(descriptor.clone()),
            inputs: Some(inputs.clone()),
            callback: callback.bootstrap(),
        },
        expected_root_sha256: None,
        expected_project_id: None,
    };
    let mut host = OwningProcess::start(&configuration);
    let first = host.ok(HostCommand::FirstSave {
        destination: path.clone(),
        snapshot: HostSnapshot::from_history(prepared.history).unwrap(),
        source: descriptor.clone(),
        inputs: inputs.clone(),
        callback: callback.bootstrap(),
    });
    let saved = first.saved.unwrap();
    let cut = gui.freeze_host_cut().unwrap();
    gui.complete_host_cut(&cut, first.history, Some(&saved.history_sha256))
        .unwrap();
    gui.set_host_association(&path, &saved.history_sha256)
        .unwrap();
    let saved = host.ok(HostCommand::Save {
        snapshot: HostSnapshot::capture(gui.core()).unwrap(),
    });
    let kinewright_agent::writer_host::HostRecoverySelection::Checkpoint {
        root: target_root,
        branch,
    } = saved.status.recovery_points[0].clone()
    else {
        panic!("checkpoint")
    };
    let target = kinewright_project::bundle_switch::SwitchTarget::Snapshot {
        root: target_root,
        branch,
        version: "c".repeat(32),
    };
    let source_root = gui
        .source()
        .unwrap()
        .store()
        .resolved_root()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let source_bytes = admission::tree(&source_root);
    let usage = gui.source().unwrap().store().usage_handle();
    let identity = "c".repeat(64);
    let grant = host.ok(HostCommand::AcquireManual {
        client: identity.clone(),
    });
    let old_permit = gui
        .bind_manual_producer(grant.manual_lease.unwrap(), grant.manual_inputs.unwrap())
        .unwrap();
    old_permit
        .request(Edit::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: true,
        }))
        .unwrap();
    let ack = host.ok(HostCommand::CommitHistory {
        client: identity.clone(),
        messages: old_permit.pending().unwrap(),
    });
    gui.acknowledge_manual(&old_permit, ack.manual_ack).unwrap();
    let old = gui.core().clone();
    let cut = gui.freeze_host_cut().unwrap();
    let source_identity = charged_identity(gui.source().unwrap().store());
    let preview = host.ok(HostCommand::PrepareSwitch { target });
    let prepared = gui
        .prepare_gui_switch(preview.switch_preview.unwrap())
        .unwrap();
    let envelope = preview.switch_envelope.unwrap();
    let switched = host.ok(HostCommand::Switch {
        envelope: Box::new(envelope.clone()),
    });
    let result = switched.switch_result.unwrap();
    let committed = switched.switch_attachment.unwrap();
    gui.activate_prepared_gui_switch(&cut, &committed, prepared)
        .unwrap();
    assert_eq!(old.attachment_state().unwrap(), AttachmentState::Sealed);
    assert!(switched.manual_lease.is_none());
    assert!(kinewright_project::namespace::ProjectNamespace::acquire(&path).is_err());
    assert_eq!(
        charged_identity(gui.source().unwrap().store()),
        source_identity
    );
    host.kill();
    drop(callback);
    callback = SourceServer::start(service.clone()).unwrap();
    let loaded = LoadedBundle::read(&path).unwrap();
    service
        .ensure_snapshot(&descriptor, &committed.history)
        .unwrap();
    configuration.path = Some(path.clone());
    configuration.origin = HostOrigin::Gui {
        initial_snapshot: HostSnapshot::from_history(committed.history.clone()).unwrap(),
        retention: HostSourceBinding {
            source: Some(descriptor),
            inputs: Some(inputs),
            callback: callback.bootstrap(),
        },
        expected_root_sha256: Some(kinewright_media::sha256_bytes(
            loaded.admission().raw_bytes(),
        )),
        expected_project_id: Some(loaded.root().unwrap().project_id.clone()),
    };
    drop(loaded);
    let mut host = OwningProcess::start(&configuration);
    let retried = host.ok(HostCommand::Switch {
        envelope: Box::new(envelope.clone()),
    });
    assert_eq!(retried.switch_result, Some(result.clone()));
    assert_eq!(retried.switch_attachment.as_ref(), Some(&committed));
    assert!(retried.manual_lease.is_none());
    assert_eq!(retried.status.branches.len(), 2);
    assert!(
        host.request(
            HostCommand::AcquireManual {
                client: identity.clone()
            },
            None
        )
        .error
        .is_some()
    );
    let grant = host.ok(HostCommand::ConfirmSwitch {
        client: identity.clone(),
        installed: Box::new(committed.identity().unwrap()),
    });
    let permit = gui
        .finish_host_switch(
            grant.manual_lease.unwrap(),
            grant.manual_inputs.unwrap(),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        )
        .unwrap();
    assert!(matches!(
        old_permit.request(Edit::Undo).unwrap(),
        Event::MutationRefused { .. }
    ));
    permit
        .request(Edit::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: false,
        }))
        .unwrap();
    let ack = host.ok(HostCommand::CommitHistory {
        client: identity,
        messages: permit.pending().unwrap(),
    });
    gui.acknowledge_manual(&permit, ack.manual_ack).unwrap();
    let captured = gui.core().export_history().unwrap();
    let source_actor = gui.core().clone();
    let cut = gui.freeze_host_cut().unwrap();
    let source_identity = charged_identity(gui.source().unwrap().store());
    let unknown = host.request(
        HostCommand::Save {
            snapshot: HostSnapshot::from_history(captured.clone()).unwrap(),
        },
        Some("root_parent_sync"),
    );
    assert_eq!(unknown.error.unwrap().code, "durability_unknown");
    let hidden = root.path("hidden-drive");
    std::fs::rename(path.parent().unwrap(), &hidden).unwrap();
    for failed in [true, false] {
        let preview = host.ok(HostCommand::PreviewRescue {
            snapshot: HostSnapshot::from_history(captured.clone()).unwrap(),
        });
        assert!(!preview.status.rescue_trim_required && !preview.status.rescue_omission_required);
        let destination = root.path(if failed {
            "failed.kinewright"
        } else {
            "b.kinewright"
        });
        let reply = host.request(
            HostCommand::RescueTo {
                destination: destination.clone(),
            },
            failed.then_some("root_before_publish"),
        );
        assert_eq!(admission::tree(&source_root), source_bytes);
        assert_eq!(
            charged_identity(gui.source().unwrap().store()),
            source_identity
        );
        if failed {
            assert!(reply.error.is_some());
            assert!(reply.activation.is_none());
            assert_eq!(gui.core().export_history().unwrap(), captured);
        } else {
            assert!(reply.error.is_none(), "{:?}", reply.error);
            assert!(reply.activation.is_some());
            let saved = reply.saved.unwrap();
            assert!(
                kinewright_project::namespace::ProjectNamespace::acquire(&destination).is_err()
            );
            gui.activate_host_cut(
                &cut,
                reply.history.unwrap(),
                &saved.history_sha256,
                HostActivationKind::Rescue,
                |_| Ok(()),
            )
            .unwrap();
            gui.set_host_association(&destination, &saved.history_sha256)
                .unwrap();
            assert_eq!(
                source_actor.attachment_state().unwrap(),
                AttachmentState::Sealed
            );
            assert_eq!(gui.core().export_history().unwrap(), captured);
        }
    }
    let b = root.path("b.kinewright");
    let b_actor = gui.core().actor_identity();
    let b_history = gui.core().export_history().unwrap();
    std::fs::rename(hidden, path.parent().unwrap()).unwrap();
    host.ok(HostCommand::Reconcile {
        destination: path.clone(),
    });
    host.ok(HostCommand::Reconcile {
        destination: path.clone(),
    });
    assert_eq!(gui.core().actor_identity(), b_actor);
    assert_eq!(gui.core().export_history().unwrap(), b_history);
    assert_eq!(
        host.ok(HostCommand::ReadHistory).history.unwrap(),
        b_history
    );
    assert_eq!(
        charged_identity(gui.source().unwrap().store()),
        source_identity
    );
    host.close();
    assert_eq!(
        admission::tree(&source_root),
        source_bytes,
        "host Drop cannot retire the actual GUI source"
    );
    assert_eq!(
        charged_identity(gui.source().unwrap().store()),
        source_identity
    );
    drop(callback);
    drop(gui);
    assert!(!source_root.exists());
    let retired = usage().unwrap();
    assert_eq!(retired.reserved_bytes, 0);
    assert_eq!(retired.reserved_copy_bytes, 0);
    assert!(!retired.damaged_accounting);
    let a = LoadedBundle::read(&path).unwrap().into_writer().unwrap();
    assert!(
        matches!(a.receipt_lookup(&envelope).unwrap(), ReceiptLookup::Committed(ref receipt) if matches!(receipt.result, ReceiptResult::Switch { .. }))
    );
    assert_eq!(a.manual_branch_id(), &result.new.branch);
    drop(a);
    let loaded = LoadedBundle::read(&b).unwrap();
    let fresh = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&fresh).unwrap();
    assert_eq!(fresh.export_history().unwrap(), b_history);
    let owner = loaded.into_writer().unwrap();
    let inputs = owner
        .store()
        .read_input_manifest(
            &owner
                .checkpoint(owner.manual_branch_id())
                .unwrap()
                .input_manifest_ref,
        )
        .unwrap();
    assert_eq!(inputs.entries.len(), 1);
    let object = &inputs.entries[0].object;
    let raw = std::fs::read(
        root.path("b.kinewright-assets/studio/objects")
            .join(&object.sha256),
    )
    .unwrap();
    assert_eq!(kinewright_media::sha256_bytes(&raw), object.sha256);
    assert_eq!(raw, expected_bytes);
    println!(
        "C12 composed wall seconds: {:.3}",
        started.elapsed().as_secs_f64()
    );
}
