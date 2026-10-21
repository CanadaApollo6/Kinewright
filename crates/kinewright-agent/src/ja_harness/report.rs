//! Bounded literal observations and artifact identities; no inferred counters.
#![allow(clippy::missing_errors_doc)]
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use serde_json::{Value, json};

use super::protocol::{CONTROL_BYTES, INDEX_BYTES, Mode, PROTOCOL, relative_path};

pub struct Report {
    pub root: PathBuf,
    pub run_id: String,
    pub mode: Mode,
    pub assertions: Vec<Value>,
    pub pending: Vec<Value>,
    pub observations: Vec<Value>,
    pub artifacts: Vec<Value>,
    used: u64,
    quota: u64,
    sequence: u64,
    started: Instant,
    events: fs::File,
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

impl Report {
    pub fn create(root: &Path, mode: Mode) -> Result<Self, String> {
        fs::create_dir(root).map_err(|e| format!("private output directory: {e}"))?;
        fs::create_dir(root.join("artifacts")).map_err(|e| e.to_string())?;
        let events = fs::File::create(root.join("events.jsonl")).map_err(|e| e.to_string())?;
        let run_id = format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos()
        );
        Ok(Self {
            root: root.to_path_buf(),
            run_id,
            mode,
            assertions: Vec::new(),
            pending: Vec::new(),
            observations: Vec::new(),
            artifacts: Vec::new(),
            used: 0,
            quota: INDEX_BYTES,
            sequence: 0,
            started: Instant::now(),
            events,
        })
    }

    pub fn set_quota(&mut self, quota: u64) -> Result<(), String> {
        self.quota = quota;
        if self.used > quota {
            return Err("output quota exceeded".into());
        }
        Ok(())
    }

    fn charge(&mut self, bytes: usize) -> Result<(), String> {
        self.used = self
            .used
            .checked_add(u64::try_from(bytes).map_err(|e| e.to_string())?)
            .ok_or("report size overflow")?;
        if self.used > self.quota {
            return Err("output quota exceeded; incomplete evidence".into());
        }
        Ok(())
    }

    pub fn artifact(&mut self, name: &str, bytes: &[u8]) -> Result<Value, String> {
        relative_path(Path::new(name))?;
        self.charge(bytes.len())?;
        let path = self.root.join("artifacts").join(name);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        let reference = json!({"relative_file":format!("artifacts/{name}"), "sha256":kinewright_media::sha256_bytes(bytes), "byte_length":bytes.len()});
        self.artifacts.push(reference.clone());
        Ok(reference)
    }

