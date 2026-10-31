//! Unpaid DNS/TCP/TLS diagnostics. Never sends a model request or reads credentials.
#[cfg(target_os = "linux")]
fn main() {
    let report = kinewright_agent::network_probe::inspect();
    println!("{}", serde_json::to_string(&report).expect("probe report"));
}
#[cfg(not(target_os = "linux"))]
fn main() {}
