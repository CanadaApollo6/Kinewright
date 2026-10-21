//! Fixture-owned native processes and acknowledged observation barriers.
//! This supervisor supplies no production guardian, receipts or leases.
#![allow(clippy::missing_errors_doc)]
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    protocol::{self, Action, CONTROL_BYTES, Call, LoadedFixture},
    report::{Report, os_start_identity},
    revised::Adapter,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    protocol_version: u32,
    sequence: u64,
    action: Action,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    protocol_version: u32,
    sequence: u64,
    participant: String,
    action_id: u32,
    result: Result<Value, String>,
}

fn line(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>, String> {
    let mut bytes = Vec::new();
    loop {
        let buffer = reader.fill_buf().map_err(|e| e.to_string())?;
        if buffer.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err("incomplete JSONL record".into())
            };
        }
        let length = buffer
            .iter()
            .position(|b| *b == b'\n')
            .map_or(buffer.len(), |i| i + 1);
        if bytes.len() + length > CONTROL_BYTES {
            return Err("oversize JSONL record".into());
        }
        bytes.extend_from_slice(&buffer[..length]);
        reader.consume(length);
        if bytes.last() == Some(&b'\n') {
            return Ok(Some(bytes));
        }
    }
}

fn write_line(out: &mut impl Write, value: &impl Serialize) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    if bytes.len() > CONTROL_BYTES {
        return Err("oversize JSONL write".into());
    }
    out.write_all(&bytes)
        .and_then(|()| out.flush())
        .map_err(|e| e.to_string())
}

struct Worker {
    child: Child,
    name: String,
    sequence: u64,
    replies: mpsc::Receiver<Result<Reply, String>>,
    stdout: Option<thread::JoinHandle<()>>,
    stderr: Option<thread::JoinHandle<Result<Vec<u8>, String>>>,
    root: PathBuf,
}