    /// Stream a completed fixture-owned file into the bounded report.
    pub fn artifact_file(&mut self, name: &str, source: &Path) -> Result<Value, String> {
        use std::io::Read as _;
        relative_path(Path::new(name))?;
        let length = fs::metadata(source).map_err(|e| e.to_string())?.len();
        if length > INDEX_BYTES {
            return Err("artifact exceeds ceiling".into());
        }
        self.charge(usize::try_from(length).map_err(|e| e.to_string())?)?;
        let destination = self.root.join("artifacts").join(name);
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|e| e.to_string())?;
        let copied = std::io::copy(
            &mut fs::File::open(source)
                .map_err(|e| e.to_string())?
                .take(length + 1),
            &mut output,
        )
        .map_err(|e| e.to_string())?;
        if copied != length {
            return Err("artifact changed while copying; incomplete".into());
        }
        let reference = json!({"relative_file":format!("artifacts/{name}"),"sha256":kinewright_media::sha256_file(&destination).map_err(|e| e.to_string())?,"byte_length":copied});
        self.artifacts.push(reference.clone());
        Ok(reference)
    }

    /// Register files produced by the exact App test adapter in our private tree.
    pub fn admit_adapter_tree(&mut self, root: &Path) -> Result<(), String> {
        let mut pending = vec![(root.to_path_buf(), 0)];
        let mut count = 0;
        while let Some((directory, depth)) = pending.pop() {
            if depth > 32 {
                return Err("adapter artifact depth quota".into());
            }
            for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                count += 1;
                if count > 10_000 || entry.file_type().map_err(|e| e.to_string())?.is_symlink() {
                    return Err("adapter artifact inventory quota/symlink".into());
                }
                let path = entry.path();
                if path.is_dir() {
                    pending.push((path, depth + 1));
                } else {
                    let relative = path
                        .strip_prefix(&self.root)
                        .map_err(|e| e.to_string())?
                        .to_path_buf();
                    relative_path(&relative)?;
                    let length = entry.metadata().map_err(|e| e.to_string())?.len();
                    self.charge(usize::try_from(length).map_err(|e| e.to_string())?)?;
                    self.artifacts.push(json!({"relative_file":relative,"byte_length":length,"sha256":kinewright_media::sha256_file(&path).map_err(|e| e.to_string())?}));
                }
            }
        }
        Ok(())
    }

    pub fn json_file(&mut self, name: &str, value: &impl Serialize) -> Result<(), String> {
        relative_path(Path::new(name))?;
        let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
        self.charge(bytes.len())?;
        fs::write(self.root.join(name), bytes).map_err(|e| e.to_string())
    }

    #[allow(clippy::needless_pass_by_value)] // Own payloads at the recording boundary.
    pub fn event(
        &mut self,
        action: u32,
        participant: &str,
        kind: &str,
        data: Value,
    ) -> Result<(), String> {
        self.sequence += 1;
        let value = json!({"sequence":self.sequence,"participant":participant,"action_id":action,"kind":kind,"elapsed_ns":self.started.elapsed().as_nanos().to_string(),"data":data});
        let mut bytes = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        if bytes.len() > CONTROL_BYTES {
            return Err("oversize event record".into());
        }
        self.charge(bytes.len())?;
        self.events.write_all(&bytes).map_err(|e| e.to_string())?;
        self.events.flush().map_err(|e| e.to_string())
    }

    pub fn header(
        &mut self,
        fixture: Option<&super::protocol::LoadedFixture>,
    ) -> Result<(), String> {
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let diff = Command::new("git")
            .args(["diff", "--binary", "HEAD"])
            .output()
            .map_err(|e| e.to_string())?;
        let diff_ref = self.artifact("source.diff", &diff.stdout)?;
        let listing = Command::new("git")
            .args([
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
                "-z",
            ])
            .output()
            .map_err(|e| e.to_string())?;
        if !listing.status.success() || listing.stdout.len() > CONTROL_BYTES {
            return Err("source inventory unavailable/oversize".into());
        }
        let names = listing
            .stdout
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .collect::<std::collections::BTreeSet<_>>();
        let mut sources = Vec::new();
        for name in names {
            let name = std::str::from_utf8(name).map_err(|e| e.to_string())?;
            let path = Path::new(name);
            if path.is_file() {
                sources.push(json!({"path":name,"sha256":kinewright_media::sha256_file(path).map_err(|e| e.to_string())?}));
            }
        }
        let source_manifest = self.artifact(
            "source-manifest.json",
            &serde_json::to_vec(&sources).map_err(|e| e.to_string())?,
        )?;
        let identity = json!({"protocol":PROTOCOL,"protocol_version":1,"run_id":self.run_id,
            "mode":self.mode,"fixture_id":fixture.map(|f| &f.fixture.fixture_id),
            "fixture_sha256":fixture.map(|f| kinewright_media::sha256_bytes(&f.bytes)),
            "lane":fixture.map(|f| f.fixture.lane),"requirements":fixture.map(|f| &f.fixture.requirements),
            "inputs":fixture.map(|f| &f.fixture.inputs),"oracles":fixture.map(|f| &f.fixture.oracles),
            "source_head":command_output("git", &["rev-parse","HEAD"]),"dirty_diff":diff_ref,"source_manifest":source_manifest,
            "binary_sha256":kinewright_media::sha256_file(&executable).map_err(|e| e.to_string())?,
            "binary":executable,"compiler":command_output("rustc", &["-vV"]),
            "profile":if cfg!(debug_assertions) {"debug/test"} else {"release"},
            "os":std::env::consts::OS,"architecture":std::env::consts::ARCH,
            "ffmpeg":command_output("ffmpeg", &["-version"]),
            "gpu_driver":command_output("nvidia-smi", &["--query-gpu=driver_version,pstate,clocks.gr,clocks.mem,utilization.gpu","--format=csv,noheader"]),
            "load_annotation":command_output("uptime", &[]),
            "controller_pid":std::process::id(),"controller_os_start_identity":os_start_identity(std::process::id()),
            "command_line":std::env::args().take(16).collect::<Vec<_>>(),
            "time_unix_ms":SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| e.to_string())?.as_millis().to_string(),
            "rss_bytes":null,"vram_bytes":null,"unobserved_counters":null});
        self.json_file("run.json", &identity)?;
        if let Some(f) = fixture {
            self.artifact("fixture.json", &f.bytes)?;
            for (i, (role, bytes)) in f.files.iter().enumerate() {
                let reference = self.artifact(&format!("input-{i}.bin"), bytes)?;
                self.event(
                    0,
                    "controller",
                    "captured_input",
                    json!({"role":role,"artifact":reference}),
                )?;
            }
        }
        Ok(())
    }

    pub fn finish(
        &mut self,
        fixture: Option<&super::protocol::Fixture>,
        error: Option<&str>,
    ) -> Result<i32, String> {
        let failed = self.assertions.iter().any(|a| a["passed"] == false);
        let exit = if error.is_some() {
            2
        } else if failed {
            1
        } else if self.assertions.is_empty() && !self.pending.is_empty() {
            3
        } else {
            0
        };
        let summary = json!({"protocol":PROTOCOL,"protocol_version":1,"run_id":self.run_id,
            "fixture_id":fixture.map(|f| &f.fixture_id),"mode":self.mode,"lane":fixture.map(|f| f.lane),"exit_code":exit,
            "positive_controls":self.assertions.iter().filter(|a| a["evidence_class"] == "control").collect::<Vec<_>>(),
            "assertions":self.assertions,"pending":self.pending,"observations":self.observations,"artifacts":self.artifacts,
            "gates":fixture.map(|f| f.gate_ids.iter().map(|gate| json!({"gate":gate,"disposition":"pending_baseline_witness","reason":"S0 observations do not implement the owning production gate; see JA-S0-CONTRACTS §8","owner":"owning JA-1/JA-4/JA-2/AW2 stage"})).collect::<Vec<_>>()),
            "disposition":if error.is_some() {"not_run"} else {"observed"},"missing_requirements":error,
            "elapsed_ns":self.started.elapsed().as_nanos().to_string()});
        // Reserve a bounded terminal setup result even when the selected quota
        // was exhausted. The normal report must never become a truncated pass.
        self.quota = INDEX_BYTES;
        self.used = 0;
        self.json_file("summary.json", &summary)?;
        Ok(exit)
    }
}

#[must_use]
pub fn os_start_identity(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let fields = stat
            .rsplit_once(')')?
            .1
            .split_whitespace()
            .collect::<Vec<_>>();
        let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
        Some(format!("{}:{}", boot.trim(), fields.get(19)?))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}
