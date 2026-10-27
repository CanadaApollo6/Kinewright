//! C8 production host routes. Native durable success is Linux-only.
use kinewright_agent::{
    ScriptedCall, ScriptedTurn,
    writer_host::{
        HOST_PROTOCOL, HostAuth, HostClient, HostCommand, HostCommandOwner, HostConfig, HostError,
        HostOrigin, HostRequest, HostSnapshot, SessionRoots, WriterHost,
    },
};
use kinewright_core::{Core, Document};
use kinewright_media::test_support::TempDirectory;
use kinewright_project::bundle::BundleError;
use serde_json::{Value, json};
use std::{
    io::{BufRead, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

fn durable_test_root() -> &'static Path {
    Path::new(env!("CARGO_TARGET_TMPDIR"))
}
fn temp(label: &str) -> TempDirectory {
    TempDirectory::new_in(durable_test_root(), label)
}
#[cfg(target_os = "linux")]
fn known_wav_f32(tone: f32) -> Vec<u8> {
    let data_length = 4_800_u32 * 4;
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend((data_length + 36).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16_u32.to_le_bytes());
    bytes.extend(3_u16.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(48_000_u32.to_le_bytes());
    bytes.extend(192_000_u32.to_le_bytes());
    bytes.extend(4_u16.to_le_bytes());
    bytes.extend(32_u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(data_length.to_le_bytes());
    for sample in 0..4_800_u16 {
        let value = 0.25 * (std::f32::consts::TAU * tone * f32::from(sample) / 48_000.0).sin();
        bytes.extend(value.to_le_bytes());
    }
    bytes
}
#[cfg(target_os = "linux")]
fn import_known_wav(
    store: &mut kinewright_project::store::ProjectStore,
    id: u64,
    locator: &Path,
    tone: f32,
) -> (
    kinewright_core::MediaAsset,
    kinewright_project::owned_inputs::OwnedInputEntry,
    Vec<kinewright_project::store::SealedObject>,
    Vec<u8>,
) {
    let bytes = known_wav_f32(tone);
    let (asset, entry, pins) = store
        .import_media(
            &mut bytes.as_slice(),
            bytes.len() as u64,
            kinewright_core::AssetId(id),
            locator,
            None,
        )
        .unwrap();
    (asset, entry, pins, bytes)
}
fn roots(path: &Path) -> SessionRoots {
    SessionRoots {
        read: vec![path.to_path_buf()],
        write: vec![path.to_path_buf()],
    }
}
fn config(path: &Path, named: Option<PathBuf>) -> HostConfig {
    HostConfig {
        project: "c8-exact-originating-context".into(),
        path: named,
        roots: roots(path),
        origin: HostOrigin::Headless,
    }
}
fn successful(
    client: &HostClient,
    command: HostCommand,
) -> kinewright_agent::writer_host::HostReply {
    let reply = client.request(command).unwrap();
    assert!(
        reply.error.is_none(),
        "{}",
        reply.error.map_or(String::new(), |e| e.to_string())
    );
    reply
}
fn snapshot() -> HostSnapshot {
    HostSnapshot::capture(&Core::spawn(Document::default()).unwrap()).unwrap()
}
fn credential(auth: &HostAuth) -> String {
    serde_json::to_value(auth)
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned()
}
fn raw_request(
    client: &HostClient,
    auth: Option<&HostAuth>,
    project: &str,
    protocol: u32,
    body: Vec<u8>,
) -> reqwest::StatusCode {
    let endpoint = client.endpoint();
    let mut request = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("http://{}/host", endpoint.address))
        .header("x-kinewright-project", project)
        .header("x-kinewright-protocol", protocol)
        .body(body);
    if let Some(auth) = auth {
        request = request.header("authorization", format!("Bearer {}", credential(auth)));
    }
    request.send().unwrap().status()
}
fn wait_child(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let status = child.wait().unwrap();
            panic!("host did not exit within its deadline: {status}");
        }
        std::thread::park_timeout(Duration::from_millis(5));
    }
}
struct ProcessHost {
    client: HostClient,
    child: Child,
}
impl ProcessHost {
    fn start(configuration: HostConfig) -> Self {
        let (client, child) =
            HostClient::launch(Path::new(env!("CARGO_BIN_EXE_kinewright")), configuration).unwrap();
        Self { client, child }
    }
    fn orderly(&mut self) {
        successful(&self.client, HostCommand::Shutdown);
        assert!(wait_child(&mut self.child).success());
    }
}
impl Drop for ProcessHost {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one authenticated positive control precedes the complete command refusal matrix"
)]
fn auth_and_project_protocol_guard_every_read_and_command() {
    let root = temp("c8-auth");
    let before = admission::tree(root.root());
    let host = WriterHost::start(config(root.root(), None)).unwrap();
    let client = host.client();
    successful(&client, HostCommand::Attach);
    let bootstrap = client.bootstrap();
    let wrong = HostAuth::generate().unwrap();
    for command in [
        HostCommand::Attach,
        HostCommand::Status,
        HostCommand::ReadHistory,
        HostCommand::Retry,
        HostCommand::PreviewForget,
        HostCommand::Forget,
        HostCommand::Release,
        HostCommand::Detach,
        HostCommand::Shutdown,
        HostCommand::Save {
            snapshot: snapshot(),
        },
        HostCommand::SaveAs {
            destination: root.path("forbidden.kinewright"),
            snapshot: snapshot(),
        },
        HostCommand::RescueTo {
            destination: root.path("forbidden-rescue.kinewright"),
        },
    ] {
        let request = serde_json::to_vec(&HostRequest {
            operation: None,
            editor: None,
            protocol: HOST_PROTOCOL,
            project: bootstrap.endpoint.project.clone(),
            roots: roots(root.root()),
            command,
        })
        .unwrap();
        assert_eq!(
            raw_request(
                &client,
                None,
                &bootstrap.endpoint.project,
                HOST_PROTOCOL,
                request.clone()
            ),
            reqwest::StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            raw_request(
                &client,
                Some(&wrong),
                &bootstrap.endpoint.project,
                HOST_PROTOCOL,
                request.clone()
            ),
            reqwest::StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            raw_request(
                &client,
                Some(&bootstrap.auth),
                "different-project",
                HOST_PROTOCOL,
                request.clone()
            ),
            reqwest::StatusCode::FORBIDDEN
        );
        assert_eq!(
            raw_request(
                &client,
                Some(&bootstrap.auth),
                &bootstrap.endpoint.project,
                HOST_PROTOCOL + 1,
                request
            ),
            reqwest::StatusCode::FORBIDDEN
        );
        assert_eq!(admission::tree(root.root()), before);
    }
    // Authentication wins even when the body cannot be decoded.
    assert_eq!(
        raw_request(
            &client,
            None,
            &bootstrap.endpoint.project,
            HOST_PROTOCOL,
            b"not json".to_vec()
        ),
        reqwest::StatusCode::UNAUTHORIZED
    );
    let discovery = serde_json::to_string(host.endpoint()).unwrap();
    let secret = credential(&bootstrap.auth);
    assert!(!discovery.contains(&secret));
    assert!(!format!("{client:?} {:?}", bootstrap.auth).contains(&secret));
    host.shutdown().unwrap();
    #[cfg(feature = "eval-harness")]
    for (mode, label) in [
        (
            kinewright_agent::ja_harness::protocol::Mode::Baseline,
            "baseline",
        ),
        (
            kinewright_agent::ja_harness::protocol::Mode::Revised,
            "revised",
        ),
    ] {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ja/s2-host-auth.json");
        let output = root.path(label);
        assert_eq!(
            kinewright_agent::ja_harness::run(&fixture, &output, mode).unwrap(),
            0
        );
        let summary: Value =
            serde_json::from_slice(&std::fs::read(output.join("summary.json")).unwrap()).unwrap();
        assert!(
            summary["assertions"]
                .as_array()
                .unwrap()
                .iter()
                .all(|a| a["passed"] == true)
        );
        if label == "baseline" {
            assert_eq!(summary["pending"].as_array().unwrap().len(), 1);
        } else {
            assert_eq!(summary["assertions"].as_array().unwrap().len(), 2);
        }
    }
}