impl Worker {
    fn spawn(
        exe: &Path,
        fixture: &Path,
        root: &Path,
        name: &str,
        mode: protocol::Mode,
    ) -> Result<Self, String> {
        let mut child = Command::new(exe)
            .args(["--role", "worker", "--fixture"])
            .arg(fixture)
            .arg("--root")
            .arg(root)
            .args([
                "--participant",
                name,
                "--mode",
                if mode == protocol::Mode::Revised {
                    "revised"
                } else {
                    "baseline"
                },
            ])
            .env_remove("REV2_HOOK")
            .env_remove("REV2_SIGNALS")
            .env_remove("REV2_EXIT")
            .env_remove("JA_HARNESS_DEADLINE_FILE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        let stdout = child.stdout.take().ok_or("missing worker stdout")?;
        let stderr = child.stderr.take().ok_or("missing worker stderr")?;
        let (sender, replies) = mpsc::sync_channel(64);
        let reader = thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let next = line(&mut reader)
                    .and_then(|record| record.ok_or_else(|| "worker EOF".into()))
                    .and_then(|bytes| protocol::unique_json(&bytes))
                    .and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string()));
                let stop = next.is_err();
                if sender.send(next).is_err() || stop {
                    break;
                }
            }
        });
        let stderr = thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr
                .take(u64::try_from(CONTROL_BYTES + 1).map_err(|e| e.to_string())?)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() > CONTROL_BYTES {
                return Err("worker stderr quota exceeded".into());
            }
            Ok(bytes)
        });
        Ok(Self {
            child,
            name: name.into(),
            sequence: 0,
            replies,
            stdout: Some(reader),
            stderr: Some(stderr),
            root: root.to_path_buf(),
        })
    }

    fn receive(&self, action: u32, deadline_ms: u64) -> Result<Value, String> {
        let reply = self
            .replies
            .recv_timeout(Duration::from_millis(deadline_ms))
            .map_err(|e| format!("worker deadline/disconnect: {e}"))??;
        if reply.protocol_version != 1
            || reply.sequence != self.sequence
            || reply.action_id != action
            || reply.participant != self.name
        {
            return Err("worker protocol identity/order mismatch".into());
        }
        reply.result
    }

    fn request(&mut self, action: &Action, timeout: u64) -> Result<Value, String> {
        self.sequence += 1;
        write_line(
            self.child.stdin.as_mut().ok_or("worker stdin closed")?,
            &Request {
                protocol_version: 1,
                sequence: self.sequence,
                action: action.clone(),
            },
        )?;
        let reference = self.receive(action.id, timeout)?;
        let relative = Path::new(
            reference["relative_file"]
                .as_str()
                .ok_or("missing worker artifact")?,
        );
        protocol::relative_path(relative)?;
        let length = reference["byte_length"]
            .as_u64()
            .ok_or("missing worker length")?;
        if length > protocol::INDEX_BYTES {
            return Err("worker artifact quota".into());
        }
        let bytes = protocol::read_bounded(&self.root.join(relative), length)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != length
            || kinewright_media::sha256_bytes(&bytes)
                != reference["sha256"].as_str().ok_or("missing worker hash")?
        {
            return Err("worker artifact identity mismatch".into());
        }
        protocol::unique_json(&bytes)
    }

    fn finish(&mut self, kill: bool, report: &mut Report) -> Result<Value, String> {
        let pid = self.child.id();
        let before = os_children(pid);
        drop(self.child.stdin.take());
        if kill {
            self.child.kill().map_err(|e| e.to_string())?;
        }
        let allowance = if kill {
            super::WORKER_REAP_MS
        } else {
            super::MEDIA_TEARDOWN_MS + super::WORKER_REAP_MS
        };
        let deadline = Instant::now() + Duration::from_millis(allowance);
        let status = loop {
            if let Some(status) = self.child.try_wait().map_err(|e| e.to_string())? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Err("fixture cleanup_blocked: worker missed reap deadline".into());
            }
            thread::sleep(Duration::from_millis(2));
        };
        if let Some(reader) = self.stdout.take() {
            reader.join().map_err(|_| "worker stdout reader panicked")?;
        }
        let stderr = self
            .stderr
            .take()
            .ok_or("worker already reaped")?
            .join()
            .map_err(|_| "worker stderr reader panicked")??;
        let stderr = report.artifact(&format!("stderr-{}-{pid}.txt", self.name), &stderr)?;
        let result = json!({"pid":pid,"reaped":true,"status":status.to_string(),"killed":kill,"os_children_before_reap":before,"stderr":stderr,
            "guardian_containment":"pending_baseline_witness"});
        report.event(0, &self.name, "process_exit", result.clone())?;
        if !kill && !status.success() {
            return Err(format!("fixture participant cleanup exited {status}"));
        }
        Ok(result)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        if self.child.try_wait().ok().flatten().is_none() {
            // EOF gives the fixture worker a chance to reap its own child.
            let deadline = Instant::now()
                + Duration::from_millis(super::MEDIA_TEARDOWN_MS + super::WORKER_REAP_MS);
            while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(2));
            }
            if self.child.try_wait().ok().flatten().is_none() {
                let _ = self.child.kill();
            }
            let _ = self.child.wait();
        }
    }
}

pub struct Controller {
    workers: BTreeMap<String, Worker>,
    dropped: BTreeSet<u32>,
    waiting: BTreeSet<String>,
}

fn freeze_fixture(loaded: &LoadedFixture, root: &Path) -> Result<PathBuf, String> {
    fs::create_dir(root).map_err(|e| e.to_string())?;
    for (prefix, refs) in [
        ("", &loaded.fixture.inputs),
        ("oracle:", &loaded.fixture.oracles),
    ] {
        for (role, reference) in refs {
            if reference.relative_file == Path::new("__ja_fixture.json") {
                return Err("reserved frozen fixture name".into());
            }
            let path = root.join(&reference.relative_file);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::write(
                path,
                super::baseline::input(loaded, &format!("{prefix}{role}"))?,
            )
            .map_err(|e| e.to_string())?;
        }
    }
    let path = root.join("__ja_fixture.json");
    fs::write(&path, &loaded.bytes).map_err(|e| e.to_string())?;
    path.canonicalize().map_err(|e| e.to_string())
}

