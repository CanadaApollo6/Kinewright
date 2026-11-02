//! Fixed, credential-free network checks for the installed Codex transport.
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    net::{TcpStream, ToSocketAddrs},
    path::Path,
    sync::Arc,
    time::Duration,
};

pub const HOSTS: [&str; 3] = ["chatgpt.com", "api.openai.com", "auth.openai.com"];
#[derive(Debug, Serialize, Deserialize)]
pub struct FileCheck {
    pub path: String,
    pub readable: bool,
    pub errno: Option<i32>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct HostCheck {
    pub host: String,
    pub getaddrinfo: bool,
    pub tcp_443: bool,
    pub tls: bool,
    pub failure: Option<String>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    pub files: Vec<FileCheck>,
    pub roots: usize,
    pub root_errors: usize,
    pub hosts: Vec<HostCheck>,
    pub environment_presence: std::collections::BTreeMap<String, bool>,
}
impl Report {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.files.iter().all(|f| f.readable)
            && self.roots > 0
            && self
                .hosts
                .iter()
                .all(|h| h.getaddrinfo && h.tcp_443 && h.tls)
    }
}
fn file_check(path: &Path) -> FileCheck {
    let result = fs::File::open(path).and_then(|mut file| {
        use std::io::Read;
        file.read(&mut [0; 1]).map(|_| ())
    });
    FileCheck {
        path: path.to_string_lossy().into_owned(),
        readable: result.is_ok(),
        errno: result.err().and_then(|e| e.raw_os_error()),
    }
}
/// Read public network configuration and perform authenticated TLS handshakes only.
#[must_use]
pub fn inspect() -> Report {
    let mut files = Vec::new();
    for path in [
        "/etc/resolv.conf",
        "/etc/hosts",
        "/etc/nsswitch.conf",
        "/etc/gai.conf",
        "/etc/ssl/certs/ca-certificates.crt",
    ] {
        files.push(file_check(Path::new(path)));
        if let Ok(target) = fs::canonicalize(path)
            && target != Path::new(path)
        {
            files.push(file_check(&target));
        }
    }
    let native = rustls_native_certs::load_native_certs();
    let root_errors = native.errors.len();
    let mut roots = rustls::RootCertStore::empty();
    let _ = roots.add_parsable_certificates(native.certs);
    let root_count = roots.len();
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    );
    let hosts = HOSTS
        .into_iter()
        .map(|host| host_check(host, &config))
        .collect();
    let environment_presence = [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "CODEX_CA_CERTIFICATE",
    ]
    .into_iter()
    .map(|key| (key.to_owned(), std::env::var_os(key).is_some()))
    .collect();
    Report {
        files,
        roots: root_count,
        root_errors,
        hosts,
        environment_presence,
    }
}
fn host_check(host: &str, config: &Arc<rustls::ClientConfig>) -> HostCheck {
    let mut report = HostCheck {
        host: host.into(),
        getaddrinfo: false,
        tcp_443: false,
        tls: false,
        failure: None,
    };
    let Ok(values) = (host, 443).to_socket_addrs() else {
        report.failure = Some("getaddrinfo".into());
        return report;
    };
    let addresses = values.take(16).collect::<Vec<_>>();
    report.getaddrinfo = !addresses.is_empty();
    let Some(mut stream) = addresses
        .iter()
        .find_map(|address| TcpStream::connect_timeout(address, Duration::from_secs(5)).ok())
    else {
        report.failure = Some("tcp_connect".into());
        return report;
    };
    report.tcp_443 = true;
    let result = (|| -> io::Result<()> {
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;
        let name = host.to_owned().try_into().map_err(io::Error::other)?;
        let mut connection =
            rustls::ClientConnection::new(config.clone(), name).map_err(io::Error::other)?;
        while connection.is_handshaking() {
            connection.complete_io(&mut stream)?;
        }
        Ok(())
    })();
    report.tls = result.is_ok();
    if !report.tls {
        report.failure = Some("tls_handshake".into());
    }
    report
}

/// C5 fix 8: the reply the unpaid witness's stand-in MCP listener sends.
pub const RELAY_REPLY: &[u8] = b"kinewright-relay-pong\n";
/// C5 fix 8: what the probe sends first on the loopback MCP port.
pub const RELAY_REQUEST: &[u8] = b"kinewright-relay-ping\n";
/// Documentation addresses: no host answers them.
const TEST_NET_V4: &str = "203.0.113.1";
const TEST_NET_V6: &str = "2001:db8::1";