#[test]
fn roots_and_forwarded_roots_cover_all_file_surfaces() {
    let root = temp("c8-roots");
    let outside = temp("c8-outside");
    let narrower = root.path("narrower");
    std::fs::create_dir(&narrower).unwrap();
    #[cfg(target_os = "linux")]
    let named = {
        use kinewright_project::bundle::{BundleWriteAdmission, ProjectBundle};
        let path = narrower.join("source.kinewright");
        let mut owner =
            ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
        let inputs = owner.empty_inputs().unwrap().reference.clone();
        owner
            .save(&Core::spawn(Document::default()).unwrap(), &inputs)
            .unwrap();
        Some(path)
    };
    #[cfg(not(target_os = "linux"))]
    let named = None;
    let host = WriterHost::start(config(root.root(), named)).unwrap();
    let client = host.client();
    #[cfg(target_os = "linux")]
    successful(
        &client,
        HostCommand::Save {
            snapshot: snapshot(),
        },
    );
    let outside_before = admission::tree(outside.root());
    let paths = [
        outside.path("escape.kinewright"),
        root.root()
            .join("..")
            .join(outside.root().file_name().unwrap())
            .join("escape.kinewright"),
    ];
    for destination in paths {
        for command in [
            HostCommand::SaveAs {
                destination: destination.clone(),
                snapshot: snapshot(),
            },
            HostCommand::RescueTo { destination },
        ] {
            let reply = client.request(command).unwrap();
            assert!(reply.error.is_some(), "outside destination was accepted");
            #[cfg(target_os = "linux")]
            assert_eq!(reply.error.unwrap().code, "root_refused");
            assert_eq!(admission::tree(outside.root()), outside_before);
        }
    }
    let scoped = client.with_roots(roots(&narrower));
    successful(&scoped, HostCommand::ReadHistory);
    let before = admission::tree(root.root());
    let reply = scoped
        .request(HostCommand::SaveAs {
            destination: root.path("not-forwarded.kinewright"),
            snapshot: snapshot(),
        })
        .unwrap();
    assert!(reply.error.is_some());
    #[cfg(target_os = "linux")]
    assert_eq!(reply.error.unwrap().code, "root_refused");
    assert_eq!(admission::tree(root.root()), before);
    #[cfg(target_os = "linux")]
    {
        std::os::unix::fs::symlink(outside.root(), root.path("alias")).unwrap();
        let reply = client
            .request(HostCommand::SaveAs {
                destination: root.path("alias/escape.kinewright"),
                snapshot: snapshot(),
            })
            .unwrap();
        assert!(reply.error.is_some());
        assert_eq!(admission::tree(outside.root()), outside_before);
    }
    host.shutdown().unwrap();
}
#[test]
fn stdio_proxy_preserves_auth_roots_and_protocol_stdout() {
    let root = temp("c8-stdio");
    let directory = root.path("source-电影-🎬");
    std::fs::create_dir(&directory).unwrap();
    let mut host = ProcessHost::start(config(&directory, None));
    let bootstrap = host.client.bootstrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_kinewright"))
        .arg("mcp-stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    writeln!(input, "{}", serde_json::to_string(&bootstrap).unwrap()).unwrap();
    for frame in [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"host_command","arguments":{"command":{"command":"status"}}}}),
    ] {
        writeln!(input, "{frame}").unwrap();
    }
    drop(input);
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "stdio process failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let output = String::from_utf8(result.stdout).unwrap();
    let frames: Vec<Value> = output
        .lines()
        .map(|line| serde_json::from_str(line).expect("stdout contains protocol JSON only"))
        .collect();
    assert_eq!(
        frames
            .iter()
            .map(|f| f["id"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert!(
        frames
            .iter()
            .all(|f| f["jsonrpc"] == "2.0" && f.get("error").is_none())
    );
    assert!(!output.contains(&credential(&bootstrap.auth)));
    assert!(!String::from_utf8_lossy(&result.stderr).contains(&credential(&bootstrap.auth)));
    let mut wrong = bootstrap.clone();
    wrong.auth = HostAuth::generate().unwrap();
    let bad = HostClient::connect(wrong.endpoint, wrong.auth, wrong.roots).unwrap();
    assert!(
        bad.proxy_stdio(
            &b"{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"tools/list\"}\n"[..],
            Vec::new()
        )
        .is_err()
    );
    host.orderly();
}

#[test]
fn host_owns_scripted_session_after_gui_detach() {
    let root = temp("c8-session");
    let directory = root.path("source-电影-🎬");
    std::fs::create_dir(&directory).unwrap();
    let mut host = ProcessHost::start(config(&directory, None));
    let reply = successful(
        &host.client,
        HostCommand::StartScripted {
            turns: vec![ScriptedTurn::new(
                vec![ScriptedCall {
                    tool: "get_timeline_state".into(),
                    arguments: json!({}),
                }],
                "c8-real-scripted-result",
            )],
            prompts: vec!["Inspect the actual host-owned Core".into()],
        },
    );
    let session = reply.session.expect("host assigned a session").id;
    successful(&host.client, HostCommand::Detach);
    let viewer = HostClient::connect(
        host.client.endpoint().clone(),
        host.client.bootstrap().auth,
        roots(&directory),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut events = Vec::new();
    loop {
        let status = successful(&viewer, HostCommand::SessionStatus { session })
            .session
            .unwrap();
        assert_eq!(status.id, session);
        assert!(!status.overflowed);
        events.extend(status.events);
        if !status.running {
            assert_eq!(status.turns, 1);
            assert!(
                events
                    .iter()
                    .any(|e| e.text.contains("c8-real-scripted-result")),
                "the actual driver did not finish"
            );
            assert!(
                events.iter().any(|e| e.kind == "tool_result"),
                "the real MCP tool did not run"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "host-owned scripted session stalled"
        );
        std::thread::park_timeout(Duration::from_millis(5));
    }
    host.orderly();
}

fn lifetime_participant() {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut line = String::new();
    input.read_line(&mut line).unwrap();
    let launch: kinewright_agent::writer_host::HostLaunch = serde_json::from_str(&line).unwrap();
    let host = WriterHost::start_with_auth(launch.config, launch.auth).unwrap();
    println!(
        "C8_ENDPOINT {}",
        serde_json::to_string(host.endpoint()).unwrap()
    );
    std::io::stdout().flush().unwrap();
    line.clear();
    input.read_line(&mut line).unwrap();
    match line.trim() {
        "orderly" => host.shutdown().unwrap(),
        "unwind" => {
            let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                let _host = host;
                panic!("C8 intentional caught host unwind");
            }));
            assert!(unwound.is_err());
        }
        _ => panic!("invalid lifetime participant command"),
    }
    println!("C8_HOST_DROPPED");
}

#[cfg(target_os = "linux")]
fn charged_identity(
    store: &kinewright_project::store::ProjectStore,
) -> (Vec<u64>, (u64, std::collections::BTreeMap<String, u64>)) {
    let usage = store.usage().unwrap();
    assert!(!usage.damaged_accounting);
    (
        vec![
            usage.physical_bytes,
            usage.owned_bytes,
            usage.metadata_bytes,
            usage.copy_bytes,
            usage.reserved_bytes,
            usage.reserved_copy_bytes,
            usage.objects,
            usage.edges,
        ],
        store.retention_probe().unwrap(),
    )
}

#[cfg(target_os = "linux")]
fn edit_gui_history_with_redo(core: &Core) {
    for command in [
        kinewright_core::Command::Do(kinewright_core::Operation::AddTrack {
            track: kinewright_core::Track {
                id: kinewright_core::TrackId(1),
                kind: kinewright_core::TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            },
        }),
        kinewright_core::Command::Do(kinewright_core::Operation::SetTrackSyncLock {
            track: kinewright_core::TrackId(1),
            locked: false,
        }),
        kinewright_core::Command::Undo,
    ] {
        assert!(matches!(
            core.request(command).unwrap(),
            kinewright_core::Event::DocumentChanged { .. }
        ));
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "each real process mode shares the same exact retained byte, pin, charge and Close oracles"
)]
fn host_failure_preserves_originating_context_or_refuses_recovery() {
    if std::env::var_os("JA_C8_LIFETIME_CHILD").is_some() {
        lifetime_participant();
        return;
    }
    #[cfg(target_os = "linux")]
    for mode in ["kill", "orderly", "unwind"].into_iter().filter(|mode| {
        match std::env::var("JA_C8_HOST_FAILURE_CASE") {
            Ok(chosen) => chosen == *mode,
            Err(_) => true,
        }
    }) {
        use kinewright_agent::writer_host::{HostLaunch, SourceServer};
        use kinewright_project::{
            bundle::LoadedBundle,
            bundle_records::{ObjectRef, RecordKind},
            editing_session::Ja1EditingSession,
            owned_inputs::{OwnedInputEntry, OwnedInputManifest},
            storage_budget::StorageLimits,
        };
        use std::io::Read as _;
        let root = temp(&format!("c8-lifetime-{mode}"));
        let core = Core::spawn(Document::default()).unwrap();
        edit_gui_history_with_redo(&core);
        let mut origin =
            Ja1EditingSession::new(root.root(), &core, StorageLimits::default()).unwrap();
        let bytes = format!("GUI-owned unique source bytes {mode}").into_bytes();
        let object = ObjectRef {
            sha256: kinewright_media::sha256_bytes(&bytes),
            byte_length: bytes.len() as u64,
            record_kind: RecordKind::OwnedData,
        };
        let source = origin.source_mut().unwrap();
        let blob = source
            .store_mut()
            .admit_blob(&mut &bytes[..], &object, &[])
            .unwrap();
        let inputs = source
            .admit_inputs(&OwnedInputManifest {
                project_id: None,
                session_id: Some(source.session_id().clone()),
                owner_epoch: source.store().owner_epoch(),
                entries: vec![OwnedInputEntry {
                    entry_id: format!("{mode}-source"),
                    object: object.clone(),
                    dependencies: vec![],
                    selected_stream: None,
                    time_base: None,
                    origin: None,
                    profile: None,
                }],
            })
            .unwrap()
            .reference
            .clone();
        drop(blob);
        origin.set_unsaved_inputs(inputs.clone()).unwrap();
        let prepared = origin.prepare_host_attachment().unwrap();
        let descriptor = prepared.source.unwrap();
        let service = prepared.source_service.unwrap();
        service
            .ensure_snapshot(&descriptor, &prepared.history)
            .unwrap();
        service
            .read_borrowed_source(&descriptor, &inputs, 0, &mut [])
            .unwrap();
        let callback = SourceServer::start(service.clone()).unwrap();
        let mut reader = service.open_borrowed_source(&descriptor, &object).unwrap();
        let source_root = origin
            .source()
            .unwrap()
            .store()
            .resolved_root()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let expected_tree = admission::tree(&source_root);
        let identity = charged_identity(origin.source().unwrap().store());
        assert!(identity.1.0 > 0 && !identity.1.1.is_empty());
        assert!(
            identity.0[4] > 0 && identity.0[5] > 0,
            "real source reservations must exist"
        );
        let auth = HostAuth::generate().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "host_failure_preserves_originating_context_or_refuses_recovery",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("JA_C8_LIFETIME_CHILD", "1")
            .env("JA_C8_SOURCE_ROOT", &source_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut private_pipe = child.stdin.take().unwrap();
        let mut configuration = config(root.root(), None);
        configuration.origin = HostOrigin::Gui {
            initial_snapshot: HostSnapshot::from_history(prepared.history.clone()).unwrap(),
            retention: kinewright_agent::writer_host::HostSourceBinding {
                source: Some(descriptor.clone()),
                inputs: Some(inputs.clone()),
                callback: callback.bootstrap(),
            },
            expected_root_sha256: None,
            expected_project_id: None,
        };
        writeln!(
            private_pipe,
            "{}",
            serde_json::to_string(&HostLaunch {
                config: configuration,
                auth: auth.clone()
            })
            .unwrap()
        )
        .unwrap();
        private_pipe.flush().unwrap();
        let output = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let reader_thread = std::thread::spawn(move || {
            let mut output = std::io::BufReader::new(output);
            let mut text = String::new();
            loop {
                let mut line = String::new();
                if output.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                text.push_str(&line);
                if let Some((_, endpoint)) = line.split_once("C8_ENDPOINT ") {
                    let parsed: kinewright_agent::writer_host::HostEndpoint =
                        serde_json::from_str(endpoint).unwrap();
                    tx.send(parsed).unwrap();
                }
            }
            text
        });
        let endpoint = match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("host participant did not start: {error}");
            }
        };
        let client = HostClient::connect(endpoint, auth.clone(), roots(root.root())).unwrap();
        assert_eq!(
            successful(&client, HostCommand::ReadHistory)
                .history
                .unwrap(),
            prepared.history,
            "host bootstrap must preserve the exact GUI history before first Save"
        );
        let path = root.path(if mode == "orderly" {
            "nested/parents/destination.kinewright"
        } else {
            "destination.kinewright"
        });
        let prior = admission::tree(root.root());
        let forbidden = client
            .request(HostCommand::FirstSave {
                destination: source_root.join("inside-source.kinewright"),
                snapshot: HostSnapshot {
                    history_sha256: kinewright_media::sha256_bytes(
                        &serde_json::to_vec(&prepared.history).unwrap(),
                    ),
                    history: prepared.history.clone(),
                },
                source: descriptor.clone(),
                inputs: inputs.clone(),
                callback: callback.bootstrap(),
            })
            .unwrap();
        assert!(
            forbidden.error.is_some(),
            "copy into the actual GUI source subtree was accepted"
        );
        assert_eq!(
            admission::tree(root.root()),
            prior,
            "source/destination separation must precede any mutation"
        );
        let reply = successful(
            &client,
            HostCommand::FirstSave {
                destination: path.clone(),
                snapshot: HostSnapshot {
                    history_sha256: kinewright_media::sha256_bytes(
                        &serde_json::to_vec(&prepared.history).unwrap(),
                    ),
                    history: prepared.history.clone(),
                },
                source: descriptor.clone(),
                inputs: inputs.clone(),
                callback: callback.bootstrap(),
            },
        );
        assert!(
            reply.saved.is_some(),
            "borrowed-source positive copy did not commit"
        );
        assert_eq!(admission::tree(&source_root), expected_tree);
        assert_eq!(charged_identity(origin.source().unwrap().store()), identity);
        if mode == "kill" {
            child.kill().unwrap();
            let _ = wait_child(&mut child);
        } else {
            writeln!(private_pipe, "{mode}").unwrap();
            private_pipe.flush().unwrap();
            assert!(
                wait_child(&mut child).success(),
                "orderly/unwind path must return after real Drop"
            );
        }
        drop(private_pipe);
        let stdout = reader_thread.join().unwrap();
        if mode != "kill" {
            assert!(stdout.contains("C8_HOST_DROPPED"));
        }
        assert!(!stdout.contains(&credential(&auth)));
        assert!(
            source_root.is_dir(),
            "host Drop deleted the GUI temporary root ({mode})"
        );
        assert_eq!(
            admission::tree(&source_root),
            expected_tree,
            "host Drop deleted GUI temporary bytes ({mode})"
        );
        assert_eq!(
            charged_identity(origin.source().unwrap().store()),
            identity,
            "host Drop uncharged a GUI source reservation ({mode})"
        );
        let mut replacement = config(root.root(), Some(path.clone()));
        replacement.origin = HostOrigin::Gui {
            initial_snapshot: HostSnapshot::from_history(prepared.history.clone()).unwrap(),
            retention: kinewright_agent::writer_host::HostSourceBinding {
                source: Some(descriptor.clone()),
                inputs: Some(inputs.clone()),
                callback: callback.bootstrap(),
            },
            expected_root_sha256: reply
                .saved
                .as_ref()
                .map(|report| report.root_sha256.clone()),
            expected_project_id: Some(
                LoadedBundle::read(&path)
                    .unwrap()
                    .root()
                    .unwrap()
                    .project_id
                    .clone(),
            ),
        };
        let prior = admission::tree(root.root());
        let (result, trace) =
            kinewright_project::io_observer::observe(|| HostCommandOwner::open(replacement));
        assert!(
            matches!(result, Err(HostError::Refused(ref refusal)) if refusal.code == "retention_lost"),
            "a new host instance or reacquired lock cannot prove old retained ownership"
        );
        assert!(trace.events.is_empty());
        assert_eq!(admission::tree(root.root()), prior);
        origin.host_lost().unwrap();
        let cut = core.export_history().unwrap();
        assert!(origin.freeze_host_cut().is_err());
        for operation in 0..3 {
            let before = admission::tree(root.root());
            let result = match operation {
                0 => origin
                    .first_save(
                        &root.path("fresh-first.kinewright"),
                        StorageLimits::default(),
                    )
                    .map(|_| ()),
                1 => origin.save_active().map(|_| ()),
                2 => origin
                    .save_as(&root.path("fresh-as.kinewright"), StorageLimits::default())
                    .map(|_| ()),
                _ => unreachable!(),
            };
            assert!(
                result.is_err(),
                "unproved replacement retention accepted route {operation}"
            );
            assert_eq!(admission::tree(root.root()), before);
            assert_eq!(core.export_history().unwrap(), cut);
            assert_eq!(charged_identity(origin.source().unwrap().store()), identity);
        }
        let mut retained_bytes = Vec::new();
        reader.read_to_end(&mut retained_bytes).unwrap();
        assert_eq!(retained_bytes, bytes);
        let usage = origin.source().unwrap().store().usage_handle();
        drop(callback);
        drop(origin);
        assert!(
            source_root.is_dir(),
            "reader must retain the actual GUI source owner"
        );
        assert!(
            service.open_borrowed_source(&descriptor, &object).is_err(),
            "Close must fence new readers"
        );
        drop(reader);
        assert!(
            !source_root.exists(),
            "last reader must permit exactly-once originating cleanup"
        );
        let retired = usage().unwrap();
        assert_eq!(retired.reserved_bytes, 0);
        assert_eq!(retired.reserved_copy_bytes, 0);
        assert!(!retired.damaged_accounting);
        let loaded = LoadedBundle::read(&path).unwrap();
        let durable = Core::spawn(Document::default()).unwrap();
        loaded.restore_manual(&durable).unwrap();
        assert_eq!(durable.export_history().unwrap(), cut);
        assert_eq!(
            std::fs::read(
                path.parent()
                    .unwrap()
                    .join("destination.kinewright-assets/studio/objects")
                    .join(&object.sha256)
            )
            .unwrap(),
            bytes,
            "destination-owned input bytes must survive after GUI source retirement"
        );
    }
    #[cfg(target_os = "linux")]
    {
        lost_callback_refuses_all_host_publication_routes();
        named_gui_bootstrap_preserves_unsaved_history_and_saved_identity();
    }
    #[cfg(not(target_os = "linux"))]
    {
        let root = temp("c8-source-refusal");
        let before = admission::tree(root.root());
        let (result, trace) = kinewright_project::io_observer::observe(|| {
            kinewright_project::bundle_copy::SessionStore::create(
                root.root(),
                kinewright_project::storage_budget::StorageLimits::default(),
            )
        });
        assert!(matches!(result, Err(BundleError::DurabilityUnsupported)));
        assert!(trace.events.is_empty());
        assert_eq!(admission::tree(root.root()), before);
    }
}

