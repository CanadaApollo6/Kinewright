//! MS2 native-test diagnostics, test-only and read-only: environment facts
//! once per test binary, and a sampler of every descendant of the test process
//! while a probe is outstanding. Every line starts with `MS2_DIAG` and is
//! written past libtest's capture. A passing run prints only the facts line.
//!
//! The sampler reads procfs only (no ptrace). Entries a non-dumpable process
//! hides (fds, namespaces, `syscall`, `wchan`, `stack`) are read through one
//! `sudo -n sh -c 'cat …'` per sample, and only when [`CI_DIAG`] is set and
//! `sudo -n true` works. A developer machine never sets it, so it never uses
//! sudo; there those entries show the kernel's refusal instead.
#![allow(
    dead_code,
    reason = "shared test support; each test binary uses a different part"
)]
use std::{
    collections::{HashMap, VecDeque},
    fmt::Write as _,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex, Once, OnceLock, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// Set by the Linux CI jobs only; enables privileged reads (see the module note).
pub const CI_DIAG: &str = "KINEWRIGHT_MS2_CI_DIAG";
const INTERVAL: Duration = Duration::from_millis(250);
const RECENT: usize = 4;
const LIMIT: usize = 96 * 1024;

static CLIENT: OnceLock<fn() -> String> = OnceLock::new();
static ACTIVE: Mutex<Vec<Arc<Mutex<Samples>>>> = Mutex::new(Vec::new());
static LOGS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static FACTS: Once = Once::new();
static PRIVILEGED: OnceLock<bool> = OnceLock::new();

fn emit(text: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = stderr.write_all(text.as_bytes());
    let _ = stderr.flush();
}

/// For a test: names the client's view of each live probe, routes the test
/// deadline's report here, and prints the environment facts once per binary.
pub fn install(client: fn() -> String) {
    fixture(client);
    FACTS.call_once(|| emit(&facts()));
}

/// For a fixture child of a test: as [`install`], without the facts line.
pub fn fixture(client: fn() -> String) {
    let _ = CLIENT.set(client);
    crate::deadline::report_with(report);
}

/// Whether a systemd user manager answers on the session bus the runtime
/// would use (`DBUS_SESSION_BUS_ADDRESS`, else the user's runtime directory).
pub fn user_manager() -> bool {
    Command::new("busctl")
        .args([
            "--user",
            "get-property",
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "Version",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Writes `<directory>/<name>`, a shell wrapper that appends the executable's
/// stderr to `<directory>/<name>.stderr` and then execs it with `extra`
/// appended to its arguments. The host's spawn sets stderr to `/dev/null`; the
/// wrapper moves only fd 2 and leaves every other descriptor as inherited.
/// Reports from this process print the log's tail.
pub fn wrap(directory: &Path, executable: &Path, name: &str, extra: &[&str]) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let quote = |text: &str| format!("'{}'", text.replace('\'', "'\\''"));
    let wrapper = directory.join(name);
    let log = directory.join(format!("{name}.stderr"));
    let mut script = format!(
        "#!/bin/sh\nexec 2>>{}\nexec {} \"$@\"",
        quote(log.to_str().unwrap()),
        quote(executable.to_str().unwrap())
    );
    for argument in extra {
        script.push(' ');
        script.push_str(&quote(argument));
    }
    script.push('\n');
    std::fs::write(&wrapper, script).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    LOGS.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(log);
    wrapper.canonicalize().unwrap()
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path)
        .map_or_else(|error| format!("<{error}>"), |text| text.trim().to_owned())
}

fn command(program: &str, arguments: &[&str]) -> String {
    Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_or_else(
            |error| format!("<{error}>"),
            |output| String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        )
}

/// The highest Landlock ABI this kernel accepts as a hard requirement, from
/// rulesets that are created and dropped, never enforced. ABI 7 and later add
/// no access right or scope, so they show as `6+`.
fn landlock_abi() -> &'static str {
    use landlock::{
        ABI, Access, AccessFs, AccessNet, CompatLevel, Compatible, Ruleset, RulesetAttr, Scope,
    };
    let accepts = |abi: ABI, net: bool, scope: bool| {
        let mut rules = Ruleset::default()
            .set_compatibility(CompatLevel::HardRequirement)
            .handle_access(AccessFs::from_all(abi));
        if net {
            rules = rules.and_then(|rules| rules.handle_access(AccessNet::from_all(abi)));
        }
        if scope {
            rules = rules.and_then(|rules| rules.scope(Scope::from_all(abi)));
        }
        rules.and_then(Ruleset::create).is_ok()
    };
    [
        (ABI::V6, true, true, "6+"),
        (ABI::V5, true, false, "5"),
        (ABI::V4, true, false, "4"),
        (ABI::V3, false, false, "3"),
        (ABI::V2, false, false, "2"),
        (ABI::V1, false, false, "1"),
    ]
    .into_iter()
    .find(|&(abi, net, scope, _)| accepts(abi, net, scope))
    .map_or("none", |(.., name)| name)
}

/// This process's descriptors: number, target, and `c` when close-on-exec.
fn own_fds() -> String {
    let mut fds: Vec<(u32, String)> = std::fs::read_dir("/proc/self/fd")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let fd = entry.file_name().to_str()?.parse().ok()?;
            let target = std::fs::read_link(entry.path()).ok()?;
            // The directory handle this listing itself holds.
            if target.ends_with("fd") && target.starts_with("/proc") {
                return None;
            }
            let flags = read(&format!("/proc/self/fdinfo/{fd}"));
            let cloexec = flags
                .lines()
                .find_map(|line| line.strip_prefix("flags:"))
                .and_then(|value| u32::from_str_radix(value.trim(), 8).ok())
                .is_some_and(|value| value & 0o2_000_000 != 0);
            Some((
                fd,
                format!(
                    "{fd}:{}{}",
                    target.display(),
                    if cloexec { ":c" } else { "" }
                ),
            ))
        })
        .collect();
    fds.sort_unstable();
    format!(
        "{} [{}]",
        fds.len(),
        fds.into_iter()
            .map(|(_, text)| text)
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn facts() -> String {
    let limits = read("/proc/self/limits");
    let limit = |name: &str| {
        limits
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .map_or_else(
                || "?".to_owned(),
                |rest| {
                    rest.split_whitespace()
                        .take(2)
                        .collect::<Vec<_>>()
                        .join("/")
                },
            )
    };
    let status = read("/proc/self/status");
    let uid = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|ids| ids.split_whitespace().next())
        .unwrap_or("?")
        .to_owned();
    let cgroup = read("/proc/self/cgroup");
    let path = cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .unwrap_or("?");
    let service = format!("/sys/fs/cgroup/user.slice/user-{uid}.slice/user@{uid}.service");
    format!(
        "MS2_DIAG facts kernel={} glibc={} landlock_abi={} lsm={} cgroup={path} \
         cgroup_controllers=[{}] user_service_controllers=[{}] user_service_subtree=[{}] \
         user_manager={} rlimit_as={} rlimit_nproc={} rlimit_nofile={} cpus={} loadavg=[{}] \
         restrict_userns={} max_user_namespaces={} ptrace_scope={} privileged_reads={} \
         fds={}\n",
        read("/proc/sys/kernel/osrelease"),
        command("getconf", &["GNU_LIBC_VERSION"]),
        landlock_abi(),
        read("/sys/kernel/security/lsm"),
        read(&format!("/sys/fs/cgroup{path}/cgroup.controllers")),
        read(&format!("{service}/cgroup.controllers")),
        read(&format!("{service}/cgroup.subtree_control")),
        if user_manager() { "yes" } else { "no" },
        limit("Max address space"),
        limit("Max processes"),
        limit("Max open files"),
        std::thread::available_parallelism().map_or(0, std::num::NonZero::get),
        read("/proc/loadavg"),
        read("/proc/sys/kernel/apparmor_restrict_unprivileged_userns"),
        read("/proc/sys/user/max_user_namespaces"),
        read("/proc/sys/kernel/yama/ptrace_scope"),
        if std::env::var_os(CI_DIAG).is_none() {
            "unset"
        } else if privileged() {
            "sudo"
        } else {
            "no-sudo"
        },
        own_fds(),
    )
}

