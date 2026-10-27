//! Fixed native observer protocol fixture. It never reads or writes a project.
use std::{
    env, fs,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::Duration,
};
fn publish(root: &Path, role: &str) {
    let text = format!(
        "{{\"version\":1,\"role\":\"{role}\",\"pid\":{}}}",
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
    if role == "writer-holder" {
        thread::sleep(Duration::from_secs(4));
        return;
    }
    publish(root, role);
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
    if role == "payload" {
        #[cfg(target_os = "linux")]
        let mut child = {
            let mut cmd = Command::new("setsid");
            cmd.arg("--").arg(env::current_exe().unwrap());
            cmd
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
    loop {
        thread::sleep(Duration::from_millis(10));
    }
}