#[cfg(target_os = "linux")]
fn named_gui_bootstrap_preserves_unsaved_history_and_saved_identity() {
    use kinewright_agent::writer_host::{HostSourceBinding, SourceServer};
    use kinewright_project::{
        bundle::{BundleWriteAdmission, ProjectBundle},
        editing_session::Ja1EditingSession,
    };
    let root = temp("c8-named-bootstrap");
    let path = root.path("named.kinewright");
    let mut bundle = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let inputs = bundle.empty_inputs().unwrap().reference.clone();
    let saved = bundle
        .save(&Core::spawn(Document::default()).unwrap(), &inputs)
        .unwrap();
    drop(bundle);
    let before = std::fs::read(&path).unwrap();
    let mut gui = Ja1EditingSession::open(&path).unwrap();
    let core = gui.core().clone();
    edit_gui_history_with_redo(&core);
    let prepared = gui.prepare_host_attachment().unwrap();
    assert!(prepared.source.is_none());
    let context = core.clone();
    let callback = SourceServer::start_context(move || {
        context
            .export_history()
            .map(|_| ())
            .map_err(|error| BundleError::Corrupt(error.to_string()))
    })
    .unwrap();
    let mut configuration = config(root.root(), prepared.named_path);
    configuration.origin = HostOrigin::Gui {
        retention: HostSourceBinding {
            source: None,
            inputs: None,
            callback: callback.bootstrap(),
        },
        initial_snapshot: HostSnapshot::from_history(prepared.history.clone()).unwrap(),
        expected_root_sha256: prepared.expected_root_sha256,
        expected_project_id: prepared.project_id,
    };
    let mut host = ProcessHost::start(configuration);
    let attached = successful(&host.client, HostCommand::ReadHistory);
    assert_eq!(
        attached.history.unwrap(),
        prepared.history,
        "named handover must seed the unsaved GUI snapshot instead of the older checkpoint"
    );
    assert_eq!(attached.status.saved_digest, Some(saved.history_sha256));
    assert!(gui.dirty().unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    host.orderly();
    assert_eq!(core.export_history().unwrap(), prepared.history);
    drop(callback);
    assert!(gui.close_originating_source().unwrap());
}

#[cfg(target_os = "linux")]
#[allow(
    clippy::too_many_lines,
    reason = "the positive publication and subsequent callback loss exercise every host publication route"
)]
fn lost_callback_refuses_all_host_publication_routes() {
    use kinewright_agent::writer_host::{HostSourceBinding, SourceServer};
    use kinewright_project::{
        bundle_records::BranchId, editing_session::Ja1EditingSession, storage_budget::StorageLimits,
    };
    let root = temp("c8-retention-proof-loss");
    let core = Core::spawn(Document::default()).unwrap();
    let mut origin = Ja1EditingSession::new(root.root(), &core, StorageLimits::default()).unwrap();
    let prepared = origin.prepare_host_attachment().unwrap();
    let descriptor = prepared.source.unwrap();
    let service = prepared.source_service.unwrap();
    service
        .ensure_snapshot(&descriptor, &prepared.history)
        .unwrap();
    let callback = SourceServer::start(service).unwrap();
    let bootstrap = callback.bootstrap();
    let mut configuration = config(root.root(), None);
    configuration.origin = HostOrigin::Gui {
        initial_snapshot: HostSnapshot::from_history(prepared.history.clone()).unwrap(),
        retention: HostSourceBinding {
            source: Some(descriptor.clone()),
            inputs: Some(prepared.inputs.clone()),
            callback: bootstrap.clone(),
        },
        expected_root_sha256: None,
        expected_project_id: None,
    };
    let mut host = HostCommandOwner::open(configuration).unwrap();
    let snapshot = HostSnapshot {
        history_sha256: kinewright_media::sha256_bytes(
            &serde_json::to_vec(&prepared.history).unwrap(),
        ),
        history: prepared.history,
    };
    let saved = host.execute(
        HostCommand::FirstSave {
            destination: root.path("actual.kinewright"),
            snapshot: snapshot.clone(),
            source: descriptor.clone(),
            inputs: prepared.inputs.clone(),
            callback: bootstrap.clone(),
        },
        roots(root.root()),
    );
    assert!(
        saved.error.is_none() && saved.saved.is_some(),
        "actual source/host positive publication must run first"
    );
    let identity = charged_identity(origin.source().unwrap().store());
    let before = admission::tree(root.root());
    drop(callback); // Lose only the transport. The actual GUI source and its charges survive.
    origin.host_lost().unwrap();
    let branch = BranchId::fresh().unwrap();
    let commands = [
        HostCommand::Save {
            snapshot: snapshot.clone(),
        },
        HostCommand::SaveAs {
            destination: root.path("fresh-save-as.kinewright"),
            snapshot: snapshot.clone(),
        },
        HostCommand::FirstSave {
            destination: root.path("fresh-first.kinewright"),
            snapshot: snapshot.clone(),
            source: descriptor,
            inputs: prepared.inputs,
            callback: bootstrap,
        },
        HostCommand::PreviewRescue { snapshot },
        HostCommand::RescueTo {
            destination: root.path("fresh-rescue.kinewright"),
        },
        HostCommand::AdmitDestination {
            destination: root.path("fresh-grant.kinewright"),
        },
        HostCommand::Retry,
        HostCommand::OpenRecovery,
        HostCommand::RemoveBranch,
        HostCommand::Forget,
        HostCommand::SaveSession {
            branch: branch.clone(),
        },
        HostCommand::CloseSession { branch },
    ];
    for command in commands {
        let reply = host.execute(command, roots(root.root()));
        assert_eq!(
            reply.error.as_ref().map(|e| e.code.as_str()),
            Some("retention_lost"),
            "all publication routes must refuse at ownership admission before their ordinary preconditions"
        );
        assert!(!reply.status.retention_proven);
        assert_eq!(admission::tree(root.root()), before);
        assert_eq!(charged_identity(origin.source().unwrap().store()), identity);
        assert_eq!(
            core.export_history().unwrap(),
            origin.core().export_history().unwrap()
        );
    }
    let queried = host.execute(HostCommand::ReadHistory, roots(root.root()));
    assert!(
        queried.error.is_none() && queried.history.is_some(),
        "queries remain available after retention loss"
    );
}