fn privileged() -> bool {
    *PRIVILEGED.get_or_init(|| {
        std::env::var_os(CI_DIAG).is_some()
            && Command::new("sudo")
                .args(["-n", "true"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
    })
}

struct Sample {
    at: Duration,
    client: String,
    processes: Vec<(u32, String)>,
}
impl Sample {
    /// The client still waits on the launch or the helper (not on cleanup).
    fn waiting(&self) -> bool {
        self.client
            .split(" waiting=")
            .skip(1)
            .any(|phase| !phase.starts_with("cleanup"))
    }
}
struct Samples {
    label: String,
    recent: VecDeque<Sample>,
    /// The newest sample taken while the client waited on the helper, once it
    /// has left `recent`: what the helper did before the kill and cleanup.
    waiting: Option<Sample>,
}
impl Samples {
    fn push(&mut self, sample: Sample) {
        if sample.waiting() {
            self.waiting = None;
        }
        self.recent.push_back(sample);
        if self.recent.len() > RECENT
            && let Some(oldest) = self.recent.pop_front()
            && oldest.waiting()
            && !self.recent.iter().any(Sample::waiting)
        {
            self.waiting = Some(oldest);
        }
    }
    fn render(&self, out: &mut String) {
        let mut previous: HashMap<u32, &str> = HashMap::new();
        let kept = self
            .waiting
            .iter()
            .map(|sample| ("last-waiting", sample))
            .chain(self.recent.iter().map(|sample| ("recent", sample)));
        for (kind, sample) in kept {
            let _ = writeln!(
                out,
                "MS2_DIAG sample label={} kind={kind} t_ms={} client={}",
                self.label,
                sample.at.as_millis(),
                sample.client
            );
            for (pid, line) in &sample.processes {
                if previous.get(pid) == Some(&line.as_str()) {
                    let _ = writeln!(out, "MS2_DIAG proc pid={pid} unchanged");
                } else {
                    let _ = writeln!(out, "MS2_DIAG proc {line}");
                }
                previous.insert(*pid, line);
            }
        }
    }
}

/// Samples every descendant while it lives. Ends with [`Sampler::finish`];
/// dropped during a panic, it prints what it kept.
pub struct Sampler {
    samples: Arc<Mutex<Samples>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

pub fn sample(label: &str) -> Sampler {
    let samples = Arc::new(Mutex::new(Samples {
        label: label.replace(' ', "_"),
        recent: VecDeque::new(),
        waiting: None,
    }));
    ACTIVE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(Arc::clone(&samples));
    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let samples = Arc::clone(&samples);
        let stop = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("ms2-diag-sampler".into())
            .spawn(move || {
                let start = Instant::now();
                while !stop.load(Ordering::Acquire) {
                    let sample = take(start.elapsed());
                    samples
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(sample);
                    std::thread::park_timeout(INTERVAL);
                }
            })
            .unwrap()
    };
    Sampler {
        samples,
        stop,
        thread: Some(thread),
    }
}

impl Sampler {
    /// Stops sampling; prints the kept samples, the client's current view and
    /// the captured stderr when the probe's outcome is not the one expected.
    pub fn finish(mut self, unexpected: bool) {
        self.end(unexpected.then_some("unexpected-outcome"));
    }
    fn end(&mut self, reason: Option<&str>) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        self.stop.store(true, Ordering::Release);
        thread.thread().unpark();
        let _ = thread.join();
        ACTIVE
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|samples| !Arc::ptr_eq(samples, &self.samples));
        if let Some(reason) = reason {
            let mut out = header(reason);
            self.samples
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .render(&mut out);
            logs(&mut out);
            emit(&truncate(out));
        }
    }
}
impl Drop for Sampler {
    fn drop(&mut self) {
        self.end(std::thread::panicking().then_some("panic"));
    }
}

