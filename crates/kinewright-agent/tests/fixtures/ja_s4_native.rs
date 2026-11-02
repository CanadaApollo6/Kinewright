//! Fixed native observer protocol fixture. It never reads or writes a project.
use std::{
    env, fs,
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::Duration,
};
fn publish(root: &Path, role: &str) {
    #[cfg(target_os = "linux")]
    let start = fs::read_to_string("/proc/self/stat")
        .unwrap()
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(19)
        .unwrap()
        .to_owned();
    #[cfg(not(target_os = "linux"))]
    let start = "0";
    let pid_field = if cfg!(target_os = "linux") {
        "namespace_pid"
    } else {
        "pid"
    };
    let text = format!(
        "{{\"version\":1,\"role\":\"{role}\",\"{pid_field}\":{},\"start\":{start}}}",
        std::process::id()
    );
    let path = root.join(format!("{role}.json"));
    fs::write(path.with_extension("partial"), text).unwrap();
    fs::rename(path.with_extension("partial"), path).unwrap();
}
fn main() {
    let args: Vec<_> = env::args_os().collect();
    let root = Path::new(&args[1]);
    let role = args[2].to_str().unwrap();
    #[cfg(target_os = "linux")]
    if role == "s4b-image" {
        // The provider can name its own sealed image, as Codex must, but it
        // cannot change it and holds no descriptor for it.
        let named = env::current_exe().is_ok_and(|path| {
            path.to_string_lossy()
                .starts_with("/memfd:kinewright-provider")
        });
        let sealed = fs::OpenOptions::new()
            .write(true)
            .open("/proc/self/exe")
            .is_err();
        let private = (0..256).all(|fd| {
            fs::read_link(format!("/proc/self/fd/{fd}"))
                .map_or(true, |link| !link.to_string_lossy().starts_with("/memfd:"))
        });
        fs::write(
            root.join("image-report.partial"),
            format!("qualified=true named={named} sealed={sealed} private_fd={private}"),
        )
        .unwrap();
        fs::rename(root.join("image-report.partial"), root.join("image-report")).unwrap();
        return;
    }
    #[cfg(target_os = "linux")]
    if role == "s4b-proc1" {
        use std::os::unix::fs::MetadataExt;
        // Records only outcomes and lengths, never init's environment or memory.
        let errno = |error: std::io::Error| format!("err:{}", error.raw_os_error().unwrap_or(0));
        let mut report = Vec::new();
        for name in ["environ", "cmdline", "mem"] {
            let path = format!("/proc/1/{name}");
            let outcome = match fs::File::open(&path) {
                Err(error) => errno(error),
                Ok(mut file) => {
                    let mut bytes = [0_u8; 4096];
                    match file.read(&mut bytes) {
                        Ok(read) => format!("opened:read_bytes={read}"),
                        Err(error) => format!("opened:{}", errno(error)),
                    }
                }
            };
            report.push(format!("\"{name}\":\"{outcome}\""));
        }
        let listing = fs::read_dir("/proc/1/fd")
            .map_or_else(errno, |entries| format!("listed:{}", entries.count()));
        report.push(format!("\"fd_list\":\"{listing}\""));
        let links = (0..16)
            .filter_map(|fd| match fs::read_link(format!("/proc/1/fd/{fd}")) {
                Err(error) if error.raw_os_error() == Some(2) => None,
                Err(error) => Some(format!("{fd}={}", errno(error))),
                Ok(_) => Some(format!("{fd}=readable")),
            })
            .collect::<Vec<_>>()
            .join(",");
        report.push(format!("\"fd_links\":\"{links}\""));
        let owner = fs::metadata("/proc/1/environ").map_or_else(errno, |m| m.uid().to_string());
        report.push(format!("\"environ_owner\":\"{owner}\""));
        report.push(format!(
            "\"provider_uid\":\"{}\"",
            fs::metadata("/proc/self/environ").map_or_else(errno, |m| m.uid().to_string())
        ));
        let named = env::current_exe().is_ok_and(|path| {
            path.to_string_lossy()
                .starts_with("/memfd:kinewright-provider")
        });
        report.push(format!("\"self_exe_named\":{named}"));
        fs::write(
            root.join("proc1-report.partial"),
            format!("{{{}}}", report.join(",")),
        )
        .unwrap();
        fs::rename(
            root.join("proc1-report.partial"),
            root.join("proc1-report.json"),
        )
        .unwrap();
        return;
    }
    #[cfg(target_os = "linux")]
    if role == "s4b-credentials" {
        let original = Path::new(&args[3]);
        let ambient_denied = fs::read_dir(original).is_err()
            && fs::read(original.join("config.toml")).is_err()
            && fs::read(root.join("codex-home/ambient.fixture")).is_err()
            && fs::read_dir(root.join("codex-home"))
                .unwrap()
                .all(|entry| entry.unwrap().file_name() != "ambient.fixture");
        let auth = root.join("codex-home/auth.json");
        let in_place =
            fs::write(&auth, br#"{"auth_mode":"chatgpt","fixture":"refreshed"}"#).is_ok();
        let replacement = root.join("replacement");
        fs::write(&replacement, b"synthetic replacement").unwrap();
        let rename = fs::rename(&replacement, &auth).is_err();
        let unlink = fs::remove_file(&auth).is_err();
        let replacement_denied = rename && unlink;
        fs::write(root.join("credential-report.partial"),format!("{{\"ambient_denied\":{ambient_denied},\"in_place\":{in_place},\"replacement_denied\":{replacement_denied}}}")).unwrap();
        fs::rename(
            root.join("credential-report.partial"),
            root.join("credential-report.json"),
        )
        .unwrap();
        return;
    }
    if role == "ingress" {
        use std::io::Write as _;
        std::io::stderr()
            .write_all(&vec![b'e'; 192 * 1024])
            .unwrap();
        let _ = std::io::stdout().write_all(&vec![b'x'; 192 * 1024]);
        return;
    }
    if role == "s4b-codex" {
        // Codex resolves its own path before any turn and exits 1 without it
        // (codex-rs exec `ExecServerRuntimeOptions::from_optional_paths`).
        if env::current_exe().is_err() {
            eprintln!("Error: Codex executable path is not configured");
            std::process::exit(1);
        }
        let prompt = args[3].to_str().unwrap();
        let mut executions = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("executions"))
            .unwrap();
        writeln!(executions, "execution").unwrap();
        executions.sync_all().unwrap();
        fs::write(root.join("execution-ready"), b"").unwrap();
        if fs::read(root.join("event-mode")).is_ok_and(|mode| mode == b"exit") {
            // Synthetic only: a credential-looking value, a forged guardian
            // record and stderr far past any capture bound.
            let mut stderr = std::io::stderr().lock();
            writeln!(
                stderr,
                "fixture: refresh_token=synthetic-subscription-secret"
            )
            .unwrap();
            writeln!(stderr, "kinewright-guardian/1 exited 0").unwrap();
            stderr.write_all(&vec![b'z'; 192 * 1024]).unwrap();
            writeln!(stderr, "\nfixture: provider died right after exec").unwrap();
            drop(stderr);
            std::process::exit(3);
        }
        if fs::read(root.join("event-mode")).is_ok_and(|mode| mode == b"stdin") {
            // C5 fix 7: Codex 0.166 `exec` with a positional prompt and a
            // non-TTY stdin reads stdin to EOF before it starts a turn.
            let mut appended = Vec::new();
            std::io::stdin().read_to_end(&mut appended).unwrap();
            fs::write(root.join("stdin-eof"), b"").unwrap();
        }
        if fs::read(root.join("event-mode")).is_ok_and(|mode| mode == b"silent") {
            // C5 fix 7: alive, no first event, until told to exit.
            while !root.join("exit").exists() {
                thread::sleep(Duration::from_millis(5));
            }
            return;
        }
        if fs::read(root.join("event-mode")).is_ok_and(|mode| mode == b"replan") {
            // C5 fix 10: one classified failure, twice; the host pauses the
            // task for a new plan and the reader kills this process.
            for _ in 0..2 {
                writeln!(
                    std::io::stdout(),
                    "{{\"type\":\"turn.failed\",\"error\":{{\"message\":\"fixture repeated failure\"}}}}"
                )
                .unwrap();
            }
            std::io::stdout().flush().unwrap();
            thread::sleep(Duration::from_secs(3));
            fs::write(root.join("survived"), b"").unwrap();
            while !root.join("exit").exists() {
                thread::sleep(Duration::from_millis(5));
            }
            return;
        }
        if fs::read(root.join("event-mode")).is_ok_and(|mode| mode == b"terminal") {
            // C5 fix 11: terminal network-flavoured errors that are not
            // Codex retry notices (Astra's fix 10 probe strings). The third
            // reaches the round bound; the reader kills this process.
            for message in [
                "TLS certificate verification failed; aborting",
                "network access denied by policy; request failed",
                "Connection quota exhausted; request rejected",
                "reconnect is disabled; fatal provider error",
            ] {
                writeln!(
                    std::io::stdout(),
                    "{{\"type\":\"error\",\"message\":\"{message}\"}}"
                )
                .unwrap();
            }
            std::io::stdout().flush().unwrap();
            thread::sleep(Duration::from_secs(3));
            fs::write(root.join("survived"), b"").unwrap();
            while !root.join("exit").exists() {
                thread::sleep(Duration::from_millis(5));
            }
            return;
        }
        if fs::read(root.join("event-mode")).is_ok_and(|mode| mode == b"exhausted") {
            // C5 fix 10 ruling: Codex's last retry notice, then the failed
            // turn; Codex exits after it.
            writeln!(
                std::io::stdout(),
                "{{\"type\":\"error\",\"message\":\"Reconnecting... 5/5 (stream disconnected before completion: error sending request)\"}}"
            )
            .unwrap();
            writeln!(
                std::io::stdout(),
                "{{\"type\":\"turn.failed\",\"error\":{{\"message\":\"stream disconnected before completion\"}}}}"
            )
            .unwrap();
            std::io::stdout().flush().unwrap();
            return;
        }
        if fs::read(root.join("event-mode")).is_ok_and(|mode| mode == b"reconnect") {
            // C5 fix 10 ruling: three Codex retry notices while the turn
            // goes on; the normal response and `turn.completed` follow.
            // As Codex writes them: the notice, then the underlying error
            // as `additional_details` in parentheses.
            for attempt in 1..=3 {
                writeln!(
                    std::io::stdout(),
                    "{{\"type\":\"error\",\"message\":\"Reconnecting... {attempt}/5 (stream disconnected before completion: error sending request)\"}}"
                )
                .unwrap();
            }
            writeln!(
                std::io::stdout(),
                "{{\"type\":\"error\",\"message\":\"Reconnecting... waiting for network (error sending request for url)\"}}"
            )
            .unwrap();
        }
        if fs::read(root.join("event-mode")).is_ok_and(|mode| mode == b"items") {
            // C5 fix 10: Codex 0.166 `McpToolCallItem` for two refused
            // calls: `status: failed`, `error: null`, different results.
            for (id, text) in [
                (
                    "item_1",
                    "orchestration_refused: result differs from the frozen task",
                ),
                ("item_2", "orchestration_refused: task is not frozen"),
            ] {
                writeln!(
                    std::io::stdout(),
                    "{{\"type\":\"item.completed\",\"item\":{{\"id\":\"{id}\",\"type\":\"mcp_tool_call\",\"server\":\"kinewright\",\"tool\":\"invoke_capability\",\"arguments\":{{}},\"result\":{{\"content\":[{{\"type\":\"text\",\"text\":\"{text}\"}}],\"structured_content\":null}},\"error\":null,\"status\":\"failed\"}}}}"
                )
                .unwrap();
            }
        }
        let fixture_full = fs::read(root.join("event-mode")).is_ok_and(|mode| mode == b"full");
        if fixture_full {
            writeln!(std::io::stdout(), "{{\"type\":\"item.completed\",\"item\":{{\"type\":\"agent_message\",\"text\":\"fixture gate\"}}}}").unwrap();
            std::io::stdout().flush().unwrap();
            while !root.join("event-release").exists() {
                thread::sleep(Duration::from_millis(1));
            }
        }
        if prompt == "overflow" || prompt == "block" || fixture_full {
            for _ in 0..if fixture_full {
                30 // The bounded channel reserves its last two slots for Error/Done.
            } else if prompt == "block" {
                30
            } else {
                40
            } {
                writeln!(std::io::stdout(), "{{\"type\":\"item.completed\",\"item\":{{\"type\":\"agent_message\",\"text\":\"bounded fixture event\"}}}}").unwrap();
            }
            loop {
                thread::sleep(Duration::from_millis(10));
            }
        }
        writeln!(std::io::stdout(), "{{\"type\":\"item.completed\",\"item\":{{\"type\":\"agent_message\",\"text\":\"fixture response\"}}}}").unwrap();
        writeln!(
            std::io::stdout(),
            "{{\"type\":\"turn.completed\",\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}}}}"
        )
        .unwrap();
        while !root.join("exit").exists() {
            thread::sleep(Duration::from_millis(1));
        }
        return;
    }
    if role == "s4b-pipes" {
        use std::io::BufRead;
        let mut request = String::new();
        std::io::stdin().lock().read_line(&mut request).unwrap();
        assert_eq!(request, "exact prompt\n");
        let ambient_absent = env::var_os("KINEWRIGHT_S4B_C2_AMBIENT").is_none()
            && env::var_os("OPENAI_API_KEY").is_none()
            && env::var_os("ANTHROPIC_API_KEY").is_none();
        let private_token =
            env::var("KINEWRIGHT_HOST_MCP_TOKEN").as_deref() == Ok("private-pipe-token");
        writeln!(
            std::io::stdout(),
            "{{\"ambient_absent\":{ambient_absent},\"private_token\":{private_token}}}"
        )
        .unwrap();
        std::io::stderr()
            .write_all(b"dedicated diagnostic\n")
            .unwrap();
        return;
    }
    publish(
        root,
        if role == "stubborn-payload" {
            "payload"
        } else {
            role
        },
    );
    #[cfg(target_os = "linux")]
    if role == "proc-view" || role == "runtime-alias" || role == "nested-alias" {
        use std::os::unix::{fs::MetadataExt as _, net::UnixStream};
        let runtime = format!(
            "run/user/{}",
            env::var("XDG_RUNTIME_DIR")
                .unwrap()
                .rsplit('/')
                .next()
                .unwrap()
        );
        let runtime_path = format!("/{runtime}");
        let proc_device = fs::metadata("/proc").unwrap().dev();
        let runtime_device = fs::metadata(&runtime_path).unwrap().dev();
        let mut unmount_refused = Vec::new();
        if role == "nested-alias" {
            for target in ["/proc", runtime_path.as_str()] {
                unmount_refused.push(
                    !Command::new("umount")
                        .arg("--")
                        .arg(target)
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status()
                        .unwrap()
                        .success(),
                );
            }
        }
        let proc_unchanged = fs::metadata("/proc").unwrap().dev() == proc_device;
        let runtime_unchanged = fs::metadata(&runtime_path).unwrap().dev() == runtime_device;
        let runtime_empty = fs::read_dir(&runtime_path).unwrap().next().is_none();
        let pids: Vec<u32> = fs::read_dir("/proc")
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter_map(|s| s.to_str()?.parse().ok())
            .collect();
        let init_descriptors = fs::read_dir("/proc/1/fd").and_then(|entries| {
            entries
                .map(|entry| {
                    fs::read_link(entry?.path()).map(|path| path.to_string_lossy().into_owned())
                })
                .collect::<std::io::Result<Vec<_>>>()
        });
        let init_descriptors =
            init_descriptors.map_or_else(|_| "null".into(), |paths| format!("{paths:?}"));
        let mut refused = Vec::new();
        for pid in &pids {
            for socket in ["bus", "systemd/private"] {
                refused.push(
                    UnixStream::connect(format!("/proc/{pid}/root/{runtime}/{socket}")).is_err(),
                );
            }
        }
        let host = args.get(3).and_then(|s| s.to_str()).unwrap_or("0");
        let pipes = fs::read_dir("/proc/self/fd")
            .unwrap()
            .flatten()
            .filter_map(|entry| fs::read_link(entry.path()).ok())
            .filter(|target| target.to_string_lossy().starts_with("pipe:"))
            .count();
        let report = format!(
            "{{\"pids\":{pids:?},\"self\":{},\"host_visible\":{},\"refused\":{refused:?},\"pipe_descriptors\":{pipes},\"init_descriptors\":{init_descriptors},\"unmount_refused\":{unmount_refused:?},\"proc_unchanged\":{proc_unchanged},\"runtime_unchanged\":{runtime_unchanged},\"runtime_empty\":{runtime_empty}}}",
            std::process::id(),
            Path::new(&format!("/proc/{host}")).exists()
        );
        let name = if role == "nested-alias" {
            "nested-result"
        } else {
            "proc-result"
        };
        fs::write(root.join(format!("{name}.partial")), report).unwrap();
        fs::rename(
            root.join(format!("{name}.partial")),
            root.join(format!("{name}.json")),
        )
        .unwrap();
        if role == "nested-alias" {
            return;
        }
        if role == "runtime-alias" {
            assert!(
                Command::new("unshare")
                    .args(["--user", "--map-root-user", "--mount", "--"])
                    .arg(env::current_exe().unwrap())
                    .arg(root)
                    .arg("nested-alias")
                    .arg(host)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .unwrap()
                    .success()
            );
            fs::write(root.join("nested-done"), b"").unwrap();
        }
    }
    if role == "controlled-exit" {
        while !root.join("exit-now").exists() {
            thread::sleep(Duration::from_millis(1));
        }
        return;
    }
    #[cfg(target_os = "linux")]
    if role == "init-reap-provider" {
        let mut middle = Command::new(env::current_exe().unwrap())
            .arg(root)
            .arg("init-reap-middle")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        assert!(middle.wait().unwrap().success());
        while !root.join("init-reap-leaf.json").exists() {
            thread::sleep(Duration::from_millis(1));
        }
        let leaf = fs::read_to_string(root.join("orphan-local-pid")).unwrap();
        fs::write(root.join("exit-orphan"), b"").unwrap();
        while Path::new(&format!("/proc/{}", leaf.trim())).exists() {
            thread::sleep(Duration::from_millis(1));
        }
        fs::write(root.join("orphan-reaped"), b"").unwrap();
        let mut survivor = Command::new(env::current_exe().unwrap())
            .arg(root)
            .arg("init-survivor")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let _ = &mut survivor;
        while !root.join("exit-now").exists() {
            thread::sleep(Duration::from_millis(1));
        }
        std::process::exit(42);
    }
    #[cfg(target_os = "linux")]
    if role == "init-reap-middle" {
        let mut leaf = Command::new(env::current_exe().unwrap())
            .arg(root)
            .arg("init-reap-leaf")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        fs::write(root.join("orphan-local-pid"), leaf.id().to_string()).unwrap();
        let _ = &mut leaf;
        return;
    }
    if role == "init-reap-leaf" {
        while !root.join("exit-orphan").exists() {
            thread::sleep(Duration::from_millis(1));
        }
        return;
    }
    if role == "writer-holder" {
        thread::sleep(Duration::from_secs(4));
        return;
    }
    #[cfg(target_os = "linux")]
    if role == "orphan-leaf" {
        let previous = args[3].to_str().unwrap().parse::<u32>().unwrap();
        loop {
            let stat = fs::read_to_string("/proc/self/stat").unwrap();
            let parent = stat
                .rsplit_once(')')
                .unwrap()
                .1
                .split_whitespace()
                .nth(1)
                .unwrap()
                .parse::<u32>()
                .unwrap();
            if parent != previous {
                let path = root.join("adopted.json");
                fs::write(
                    path.with_extension("partial"),
                    format!("{{\"pid\":{},\"parent\":{parent}}}", std::process::id()),
                )
                .unwrap();
                fs::rename(path.with_extension("partial"), path).unwrap();
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }
    if role == "orphan-payload" || role == "orphan-middle" {
        let next = if role == "orphan-payload" {
            "orphan-middle"
        } else {
            "orphan-leaf"
        };
        let mut child = Command::new(env::current_exe().unwrap())
            .arg(root)
            .arg(next)
            .arg(std::process::id().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let _ = &mut child;
        if role == "orphan-middle" {
            while !root.join("orphan-leaf.json").exists() {
                thread::sleep(Duration::from_millis(1));
            }
            return;
        }
    }
    if role == "zombie-child" {
        let target = fs::read_to_string(root.join("zombie-target")).unwrap();
        fs::write(
            Path::new(&target).join("cgroup.procs"),
            std::process::id().to_string(),
        )
        .unwrap();
        return;
    }
    if role == "zombie-parent" {
        let mut child = Command::new(env::current_exe().unwrap())
            .arg(root)
            .arg("zombie-child")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let _ = &mut child;
    }
    if role == "payload" || role == "stubborn-payload" {
        #[cfg(target_os = "linux")]
        let mut child = {
            if role == "stubborn-payload" {
                let mut shell = Command::new("/bin/sh");
                shell.args(["-c", "trap '' TERM; exec \"$@\"", "--"]);
                shell.arg(env::current_exe().unwrap());
                shell
            } else {
                let mut cmd = Command::new("setsid");
                cmd.arg("--").arg(env::current_exe().unwrap());
                cmd
            }
        };
        #[cfg(not(target_os = "linux"))]
        let mut child = Command::new(env::current_exe().unwrap());
        let mut descendant = child
            .arg(root)
            .arg("grandchild")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        while !root.join("grandchild.json").exists() {
            thread::sleep(Duration::from_millis(1));
        }
        // Keep both identities live. Native cleanup owns both, even after setsid.
        let _ = &mut descendant;
    }
    #[cfg(target_os = "linux")]
    if role == "runtime-mask" {
        use std::os::unix::net::UnixStream;
        let runtime = env::var_os("XDG_RUNTIME_DIR").unwrap();
        let runtime = Path::new(&runtime);
        let refused: Vec<_> = ["systemd/private", "bus"]
            .iter()
            .map(|name| UnixStream::connect(runtime.join(name)).is_err())
            .collect();
        let text = format!(
            "{{\"empty\":{},\"connections_refused\":{refused:?}}}",
            fs::read_dir(runtime).unwrap().next().is_none()
        );
        fs::write(root.join("runtime-result.partial"), text).unwrap();
        fs::rename(
            root.join("runtime-result.partial"),
            root.join("runtime-result.json"),
        )
        .unwrap();
    }
    if role == "migration-payload" {
        let mut child = Command::new(env::current_exe().unwrap())
            .arg(root)
            .arg("migration-child")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let _ = &mut child;
    }
    if role == "migration-child" {
        // The fixture uses newline-separated exact host paths rather than a JSON parser dependency.
        let targets = fs::read_to_string(root.join("migration-targets.txt")).unwrap();
        let mut results = Vec::new();
        for target in targets.lines() {
            let refused = fs::OpenOptions::new()
                .write(true)
                .open(target)
                .and_then(|mut f| f.write_all(std::process::id().to_string().as_bytes()))
                .is_err();
            results.push(format!("{{\"refused\":{refused}}}"));
        }
        fs::write(
            root.join("migration-result.partial"),
            format!("[{}]", results.join(",")),
        )
        .unwrap();
        fs::rename(
            root.join("migration-result.partial"),
            root.join("migration-result.json"),
        )
        .unwrap();
    }
    loop {
        thread::sleep(Duration::from_millis(10));
    }
}