/// Invoke only the built App libtest's exact fixture entry. The process exit,
/// inner common report, and stdout test summary are retained independently.
#[allow(clippy::too_many_lines)] // Fixed process launch, bounded capture and common-report verification.
pub fn app_fixture(
    action: &Action,
    loaded: &LoadedFixture,
    report: &mut Report,
) -> Result<(), String> {
    let binary = std::env::var_os("JA_HARNESS_APP_TEST_BINARY")
        .ok_or("App test binary prerequisite missing; NOT RUN")?;
    let binary = PathBuf::from(binary)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !binary
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.starts_with("kinewright_app-"))
    {
        return Err("expected built App test executable".into());
    }
    let fixture = freeze_fixture(
        loaded,
        &report.root.join(format!("app-input-{}", action.id)),
    )?;
    let out = report.root.join(format!("app-{}", action.id));
    let executable_sha256 = kinewright_media::sha256_file(&binary).map_err(|e| e.to_string())?;
    let child = Command::new(&binary)
        .args([
            "--exact",
            "ja_harness::tests::run_fixture_from_env",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("JA_HARNESS_FIXTURE", &fixture)
        .env("JA_HARNESS_OUTPUT", &out)
        .env_remove("JA_HARNESS_DEADLINE_FILE")
        .env(
            "JA_HARNESS_MODE",
            if report.mode == protocol::Mode::Baseline {
                "baseline"
            } else {
                "revised"
            },
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut child = OwnedChild(child);
    report.event(action.id,"app-test","process_spawn",json!({"pid":child.0.id(),"os_start_identity":os_start_identity(child.0.id()),"executable_sha256":executable_sha256}))?;
    let stdout = child.0.stdout.take().ok_or("App stdout missing")?;
    let stderr = child.0.stderr.take().ok_or("App stderr missing")?;
    let capture = |mut stream: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            stream
                .by_ref()
                .take(u64::try_from(CONTROL_BYTES + 1).map_err(|e| e.to_string())?)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() > CONTROL_BYTES {
                return Err("App output capture quota".into());
            }
            Ok::<_, String>(bytes)
        })
    };
    let stdout = capture(Box::new(stdout));
    let stderr = capture(Box::new(stderr));
    let deadline = Instant::now() + Duration::from_millis(loaded.fixture.limits.action_timeout_ms);
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.0.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            child.0.kill().map_err(|e| e.to_string())?;
            break child.0.wait().map_err(|e| e.to_string())?;
        }
        thread::sleep(Duration::from_millis(2));
    };
    let stdout = stdout.join().map_err(|_| "App stdout capture panic")??;
    let stderr = stderr.join().map_err(|_| "App stderr capture panic")??;
    report.artifact(&format!("app-{}-stdout.txt", action.id), &stdout)?;
    report.artifact(&format!("app-{}-stderr.txt", action.id), &stderr)?;
    report.event(action.id, "app-test", "process_exit", json!({"pid":child.0.id(),"reaped":true,"status":status.to_string(),"executable_sha256":executable_sha256,"timed_out":timed_out}))?;
    if out.exists() {
        report.admit_adapter_tree(&out)?;
    }
    if timed_out || status.code().is_none() {
        return Err(format!(
            "App test adapter crashed or timed out: {status}; NOT RUN"
        ));
    }
    let mut summary = protocol::unique_json(&protocol::read_bounded(
        &out.join("summary.json"),
        protocol::INDEX_BYTES,
    )?)?;
    let run = protocol::unique_json(&protocol::read_bounded(
        &out.join("run.json"),
        protocol::INDEX_BYTES,
    )?)?;
    let assertions = summary["assertions"]
        .as_array()
        .ok_or("App assertions absent")?;
    let failed = assertions.iter().any(|a| a["passed"] == false);
    let control_failed = assertions
        .iter()
        .any(|a| a["evidence_class"] == "control" && a["passed"] == false);
    let exit = summary["exit_code"].as_i64().ok_or("App exit absent")?;
    if !app_exit_matches_report(status, timed_out, Some(exit)) {
        return Err(format!(
            "App test adapter abnormal exit/report combination: {status}, inner exit {exit}; NOT RUN"
        ));
    }
    let expected_exit = if summary["disposition"] == "not_run" {
        2
    } else if failed {
        1
    } else if assertions.is_empty() {
        3
    } else {
        0
    };
    if summary["protocol"] != protocol::PROTOCOL
        || summary["protocol_version"] != 1
        || summary["fixture_id"] != loaded.fixture.fixture_id
        || summary["mode"] != serde_json::to_value(report.mode).map_err(|e| e.to_string())?
        || summary["run_id"] != run["run_id"]
        || !run["run_id"].is_string()
        || run["fixture_sha256"] != kinewright_media::sha256_bytes(&loaded.bytes)
        || run["binary_sha256"] != executable_sha256
        || exit != expected_exit
        || summary["incomplete_artifact_index"] == true
        || (control_failed && exit != 2 && summary["disposition"] != "control_failed")
        || (exit == 2 && !summary["missing_requirements"].is_string())
    {
        return Err("App common report identity/classification incomplete; NOT RUN".into());
    }
    // Validate every evidence reference against the retained inner files, then
    // rebase it into the native report. libtest's 101 does not erase exit 1.
    rebase_app_evidence(&mut summary, &out, Path::new(&format!("app-{}", action.id)))?;
    for (key, destination) in [
        ("assertions", &mut report.assertions),
        ("observations", &mut report.observations),
        ("pending", &mut report.pending),
    ] {
        destination.extend(
            summary[key]
                .as_array()
                .ok_or("App common report array absent")?
                .iter()
                .cloned(),
        );
    }
    report.event(action.id, "app-test", "common_report_imported",
        json!({"exit_code":exit,"libtest_status":status.to_string(),"summary":format!("app-{}/summary.json", action.id)}))?;
    if exit == 2 {
        return Err(summary["missing_requirements"]
            .as_str()
            .unwrap_or("App setup incomplete")
            .into());
    }
    Ok(())
}