#[path = "../../kinewright-project/tests/support/ja_s2_c6_admission.rs"]
mod admission;

fn bundle_error(error: HostError) -> BundleError {
    match error {
        HostError::Refused(error) => match error.code.as_str() {
            "durability_unsupported" => BundleError::DurabilityUnsupported,
            "unsupported_bundle" => BundleError::Unsupported(error.message),
            "damaged_bundle" => BundleError::Corrupt(error.message),
            _ => BundleError::Identity(error.to_string()),
        },
        HostError::Io(error) => BundleError::Io(error),
        error => BundleError::Corrupt(error.to_string()),
    }
}

#[test]
fn w14_headless_and_agent_admission_precedes_mutation() {
    admission::matrix(|path| {
        let mut host =
            HostCommandOwner::open(config(path.parent().unwrap(), Some(path.to_path_buf())))
                .map_err(bundle_error)?;
        let reply = host.execute(HostCommand::ReadHistory, roots(path.parent().unwrap()));
        let history = reply.history.ok_or(BundleError::HistoryBarrierInvariant)?;
        if reply.status.frozen {
            let current = host.execute(
                HostCommand::CurrentAttachment,
                roots(path.parent().unwrap()),
            );
            let current = current
                .switch_attachment
                .ok_or(BundleError::HistoryBarrierInvariant)?;
            current.verify()?;
            assert_eq!(current.history, history);
            let confirmed = host.execute(
                HostCommand::ConfirmSwitch {
                    client: "a".repeat(64),
                    installed: Box::new(current.identity()?),
                },
                roots(path.parent().unwrap()),
            );
            if let Some(error) = confirmed.error {
                return Err(bundle_error(error.into()));
            }
            assert!(confirmed.manual_lease.is_some());
        }
        let snapshot = HostSnapshot {
            history_sha256: kinewright_media::sha256_bytes(&serde_json::to_vec(&history).unwrap()),
            history,
        };
        let reply = host.execute(
            HostCommand::Save { snapshot },
            roots(path.parent().unwrap()),
        );
        if let Some(error) = reply.error {
            return Err(bundle_error(error.into()));
        }
        assert!(reply.saved.is_some(), "host save did not publish");
        Ok(())
    });
}