fn header(reason: &str) -> String {
    format!(
        "MS2_DIAG report reason={reason} pid={} now={}\n",
        std::process::id(),
        CLIENT.get().map_or_else(String::new, |client| client())
    )
}

fn truncate(mut out: String) -> String {
    if out.len() > LIMIT {
        let mut end = LIMIT;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
        out.push_str("\nMS2_DIAG truncated\n");
    }
    out
}

fn logs(out: &mut String) {
    for log in LOGS.lock().unwrap_or_else(PoisonError::into_inner).iter() {
        let Ok(text) = std::fs::read_to_string(log) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        for line in &lines[lines.len().saturating_sub(40)..] {
            let _ = writeln!(out, "MS2_DIAG stderr log={} {line}", log.display());
        }
    }
}

/// For the test deadline: the client's view, every running sampler's samples
/// and the captured stderr.
pub fn report() -> String {
    let mut out = header("deadline");
    for samples in ACTIVE.lock().unwrap_or_else(PoisonError::into_inner).iter() {
        samples
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .render(&mut out);
    }
    logs(&mut out);
    truncate(out)
}

/// Every descendant of this process, with its depth, in walk order.
fn descendants() -> Vec<(u32, usize)> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        if let Some(parent) = stat
            .rsplit_once(") ")
            .and_then(|(_, rest)| rest.split(' ').nth(1))
            .and_then(|field| field.parse::<u32>().ok())
        {
            children.entry(parent).or_default().push(pid);
        }
    }
    let mut out = Vec::new();
    let mut queue = vec![(std::process::id(), 0)];
    while let Some((pid, depth)) = queue.pop() {
        out.push((pid, depth));
        for &child in children.get(&pid).into_iter().flatten() {
            queue.push((child, depth + 1));
        }
    }
    out
}