fn app_exit_matches_report(status: ExitStatus, timed_out: bool, exit: Option<i64>) -> bool {
    !timed_out
        && matches!(
            (status.code(), exit),
            (Some(0), Some(0)) | (Some(101), Some(1..=3))
        )
}

fn rebase_app_evidence(value: &mut Value, root: &Path, prefix: &Path) -> Result<(), String> {
    match value {
        Value::Object(fields) => {
            if let Some(relative) = fields.get("relative_file").and_then(Value::as_str) {
                let path = Path::new(relative);
                protocol::relative_path(path)?;
                let bytes = protocol::read_bounded(&root.join(path), protocol::INDEX_BYTES)?;
                if fields.get("byte_length").and_then(Value::as_u64) != Some(bytes.len() as u64)
                    || fields.get("sha256").and_then(Value::as_str)
                        != Some(kinewright_media::sha256_bytes(&bytes).as_str())
                {
                    return Err("App evidence identity mismatch; NOT RUN".into());
                }
                fields.insert("relative_file".into(), json!(prefix.join(path)));
            }
            for child in fields.values_mut() {
                rebase_app_evidence(child, root, prefix)?;
            }
        }
        Value::Array(values) => {
            for child in values {
                rebase_app_evidence(child, root, prefix)?;
            }
        }
        _ => {}
    }
    Ok(())
}