#[test]
fn g04_two_process_lock_save_alias_and_discovery() {
    #[cfg(target_os = "linux")]
    {
        use kinewright_project::{
            bundle::{BundleWriteAdmission, LoadedBundle, ProjectBundle},
            storage_budget::StorageLimits,
        };
        let root = temp("c8-real-writers");
        let path = root.path("a.kinewright");
        let core = Core::spawn(Document::default()).unwrap();
        let mut owner =
            ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
        let inputs = owner.empty_inputs().unwrap().reference.clone();
        owner.save(&core, &inputs).unwrap();
        drop(owner);
        let mut first = ProcessHost::start(config(root.root(), Some(path.clone())));
        let captured = successful(&first.client, HostCommand::ReadHistory)
            .history
            .unwrap();
        let snapshot = HostSnapshot {
            history_sha256: kinewright_media::sha256_bytes(&serde_json::to_vec(&captured).unwrap()),
            history: captured,
        };
        successful(
            &first.client,
            HostCommand::Save {
                snapshot: snapshot.clone(),
            },
        );
        let discovery = kinewright_project::discovery_path_for_project(Some(&path)).unwrap();
        if discovery.exists() {
            assert!(
                !String::from_utf8_lossy(&std::fs::read(&discovery).unwrap())
                    .contains(&credential(&first.client.bootstrap().auth)),
                "project discovery cannot disclose attachment credentials"
            );
            std::fs::remove_file(&discovery).unwrap();
        }
        assert!(!discovery.exists());
        let bytes = admission::tree(root.root());
        assert!(
            HostClient::launch(
                Path::new(env!("CARGO_BIN_EXE_kinewright")),
                config(root.root(), Some(path.clone()))
            )
            .is_err(),
            "second process stole a held writer"
        );
        assert_eq!(admission::tree(root.root()), bytes);
        let alias = root.path("alias.kinewright");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        let before = admission::tree(root.root());
        assert!(
            HostClient::launch(
                Path::new(env!("CARGO_BIN_EXE_kinewright")),
                config(root.root(), Some(alias))
            )
            .is_err()
        );
        assert_eq!(admission::tree(root.root()), before);
        first.orderly();
        let mut next = ProcessHost::start(config(root.root(), Some(path.clone())));
        successful(&next.client, HostCommand::Save { snapshot });
        next.orderly();
        let loaded = LoadedBundle::read_with_limits_probe(&path, StorageLimits::default()).unwrap();
        let reader = Core::spawn(Document::default()).unwrap();
        loaded.restore_manual(&reader).unwrap();
        assert_eq!(
            reader.export_history().unwrap(),
            core.export_history().unwrap()
        );
    }
    #[cfg(not(target_os = "linux"))]
    {
        let root = temp("c8-windows-refusal");
        let path = admission::fixture(root.root(), "ordinary", "supported");
        let before = admission::tree(root.root());
        let Err(error) = HostCommandOwner::open(config(root.root(), Some(path))) else {
            panic!("uncertified Studio writer admitted")
        };
        assert!(matches!(
            bundle_error(error),
            BundleError::DurabilityUnsupported
        ));
        assert_eq!(admission::tree(root.root()), before);
    }
}

