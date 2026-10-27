//! Fixed native observer protocol fixture. It never reads or writes a project.
use std::{
    env, fs,
    io::Write,
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
        use std::os::unix::net::UnixStream;
        let pids: Vec<u32> = fs::read_dir("/proc")
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter_map(|s| s.to_str()?.parse().ok())
            .collect();
        let runtime = format!(
            "run/user/{}",
            env::var("XDG_RUNTIME_DIR")
                .unwrap()
                .rsplit('/')
                .next()
                .unwrap()
        );
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
            "{{\"pids\":{pids:?},\"self\":{},\"host_visible\":{},\"refused\":{refused:?},\"pipe_descriptors\":{pipes}}}",
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
                    .args(["--user", "--map-root-user", "--"])
                    .arg(env::current_exe().unwrap())
                    .arg(root)
                    .arg("nested-alias")
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
