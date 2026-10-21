//! Bounded native fixture execution. This supervises only our fixed eval roles;
//! production pre-exec, host death, escaped descendants and resume stay pending.
use super::{
    protocol::{self, Mode},
    report::{gate_allocation, os_start_identity},
};
use serde_json::{Value, json};
use std::{
    fs,
    io::Read,
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

const DEADLINE_ENV: &str = "JA_HARNESS_DEADLINE_FILE";
const CAPTURE: u64 = 64 * 1024;
const MAX_PHASE_MS: u64 = 600_000 + 3 * super::MEDIA_TEARDOWN_MS + 2 * super::WORKER_REAP_MS;

pub(super) fn heartbeat(action: u32, phase: &str, timeout_ms: u64) -> Result<(), String> {
    if let Some(path) = std::env::var_os(DEADLINE_ENV) {
        fs::write(
            path,
            serde_json::to_vec(&json!({"action_id":action,"phase":phase,"timeout_ms":timeout_ms}))
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub(super) fn validate_reserved(root: &Path) -> Result<(), String> {
    if std::env::var_os(DEADLINE_ENV).as_deref()
        != Some(root.join("supervisor-deadline.json").as_os_str())
    {
        return Err("fixture role requires its reserved supervisor directory".into());
    }
    Ok(())
}

struct OwnedFixture {
    child: Child,
    released: bool,
}

#[cfg(unix)]
fn termination_result(result: rustix::io::Result<()>) -> Result<(), String> {
    match result {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        Err(error) => Err(format!("fixture group termination: {error}")),
    }
}

impl OwnedFixture {
    fn exited(&mut self) -> Result<bool, String> {
        #[cfg(unix)]
        {
            use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
            let pid = Pid::from_raw(i32::try_from(self.child.id()).map_err(|e| e.to_string())?)
                .ok_or("invalid fixture PID")?;
            // Do not reap: the waitable leader reserves its PID/group identity
            // until the last owned descendant has been terminated and recorded.
            waitid(
                WaitId::Pid(pid),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            )
            .map(|status| status.is_some())
            .map_err(|e| e.to_string())
        }
        #[cfg(not(unix))]
        self.child
            .try_wait()
            .map(|status| status.is_some())
            .map_err(|e| e.to_string())
    }

    fn stop(&mut self) -> Result<(), String> {
        #[cfg(unix)]
        {
            use rustix::process::{Pid, Signal, kill_process_group};
            let pid = Pid::from_raw(i32::try_from(self.child.id()).map_err(|e| e.to_string())?)
                .ok_or("invalid fixture PID")?;
            termination_result(kill_process_group(pid, Signal::KILL))?;
        }
        #[cfg(windows)]
        if !self.exited()? {
            let status = Command::new("taskkill")
                .args(["/PID", &self.child.id().to_string(), "/T", "/F"])
                .status()
                .map_err(|e| e.to_string())?;
            if !status.success() && !self.exited()? {
                return Err("Windows fixture tree termination unavailable; NOT RUN".into());
            }
        }
        #[cfg(not(any(unix, windows)))]
        if !self.exited()? {
            self.child.kill().map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn finish(&mut self) -> (Option<ExitStatus>, Value, Option<String>) {
        let started = Instant::now();
        let deadline = started + Duration::from_millis(super::WORKER_REAP_MS);
        let mut error = self.stop().err();
        #[cfg(target_os = "linux")]
        let mut reaped = Vec::<u32>::new();
        #[cfg(not(target_os = "linux"))]
        let reaped = Vec::<u32>::new();
        let status = loop {
            #[cfg(target_os = "linux")]
            let descendants_done = match reap_group_descendants(self.child.id(), &mut reaped) {
                Ok(done) => done,
                Err(reason) => {
                    error.get_or_insert(reason);
                    false
                }
            };
            #[cfg(not(target_os = "linux"))]
            let descendants_done = true;
            if descendants_done && self.exited().unwrap_or(false) {
                match self.child.wait() {
                    Ok(status) => {
                        self.released = true;
                        break Some(status);
                    }
                    Err(reason) => {
                        error.get_or_insert(reason.to_string());
                        break None;
                    }
                }
            }
            if Instant::now() >= deadline {
                error.get_or_insert(
                    "fixture cleanup_blocked: owned group missed reap deadline".into(),
                );
                break None;
            }
            thread::sleep(Duration::from_millis(2));
        };
        let receipt = json!({"termination_requested":true,"leader_identity_held_through_cleanup":cfg!(unix),
            "descendants_reaped":reaped,"completed":self.released,"elapsed_ms":started.elapsed().as_millis(),
            "deadline_ms":super::WORKER_REAP_MS,"error":error});
        (status, receipt, error)
    }
}

// The native CLI is a Linux subreaper only for its fixed fixture roles. Reap
// adopted group members individually, leaving the leader waitable until last.
#[cfg(target_os = "linux")]
fn reap_group_descendants(group: u32, reaped: &mut Vec<u32>) -> Result<bool, String> {
    use rustix::process::{Pid, WaitOptions, waitpid};
    let mut remaining = false;
    for entry in fs::read_dir("/proc").map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == group {
            continue;
        }
        let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some(fields) = stat
            .rsplit_once(')')
            .map(|(_, tail)| tail.split_whitespace().collect::<Vec<_>>())
        else {
            continue;
        };
        if fields.get(2).and_then(|p| p.parse::<u32>().ok()) != Some(group) {
            continue;
        }
        let identity = Pid::from_raw(i32::try_from(pid).map_err(|e| e.to_string())?)
            .ok_or("invalid descendant PID")?;
        match waitpid(Some(identity), WaitOptions::NOHANG) {
            Ok(Some(_)) => reaped.push(pid),
            Ok(None) | Err(rustix::io::Errno::CHILD) => remaining = true,
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(!remaining)
}

impl Drop for OwnedFixture {
    fn drop(&mut self) {
        if !self.released {
            let _ = self.finish();
        }
    }
}

fn capture(
    mut stream: impl Read + Send + 'static,
) -> std::sync::mpsc::Receiver<Result<Vec<u8>, String>> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    thread::spawn(move || {
        let result = (|| {
            let mut bytes = Vec::new();
            stream
                .by_ref()
                .take(CAPTURE + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() as u64 > CAPTURE {
                return Err("fixture process output quota; NOT RUN".into());
            }
            Ok(bytes)
        })();
        let _ = sender.send(result);
    });
    receiver
}

#[allow(clippy::too_many_lines)]
pub(super) fn run(fixture: &Path, output: &Path, mode: Mode) -> Result<i32, String> {
    fs::create_dir(output).map_err(|e| format!("private output directory: {e}"))?;
    let root = output.canonicalize().map_err(|e| e.to_string())?;
    let control = root.join("supervisor-deadline.json");
    fs::write(&control, b"{}").map_err(|e| e.to_string())?;
    let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command
        .args([
            "--role",
            "fixture",
            "--mode",
            if mode == Mode::Baseline {
                "baseline"
            } else {
                "revised"
            },
            "--fixture",
        ])
        .arg(fixture)
        .arg("--output")
        .arg(&root)
        .env(DEADLINE_ENV, &control)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    #[cfg(target_os = "linux")]
    rustix::process::set_child_subreaper(Some(rustix::process::Pid::INIT))
        .map_err(|e| format!("fixed-role subreaper prerequisite: {e}"))?;
    let mut child = OwnedFixture {
        child: command.spawn().map_err(|e| e.to_string())?,
        released: false,
    };
    let pid = child.child.id();
    let identity = os_start_identity(pid);
    let stdout = capture(child.child.stdout.take().ok_or("fixture stdout")?);
    let stderr = capture(child.child.stderr.take().ok_or("fixture stderr")?);
    let birth = json!({"pid":pid,"os_start_identity":identity,"owned_process_group":cfg!(unix),"production_guardian_certified":false});
    fs::write(
        root.join("supervisor-process.json"),
        serde_json::to_vec_pretty(&birth).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let born = Instant::now();
    let mut phase = json!({"action_id":0,"phase":"setup","timeout_ms":30_000});
    let mut deadline = born + Duration::from_secs(30);
    let mut phase_started = born;
    let mut timed_out = false;
    let supervision_error = loop {
        match child.exited() {
            Ok(true) => break None,
            Ok(false) => {}
            Err(error) => break Some(error),
        }
        if let Ok(bytes) = protocol::read_bounded(&control, protocol::CONTROL_BYTES as u64)
            && let Ok(value) = protocol::unique_json(&bytes)
            && value != phase
            && let Some(ms) = value["timeout_ms"]
                .as_u64()
                .filter(|n| (1..=MAX_PHASE_MS).contains(n))
        {
            phase = value;
            phase_started = Instant::now();
            deadline = if phase["phase"] == "setup" {
                born
            } else {
                Instant::now()
            } + Duration::from_millis(ms);
        }
        if Instant::now() >= deadline {
            timed_out = true;
            break None;
        }
        thread::sleep(Duration::from_millis(2));
    };
    let (status, cleanup, cleanup_error) = child.finish();
    let mut capture_error = None;
    for (name, handle) in [
        ("supervisor-stdout.txt", stdout),
        ("supervisor-stderr.txt", stderr),
    ] {
        let bytes = match handle
            .recv_timeout(Duration::from_millis(super::WORKER_REAP_MS))
            .unwrap_or_else(|e| Err(format!("fixture capture incomplete: {e}")))
        {
            Ok(bytes) => bytes,
            Err(error) => {
                capture_error = Some(error);
                Vec::new()
            }
        };
        fs::write(root.join(name), bytes).map_err(|e| e.to_string())?;
    }
    let receipt = json!({"pid":pid,"os_start_identity":identity,"reaped":status.is_some(),"status":status.map(|s| s.to_string()),"timed_out":timed_out,"phase":phase,"phase_elapsed_ms":phase_started.elapsed().as_millis(),"fixture_group_termination_requested":cfg!(unix),"cleanup":cleanup,"production_guardian_certified":false});
    fs::write(
        root.join("supervisor-process.json"),
        serde_json::to_vec_pretty(&receipt).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let error = if timed_out {
        Some(format!(
            "native fixture deadline exceeded during {} action {}",
            phase["phase"], phase["action_id"]
        ))
    } else {
        supervision_error.or(cleanup_error).or(capture_error)
    };
    let path = root.join("summary.json");
    let mut summary = fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    if error.is_some()
        || status.and_then(|s| s.code()).is_none()
        || summary.as_ref().and_then(|v| v["exit_code"].as_i64())
            != status.and_then(|s| s.code()).map(i64::from)
    {
        let raw =
            protocol::read_bounded(&root.join("artifacts/fixture.json"), protocol::INDEX_BYTES)
                .ok()
                .and_then(|b| protocol::unique_json(&b).ok());
        let gates = raw
            .as_ref()
            .and_then(|v| v["gate_ids"].as_array())
            .map(|g| {
                g.iter()
                    .filter_map(Value::as_str)
                    .map(gate_allocation)
                    .collect::<Vec<_>>()
            });
        let run: Value = fs::read(root.join("run.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(Value::Null);
        summary = Some(
            json!({"protocol":protocol::PROTOCOL,"protocol_version":1,"run_id":run["run_id"],"fixture_id":raw.as_ref().map(|v| &v["fixture_id"]),"mode":mode,"lane":run["lane"],"exit_code":2,"disposition":"not_run","missing_requirements":error.unwrap_or_else(|| "native fixture process failed before a complete report".into()),"incomplete_artifact_index":true,"positive_controls":[],"assertions":[],"observations":[],"artifacts":[],"pending":[],"gates":gates}),
        );
    }
    let mut summary = summary.ok_or("native fixture report unavailable")?;
    for name in [
        "supervisor-process.json",
        "supervisor-deadline.json",
        "supervisor-stdout.txt",
        "supervisor-stderr.txt",
    ] {
        let path = root.join(name);
        let reference = json!({"relative_file":name,"byte_length":fs::metadata(&path).map_err(|e| e.to_string())?.len(),"sha256":kinewright_media::sha256_file(&path).map_err(|e| e.to_string())?});
        if let Some(artifacts) = summary["artifacts"].as_array_mut() {
            artifacts.push(reference);
        }
    }
    let quota = protocol::read_bounded(&root.join("artifacts/fixture.json"), protocol::INDEX_BYTES)
        .ok()
        .and_then(|b| protocol::unique_json(&b).ok())
        .and_then(|v| v["limits"]["output_bytes"].as_u64())
        .unwrap_or(protocol::INDEX_BYTES)
        .min(protocol::INDEX_BYTES);
    let bytes = serde_json::to_vec_pretty(&summary).map_err(|e| e.to_string())?;
    // The child's terminal reserve also accommodates these bounded supervisor
    // records. Refuse an overflowing final index rather than reporting a pass.
    let total = retained_bytes(&root)?
        .saturating_sub(fs::metadata(&path).map_or(0, |m| m.len()))
        .saturating_add(bytes.len() as u64);
    if total > quota {
        summary = json!({"protocol":protocol::PROTOCOL,"protocol_version":1,"run_id":summary["run_id"],"fixture_id":summary["fixture_id"],"mode":mode,"lane":summary["lane"],"exit_code":2,"disposition":"not_run","incomplete_artifact_index":true,"missing_requirements":"supervisor report quota exceeded","assertions":[],"positive_controls":[],"observations":[],"artifacts":[],"pending":[],"gates":summary["gates"]});
    }
    fs::write(
        &path,
        serde_json::to_vec_pretty(&summary).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(summary["exit_code"]
        .as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .unwrap_or(2))
}

fn retained_bytes(root: &Path) -> Result<u64, String> {
    let mut total = 0_u64;
    let mut directories = vec![(root.to_path_buf(), 0)];
    let mut count = 0;
    while let Some((directory, depth)) = directories.pop() {
        if depth > 32 {
            return Err("supervisor inventory depth quota".into());
        }
        for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
            count += 1;
            if count > 20_000 {
                return Err("supervisor inventory item quota".into());
            }
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.path() == root.join("work") {
                continue;
            }
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() {
                directories.push((entry.path(), depth + 1));
            } else {
                total = total.saturating_add(entry.metadata().map_err(|e| e.to_string())?.len());
            }
        }
    }
    Ok(total)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn deadline_exit_race_accepts_esrch_and_preserves_permission_errors() {
        // Deterministically inject the kernel's "group already gone" result.
        // A wall-clock race would not reliably exercise this branch.
        assert!(termination_result(Err(rustix::io::Errno::SRCH)).is_ok());
        assert!(termination_result(Ok(())).is_ok());
        assert!(termination_result(Err(rustix::io::Errno::PERM)).is_err());
    }
}