#[cfg(target_os = "linux")]
#[test]
#[allow(clippy::too_many_lines)]
fn g05l_two_clients_one_editor_ordered_history() {
    use kinewright_core::{Command as Edit, Event, Operation, Track, TrackId, TrackKind};
    use kinewright_project::{
        bundle::LoadedBundle, editing_session::Ja1EditingSession, storage_budget::StorageLimits,
    };
    let root = temp("c9-manual");
    let document = Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    };
    let gui = Core::spawn(document).unwrap();
    let path = root.path("manual.kinewright");
    let mut source = Ja1EditingSession::new(root.root(), &gui, StorageLimits::default()).unwrap();
    source.first_save(&path, StorageLimits::default()).unwrap();
    let _transfer = source.prepare_host_attachment().unwrap();
    let cut = source.freeze_host_cut().unwrap();
    source.complete_host_cut(&cut, None, None).unwrap();
    let host = WriterHost::start(config(root.root(), Some(path.clone()))).unwrap();
    let first = host.client();
    let second = HostClient::connect(
        first.endpoint().clone(),
        first.bootstrap().auth,
        roots(root.root()),
    )
    .unwrap();
    let identity = "a".repeat(64);
    let grant = successful(
        &first,
        HostCommand::AcquireManual {
            client: identity.clone(),
        },
    );
    let lease = grant.manual_lease.unwrap();
    let input_context = grant.manual_inputs.unwrap();
    assert_eq!(
        second
            .request(HostCommand::AcquireManual {
                client: "b".repeat(64)
            })
            .unwrap()
            .error
            .unwrap()
            .code,
        "manual_busy"
    );
    let first = first.with_manual_editor(identity.clone());
    assert_eq!(
        second
            .request(HostCommand::Save {
                snapshot: HostSnapshot::capture(&gui).unwrap()
            })
            .unwrap()
            .error
            .unwrap()
            .code,
        "manual_editor"
    );
    let narrow = root.path("viewer-narrow");
    std::fs::create_dir(&narrow).unwrap();
    let refused = second
        .with_roots(roots(&narrow))
        .request(HostCommand::Save {
            snapshot: HostSnapshot::capture(&gui).unwrap(),
        })
        .unwrap();
    assert_eq!(refused.error.unwrap().code, "manual_editor");
    assert!(
        refused.status.association.is_none(),
        "a role refusal before root admission cannot disclose protected project metadata"
    );
    let permit = source
        .bind_manual_producer(lease.clone(), input_context)
        .unwrap();
    let saved_before = successful(&first, HostCommand::Status)
        .status
        .saved_digest
        .unwrap();
    let mut times = Vec::new();
    for locked in [false, true, false] {
        let started = Instant::now();
        assert!(matches!(
            permit
                .request(Edit::DoBatchCoalesced {
                    operations: vec![Operation::SetTrackSyncLock {
                        track: TrackId(1),
                        locked
                    }],
                    coalesce_key: "drag#original".into()
                })
                .unwrap(),
            Event::DocumentChanged { .. }
        ));
        times.push(started);
    }
    let accepted = permit.pending().unwrap();
    let expected = gui.export_history().unwrap();
    assert_eq!(expected.cursor.undo.len(), 1);
    assert_eq!(expected.batches.len(), 3);
    assert_ne!(
        saved_before,
        kinewright_project::manual_history::history_sha256(&expected).unwrap()
    );
    assert_eq!(
        successful(&first, HostCommand::Status)
            .status
            .saved_digest
            .as_deref(),
        Some(saved_before.as_str()),
        "acceptance must stay pending until publication"
    );
    assert!(
        second
            .request(HostCommand::CommitHistory {
                client: "b".repeat(64),
                messages: accepted.clone()
            })
            .unwrap()
            .error
            .is_some()
    );
    assert!(matches!(
        gui.request(Edit::Undo).unwrap(),
        Event::MutationRefused { .. }
    ));
    for field in ["digest", "sequence", "batch_id"] {
        let mut mismatch = accepted[0].clone();
        match field {
            "digest" => mismatch.digest.push('f'),
            "sequence" => mismatch.sequence += 1,
            _ => mismatch.batch_id.push('f'),
        }
        assert!(
            first
                .request(HostCommand::CommitHistory {
                    client: identity.clone(),
                    messages: vec![mismatch]
                })
                .unwrap()
                .error
                .is_some(),
            "the actor-stamped {field} must match its original envelope"
        );
    }
    let mut tampered = accepted[0].clone();
    let mut envelope = kinewright_project::manual_history::AcceptedManualCommand::decode(&tampered)
        .unwrap()
        .envelope;
    envelope.manual.as_mut().unwrap().predecessor =
        Some(kinewright_project::bundle_records::BatchId::fresh().unwrap());
    tampered.envelope = serde_json::to_vec(&envelope).unwrap();
    tampered.digest = serde_json::to_value(envelope.digest().unwrap())
        .unwrap()
        .as_str()
        .unwrap()
        .into();
    assert!(
        first
            .request(HostCommand::CommitHistory {
                client: identity.clone(),
                messages: vec![tampered]
            })
            .unwrap()
            .error
            .is_some(),
        "the exact predecessor is mandatory even with matching history bytes"
    );
    for pointer in [
        "/manual/after_sha256",
        "/manual/cursor_sha256",
        "/manual/batch/undo_entries",
    ] {
        let mut tampered = accepted[0].clone();
        let mut value: Value = serde_json::from_slice(&tampered.envelope).unwrap();
        *value.pointer_mut(pointer).unwrap() = if pointer.ends_with("undo_entries") {
            json!(900)
        } else {
            json!("0".repeat(64))
        };
        let envelope: kinewright_project::receipts::CommandEnvelope =
            serde_json::from_value(value).unwrap();
        tampered.envelope = serde_json::to_vec(&envelope).unwrap();
        tampered.digest = serde_json::to_value(envelope.digest().unwrap())
            .unwrap()
            .as_str()
            .unwrap()
            .into();
        let before = std::fs::read(&path).unwrap();
        assert!(
            first
                .request(HostCommand::CommitHistory {
                    client: identity.clone(),
                    messages: vec![tampered]
                })
                .unwrap()
                .error
                .is_some(),
            "tampered accepted result was published: {pointer}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    let bypass = first
        .request(HostCommand::Save {
            snapshot: HostSnapshot::capture(&gui).unwrap(),
        })
        .unwrap();
    assert_eq!(
        bypass.error.unwrap().code,
        "manual_stream_required",
        "a foreground snapshot cannot replace accepted manual messages"
    );
    let mut latencies = Vec::new();
    for (message, started) in accepted.iter().zip(times) {
        let reply = successful(
            &first,
            HostCommand::CommitHistory {
                client: identity.clone(),
                messages: vec![message.clone()],
            },
        );
        latencies.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(reply.manual_ack, vec![message.clone()]);
        let published = LoadedBundle::read(&path).unwrap();
        let restored = Core::spawn(Document::default()).unwrap();
        published.restore_manual(&restored).unwrap();
        let durable = restored.export_history().unwrap();
        let envelope = kinewright_project::manual_history::AcceptedManualCommand::decode(message)
            .unwrap()
            .envelope;
        let metadata = envelope.manual.as_ref().unwrap();
        assert_eq!(
            kinewright_project::manual_history::history_sha256(&durable).unwrap(),
            metadata.after_sha256
        );
        assert_eq!(durable.batches.last(), Some(&metadata.batch));
        assert_eq!(
            published
                .manual_checkpoint()
                .unwrap()
                .batch_ids
                .last()
                .unwrap()
                .as_str(),
            message.batch_id
        );
        assert_eq!(reply.saved.unwrap().history_sha256, metadata.after_sha256);
    }
    source
        .acknowledge_manual(&permit, accepted.clone())
        .unwrap();
    let durable = manual_history_from(&path);
    assert_eq!(
        durable, expected,
        "no intermediate drag message may disappear"
    );
    let before_retry = std::fs::read(&path).unwrap();
    let retry = successful(
        &first,
        HostCommand::CommitHistory {
            client: identity.clone(),
            messages: accepted.clone(),
        },
    );
    assert_eq!(retry.manual_ack, accepted);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before_retry,
        "original-envelope retry must not republish or execute again"
    );
    for command in [Edit::Undo, Edit::Redo] {
        assert!(matches!(
            permit.request(command).unwrap(),
            Event::DocumentChanged { .. }
        ));
        let messages = permit.pending().unwrap();
        let reply = successful(
            &first,
            HostCommand::CommitHistory {
                client: identity.clone(),
                messages: messages.clone(),
            },
        );
        assert_eq!(reply.manual_ack, messages);
        source.acknowledge_manual(&permit, messages).unwrap();
        assert_eq!(manual_history_from(&path), gui.export_history().unwrap());
    }
    // A contiguous transport prefix can include many original drag messages.
    // Every one must remain in history and retain its actor-assigned BatchId.
    for locked in [true, false, true, false, true, false] {
        permit
            .request(Edit::DoBatchCoalesced {
                operations: vec![Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked,
                }],
                coalesce_key: "drag#second".into(),
            })
            .unwrap();
    }
    let messages = permit.pending().unwrap();
    let reply = successful(
        &first,
        HostCommand::CommitHistory {
            client: identity.clone(),
            messages: messages.clone(),
        },
    );
    assert_eq!(reply.manual_ack, messages);
    source
        .acknowledge_manual(&permit, messages.clone())
        .unwrap();
    let loaded = LoadedBundle::read(&path).unwrap();
    assert_eq!(manual_history_from(&path), gui.export_history().unwrap());
    assert!(
        loaded
            .manual_checkpoint()
            .unwrap()
            .batch_ids
            .iter()
            .rev()
            .zip(messages.iter().rev())
            .all(|(id, message)| id.as_str() == message.batch_id)
    );
    // Report-only uninterrupted drag timing: accept, then exact durable receipt.
    latencies.clear();
    for frame in 0..32 {
        let started = Instant::now();
        assert!(matches!(
            permit
                .request(Edit::DoBatchCoalesced {
                    operations: vec![Operation::SetTrackSyncLock {
                        track: TrackId(1),
                        locked: frame % 2 == 0
                    }],
                    coalesce_key: "drag#timed".into()
                })
                .unwrap(),
            Event::DocumentChanged { .. }
        ));
        let messages = permit.pending().unwrap();
        let reply = successful(
            &first,
            HostCommand::CommitHistory {
                client: identity.clone(),
                messages: messages.clone(),
            },
        );
        latencies.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(reply.manual_ack, messages);
        source.acknowledge_manual(&permit, messages).unwrap();
    }
    if let Some(output) = std::env::var_os("JA_MANUAL_LATENCY_OUTPUT") {
        std::fs::write(output, serde_json::to_vec_pretty(&json!({"accept_to_ack_ms": latencies, "samples":32, "model":"uninterrupted 32-message coalesced drag, sequential accept then durable loopback acknowledgement, test/debug profile"})).unwrap()).unwrap();
    }
    host.shutdown().unwrap();
}

