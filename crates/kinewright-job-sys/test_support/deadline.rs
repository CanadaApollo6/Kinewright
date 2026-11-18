//! Test-only watchdog for native calls (including joins and child I/O) that
//! cannot return an error to the test thread once they stall.
use std::{
    io::Write as _,
    sync::{
        OnceLock,
        mpsc::{self, RecvTimeoutError, Sender},
    },
    time::Duration,
};

/// What the code under test knows when a deadline fires (for the media
/// client, which reply each live probe waits for). Set once per process.
static REPORT: OnceLock<fn() -> String> = OnceLock::new();

#[allow(
    dead_code,
    reason = "shared test support; only some crates add a report"
)]
pub fn report_with(report: fn() -> String) {
    let _ = REPORT.set(report);
}

/// Writes one named line past libtest's output capture, so it reaches the CI
/// job log even when the test passes (for example which resource modes ran).
#[allow(
    dead_code,
    reason = "shared test support; only some crates announce a line"
)]
pub fn announce(line: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{line}");
    let _ = stderr.flush();
}

/// Every descendant of this process, with the kernel's view of where it is.
/// Reads procfs only (no ptrace); fields it may not read are left out.
fn processes() -> String {
    use std::fmt::Write as _;
    let mut parents = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<u32>().ok())
            else {
                continue;
            };
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
                continue;
            };
            let parent = stat
                .rsplit_once(") ")
                .and_then(|(_, rest)| rest.split(' ').nth(1))
                .and_then(|field| field.parse::<u32>().ok());
            if let Some(parent) = parent {
                parents.push((pid, parent));
            }
        }
    }
    let mut out = String::new();
    let mut queue = vec![(std::process::id(), 0_usize)];
    while let Some((pid, depth)) = queue.pop() {
        if out.len() > 16 * 1024 {
            out.push_str("MS2_TEST_PROCESS truncated\n");
            break;
        }
        let base = format!("/proc/{pid}");
        let status = std::fs::read_to_string(format!("{base}/status")).unwrap_or_default();
        let field = |key: &str| {
            status
                .lines()
                .find_map(|line| line.strip_prefix(key))
                .map_or("?", str::trim)
                .to_owned()
        };
        let wchan = std::fs::read_to_string(format!("{base}/wchan"))
            .unwrap_or_else(|error| format!("<{error}>"));
        let command: String = std::fs::read(format!("{base}/cmdline"))
            .map(|bytes| {
                bytes
                    .split(|byte| *byte == 0)
                    .take(3)
                    .map(|part| String::from_utf8_lossy(part).into_owned())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default()
            .chars()
            .take(160)
            .collect();
        let _ = writeln!(
            out,
            "MS2_TEST_PROCESS depth={depth} pid={pid} name={} state={} seccomp={} \
             no_new_privs={} wchan={wchan} cmd={command}",
            field("Name:"),
            field("State:"),
            field("Seccomp:"),
            field("NoNewPrivs:"),
        );
        for (child, parent) in &parents {
            if *parent == pid {
                queue.push((*child, depth + 1));
            }
        }
    }
    out
}

#[must_use]
pub fn start(reason: &'static str, timeout: Duration) -> Sender<()> {
    let test = std::thread::current();
    let (done, wait) = mpsc::channel();
    std::thread::Builder::new()
        .name("ms2-test-deadline".into())
        .spawn(move || {
            if wait.recv_timeout(timeout) == Err(RecvTimeoutError::Timeout) {
                // Bypass libtest's output capture: the blocked test thread
                // never gets a chance to report its captured output.
                let test = test.name().unwrap_or("unnamed");
                let mut stderr = std::io::stderr().lock();
                let _ = writeln!(
                    stderr,
                    "MS2_TEST_DEADLINE {reason} test={test} ({timeout:?})"
                );
                if let Some(report) = REPORT.get() {
                    let _ = writeln!(stderr, "{}", report());
                }
                let _ = write!(stderr, "{}", processes());
                let _ = stderr.flush();
                std::process::exit(124);
            }
        })
        .expect("start MS2 test deadline");
    done
}

#[allow(
    dead_code,
    reason = "shared test support; some crates name every phase"
)]
#[must_use]
pub fn test() -> Sender<()> {
    start("native fixture", Duration::from_secs(120))
}
