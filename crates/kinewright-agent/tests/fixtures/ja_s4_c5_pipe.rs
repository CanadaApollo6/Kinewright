//! Original Kinewright pipe fixture. It never opens a project or reads a reply.
use std::{
    fs,
    io::{self, BufRead, Write},
    path::Path,
    thread,
    time::{Duration, Instant},
};

fn main() {
    let arguments: Vec<_> = std::env::args_os().collect();
    let mut prompt = String::new();
    io::stdin().lock().read_line(&mut prompt).unwrap();
    if arguments[1] == "reply" {
        println!("provider replied");
        return;
    }
    let root = Path::new(&arguments[2]);
    println!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{}}}}");
    println!(
        "{{\"jsonrpc\":\"2.0\",\"method\":\"item/completed\",\"params\":{{\"sessionId\":\"blocked\",\"item\":{{\"kind\":\"agentMessage\",\"text\":\"accepted before fault\"}}}}}}"
    );
    for _ in 0..4 {
        println!(
            "{{\"jsonrpc\":\"2.0\",\"id\":\"{}\",\"method\":\"unknown/request\"}}",
            "r".repeat(60 * 1024)
        );
    }
    io::stdout().flush().unwrap();
    fs::write(root.join("request-sent"), b"").unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !root.join("overflow").exists() {
        assert!(Instant::now() < deadline, "overflow trigger missing");
        thread::sleep(Duration::from_millis(1));
    }
    for ordinal in 0..40 {
        println!(
            "{{\"jsonrpc\":\"2.0\",\"method\":\"noise\",\"params\":{{\"ordinal\":{ordinal}}}}}"
        );
    }
    io::stdout().flush().unwrap();
    thread::sleep(Duration::from_secs(30));
}