/// One route the unpaid probe tried. `reached` is a TCP connect, or for
/// loopback MCP the full reply, or for UDP an accepted send.
#[derive(Debug, Serialize, Deserialize)]
pub struct RouteCheck {
    pub route: String,
    pub reached: bool,
    pub errno: Option<i32>,
}
/// One write the unpaid probe tried on the mounted synthetic sign-in.
#[derive(Debug, Serialize, Deserialize)]
pub struct CredentialCheck {
    pub operation: String,
    pub refused: bool,
    pub errno: Option<i32>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct UnpaidReport {
    pub base: Report,
    /// Routes that must stay open: the loopback MCP port.
    pub open: Vec<RouteCheck>,
    /// Routes that must fail inside the unpaid policy.
    pub closed: Vec<RouteCheck>,
    pub credential: Vec<CredentialCheck>,
    pub credential_read_only: Option<bool>,
    pub store_read_only: Option<bool>,
    pub credential_readable: Option<bool>,
    /// The private home as the shared unpaid check sees it from inside.
    pub home_entries: Vec<String>,
    pub home_refusal: Option<String>,
}

/// What the unpaid probe is given: the MCP port, a non-loopback address of
/// this machine with listeners the witness owns, and in the contained run the
/// mounted credential file and the sign-in's identity.
#[derive(Debug)]
pub struct UnpaidTargets {
    /// Outside containment: only the routes the witness owns a listener
    /// for, so the control proves they exist without sending elsewhere.
    pub control: bool,
    pub mcp_port: u16,
    pub lan: std::net::IpAddr,
    pub lan_udp: u16,
    pub credential: Option<std::path::PathBuf>,
    pub identity: Option<[u64; 2]>,
}
impl UnpaidTargets {
    /// `unpaid|unpaid-control <mcp port> <lan address> <lan udp port>
    /// [<credential> <device>:<inode>]`
    ///
    /// # Errors
    /// Refuses any other argument list.
    pub fn parse(arguments: &[String]) -> io::Result<Self> {
        let bad = || io::Error::other("unpaid probe arguments");
        match arguments {
            [mode, port, lan, udp, rest @ ..]
                if matches!(mode.as_str(), "unpaid" | "unpaid-control")
                    && rest.len() != 1
                    && rest.len() <= 2 =>
            {
                Ok(Self {
                    control: mode == "unpaid-control",
                    mcp_port: port.parse().map_err(|_| bad())?,
                    lan: lan.parse().map_err(|_| bad())?,
                    lan_udp: udp.parse().map_err(|_| bad())?,
                    credential: rest.first().map(Into::into),
                    identity: rest
                        .get(1)
                        .map(|value| {
                            let (device, inode) = value.split_once(':').ok_or_else(bad)?;
                            Ok::<_, io::Error>([
                                device.parse().map_err(|_| bad())?,
                                inode.parse().map_err(|_| bad())?,
                            ])
                        })
                        .transpose()?,
                })
            }
            _ => Err(bad()),
        }
    }
}

fn tcp(route: &str, address: std::net::SocketAddr) -> RouteCheck {
    let result = TcpStream::connect_timeout(&address, Duration::from_secs(3));
    RouteCheck {
        route: route.into(),
        reached: result.is_ok(),
        errno: result.err().and_then(|e| e.raw_os_error()),
    }
}
fn udp(route: &str, address: std::net::SocketAddr) -> RouteCheck {
    let bind: std::net::SocketAddr = if address.is_ipv4() {
        (std::net::Ipv4Addr::UNSPECIFIED, 0).into()
    } else {
        (std::net::Ipv6Addr::UNSPECIFIED, 0).into()
    };
    let result = std::net::UdpSocket::bind(bind)
        .and_then(|socket| socket.send_to(b"kinewright-unpaid-probe", address));
    RouteCheck {
        route: route.into(),
        reached: result.is_ok(),
        errno: result.err().and_then(|e| e.raw_os_error()),
    }
}
fn loopback_mcp(port: u16) -> RouteCheck {
    use std::io::{Read, Write};
    let result = (|| -> io::Result<()> {
        let mut stream = TcpStream::connect_timeout(
            &(std::net::Ipv4Addr::LOCALHOST, port).into(),
            Duration::from_secs(3),
        )?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.write_all(RELAY_REQUEST)?;
        let mut reply = vec![0; RELAY_REPLY.len()];
        stream.read_exact(&mut reply)?;
        if reply != RELAY_REPLY {
            return Err(io::Error::other("relay reply"));
        }
        Ok(())
    })();
    RouteCheck {
        route: format!("tcp 127.0.0.1:{port} (MCP)"),
        reached: result.is_ok(),
        errno: result.err().and_then(|e| e.raw_os_error()),
    }
}

/// Every route a provider could use to leave the machine, plus the one it
/// must keep. Inside the unpaid policy only the loopback MCP port answers.
#[must_use]
pub fn inspect_unpaid(targets: &UnpaidTargets) -> UnpaidReport {
    use std::net::{IpAddr, SocketAddr};
    let port = targets.mcp_port;
    let v4: IpAddr = TEST_NET_V4.parse().expect("documentation address");
    let v6: IpAddr = TEST_NET_V6.parse().expect("documentation address");
    let lan = targets.lan;
    let open = vec![loopback_mcp(port)];
    if targets.control {
        return UnpaidReport {
            base: inspect(),
            open,
            closed: vec![
                tcp(
                    &format!("tcp {lan}:{port} (MCP port, non-loopback)"),
                    SocketAddr::new(lan, port),
                ),
                udp(
                    &format!("udp {lan}:{} (listener)", targets.lan_udp),
                    SocketAddr::new(lan, targets.lan_udp),
                ),
            ],
            credential: vec![],
            credential_read_only: None,
            store_read_only: None,
            credential_readable: None,
            home_entries: vec![],
            home_refusal: None,
        };
    }
    let mut closed = vec![
        tcp(
            &format!("tcp {lan}:{port} (MCP port, non-loopback)"),
            SocketAddr::new(lan, port),
        ),
        tcp(&format!("tcp {lan}:443"), SocketAddr::new(lan, 443)),
        tcp(&format!("tcp {v4}:443"), SocketAddr::new(v4, 443)),
        tcp(&format!("tcp {v4}:{port}"), SocketAddr::new(v4, port)),
        tcp(&format!("tcp [{v6}]:443"), SocketAddr::new(v6, 443)),
        tcp(&format!("tcp [{v6}]:{port}"), SocketAddr::new(v6, port)),
        tcp(
            &format!("tcp [::1]:{port}"),
            SocketAddr::new(std::net::Ipv6Addr::LOCALHOST.into(), port),
        ),
        udp(
            &format!("udp {lan}:{} (listener)", targets.lan_udp),
            SocketAddr::new(lan, targets.lan_udp),
        ),
        udp(&format!("udp {lan}:443"), SocketAddr::new(lan, 443)),
        udp(&format!("udp {lan}:53"), SocketAddr::new(lan, 53)),
        udp(&format!("udp {v4}:443"), SocketAddr::new(v4, 443)),
        udp(&format!("udp {v4}:53"), SocketAddr::new(v4, 53)),
        udp(&format!("udp [{v6}]:443"), SocketAddr::new(v6, 443)),
        udp(&format!("udp [{v6}]:53"), SocketAddr::new(v6, 53)),
    ];
    // Any configured resolver that is not loopback. A loopback resolver
    // inside the namespace is the namespace's own, empty loopback.
    for server in fs::read_to_string("/etc/resolv.conf")
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.strip_prefix("nameserver"))
        .filter_map(|value| value.trim().parse::<IpAddr>().ok())
        .filter(|server| !server.is_loopback())
        .take(4)
    {
        closed.push(udp(
            &format!("udp {server}:53 (resolver)"),
            SocketAddr::new(server, 53),
        ));
        closed.push(tcp(
            &format!("tcp {server}:53 (resolver)"),
            SocketAddr::new(server, 53),
        ));
    }
    let mut report = UnpaidReport {
        base: inspect(),
        open,
        closed,
        credential: vec![],
        credential_read_only: None,
        store_read_only: None,
        credential_readable: None,
        home_entries: vec![],
        home_refusal: None,
    };
    if let (Some(credential), Some(identity)) = (&targets.credential, targets.identity) {
        credential_checks(credential, identity, &mut report);
    }
    report
}