#[cfg(target_os = "linux")]
fn manual_history_from(path: &Path) -> kinewright_core::HistorySnapshot {
    let core = Core::spawn(Document::default()).unwrap();
    kinewright_project::bundle::LoadedBundle::read(path)
        .unwrap()
        .restore_manual(&core)
        .unwrap();
    core.export_history().unwrap()
}

#[cfg(target_os = "linux")]
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "One barrier witness retains the before, pending and exact publisher result together"
)]
fn g05l_disk_barrier_keeps_acceptance_local_and_saved_exact() {
    use kinewright_core::{Command as Edit, Event, Operation, Track, TrackId, TrackKind};
    use kinewright_project::{
        editing_session::Ja1EditingSession, io_observer, storage_budget::StorageLimits,
    };
    let root = temp("c9-disk-barrier");
    let gui = Core::spawn(Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    })
    .unwrap();
    let path = root.path("barrier.kinewright");
    let mut source = Ja1EditingSession::new(root.root(), &gui, StorageLimits::default()).unwrap();
    source.first_save(&path, StorageLimits::default()).unwrap();
    source.prepare_host_attachment().unwrap();
    let cut = source.freeze_host_cut().unwrap();
    source.complete_host_cut(&cut, None, None).unwrap();
    let mut host = HostCommandOwner::open(config(root.root(), Some(path.clone()))).unwrap();
    let identity = "c".repeat(64);
    let reply = host.execute(
        HostCommand::AcquireManual {
            client: identity.clone(),
        },
        roots(root.root()),
    );
    assert!(reply.error.is_none(), "{:?}", reply.error);
    let saved = reply.status.saved_digest.unwrap();
    let permit = source
        .bind_manual_producer(reply.manual_lease.unwrap(), reply.manual_inputs.unwrap())
        .unwrap();
    let command = |locked| Edit::DoBatchCoalesced {
        operations: vec![Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked,
        }],
        coalesce_key: "disk-stalled-drag".into(),
    };
    assert!(matches!(
        permit.request(command(false)).unwrap(),
        Event::DocumentChanged { .. }
    ));
    let messages = permit.pending().unwrap();
    let original = std::fs::read(&path).unwrap();
    let signals = std::sync::Arc::new(std::sync::Barrier::new(2));
    let release = signals.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let requested_roots = roots(root.root());
    let worker = std::thread::spawn(move || {
        let reply = io_observer::at_barrier(
            "root_file_sync",
            move || {
                ready_tx.send(()).unwrap();
                release.wait();
            },
            || {
                host.execute(
                    HostCommand::CommitHistory {
                        client: identity,
                        messages,
                    },
                    requested_roots,
                )
            },
        );
        (host, reply)
    });
    ready_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        original,
        "B5 has not happened"
    );
    let events = gui.subscribe().unwrap();
    let _ = events.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(
        permit.request(command(true)).unwrap(),
        Event::DocumentChanged { .. }
    ));
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(1)).unwrap(),
        Event::DocumentChanged {
            journal_command: Some(_),
            ..
        }
    ));
    assert_eq!(
        permit.pending().unwrap().len(),
        2,
        "preview/local acceptance never waits for disk"
    );
    signals.wait();
    let (mut host, reply) = worker.join().unwrap();
    assert!(reply.error.is_none(), "{:?}", reply.error);
    assert_eq!(reply.manual_ack.len(), 1);
    assert_ne!(reply.status.saved_digest.as_deref(), Some(saved.as_str()));
    assert_ne!(
        reply.status.saved_digest.as_deref(),
        Some(
            kinewright_project::manual_history::history_sha256(&gui.export_history().unwrap())
                .unwrap()
                .as_str()
        ),
        "the later accepted suffix cannot be labelled Saved"
    );
    assert_eq!(manual_history_from(&path).batches.len(), 1);
    source
        .acknowledge_manual(&permit, reply.manual_ack)
        .unwrap();
    let messages = permit.pending().unwrap();
    let reply = host.execute(
        HostCommand::CommitHistory {
            client: "c".repeat(64),
            messages,
        },
        roots(root.root()),
    );
    assert!(reply.error.is_none(), "{:?}", reply.error);
    source
        .acknowledge_manual(&permit, reply.manual_ack)
        .unwrap();
    assert_eq!(manual_history_from(&path), gui.export_history().unwrap());
}

