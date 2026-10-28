#![allow(
    clippy::too_many_lines,
    reason = "real participant cuts keep original envelopes and independent restore oracles together"
)]
use super::*;

fn switch_host(configuration: HostConfig) -> ProcessHost {
    let executable = std::env::var_os("JA_SWITCH_HOST_BINARY").map_or_else(
        || PathBuf::from(env!("CARGO_BIN_EXE_kinewright")),
        PathBuf::from,
    );
    let (client, child) = HostClient::launch(&executable, configuration).unwrap();
    ProcessHost { client, child }
}

pub(super) fn fast() {
    #[cfg(target_os = "linux")]
    for restored in [false, true] {
        linux::restart(restored);
    }
    #[cfg(not(target_os = "linux"))]
    portable_restart();
}
#[cfg(not(target_os = "linux"))]
fn portable_restart() {
    let root = temp("c11-native-protocol");
    let before = admission::tree(root.root());
    let mut host = switch_host(config(root.root(), None));
    assert!(
        host.client
            .request(HostCommand::CurrentAttachment)
            .unwrap()
            .error
            .is_some()
    );
    host.child.kill().unwrap();
    host.child.wait().unwrap();
    host = switch_host(config(root.root(), None));
    assert!(
        host.client
            .request(HostCommand::AcquireManual {
                client: "a".repeat(64)
            })
            .unwrap()
            .error
            .is_some()
    );
    assert!(
        host.client
            .request(HostCommand::PrepareSwitch {
                target: kinewright_project::bundle_switch::SwitchTarget::WorkingBranch {
                    branch: kinewright_project::bundle_records::BranchId::fresh().unwrap()
                }
            })
            .unwrap()
            .error
            .is_some()
    );
    host.orderly();
    assert_eq!(admission::tree(root.root()), before);
}

#[cfg(target_os = "linux")]
pub(super) mod linux {
    use super::*;
    use kinewright_core::{Command as Edit, Event, Operation, Track, TrackId, TrackKind};
    use kinewright_project::{
        bundle_recovery::RecoverySelection,
        bundle_switch::{SwitchAttachment, SwitchTarget},
        editing_session::Ja1EditingSession,
        receipts::CommandEnvelope,
        storage_budget::StorageLimits,
    };