/// One `sudo -n sh -c` per sample: fds, namespaces, syscall, wchan, stack and
/// per-task syscall of each pid, keyed `pid` then entry name.
fn privileged_reads(pids: &[u32]) -> HashMap<u32, HashMap<String, String>> {
    const SCRIPT: &str = r#"for p in "$@"; do
  for f in syscall wchan stack; do printf '@@ %s %s\n' "$p" "$f"; cat "/proc/$p/$f" 2>&1; echo; done
  printf '@@ %s fd\n' "$p"
  for f in /proc/$p/fd/*; do printf '%s:%s,' "${f##*/}" "$(readlink "$f" 2>&1)"; done; echo
  printf '@@ %s ns\n' "$p"
  for n in /proc/$p/ns/*; do printf '%s,' "$(readlink "$n" 2>&1)"; done; echo
  printf '@@ %s tasks\n' "$p"
  for t in /proc/$p/task/*; do printf '%s:%s,' "${t##*/}" "$(cut -d' ' -f1 "$t/syscall" 2>&1)"; done; echo
done"#;
    let mut out: HashMap<u32, HashMap<String, String>> = HashMap::new();
    let Ok(output) = Command::new("sudo")
        .args(["-n", "sh", "-c", SCRIPT, "sh"])
        .args(pids.iter().map(u32::to_string))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return out;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut current: Option<(u32, &str)> = None;
    for line in text.lines() {
        if let Some(key) = line.strip_prefix("@@ ") {
            current = key
                .split_once(' ')
                .and_then(|(pid, name)| Some((pid.parse().ok()?, name)));
            continue;
        }
        if let Some((pid, name)) = current {
            let value = out
                .entry(pid)
                .or_default()
                .entry(name.to_owned())
                .or_default();
            if !line.is_empty() {
                if !value.is_empty() {
                    value.push('|');
                }
                value.push_str(line.trim());
            }
        }
    }
    out
}

fn take(at: Duration) -> Sample {
    let client = CLIENT.get().map_or_else(String::new, |client| client());
    let pids = descendants();
    let mut privileged = if privileged() {
        privileged_reads(&pids.iter().map(|&(pid, _)| pid).collect::<Vec<_>>())
    } else {
        HashMap::new()
    };
    let processes = pids
        .into_iter()
        .map(|(pid, depth)| {
            let entries = privileged.remove(&pid).unwrap_or_default();
            (pid, describe(pid, depth, entries))
        })
        .collect();
    Sample {
        at,
        client,
        processes,
    }
}

