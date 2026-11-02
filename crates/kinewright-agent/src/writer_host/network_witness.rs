//! Credential-free control and contained runs of the exact native network policy.
use super::{ProbeOwner, guardian};
use crate::{
    job_guardian::protocol::{AttemptIdentity, Program, ProviderSandbox},
    orchestration::adapters::{
        CODEX_AUTH_CONTRACT, CredentialFile, CredentialStoreScope, ExecutableIdentity,
    },
};
use std::{
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

struct ProbeRoot(PathBuf);
impl Drop for ProbeRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Compare a fixed unpaid probe outside containment and inside the native policy.
///
/// # Panics
/// Refuses any failed network stage or incomplete native cleanup.
#[doc(hidden)]
pub fn run_installed_codex_network_witness() {
    assert!(
        !crate::native_launcher::DenyProviderEgress::engaged(),
        "C5C paid-policy probe under the dry policy"
    );
    let (control, contained, _) = probe_reports("network");
    eprintln!(
        "C5C_UNPAID control={} contained={} roots={}/{} cleanup=true",
        control.passed(),
        contained.passed(),
        control.roots,
        contained.roots
    );
    assert!(control.passed(), "C5C_NETWORK_CONTROL");
    assert!(contained.passed(), "C5C_NETWORK_CONTAINED");
}

/// C5 fix 8: the dry launch's preflight on its own. It runs only the probe
/// binary and synthetic credentials, never Codex. The label names the
/// report under `target/s4b-c5/`.
///
/// # Panics
/// Refuses unless egress is provably denied and the sign-in provably
/// read-only.
#[doc(hidden)]
pub fn run_unpaid_policy_witness() {
    let label = std::env::var("KINEWRIGHT_S4B_C5_DRY_LABEL").unwrap_or_else(|_| "unpaid".into());
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/s4b-c5");
    fs::create_dir_all(&out).unwrap();
    let deny = crate::native_launcher::DenyProviderEgress::engage();
    let evidence = std::panic::catch_unwind(run_dry_network_witness);
    drop(deny);
    let passed = evidence.is_ok();
    fs::write(
        out.join(format!("unpaid-policy-{label}.json")),
        serde_json::to_vec_pretty(&serde_json::json!({
            "passed": passed,
            "evidence": evidence.as_ref().ok(),
        }))
        .unwrap(),
    )
    .unwrap();
    if let Err(panic) = evidence {
        std::panic::resume_unwind(panic);
    }
    eprintln!("C5_FIX8_UNPAID_POLICY passed=true label={label}");
}

/// C5 fix 7 and fix 8: before the dry launch, the probe runs in the same
/// unpaid policy, under the guardian, with a synthetic sign-in mounted, and
/// proves every route out of the machine is refused while loopback MCP still
/// reaches the host through init's relay, and that the mounted sign-in and
/// its store refuse every write. The control outside containment proves the
/// same routes exist. Any failure stops before the real launch.
///
/// # Panics
/// Refuses unless egress is provably denied and the sign-in provably
/// read-only.
pub(super) fn run_dry_network_witness() -> serde_json::Value {
    assert!(
        crate::native_launcher::DenyProviderEgress::engaged(),
        "C5 fix7 dry probe without the dry policy"
    );
    let lan = lan_address();
    let mcp = Listener::mcp();
    let port = mcp.port;
    let lan_tcp =
        Listener::count(std::net::TcpListener::bind((lan, port)).expect("C5_FIX8_LAN_TCP"));
    let lan_udp = UdpCount::bind(lan);
    let udp_port = lan_udp.port;
    let common = move || vec![port.to_string(), lan.to_string(), udp_port.to_string()];
    let control_arguments = [vec!["unpaid-control".to_owned()], common()].concat();
    let probe: Probe<crate::network_probe::UnpaidReport> = run_probe(
        "dry-network",
        port,
        &control_arguments,
        |credential, identity| {
            [
                vec!["unpaid".to_owned()],
                common(),
                vec![
                    credential.to_string_lossy().into_owned(),
                    format!("{}:{}", identity[0], identity[1]),
                ],
            ]
            .concat()
        },
        || (mcp.seen(), lan_tcp.seen(), lan_udp.seen()),
    );
    let (control, contained) = (&probe.control, &probe.contained);
    let [after_control, after_contained] = probe.counts;
    let unpaid = probe.program.sandbox.unpaid().cloned();
    let connect_tcp = match &probe.program.sandbox {
        ProviderSandbox::LinuxAllowlist { connect_tcp, .. } => connect_tcp.clone(),
        ProviderSandbox::ContainmentOnly => vec![],
    };
    let control_routes = control
        .open
        .iter()
        .chain(&control.closed)
        .all(|r| r.reached)
        && after_control.0 == 1
        && after_control.1 == 1
        && after_control.2 >= 1;
    let relay =
        contained.open.iter().all(|r| r.reached) && after_contained.0 == after_control.0 + 1;
    let closed = contained.closed.iter().all(|r| !r.reached)
        && after_contained.1 == after_control.1
        && after_contained.2 == after_control.2;
    let hosts = crate::network_probe::HOSTS.iter().all(|host| {
        contained
            .base
            .hosts
            .iter()
            .any(|check| check.host == *host && !check.tcp_443 && !check.tls)
    });
    // C5 fix 8 ruling: exactly this shape. The sign-in is a read-only bind,
    // the private per-attempt home around it is writable, and the home holds
    // nothing else of the person's.
    let credential = contained.credential_read_only == Some(true)
        && contained.store_read_only == Some(false)
        && contained.credential_readable == Some(true)
        && contained.home_refusal.is_none()
        && contained
            .home_entries
            .iter()
            .filter(|entry| entry.ends_with("(read-only bind of the sign-in)"))
            .count()
            == 1
        && contained.credential.len() == 7
        && contained.credential.iter().all(|check| check.refused)
        && probe.source_unchanged;
    eprintln!(
        "C5_FIX8_DRY_EGRESS control={} control_routes={control_routes} relay={relay} closed={closed} hosts_refused={hosts} credential_read_only={credential} connect_tcp={connect_tcp:?} unpaid={unpaid:?}",
        control.base.passed()
    );
    for route in contained.open.iter().chain(&contained.closed) {
        eprintln!(
            "C5_FIX8_ROUTE {} reached={} errno={:?}",
            route.route, route.reached, route.errno
        );
    }
    eprintln!(
        "C5_FIX8_HOME entries={:?} refusal={:?}",
        contained.home_entries, contained.home_refusal
    );
    for check in &contained.credential {
        eprintln!(
            "C5_FIX8_CREDENTIAL {} refused={} errno={:?}",
            check.operation, check.refused, check.errno
        );
    }
    assert!(control.base.passed(), "C5_FIX7_DRY_CONTROL");
    assert!(control_routes, "C5_FIX8_CONTROL_ROUTES {after_control:?}");
    // What the probe observed decides first; the policy shape after.
    assert!(relay, "C5_FIX8_LOOPBACK_RELAY {after_contained:?}");
    assert!(closed, "C5_FIX8_EGRESS_NOT_REFUSED {after_contained:?}");
    assert!(hosts, "C5_FIX7_DRY_EGRESS_NOT_REFUSED");
    assert!(credential, "C5_FIX8_CREDENTIAL_WRITABLE");
    assert_eq!(
        unpaid.map(|policy| policy.mcp_port),
        Some(port),
        "C5_FIX8_UNPAID_POLICY"
    );
    assert_eq!(connect_tcp, [port], "C5_FIX7_DRY_POLICY_443");
    serde_json::json!({
        "connect_tcp": connect_tcp,
        "network_namespace": "loopback only",
        "loopback_mcp_relayed": contained.open.iter().map(|r| r.route.clone()).collect::<Vec<_>>(),
        "refused": contained.closed.iter().map(|r| (r.route.clone(), r.errno)).collect::<std::collections::BTreeMap<_, _>>(),
        "tcp_443_refused": crate::network_probe::HOSTS,
        "contained_failures": contained.base.hosts.iter().map(|check| (check.host.clone(), check.failure.clone())).collect::<std::collections::BTreeMap<_, _>>(),
        "control_passed": true,
        "control_reached": control.open.iter().chain(&control.closed).map(|r| r.route.clone()).collect::<Vec<_>>(),
        "synthetic_credential": {
            "mount_read_only": true,
            "private_home_writable": true,
            "home_entries": contained.home_entries,
            "refused": contained.credential.iter().map(|c| (c.operation.clone(), c.errno)).collect::<std::collections::BTreeMap<_, _>>(),
            "source_unchanged": true,
        },
    })
}

/// A non-loopback address of this machine: the source address the kernel
/// would pick for a documentation address. Nothing is sent.
fn lan_address() -> std::net::IpAddr {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").expect("C5_FIX8_LAN_ADDRESS");
    socket
        .connect("203.0.113.1:9")
        .expect("C5_FIX8_LAN_ADDRESS");
    let address = socket.local_addr().expect("C5_FIX8_LAN_ADDRESS").ip();
    assert!(
        !address.is_loopback() && !address.is_unspecified(),
        "C5_FIX8_LAN_ADDRESS"
    );
    address
}

/// A TCP listener the witness owns. The MCP stand-in answers the probe's
/// request; the other only counts.
struct Listener {
    port: u16,
    seen: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    address: std::net::SocketAddr,
}
impl Listener {
    fn mcp() -> Self {
        Self::start(
            std::net::TcpListener::bind("127.0.0.1:0").expect("C5_FIX8_MCP"),
            true,
        )
    }
    fn count(listener: std::net::TcpListener) -> Self {
        Self::start(listener, false)
    }
    fn start(listener: std::net::TcpListener, answer: bool) -> Self {
        use std::io::{Read, Write};
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (count, done) = (Arc::clone(&seen), Arc::clone(&stop));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if done.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                count.fetch_add(1, Ordering::SeqCst);
                if answer {
                    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(10)));
                    let mut request = vec![0; crate::network_probe::RELAY_REQUEST.len()];
                    if stream.read_exact(&mut request).is_ok()
                        && request == crate::network_probe::RELAY_REQUEST
                    {
                        let _ = stream.write_all(crate::network_probe::RELAY_REPLY);
                    }
                }
            }
        });
        Self {
            port: address.port(),
            seen,
            stop,
            address,
        }
    }
    fn seen(&self) -> usize {
        self.seen.load(Ordering::SeqCst)
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(self.address);
    }
}