type Attempt<'a> = Box<dyn Fn() -> io::Result<()> + 'a>;

/// List the private home with the shared unpaid check, then try every way
/// to change the mounted sign-in. Never reads its bytes.
fn credential_checks(credential: &Path, identity: [u64; 2], report: &mut UnpaidReport) {
    let home = credential.parent().unwrap_or(credential);
    match kinewright_job_sys::verify_unpaid_home(home, identity) {
        Ok(entries) => report.home_entries = entries,
        Err(error) => report.home_refusal = Some(error.to_string()),
    }
    report.credential_read_only = kinewright_job_sys::read_only(credential).ok();
    report.store_read_only = kinewright_job_sys::read_only(home).ok();
    report.credential_readable = Some(fs::File::open(credential).is_ok());
    // A refresh writes a file beside the sign-in, then renames it over.
    let replacement = home.join("auth.json.kinewright-replacement");
    let _ = fs::write(&replacement, b"{}");
    let attempts: [(&str, Attempt<'_>); 7] = [
        (
            "write",
            Box::new(|| {
                use std::io::Write;
                fs::OpenOptions::new()
                    .write(true)
                    .open(credential)
                    .and_then(|mut file| file.write_all(b"{}"))
            }),
        ),
        (
            "append",
            Box::new(|| {
                use std::io::Write;
                fs::OpenOptions::new()
                    .append(true)
                    .open(credential)
                    .and_then(|mut file| file.write_all(b" "))
            }),
        ),
        (
            "truncate",
            Box::new(|| {
                fs::OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(credential)
                    .map(drop)
            }),
        ),
        (
            "rename over",
            Box::new(|| fs::rename(&replacement, credential)),
        ),
        (
            "rename away",
            Box::new(|| fs::rename(credential, home.join("auth.json.moved"))),
        ),
        ("remove", Box::new(|| fs::remove_file(credential))),
        (
            "chmod",
            Box::new(|| {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(credential, fs::Permissions::from_mode(0o644))
            }),
        ),
    ];
    for (operation, attempt) in attempts {
        let result = attempt();
        report.credential.push(CredentialCheck {
            operation: operation.into(),
            refused: result.is_err(),
            errno: result.err().and_then(|e| e.raw_os_error()),
        });
    }
    let _ = fs::remove_file(&replacement);
}