    pub(super) fn setup() -> (
        TempDirectory,
        PathBuf,
        Ja1EditingSession,
        SwitchTarget,
        kinewright_project::owned_inputs::OwnedInputEntry,
    ) {
        let root = temp("c11-switch");
        let path = root.path("a.kinewright");
        let core = Core::spawn(Document {
            duration: kinewright_core::TimeCode(90),
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![serde_json::from_value(json!({"id":1,"asset":0,"source_range":{"start":0,"end":90},"content":{"solid":{"r":20,"g":40,"b":60}},"timeline_start":0,"effects":[],"transition_in":null})).unwrap()],
            }],
            ..Document::default()
        })
        .unwrap();
        let mut source =
            Ja1EditingSession::new(root.root(), &core, StorageLimits::default()).unwrap();
        let store = source.source_mut().unwrap();
        let (asset, entry, media, _expected_bytes) =
            import_known_wav(store.store_mut(), 700, &root.path("source.wav"), 550.0);
        let inputs = store
            .admit_inputs(&kinewright_project::owned_inputs::OwnedInputManifest {
                project_id: None,
                session_id: Some(store.session_id().clone()),
                owner_epoch: store.store().owner_epoch(),
                entries: vec![entry.clone()],
            })
            .unwrap();
        source.set_unsaved_inputs(inputs.reference.clone()).unwrap();
        core.request(Edit::Do(Operation::AddAsset { asset }))
            .unwrap();
        source.first_save(&path, StorageLimits::default()).unwrap();
        drop(inputs);
        drop(media);
        core.request(Edit::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: false,
        }))
        .unwrap();
        source.save_active().unwrap();
        let RecoverySelection::Checkpoint {
            root: checkpoint,
            branch,
        } = source.list_recovery_points().unwrap().remove(0)
        else {
            panic!("snapshot control")
        };
        let target = SwitchTarget::Snapshot {
            root: checkpoint,
            branch,
            version: "2".repeat(32),
        };
        source.prepare_host_attachment().unwrap();
        let cut = source.freeze_host_cut().unwrap();
        source.complete_host_cut(&cut, None, None).unwrap();
        (root, path, source, target, entry)
    }
    fn acquire(
        host: &ProcessHost,
        source: &Ja1EditingSession,
        identity: &str,
    ) -> kinewright_core::ManualPermit {
        let reply = successful(
            &host.client,
            HostCommand::AcquireManual {
                client: identity.into(),
            },
        );
        source
            .bind_manual_producer(reply.manual_lease.unwrap(), reply.manual_inputs.unwrap())
            .unwrap()
    }
    fn confirm(
        host: &ProcessHost,
        source: &mut Ja1EditingSession,
        attachment: &SwitchAttachment,
        identity: &str,
    ) -> kinewright_core::ManualPermit {
        let granted = successful(
            &host.client,
            HostCommand::ConfirmSwitch {
                client: identity.into(),
                installed: Box::new(attachment.identity().unwrap()),
            },
        );
        source
            .finish_host_switch(
                granted.manual_lease.unwrap(),
                granted.manual_inputs.unwrap(),
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            )
            .unwrap()
    }
    fn note(label: &str, started: Instant) {
        if let Some(path) = std::env::var_os("JA_SWITCH_TIMINGS") {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap();
            writeln!(
                file,
                "{}",
                json!({"case":label,"nanoseconds":started.elapsed().as_nanos()})
            )
            .unwrap();
        }
    }
    pub(super) fn restart(restored: bool) {
        let (root, path, mut source, target, expected_input) = setup();
        let mut host = switch_host(config(root.root(), Some(path.clone())));
        let identity = "d".repeat(64);
        let old_permit = acquire(&host, &source, &identity);
        let client = host.client.with_manual_editor(identity.clone());
        old_permit
            .request(Edit::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false,
            }))
            .unwrap();
        let delayed = old_permit.pending().unwrap();
        let ack = successful(
            &client,
            HostCommand::CommitHistory {
                client: identity.clone(),
                messages: delayed.clone(),
            },
        );
        source
            .acknowledge_manual(&old_permit, ack.manual_ack)
            .unwrap();
        let old = source.core().export_history().unwrap();
        let old_core = source.core().clone();
        let old_batch_ids = kinewright_project::bundle::LoadedBundle::read(&path)
            .unwrap()
            .manual_checkpoint()
            .unwrap()
            .batch_ids
            .clone();
        let started = Instant::now();
        let cut = source.freeze_host_cut().unwrap();
        let preview = successful(&client, HostCommand::PrepareSwitch { target });
        let payload = preview.switch_preview.unwrap();
        let envelope = preview.switch_envelope.unwrap();
        let published = successful(
            &client,
            HostCommand::Switch {
                envelope: Box::new(envelope.clone()),
            },
        );
        assert!(published.manual_lease.is_none());
        let original = published.switch_result.unwrap();
        let committed = published.switch_attachment.unwrap();
        let adopted_batch_ids = kinewright_project::bundle::LoadedBundle::read(&path)
            .unwrap()
            .manual_checkpoint()
            .unwrap()
            .batch_ids
            .clone();
        assert_eq!(original.target_version, Some("2".repeat(32)));
        committed.verify().unwrap();
        assert_eq!(committed.history, payload.history);
        assert_eq!(committed.inputs, payload.inputs);
        assert_eq!(committed.lineage, payload.lineage);
        assert!(!committed.lineage.current.is_empty());
        assert_eq!(committed.inputs.entries.len(), 1);
        assert_eq!(committed.inputs.entries[0], expected_input);
        assert_ne!(committed.current.branch, original.old.branch);
        assert!(
            host.client
                .request(HostCommand::AcquireManual {
                    client: "e".repeat(64)
                })
                .unwrap()
                .error
                .is_some()
        );
        if restored {
            source
                .activate_host_switch(&cut, &committed, |_| Ok(()))
                .unwrap();
        }
        // The original Switch reply is deliberately lost to the caller; real B5 completed.
        host.child.kill().unwrap();
        host.child.wait().unwrap();
        host = switch_host(config(root.root(), Some(path.clone())));
        let client = host.client.with_manual_editor(identity.clone());
        let replay = successful(
            &client,
            HostCommand::Switch {
                envelope: Box::new(envelope.clone()),
            },
        );
        assert_eq!(replay.switch_result.as_ref(), Some(&original));
        assert_eq!(replay.switch_attachment.as_ref(), Some(&committed));
        assert!(
            replay.manual_lease.is_none(),
            "a receipt never grants a lease"
        );
        assert_eq!(
            replay.status.branches.len(),
            2,
            "retry must not create another adopted branch"
        );
        if !restored {
            source
                .activate_host_switch(&cut, &committed, |_| Ok(()))
                .unwrap();
        }
        assert_eq!(
            source.core().attachment_state().unwrap(),
            kinewright_core::AttachmentState::Frozen
        );
        assert_eq!(
            old_core.attachment_state().unwrap(),
            kinewright_core::AttachmentState::Sealed
        );
        let wire = serde_json::to_vec(&HostCommand::ConfirmSwitch {
            client: identity.clone(),
            installed: Box::new(committed.identity().unwrap()),
        })
        .unwrap();
        let decoded = serde_json::from_slice::<HostCommand>(&wire);
        assert!(
            decoded.is_ok(),
            "confirmation wire must round-trip: {:?}",
            decoded.err()
        );
        let mut wrong = committed.clone();
        wrong.current.head = original.old.head.clone();
        assert!(
            client
                .request(HostCommand::ConfirmSwitch {
                    client: identity.clone(),
                    installed: Box::new(wrong.identity().unwrap())
                })
                .unwrap()
                .error
                .is_some()
        );
        let mut wrong = committed.clone();
        wrong.inputs.entries.clear();
        assert!(
            wrong.identity().is_err(),
            "resource data must hash to its committed input reference"
        );
        let mut wrong = committed.identity().unwrap();
        wrong.current.inputs.sha256 = "0".repeat(64);
        assert!(
            client
                .request(HostCommand::ConfirmSwitch {
                    client: identity.clone(),
                    installed: Box::new(wrong)
                })
                .unwrap()
                .error
                .is_some()
        );
        let permit = confirm(&host, &mut source, &committed, &identity);
        assert_eq!(
            kinewright_project::bundle::LoadedBundle::read(&path)
                .unwrap()
                .manual_checkpoint()
                .unwrap()
                .batch_ids,
            adopted_batch_ids
        );
        note(
            if restored {
                "switch-restart-after-restore"
            } else {
                "switch-restart-before-restore"
            },
            started,
        );
        assert!(matches!(
            old_permit.request(Edit::Undo).unwrap(),
            Event::MutationRefused { .. }
        ));
        let adopted = source.core().export_history().unwrap();
        assert!(matches!(
            permit
                .request(Edit::Do(Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: !adopted.document.tracks[0].sync_lock
                }))
                .unwrap(),
            Event::DocumentChanged { .. }
        ));
        permit.request(Edit::Undo).unwrap();
        assert_eq!(
            source.core().export_history().unwrap().document,
            adopted.document
        );
        let accepted = permit.pending().unwrap();
        let ack = successful(
            &client,
            HostCommand::CommitHistory {
                client: identity.clone(),
                messages: accepted,
            },
        );
        source.acknowledge_manual(&permit, ack.manual_ack).unwrap();
        assert_eq!(
            &kinewright_project::bundle::LoadedBundle::read(&path)
                .unwrap()
                .manual_checkpoint()
                .unwrap()
                .batch_ids[..adopted_batch_ids.len()],
            adopted_batch_ids.as_slice()
        );
        let returning = Instant::now();
        let cut = source.freeze_host_cut().unwrap();
        let back = successful(
            &client,
            HostCommand::PrepareSwitch {
                target: SwitchTarget::WorkingBranch {
                    branch: original.old.branch.clone(),
                },
            },
        )
        .switch_envelope
        .unwrap();
        let result = successful(
            &client,
            HostCommand::Switch {
                envelope: Box::new(back),
            },
        );
        let returned = result.switch_attachment.unwrap();
        assert_eq!(returned.current.branch, original.old.branch);
        assert_eq!(
            returned.history, old,
            "return restores exact old Undo/Redo, not just its Document"
        );
        source
            .activate_host_switch(&cut, &returned, |_| Ok(()))
            .unwrap();
        let back_permit = confirm(&host, &mut source, &returned, &identity);
        assert_eq!(
            kinewright_project::bundle::LoadedBundle::read(&path)
                .unwrap()
                .manual_checkpoint()
                .unwrap()
                .batch_ids,
            old_batch_ids
        );
        note("return", returning);
        assert!(
            matches!(
                old_permit.request(Edit::Redo).unwrap(),
                Event::MutationRefused { .. }
            ),
            "old lease must still refuse after return"
        );
        assert!(matches!(
            permit.request(Edit::Undo).unwrap(),
            Event::MutationRefused { .. }
        ));
        assert!(
            client
                .request(HostCommand::CommitHistory {
                    client: identity.clone(),
                    messages: delayed
                })
                .unwrap()
                .error
                .is_some(),
            "delayed old lease must refuse after return to the same BranchId"
        );
        let historical = successful(
            &client,
            HostCommand::Switch {
                envelope: Box::new(envelope.clone()),
            },
        );
        assert_eq!(
            historical.status.branches.len(),
            2,
            "later L saves must retain the ordinary R branch"
        );
        assert_eq!(historical.switch_result, Some(original.clone()));
        assert_eq!(
            historical.switch_attachment.unwrap().current.branch,
            returned.current.branch
        );
        assert!(historical.manual_lease.is_none());
        let mut conflict: CommandEnvelope = envelope;
        conflict.switch.as_mut().unwrap().switch_generation += 1;
        assert!(
            client
                .request(HostCommand::SwitchReceipt {
                    envelope: Box::new(conflict)
                })
                .unwrap()
                .error
                .is_some()
        );
        assert!(matches!(
            back_permit.request(Edit::Redo).unwrap(),
            Event::DocumentChanged { .. }
        ));
        host.client = client;
        host.orderly();
    }

    fn atomic_json(path: &Path, value: &impl serde::Serialize) {
        let partial = path.with_extension("partial");
        std::fs::write(&partial, serde_json::to_vec(value).unwrap()).unwrap();
        std::fs::rename(partial, path).unwrap();
    }
    fn marker(directory: &Path, name: &str) {
        std::fs::write(directory.join(name), []).unwrap();
    }
    fn wait_marker(directory: &Path, name: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !directory.join(name).exists() {
            assert!(
                Instant::now() < deadline,
                "participant marker timed out: {name}"
            );
            std::thread::park_timeout(Duration::from_millis(5));
        }
    }
    #[derive(serde::Serialize, serde::Deserialize)]
    struct GuiBootstrap {
        host: kinewright_agent::writer_host::HostBootstrap,
        history: kinewright_core::HistorySnapshot,
        identity: String,
    }
    struct GuiProcess(Child);
    impl Drop for GuiProcess {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn gui_start(
        directory: &Path,
        host: &ProcessHost,
        history: kinewright_core::HistorySnapshot,
        identity: &str,
    ) -> GuiProcess {
        std::fs::create_dir_all(directory).unwrap();
        atomic_json(
            &directory.join("private.json"),
            &GuiBootstrap {
                host: host.client.bootstrap(),
                history,
                identity: identity.into(),
            },
        );
        let log = std::fs::File::create(directory.join("participant.log")).unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "c11_switch_gui_participant",
                "--ignored",
                "--test-threads=1",
            ])
            .env("JA_C11_GUI_DIRECTORY", directory)
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap();
        let process = GuiProcess(child);
        wait_marker(directory, "ready");
        process
    }
    pub(crate) fn gui_participant() {
        let directory =
            PathBuf::from(std::env::var_os("JA_C11_GUI_DIRECTORY").expect("private GUI directory"));
        let bootstrap: GuiBootstrap =
            serde_json::from_slice(&std::fs::read(directory.join("private.json")).unwrap())
                .unwrap();
        let core = Core::spawn(Document::default()).unwrap();
        core.restore_history(bootstrap.history).unwrap();
        let mut owner =
            Ja1EditingSession::new(&directory, &core, StorageLimits::default()).unwrap();
        owner.prepare_host_attachment().unwrap();
        let cut = owner.freeze_host_cut().unwrap();
        marker(&directory, "ready");
        wait_marker(&directory, "install.json");
        let committed: SwitchAttachment =
            serde_json::from_slice(&std::fs::read(directory.join("install.json")).unwrap())
                .unwrap();
        let payload = kinewright_project::bundle_switch::SwitchPayload {
            history: committed.history.clone(),
            inputs: committed.inputs.clone(),
            lineage: committed.lineage.clone(),
        };
        let prepared = owner.prepare_gui_switch(payload).unwrap();
        marker(&directory, "before-restore");
        wait_marker(&directory, "continue-before");
        owner
            .activate_prepared_gui_switch(&cut, &committed, prepared)
            .unwrap();
        assert_eq!(
            owner.core().attachment_state().unwrap(),
            kinewright_core::AttachmentState::Frozen
        );
        marker(&directory, "after-restore");
        wait_marker(&directory, "continue-after");
        let current_host: kinewright_agent::writer_host::HostBootstrap =
            serde_json::from_slice(&std::fs::read(directory.join("host.json")).unwrap()).unwrap();
        let client =
            HostClient::connect(current_host.endpoint, current_host.auth, current_host.roots)
                .unwrap()
                .with_manual_editor(bootstrap.identity.clone());
        let reply = successful(
            &client,
            HostCommand::ConfirmSwitch {
                client: bootstrap.identity,
                installed: Box::new(committed.identity().unwrap()),
            },
        );
        owner
            .finish_host_switch(
                reply.manual_lease.unwrap(),
                reply.manual_inputs.unwrap(),
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            )
            .unwrap();
        assert_eq!(owner.core().export_history().unwrap(), committed.history);
        atomic_json(
            &directory.join("result.json"),
            &owner.core().export_history().unwrap(),
        );
        wait_marker(&directory, "quit");
    }
    pub(crate) fn matrix() {
        for gui_death in [false, true] {
            for after_restore in [false, true] {
                let (root, path, source, target, _) = setup();
                let mut host = switch_host(config(root.root(), Some(path.clone())));
                let identity = "c".repeat(64);
                successful(
                    &host.client,
                    HostCommand::AcquireManual {
                        client: identity.clone(),
                    },
                );
                let client = host.client.with_manual_editor(identity.clone());
                let directory = root.path("gui-1");
                let mut gui = gui_start(
                    &directory,
                    &host,
                    source.core().export_history().unwrap(),
                    &identity,
                );
                let envelope = successful(&client, HostCommand::PrepareSwitch { target })
                    .switch_envelope
                    .unwrap();
                let published = successful(
                    &client,
                    HostCommand::Switch {
                        envelope: Box::new(envelope.clone()),
                    },
                );
                assert!(published.manual_lease.is_none());
                let committed = published.switch_attachment.unwrap();
                let original = published.switch_result.unwrap();
                atomic_json(&directory.join("install.json"), &committed);
                wait_marker(&directory, "before-restore");
                if after_restore {
                    marker(&directory, "continue-before");
                    wait_marker(&directory, "after-restore");
                }
                let (gui_directory, mut gui) = if gui_death {
                    gui.0.kill().unwrap();
                    gui.0.wait().unwrap();
                    let second = root.path("gui-2");
                    let restarted = gui_start(&second, &host, committed.history.clone(), &identity);
                    atomic_json(&second.join("install.json"), &committed);
                    wait_marker(&second, "before-restore");
                    (second, restarted)
                } else {
                    host.child.kill().unwrap();
                    host.child.wait().unwrap();
                    host = switch_host(config(root.root(), Some(path.clone())));
                    (directory, gui)
                };
                let client = host.client.with_manual_editor(identity.clone());
                let retried = successful(
                    &client,
                    HostCommand::Switch {
                        envelope: Box::new(envelope),
                    },
                );
                assert!(retried.status.frozen);
                assert_eq!(retried.switch_result, Some(original));
                assert!(retried.manual_lease.is_none());
                assert_eq!(retried.status.branches.len(), 2);
                assert_eq!(retried.switch_attachment.as_ref(), Some(&committed));
                atomic_json(&gui_directory.join("host.json"), &host.client.bootstrap());
                marker(&gui_directory, "continue-before");
                marker(&gui_directory, "continue-after");
                wait_marker(&gui_directory, "result.json");
                let history: kinewright_core::HistorySnapshot = serde_json::from_slice(
                    &std::fs::read(gui_directory.join("result.json")).unwrap(),
                )
                .unwrap();
                assert_eq!(history, committed.history);
                assert_eq!(
                    successful(&client, HostCommand::ReadHistory)
                        .history
                        .unwrap(),
                    committed.history
                );
                marker(&gui_directory, "quit");
                assert!(wait_child(&mut gui.0).success());
                host.client = client;
                host.orderly();
            }
        }
        publication_cuts();
    }
    fn publication_cuts() {
        for committed in [false, true] {
            let (root, path, mut source, target, _) = setup();
            let signals = root.path("signals");
            std::fs::create_dir(&signals).unwrap();
            let configuration = config(root.root(), Some(path.clone()));
            let auth = HostAuth::generate().unwrap();
            let mut child = Command::new(env!("CARGO_BIN_EXE_kinewright"))
                .arg("host")
                .env(
                    "REV2_HOOK",
                    if committed {
                        "switch_after_publish"
                    } else {
                        "switch_before_publish"
                    },
                )
                .env("REV2_SIGNALS", &signals)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let mut stdin = child.stdin.take().unwrap();
            serde_json::to_writer(
                &mut stdin,
                &kinewright_agent::writer_host::HostLaunch {
                    issuer: None,
                    config: configuration.clone(),
                    auth: auth.clone(),
                },
            )
            .unwrap();
            writeln!(stdin).unwrap();
            drop(stdin);
            let mut line = String::new();
            std::io::BufReader::new(child.stdout.take().unwrap())
                .read_line(&mut line)
                .unwrap();
            let endpoint = serde_json::from_str(&line).unwrap();
            let client = HostClient::connect(endpoint, auth, configuration.roots).unwrap();
            let mut host = ProcessHost { client, child };
            let identity = "f".repeat(64);
            acquire(&host, &source, &identity);
            let client = host.client.with_manual_editor(identity.clone());
            let cut = source.freeze_host_cut().unwrap();
            let envelope = successful(&client, HostCommand::PrepareSwitch { target })
                .switch_envelope
                .unwrap();
            let pending_client = client.clone();
            let pending_envelope = envelope.clone();
            let pending = std::thread::spawn(move || {
                pending_client.request(HostCommand::Switch {
                    envelope: Box::new(pending_envelope),
                })
            });
            wait_marker(&signals, "paused");
            assert_eq!(
                std::fs::read_to_string(signals.join("paused")).unwrap(),
                if committed {
                    "switch_after_publish"
                } else {
                    "switch_before_publish"
                }
            );
            host.child.kill().unwrap();
            host.child.wait().unwrap();
            assert!(pending.join().unwrap().is_err());
            host = switch_host(config(root.root(), Some(path)));
            let client = host.client.with_manual_editor(identity.clone());
            if committed {
                let replay = successful(
                    &client,
                    HostCommand::Switch {
                        envelope: Box::new(envelope),
                    },
                );
                assert!(replay.manual_lease.is_none());
                assert_eq!(replay.status.branches.len(), 2);
                let current = replay.switch_attachment.unwrap();
                source
                    .activate_host_switch(&cut, &current, |_| Ok(()))
                    .unwrap();
                confirm(&host, &mut source, &current, &identity);
            } else {
                assert!(
                    client
                        .request(HostCommand::Switch {
                            envelope: Box::new(envelope)
                        })
                        .unwrap()
                        .error
                        .is_some()
                );
                assert_eq!(
                    successful(&client, HostCommand::CurrentAttachment)
                        .switch_attachment
                        .unwrap()
                        .switch_generation,
                    0
                );
                let barrier = successful(
                    &client,
                    HostCommand::BeginRecovery {
                        client: identity.clone(),
                        frozen: Some(HostSnapshot::from_history(cut.history().clone()).unwrap()),
                        pending: vec![],
                        discard_batches: None,
                    },
                )
                .recovery
                .unwrap();
                successful(
                    &client,
                    HostCommand::PublishBarrier {
                        barrier: Box::new(barrier.clone()),
                    },
                );
                let mut endpoint = source.manual_recovery_endpoint(&cut, vec![]).unwrap();
                endpoint.install(barrier.after.history.clone()).unwrap();
                let reply = successful(
                    &client,
                    HostCommand::ConfirmBarrier {
                        barrier: Box::new(barrier.clone()),
                        installed: HostSnapshot::from_history(endpoint.snapshot().unwrap())
                            .unwrap(),
                    },
                );
                endpoint
                    .finish(
                        reply.manual_lease.unwrap(),
                        reply.manual_inputs.unwrap(),
                        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
                    )
                    .unwrap();
                source
                    .finish_host_manual_recovery(&barrier.after.history_sha256)
                    .unwrap();
                assert_eq!(
                    successful(&client, HostCommand::Status)
                        .status
                        .branches
                        .len(),
                    1
                );
            }
            host.client = client;
            host.orderly();
        }
    }
}
