//! Unpaid DNS/TCP/TLS diagnostics. Never sends a model request or reads credentials.
#[cfg(target_os = "linux")]
fn main() {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let report = if arguments.is_empty() {
        serde_json::to_string(&kinewright_agent::network_probe::inspect())
    } else {
        // C5 fix 8: the unpaid policy's egress and read-only sign-in checks.
        let targets = kinewright_agent::network_probe::UnpaidTargets::parse(&arguments)
            .expect("unpaid probe arguments");
        serde_json::to_string(&kinewright_agent::network_probe::inspect_unpaid(&targets))
    };
    println!("{}", report.expect("probe report"));
}
#[cfg(not(target_os = "linux"))]
fn main() {}
