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