impl Controller {
    pub fn new(loaded: &LoadedFixture, report: &mut Report) -> Result<Self, String> {
        let mut controller = Self {
            workers: BTreeMap::new(),
            dropped: BTreeSet::new(),
            waiting: BTreeSet::new(),
        };
        if loaded.fixture.initial.participants.is_empty() {
            return Ok(controller);
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        if exe.file_stem().and_then(|s| s.to_str()) != Some("kinewright-ja-baseline") {
            return Err("native baseline worker binary prerequisite missing".into());
        }
        let fixture = freeze_fixture(loaded, &report.root.join("frozen"))?;
        let root = report
            .root
            .join("work")
            .canonicalize()
            .map_err(|e| e.to_string())?;
        for name in &loaded.fixture.initial.participants {
            if !matches!(name.as_str(), "A" | "B") || controller.workers.contains_key(name) {
                return Err("participants must be distinct A/B".into());
            }
            let worker = Worker::spawn(
                &exe,
                &fixture.canonicalize().map_err(|e| e.to_string())?,
                &root,
                name,
                report.mode,
            )?;
            let ready = worker.receive(0, loaded.fixture.limits.setup_timeout_ms)?;
            if ready["pid"] != worker.child.id()
                || ready["os_start_identity"] != json!(os_start_identity(worker.child.id()))
            {
                return Err("worker birth identity differs from owned process".into());
            }
            report.event(0, name, "process_spawn", ready)?;
            controller.workers.insert(name.clone(), worker);
        }
        controller.sample(report)?;
        Ok(controller)
    }

    pub fn sample(&self, report: &mut Report) -> Result<(), String> {
        let started = Instant::now();
        let mut pids = vec![std::process::id()];
        pids.extend(self.workers.values().map(|w| w.child.id()));
        let processes = pids.into_iter().map(|pid| json!({"pid":pid,"os_start_identity":os_start_identity(pid),"rss_bytes":super::report::rss_bytes(pid),"vram_bytes":null})).collect::<Vec<_>>();
        let aggregate = processes
            .iter()
            .try_fold(0_u64, |sum, p| sum.checked_add(p["rss_bytes"].as_u64()?));
        report.event(0,"controller","process_resource_sample",json!({"scope":"controller_and_direct_participants; not a descendant or whole-job allowance","processes":processes,"sampled_concurrent_rss_bytes":aggregate,"capture_span_ns":started.elapsed().as_nanos().to_string(),"peak_claim":false}))
    }

    #[allow(clippy::too_many_lines)]
    pub fn dispatch(
        &mut self,
        action: &Action,
        loaded: &LoadedFixture,
        observations: &BTreeMap<u32, Value>,
        report: &mut Report,
    ) -> Result<Option<(Value, bool)>, String> {
        if let Some(original) = action.retry_of {
            let original = loaded
                .fixture
                .actions
                .iter()
                .find(|a| a.id == original && a.id < action.id)
                .ok_or("invalid retry origin")?;
            if original.participant != action.participant
                || serde_json::to_value(&original.call).map_err(|e| e.to_string())?
                    != serde_json::to_value(&action.call).map_err(|e| e.to_string())?
            {
                return Err("retry changed original action bytes/participant".into());
            }
            report.event(
                action.id,
                &action.participant,
                "retry_original_action",
                json!({"original_action_id":original.id}),
            )?;
        }
        match &action.call {
            Call::DropReply(args) => {
                if args.action_id <= action.id
                    || !loaded.fixture.actions.iter().any(|a| {
                        a.id == args.action_id
                            && matches!(
                                a.call,
                                Call::CoreSend(_)
                                    | Call::HeadlessSave(_)
                                    | Call::ProjectSave(_)
                                    | Call::CommentSubmit(_)
                            )
                    })
                {
                    return Err("invalid reply-drop target".into());
                }
                self.dropped.insert(args.action_id);
                Ok(Some((json!({"armed_for":args.action_id}), true)))
            }
            Call::BarrierWait(args) => {
                let barrier = loaded
                    .fixture
                    .barriers
                    .iter()
                    .find(|b| b.name == args.name)
                    .ok_or("unknown barrier")?;
                super::require_control(&barrier.positive_control, report)?;
                if barrier.after >= action.id || !observations.contains_key(&barrier.after) {
                    return Err("barrier lacks actual acknowledged observation".into());
                }
                self.waiting.insert(args.name.clone());
                Ok(Some((
                    json!({"barrier":args.name,"acknowledged_action":barrier.after,"waiting":true}),
                    true,
                )))
            }
            Call::BarrierRelease(args) => {
                if !self.waiting.remove(&args.name) {
                    return Err("barrier was not acknowledged".into());
                }
                Ok(Some((json!({"barrier":args.name,"released":true}), true)))
            }
            Call::ParticipantRestart(_) => {
                if self.workers.contains_key(&action.participant)
                    || !loaded
                        .fixture
                        .initial
                        .participants
                        .contains(&action.participant)
                {
                    return Err("restart requires a retired original fixed participant".into());
                }
                let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                let fixture = report
                    .root
                    .join("frozen/__ja_fixture.json")
                    .canonicalize()
                    .map_err(|e| e.to_string())?;
                let root = report
                    .root
                    .join("work")
                    .canonicalize()
                    .map_err(|e| e.to_string())?;
                let worker =
                    Worker::spawn(&exe, &fixture, &root, &action.participant, report.mode)?;
                let ready = worker.receive(0, loaded.fixture.limits.setup_timeout_ms)?;
                if ready["pid"] != worker.child.id()
                    || ready["os_start_identity"] != json!(os_start_identity(worker.child.id()))
                {
                    return Err("restarted worker birth identity differs".into());
                }
                report.event(
                    action.id,
                    &action.participant,
                    "process_spawn",
                    ready.clone(),
                )?;
                self.workers.insert(action.participant.clone(), worker);
                Ok(Some((ready, true)))
            }
            Call::ParticipantExit(_) | Call::ParticipantKill(_) => {
                let mut worker = self
                    .workers
                    .remove(&action.participant)
                    .ok_or("unknown/live participant")?;
                let kill = matches!(action.call, Call::ParticipantKill(_));
                let admission = (|| {
                    if kill {
                        // Never manufacture guardian behavior: forceful death with
                        // descendants remains pending. Only a verified empty worker
                        // may be killed by this fixture control.
                        let inventory = Action {
                            call: Call::ParticipantInventory(protocol::Empty {}),
                            ..action.clone()
                        };
                        let inventory =
                            worker.request(&inventory, loaded.fixture.limits.action_timeout_ms)?;
                        if inventory["owned_child"].is_object()
                            || os_children(worker.child.id()).is_some_and(|v| !v.is_empty())
                        {
                            return Err(
                                "forceful descendant cleanup needs production guardian; pending"
                                    .into(),
                            );
                        }
                    } else {
                        worker.request(action, loaded.fixture.limits.action_timeout_ms)?;
                    }
                    Ok(())
                })();
                if let Err(error) = admission {
                    let cleanup = worker.finish(false, report);
                    return Err(match cleanup {
                        Ok(_) => error,
                        Err(cleanup) => format!("{error}; participant cleanup: {cleanup}"),
                    });
                }
                Ok(Some((worker.finish(kill, report)?, true)))
            }
            _ if self.workers.contains_key(&action.participant) => {
                let result = self
                    .workers
                    .get_mut(&action.participant)
                    .ok_or("missing worker")?
                    .request(action, loaded.fixture.limits.action_timeout_ms)?;
                Ok(Some((result, !self.dropped.remove(&action.id))))
            }
            _ if action.participant != "main" && action.participant != "controller" => {
                Err("participant not live".into())
            }
            _ => Ok(None),
        }
    }

    pub fn deliver_reply(&mut self, action: u32) -> bool {
        !self.dropped.remove(&action)
    }

    pub fn cleanup(&mut self, report: &mut Report) -> Result<(), String> {
        let mut failures = Vec::new();
        for (_, mut worker) in std::mem::take(&mut self.workers) {
            if let Err(error) = worker.finish(false, report) {
                failures.push(error);
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }
}

fn os_children(pid: u32) -> Option<Vec<u32>> {
    #[cfg(target_os = "linux")]
    {
        let mut children = BTreeSet::new();
        for task in fs::read_dir(format!("/proc/{pid}/task")).ok()? {
            let task = task.ok()?;
            let text = fs::read_to_string(task.path().join("children")).ok()?;
            for child in text.split_whitespace() {
                children.insert(child.parse().ok()?);
            }
        }
        Some(children.into_iter().collect())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

struct OwnedChild(Child);
impl OwnedChild {
    fn retire(&mut self) -> Result<Value, String> {
        let pid = self.0.id();
        drop(self.0.stdin.take());
        let status = self.0.wait().map_err(|e| e.to_string())?;
        Ok(json!({"pid":pid,"reaped":true,"status":status.to_string()}))
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

/// Exact fixed worker/sentinel roles. No fixture shell snippets are executable.
#[allow(clippy::too_many_lines)] // One fixed wire loop and its exhaustive process-only controls.
pub fn worker_cli(args: &[String]) -> Result<i32, String> {
    if args == ["--role", "sentinel"] {
        let mut byte = [0_u8];
        while std::io::stdin()
            .read(&mut byte)
            .map_err(|e| e.to_string())?
            != 0
        {}
        return Ok(0);
    }
    if args.len() != 10
        || args[0] != "--role"
        || args[1] != "worker"
        || args[2] != "--fixture"
        || args[4] != "--root"
        || args[6] != "--participant"
        || args[8] != "--mode"
    {
        return Err("invalid fixed worker arguments".into());
    }
    let loaded = protocol::load(Path::new(&args[3]))?;
    let root = Path::new(&args[5]);
    let participant = &args[7];
    if !matches!(participant.as_str(), "A" | "B") {
        return Err("invalid worker identity".into());
    }
    let mode = match args[9].as_str() {
        "baseline" => protocol::Mode::Baseline,
        "revised" => protocol::Mode::Revised,
        _ => return Err("invalid worker mode".into()),
    };
    let mut baseline = Adapter::new(root, mode)?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut stdout = std::io::stdout().lock();
    write_line(
        &mut stdout,
        &Reply {
            protocol_version: 1,
            sequence: 0,
            action_id: 0,
            participant: participant.clone(),
            result: Ok(
                json!({"pid":std::process::id(),"os_start_identity":os_start_identity(std::process::id()),"executable_sha256":kinewright_media::sha256_file(&exe).map_err(|e| e.to_string())?,"os_children":os_children(std::process::id())}),
            ),
        },
    )?;
    let mut stdin = BufReader::new(std::io::stdin().lock());
    let mut sequence = 0;
    let mut previous = 0;
    let mut child: Option<OwnedChild> = None;
    let mut written = 0_u64;
    while let Some(bytes) = line(&mut stdin)? {
        let request: Request =
            serde_json::from_value(protocol::unique_json(&bytes)?).map_err(|e| e.to_string())?;
        sequence += 1;
        if request.protocol_version != 1
            || request.sequence != sequence
            || request.action.participant != *participant
            || request.action.id <= previous
        {
            return Err("worker request identity/order mismatch".into());
        }
        previous = request.action.id;
        let exit = matches!(request.action.call, Call::ParticipantExit(_));
        let result = match &request.action.call {
            Call::ParticipantInventory(_) => Ok(
                json!({"pid":std::process::id(),"owned_child":child.as_ref().map(|c| json!({"pid":c.0.id(),"os_start_identity":os_start_identity(c.0.id())})),"os_children":os_children(std::process::id())}),
            ),
            Call::SpawnChild(_) => {
                if child.is_some() {
                    return Err("one fixture child allowed".into());
                }
                let spawned = Command::new(&exe)
                    .args(["--role", "sentinel"])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|e| e.to_string())?;
                let identity = json!({"pid":spawned.id(),"os_start_identity":os_start_identity(spawned.id()),"executable_sha256":kinewright_media::sha256_file(&exe).map_err(|e| e.to_string())?});
                child = Some(OwnedChild(spawned));
                Ok(identity)
            }
            Call::ChildExit(_) => child.take().ok_or("no fixture child")?.retire(),
            Call::ParticipantExit(_) => {
                let retired = child.take().map(|mut c| c.retire()).transpose()?;
                Ok(
                    json!({"owned_child_retired":retired,"os_children":os_children(std::process::id())}),
                )
            }
            _ => baseline.execute(&request.action, &loaded),
        };
        let result = result.and_then(|value| {
            let bytes = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
            written = written.saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
            if written > loaded.fixture.limits.output_bytes { return Err("worker output quota".into()); }
            let relative = format!("worker-{participant}-{previous}.json");
            fs::write(root.join(&relative), &bytes).map_err(|e| e.to_string())?;
            Ok(json!({"relative_file":relative,"byte_length":bytes.len(),"sha256":kinewright_media::sha256_bytes(&bytes)}))
        });
        write_line(
            &mut stdout,
            &Reply {
                protocol_version: 1,
                sequence,
                participant: participant.clone(),
                action_id: previous,
                result,
            },
        )?;
        if exit {
            break;
        }
    }
    if let Some(mut child) = child.take() {
        child.retire()?;
    }
    baseline.finish()?;
    Ok(0)
}

#[cfg(all(test, unix))]
pub(super) fn witness_worker_teardown_window() {
    use kinewright_media::test_support::TempDirectory;
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDirectory::new("ja-worker-teardown");
    let executable = temp.path("worker");
    // Fixed test worker: acknowledge EOF, then perform teardown beyond the old
    // reap window. No GPU timing or unavailable-device fallback controls this.
    fs::write(&executable, b"#!/bin/sh\ncat >/dev/null\nexec sleep 3\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let mut report = Report::create(&temp.path("report"), protocol::Mode::Baseline).unwrap();
    let mut worker = Worker::spawn(
        &executable,
        Path::new("unused"),
        &report.root,
        "A",
        protocol::Mode::Baseline,
    )
    .unwrap();
    let started = Instant::now();
    let result = worker.finish(false, &mut report);
    assert!(
        result.is_ok(),
        "worker-owned teardown must precede the reap deadline: {result:?}"
    );
    let receipt = result.unwrap();
    assert!(started.elapsed() > Duration::from_millis(super::WORKER_REAP_MS));
    assert_eq!(receipt["reaped"], true);
    assert_eq!(receipt["killed"], false);
    let events = fs::read_to_string(report.root.join("events.jsonl")).unwrap();
    assert_eq!(events.lines().count(), 1);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wire_reader_refuses_oversize_partial_and_preserves_i64() {
        assert!(line(&mut BufReader::new(&b"partial"[..])).is_err());
        assert!(line(&mut BufReader::new(&vec![b'x'; CONTROL_BYTES + 1][..])).is_err());
        let raw = b"{\"frame\":-9223372036854775808}\n";
        assert_eq!(line(&mut BufReader::new(&raw[..])).unwrap().unwrap(), raw);
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn app_exit_classifier_rejects_abnormal_numeric_status_despite_completed_report() {
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt;
        #[cfg(windows)]
        use std::os::windows::process::ExitStatusExt;
        #[cfg(windows)]
        let status = ExitStatus::from_raw;
        #[cfg(unix)]
        let status = |code| ExitStatus::from_raw(code << 8);
        #[cfg(windows)]
        let abnormal = ExitStatus::from_raw(0xC000_0005);
        #[cfg(unix)]
        let abnormal = status(86);
        assert!(app_exit_matches_report(status(0), false, Some(0)));
        for exit in 1..=3 {
            assert!(app_exit_matches_report(status(101), false, Some(exit)));
        }
        for exit in 0..=3 {
            assert!(!app_exit_matches_report(abnormal, false, Some(exit)));
            assert!(!app_exit_matches_report(status(0), true, Some(exit)));
        }
        assert!(!app_exit_matches_report(status(0), false, Some(1)));
        assert!(!app_exit_matches_report(status(101), false, Some(0)));
        assert!(!app_exit_matches_report(status(101), false, None));
        assert!(!app_exit_matches_report(status(0), false, None));
        #[cfg(unix)]
        assert!(!app_exit_matches_report(
            ExitStatus::from_raw(9),
            false,
            Some(0)
        ));
    }
}