/// A UDP socket the witness owns on the non-loopback address; counts datagrams.
struct UdpCount {
    port: u16,
    seen: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}
impl UdpCount {
    fn bind(address: std::net::IpAddr) -> Self {
        let socket = std::net::UdpSocket::bind((address, 0)).expect("C5_FIX8_LAN_UDP");
        socket
            .set_read_timeout(Some(std::time::Duration::from_millis(50)))
            .unwrap();
        let port = socket.local_addr().unwrap().port();
        let seen = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (count, done) = (Arc::clone(&seen), Arc::clone(&stop));
        std::thread::spawn(move || {
            let mut buffer = [0; 64];
            while !done.load(Ordering::SeqCst) {
                if socket.recv_from(&mut buffer).is_ok() {
                    count.fetch_add(1, Ordering::SeqCst);
                }
            }
        });
        Self { port, seen, stop }
    }
    fn seen(&self) -> usize {
        // A datagram the control sent may still be in flight.
        std::thread::sleep(std::time::Duration::from_millis(200));
        self.seen.load(Ordering::SeqCst)
    }
}
impl Drop for UdpCount {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

struct Probe<T> {
    control: T,
    contained: T,
    program: Program,
    counts: [(usize, usize, usize); 2],
    source_unchanged: bool,
}

fn probe_reports(
    label: &str,
) -> (
    crate::network_probe::Report,
    crate::network_probe::Report,
    Vec<u16>,
) {
    let endpoint = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let probe: Probe<crate::network_probe::Report> = run_probe(
        label,
        endpoint.local_addr().unwrap().port(),
        &[],
        |_, _| vec![],
        || (0, 0, 0),
    );
    let connect_tcp = match &probe.program.sandbox {
        ProviderSandbox::LinuxAllowlist { connect_tcp, .. } => connect_tcp.clone(),
        ProviderSandbox::ContainmentOnly => vec![],
    };
    drop(endpoint);
    (probe.control, probe.contained, connect_tcp)
}

fn credential_state(path: &std::path::Path) -> (Vec<u8>, [i64; 6]) {
    let meta = fs::symlink_metadata(path).unwrap();
    (
        fs::read(path).unwrap(),
        [
            meta.dev().cast_signed(),
            meta.ino().cast_signed(),
            meta.size().cast_signed(),
            meta.mtime_nsec() + meta.mtime() * 1_000_000_000,
            meta.ctime_nsec() + meta.ctime() * 1_000_000_000,
            i64::from(meta.mode()),
        ],
    )
}

#[allow(
    clippy::too_many_lines,
    reason = "one probe crosses the same guardian barriers as a launch"
)]
fn run_probe<T: serde::de::DeserializeOwned + serde::Serialize>(
    label: &str,
    mcp_port: u16,
    control_arguments: &[String],
    contained_arguments: impl FnOnce(&std::path::Path, [u64; 2]) -> Vec<String>,
    counts: impl Fn() -> (usize, usize, usize),
) -> Probe<T> {
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/s4b-c5c");
    fs::create_dir_all(&out).unwrap();
    let root = ProbeRoot(std::env::temp_dir().join(format!("n5-{}", std::process::id())));
    fs::create_dir(&root.0).unwrap();
    let scratch = root.0.join("s");
    fs::create_dir(&scratch).unwrap();
    fs::set_permissions(&scratch, fs::Permissions::from_mode(0o700)).unwrap();
    let home = crate::codex_credentials::prepare_home(&scratch).unwrap();
    fs::create_dir(root.0.join("source")).unwrap();
    let source = root.0.join("source/auth.json");
    // Synthetic: never a real sign-in.
    fs::write(&source, br#"{"auth_mode":"chatgpt"}"#).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
    let before = credential_state(&source);
    let meta = fs::metadata(&source).unwrap();
    let identity = [meta.dev(), meta.ino()];
    let scope = CredentialStoreScope {
        root: home.clone(),
        single_file: Some(CredentialFile {
            source: source.clone(),
            identity: [meta.dev(), meta.ino()],
        }),
        credential_files: vec!["auth.json".into()],
        lock_files: vec![],
        temporary_file_prefixes: vec![],
        refresh: CODEX_AUTH_CONTRACT.refresh,
    };
    let probe = guardian().with_file_name("kinewright-network-probe");
    let meta = fs::metadata(&probe).unwrap();
    let codex = crate::codex_qualification::executable().unwrap();
    let executable = ExecutableIdentity {
        path: probe.clone(),
        file_identity: [meta.dev(), meta.ino()],
    };
    let mut sandbox =
        crate::native_launcher::sandbox(&codex, &scratch, &scope, mcp_port, true).unwrap();
    if let ProviderSandbox::LinuxAllowlist {
        executable_identity,
        ..
    } = &mut sandbox
    {
        *executable_identity = executable.file_identity;
    }
    let unpaid = sandbox.unpaid().map(|policy| policy.mcp_port);
    let environment = crate::native_launcher::environment(&scratch, &home);
    let control = std::process::Command::new(&probe)
        .args(control_arguments)
        .env_clear()
        .envs(&environment)
        .current_dir(&scratch)
        .output()
        .unwrap();
    assert!(control.status.success(), "C5C control probe process");
    let control: T = serde_json::from_slice(&control.stdout).unwrap();
    let after_control = counts();
    fs::write(
        out.join(format!("{label}-control.json")),
        serde_json::to_vec_pretty(&control).unwrap(),
    )
    .unwrap();
    let pipes = crate::provider_pipes::ProviderPipeListeners::bind(&scratch.join("io")).unwrap();
    let pipes = if unpaid.is_some() {
        pipes.with_relay().unwrap()
    } else {
        pipes
    };
    let mut program = Program {
        image: None,
        credential_root: None,
        executable: probe,
        arguments: contained_arguments(&home.join("auth.json"), identity),
        environment,
        working_directory: scratch.clone(),
        pipe_directory: Some(pipes.directory().into()),
        sandbox,
    };
    let _image = program.fixture_image().unwrap();
    fs::write(
        out.join(format!("{label}-policy.json")),
        serde_json::to_vec_pretty(&program).unwrap(),
    )
    .unwrap();
    let fresh = || {
        kinewright_project::bundle_records::SessionId::fresh()
            .unwrap()
            .as_str()
            .to_owned()
    };
    let id = AttemptIdentity {
        project: fresh(),
        run: fresh(),
        task: fresh(),
        attempt: fresh(),
        nonce: fresh(),
        owner_instance: fresh(),
    };
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &root.0.join("claim"),
        &id.project,
        &id.run,
    )
    .unwrap();
    let mut owner = ProbeOwner(root.0.clone());
    let mut session = crate::guardian::ContainedSession::prepare(
        &mut claim,
        &mut owner,
        id,
        program.clone(),
        &guardian(),
        &crate::guardian::CertifiedProfile::linux_allowlist(),
    )
    .unwrap();
    let mut pipes = pipes.accept().unwrap();
    if let Some(port) = unpaid {
        pipes.start_relay(port).unwrap();
    }
    pipes.credentials(b"").unwrap();
    let reader = pipes.into_reader();
    reader
        .output
        .set_read_timeout(Some(std::time::Duration::from_secs(120)))
        .unwrap();
    session.commit(&mut owner).unwrap();
    let crate::provider_pipes::ProviderReader {
        output,
        diagnostics,
        guardian: _status,
        relay,
    } = reader;
    let diagnostics = crate::protocol::limits::stderr_capture(diagnostics, "c5c-probe").unwrap();
    let mut bytes = Vec::new();
    let read = output.take(65537).read_to_end(&mut bytes);
    session.close_lifetime();
    let proof = session.cleanup().unwrap();
    claim.retire(&proof).unwrap();
    claim.finish().unwrap();
    drop(relay);
    let _ = diagnostics.join().unwrap();
    read.unwrap();
    let contained: T = serde_json::from_slice(&bytes).unwrap();
    let after_contained = counts();
    fs::write(
        out.join(format!("{label}-contained.json")),
        serde_json::to_vec_pretty(&contained).unwrap(),
    )
    .unwrap();
    let source_unchanged = credential_state(&source) == before;
    Probe {
        control,
        contained,
        program,
        counts: [after_control, after_contained],
        source_unchanged,
    }
}