/// One process: identity, scheduler state and CPU ticks (a busy process gains
/// ticks between samples; a blocked one does not), seccomp and `no_new_privs`,
/// memory, then what it is blocked in and what it holds.
fn describe(pid: u32, depth: usize, mut privileged: HashMap<String, String>) -> String {
    let base = format!("/proc/{pid}");
    let stat = read(&format!("{base}/stat"));
    let comm = stat
        .split_once(" (")
        .and_then(|(_, rest)| rest.rsplit_once(") "))
        .map_or("?", |(name, _)| name);
    let fields: Vec<&str> = stat
        .rsplit_once(") ")
        .map(|(_, rest)| rest.split(' ').collect())
        .unwrap_or_default();
    let field = |index: usize| fields.get(index).copied().unwrap_or("?");
    let status = read(&format!("{base}/status"));
    let status_field = |key: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .map_or("?", |value| value.trim().trim_end_matches(" kB"))
    };
    let mut entry =
        |name: &str, direct: &dyn Fn() -> String| privileged.remove(name).unwrap_or_else(direct);
    let fds = entry("fd", &|| direct_fds(&base));
    let namespaces = entry("ns", &|| direct_namespaces(&base));
    let syscall = entry("syscall", &|| read(&format!("{base}/syscall")));
    let wchan = entry("wchan", &|| read(&format!("{base}/wchan")));
    let stack = entry("stack", &|| {
        read(&format!("{base}/stack"))
            .lines()
            .take(12)
            .collect::<Vec<_>>()
            .join("|")
    });
    let tasks = entry("tasks", &|| direct_tasks(&base));
    format!(
        "pid={pid} depth={depth} ppid={} comm={comm} state={} cpu_ticks={}+{} threads={} \
         seccomp={} filters={} nnp={} vm_peak_kb={} vm_size_kb={} rss_kb={} sig_pending={} \
         syscall=[{syscall}] wchan={wchan} stack=[{stack}] tasks=[{tasks}] ns=[{namespaces}] \
         fds=[{fds}]",
        field(1),
        field(0),
        field(11),
        field(12),
        field(17),
        status_field("Seccomp:"),
        status_field("Seccomp_filters:"),
        status_field("NoNewPrivs:"),
        status_field("VmPeak:"),
        status_field("VmSize:"),
        status_field("VmRSS:"),
        status_field("ShdPnd:"),
    )
}

/// Descriptor number and target, as this process may read them.
fn direct_fds(base: &str) -> String {
    std::fs::read_dir(format!("{base}/fd")).map_or_else(
        |error| format!("<{error}>"),
        |entries| {
            let mut fds: Vec<String> = entries
                .flatten()
                .map(|entry| {
                    let target = std::fs::read_link(entry.path()).map_or_else(
                        |error| format!("<{error}>"),
                        |path| path.display().to_string(),
                    );
                    format!("{}:{target}", entry.file_name().to_string_lossy())
                })
                .collect();
            fds.sort_by_key(|fd| {
                fd.split_once(':')
                    .and_then(|(number, _)| number.parse::<u32>().ok())
            });
            fds.join(",")
        },
    )
}

fn direct_namespaces(base: &str) -> String {
    ["user", "pid", "mnt", "net", "ipc", "uts", "cgroup"]
        .iter()
        .map(|name| {
            std::fs::read_link(format!("{base}/ns/{name}")).map_or_else(
                |error| format!("{name}:<{error}>"),
                |link| link.display().to_string(),
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Each task's id and scheduler state (the first 16 tasks).
fn direct_tasks(base: &str) -> String {
    std::fs::read_dir(format!("{base}/task")).map_or_else(
        |error| format!("<{error}>"),
        |entries| {
            entries
                .flatten()
                .take(16)
                .map(|task| {
                    let state = std::fs::read_to_string(task.path().join("stat"))
                        .ok()
                        .and_then(|stat| {
                            stat.rsplit_once(") ")
                                .and_then(|(_, rest)| rest.split(' ').next().map(str::to_owned))
                        })
                        .unwrap_or_else(|| "?".into());
                    format!("{}:{state}", task.file_name().to_string_lossy())
                })
                .collect::<Vec<_>>()
                .join(",")
        },
    )
}
