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
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const DEADLINE_ENV: &str = "JA_HARNESS_DEADLINE_FILE";
const CAPTURE: u64 = 64 * 1024;

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

struct OwnedFixture(Child);
impl OwnedFixture {
    fn stop(&mut self) -> Result<(), String> {
        #[cfg(unix)]
        {
            // process_group(0) made this owned child's PID its group ID. Every
            // descendant is a fixed fixture role; no arbitrary programs run.
            let status = Command::new("/bin/kill")
                .args(["-KILL", "--", &format!("-{}", self.0.id())])
                .status()
                .map_err(|e| e.to_string())?;
            if !status.success() {
                return Err("fixture group termination failed".into());
            }
        }
        #[cfg(windows)]
        {
            let status = Command::new("taskkill")
                .args(["/PID", &self.0.id().to_string(), "/T", "/F"])
                .status()
                .map_err(|e| e.to_string())?;
            if !status.success() {
                return Err("Windows fixture tree termination unavailable; NOT RUN".into());
            }
        }
        #[cfg(not(any(unix, windows)))]
        self.0.kill().map_err(|e| e.to_string())?;
        Ok(())
    }
}
impl Drop for OwnedFixture {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.stop();
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn capture(mut stream: impl Read + Send + 'static) -> thread::JoinHandle<Result<Vec<u8>, String>> {
    thread::spawn(move || {
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
    })
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
    let mut child = OwnedFixture(command.spawn().map_err(|e| e.to_string())?);
    let pid = child.0.id();
    let identity = os_start_identity(pid);
    let stdout = capture(child.0.stdout.take().ok_or("fixture stdout")?);
    let stderr = capture(child.0.stderr.take().ok_or("fixture stderr")?);
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
    let status = loop {
        if let Some(status) = child.0.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if let Ok(bytes) = protocol::read_bounded(&control, protocol::CONTROL_BYTES as u64)
            && let Ok(value) = protocol::unique_json(&bytes)
            && value != phase
            && let Some(ms) = value["timeout_ms"]
                .as_u64()
                .filter(|n| (1..=600_000).contains(n))
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
            child.stop()?;
            break child.0.wait().map_err(|e| e.to_string())?;
        }
        thread::sleep(Duration::from_millis(2));
    };
    let mut capture_error = None;
    for (name, handle) in [
        ("supervisor-stdout.txt", stdout),
        ("supervisor-stderr.txt", stderr),
    ] {
        let bytes = match handle.join().map_err(|_| "fixture capture panicked")? {
            Ok(bytes) => bytes,
            Err(error) => {
                capture_error = Some(error);
                Vec::new()
            }
        };
        fs::write(root.join(name), bytes).map_err(|e| e.to_string())?;
    }
    let receipt = json!({"pid":pid,"os_start_identity":identity,"reaped":true,"status":status.to_string(),"timed_out":timed_out,"phase":phase,"phase_elapsed_ms":phase_started.elapsed().as_millis(),"fixture_group_termination_requested":timed_out && cfg!(unix),"production_guardian_certified":false});
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
        capture_error
    };
    let path = root.join("summary.json");
    let mut summary = fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    if error.is_some()
        || summary.as_ref().and_then(|v| v["exit_code"].as_i64()) != status.code().map(i64::from)
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
