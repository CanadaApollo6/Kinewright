//! S4b C6 version-probe fixture. A probe passes only `--version`, so each copy
//! picks its behaviour from its own file name (`argv[0]`). Compiled with
//! `rustc --edition 2024` by the version-probe tests.
use std::io::Write;

fn main() {
    let argv0 = std::env::args().next().unwrap_or_default();
    let path = std::path::Path::new(&argv0);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let mut out = std::io::stdout();
    let print = |out: &mut std::io::Stdout, bytes: &[u8]| {
        let _ = out.write_all(bytes);
        let _ = out.flush();
    };
    match name {
        "codex-ok" => print(&mut out, b"codex-cli 0.166.0\n"),
        "claude-ok" => print(&mut out, b"2.1.317 (Claude Code)\n"),
        "claude-bare" => print(&mut out, b"2.1.317\n"),
        "codex-banner" => print(
            &mut out,
            b"codex-cli 0.165.9-beta (upgrade to 0.200.0 recommended)\n",
        ),
        "codex-beta" => print(&mut out, b"codex-cli 0.166.0-beta.1\n"),
        "codex-lines" => print(&mut out, b"codex-cli 0.166.0\ncodex-cli 0.200.0\n"),
        "codex-bytes" => print(&mut out, b"codex-cli 0.166.\xff\n"),
        "codex-malformed" => print(&mut out, b"codex-cli version unknown\n"),
        "codex-fails" => {
            print(&mut out, b"codex-cli 0.166.0\n");
            std::process::exit(1);
        }
        "codex-flood" => {
            for _ in 0..64 {
                print(&mut out, &[b'x'; 1024]);
            }
        }
        "codex-hang" => std::thread::sleep(std::time::Duration::from_secs(3600)),
        "codex-descendant" => {
            print(&mut out, b"codex-cli 0.166.0\n");
            // The descendant inherits stdout and keeps it open.
            let _ = std::process::Command::new("/usr/bin/sleep")
                .arg("30")
                .spawn();
        }
        escape if escape.starts_with("codex-escape-") => {
            // Outside its private home: next to the fixture itself.
            let _ = std::fs::write(path.with_file_name("escaped"), b"escaped");
            if let Ok(port) = escape["codex-escape-".len()..].parse::<u16>() {
                let _ = std::net::TcpStream::connect_timeout(
                    &std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port)),
                    std::time::Duration::from_secs(1),
                );
            }
            print(&mut out, b"codex-cli 0.166.0\n");
        }
        _ => std::process::exit(2),
    }
}