#[cfg(target_os = "linux")]
#[test]
fn g05l_ack_prefix_is_the_independently_reopened_history() {
    use kinewright_core::{Command as Edit, Operation, Track, TrackId, TrackKind};
    use kinewright_project::{editing_session::Ja1EditingSession, storage_budget::StorageLimits};
    let root = temp("c9-ack");
    let gui = Core::spawn(Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    })
    .unwrap();
    let path = root.path("ack.kinewright");
    let mut source = Ja1EditingSession::new(root.root(), &gui, StorageLimits::default()).unwrap();
    source.first_save(&path, StorageLimits::default()).unwrap();
    source.prepare_host_attachment().unwrap();
    let cut = source.freeze_host_cut().unwrap();
    source.complete_host_cut(&cut, None, None).unwrap();
    let host = WriterHost::start(config(root.root(), Some(path.clone()))).unwrap();
    let client = host.client();
    let id = "d".repeat(64);
    let grant = successful(&client, HostCommand::AcquireManual { client: id.clone() });
    let permit = source
        .bind_manual_producer(grant.manual_lease.unwrap(), grant.manual_inputs.unwrap())
        .unwrap();
    let baseline = manual_history_from(&path);
    for locked in [false, true, false] {
        permit
            .request(Edit::DoBatchCoalesced {
                operations: vec![Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked,
                }],
                coalesce_key: "ack-exact".into(),
            })
            .unwrap();
    }
    let messages = permit.pending().unwrap();
    assert_eq!(manual_history_from(&path), baseline);
    let reply = successful(
        &client,
        HostCommand::CommitHistory {
            client: id,
            messages: messages.clone(),
        },
    );
    assert_eq!(reply.manual_ack, messages);
    assert_eq!(
        manual_history_from(&path),
        gui.export_history().unwrap(),
        "acknowledged prefix must already be independently durable, including intermediate messages"
    );
    let loaded = kinewright_project::bundle::LoadedBundle::read(&path).unwrap();
    assert_eq!(loaded.manual_checkpoint().unwrap().batch_ids.len(), 3);
    assert!(
        loaded
            .manual_checkpoint()
            .unwrap()
            .batch_ids
            .iter()
            .zip(&messages)
            .all(|(batch, message)| batch.as_str() == message.batch_id)
    );
    source
        .acknowledge_manual(&permit, reply.manual_ack)
        .unwrap();
    assert!(permit.pending().unwrap().is_empty());
    host.shutdown().unwrap();
}

#[cfg(target_os = "linux")]
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "Input admission and refusal share the same actor, manifest and publisher"
)]
fn g05l_input_context_is_admitted_before_local_acceptance() {
    use kinewright_core::{AssetId, Command as Edit, CoreMutationRefusal, Event, Operation};
    use kinewright_project::{
        editing_session::Ja1EditingSession, owned_inputs::OwnedInputManifest,
        storage_budget::StorageLimits,
    };
    let root = temp("c9-input-context");
    let gui = Core::spawn(Document::default()).unwrap();
    let mut source = Ja1EditingSession::new(root.root(), &gui, StorageLimits::default()).unwrap();
    let store = source.source_mut().unwrap();
    let (asset, entry, media, _expected_bytes) =
        import_known_wav(store.store_mut(), 700, &root.path("source.wav"), 440.0);
    let manifest = OwnedInputManifest {
        project_id: None,
        session_id: Some(store.session_id().clone()),
        owner_epoch: store.store().owner_epoch(),
        entries: vec![entry],
    };
    let inputs = store.admit_inputs(&manifest).unwrap();
    source.set_unsaved_inputs(inputs.reference.clone()).unwrap();
    let path = root.path("input-context.kinewright");
    source.first_save(&path, StorageLimits::default()).unwrap();
    drop(inputs);
    drop(media);
    source.prepare_host_attachment().unwrap();
    let cut = source.freeze_host_cut().unwrap();
    source.complete_host_cut(&cut, None, None).unwrap();
    let host = WriterHost::start(config(root.root(), Some(path.clone()))).unwrap();
    let client = host.client();
    let id = "e".repeat(64);
    let grant = successful(&client, HostCommand::AcquireManual { client: id.clone() });
    let lease = grant.manual_lease.unwrap();
    let input_context = grant.manual_inputs.unwrap();
    let mut changed_context = input_context.clone();
    changed_context.owner_epoch += 1;
    assert!(
        source
            .bind_manual_producer(lease.clone(), changed_context)
            .is_err(),
        "input context must match the exact admitted record bytes"
    );
    let permit = source.bind_manual_producer(lease, input_context).unwrap();
    let mut unadmitted = asset.clone();
    unadmitted.id = AssetId(701);
    let before = gui.export_history().unwrap();
    assert!(before.document.media_pool.is_empty());
    let root_before = std::fs::read(&path).unwrap();
    assert!(
        matches!(
            permit
                .request(Edit::Do(Operation::AddAsset { asset: unadmitted }))
                .unwrap(),
            Event::MutationRefused {
                reason: CoreMutationRefusal::ManualInputRequired,
                ..
            }
        ),
        "unadmitted import must refuse before local acceptance"
    );
    assert_eq!(gui.export_history().unwrap(), before);
    assert!(permit.pending().unwrap().is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), root_before);
    assert!(
        matches!(
            permit
                .request(Edit::Do(Operation::AddAsset { asset }))
                .unwrap(),
            Event::DocumentChanged { .. }
        ),
        "the real owner-admitted input must remain editable"
    );
    let messages = permit.pending().unwrap();
    let reply = successful(
        &client,
        HostCommand::CommitHistory {
            client: id,
            messages: messages.clone(),
        },
    );
    assert_eq!(reply.manual_ack, messages);
    source
        .acknowledge_manual(&permit, reply.manual_ack)
        .unwrap();
    assert_eq!(manual_history_from(&path), gui.export_history().unwrap());
    host.shutdown().unwrap();
}

#[path = "support/ja_s2_c10.rs"]
mod c10;
#[path = "support/ja_s2_c11.rs"]
mod c11;

#[test]
fn g05a_fast_b5_switch_reply_loss_before_and_after_restore() {
    c11::fast();
}

#[path = "support/ja_s2_c12.rs"]
mod c12;

#[test]
fn rescue_and_switch_share_one_owner_after_restart() {
    c12::run();
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn g05a_switch_reply_drop_and_participant_death() {
    #[cfg(target_os = "linux")]
    c11::linux::matrix();
    #[cfg(not(target_os = "linux"))]
    c11::fast();
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "on-demand C11 GUI process participant: private bootstrap required"]
fn c11_switch_gui_participant() {
    c11::linux::gui_participant();
}

#[test]
fn g05l_fast_restart_absent_vs_committed_lost_reply_suffix() {
    c10::fast_suffix();
}
#[test]
fn g05l_fast_restart_shared_floor_and_close_reply_loss() {
    c10::fast_floor();
}
#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn g05l_host_death_recovers_absent_and_committed_drag_suffix() {
    #[cfg(target_os = "linux")]
    {
        for suffix in 0..3 {
            for committed in [false, true] {
                c10::restart_case(committed, false, true, suffix);
            }
        }
        c10::prefix_and_pause_cases(None);
        c10::empty_acknowledged_prefix();
    }
    #[cfg(not(target_os = "linux"))]
    c10::fast_suffix();
}
#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn g05l_shared_floor_and_recovery_close_reply_drop() {
    #[cfg(target_os = "linux")]
    for suffix in 0..3 {
        for committed in [false, true] {
            c10::restart_case(committed, true, true, suffix);
        }
    }
    #[cfg(not(target_os = "linux"))]
    c10::fast_floor();
}

#[test]
fn g05l_parent_replacement_does_not_prove_absence() {
    #[cfg(target_os = "linux")]
    c10::prefix_and_pause_cases(Some(3));
    #[cfg(not(target_os = "linux"))]
    c10::fast_suffix();
}

#[cfg(all(target_os = "linux", feature = "eval-harness"))]
#[test]
fn g05l_absent_suffix_pressure_publishes_one_shared_floor() {
    c10::pressure_floor();
}
