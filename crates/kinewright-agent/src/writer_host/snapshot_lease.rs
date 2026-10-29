//! Attachment-owned immutable snapshot leases. No writer or mutable path is lent.
use super::{
    Arc, Duration, HOST_BODY_BYTES, HOST_PROTOCOL, HostClient, HostCommand, HostCommandOwner,
    HostError, HostRefusal, HostReply, HostRequest, HttpState, IntoResponse, Request, Response,
    State, StatusCode, check_host_headers, to_bytes,
};
use kinewright_project::{storage_budget::StorageReservation, store::StorePin};

pub(super) struct LeasePin {
    payload_sha256: String,
    owner_epoch: u64,
    _pin: Option<StorePin>,
    _charge: StorageReservation,
}
pub(super) fn is_lease_command(command: &HostCommand) -> bool {
    matches!(
        command,
        HostCommand::LeaseSnapshot { .. }
            | HostCommand::SnapshotLeaseStatus { .. }
            | HostCommand::ReleaseSnapshot { .. }
    )
}
impl HostCommandOwner {
    pub(super) fn execute_snapshot_lease(
        &mut self,
        index: usize,
        command: HostCommand,
        roots: &super::SessionRoots,
    ) -> HostReply {
        let mut reply = HostReply::default();
        let result = (|| -> Result<(), HostError> {
            if !matches!(command, HostCommand::ReleaseSnapshot { .. }) {
                self.check_named_access(roots, false)?;
            }
            match command {
                HostCommand::LeaseSnapshot { key } => {
                    if self.attachments[index].snapshot_leases.len() >= 64 {
                        return Err(HostRefusal::new(
                            "capacity",
                            "64 snapshot leases per attachment maximum",
                        )
                        .into());
                    }
                    let mut random = [0_u8; 32];
                    getrandom::fill(&mut random).map_err(|e| HostError::Protocol(e.to_string()))?;
                    let id = kinewright_media::sha256_bytes(&random);
                    let (lease, pinned) = self.owner()?.lease_snapshot(&key, id.clone())?;
                    let encoded = serde_json::to_vec(&lease)?;
                    if encoded.len() > HOST_BODY_BYTES / 2 {
                        return Err(HostRefusal::new(
                            "capacity",
                            "snapshot lease graph exceeds reply capacity",
                        )
                        .into());
                    }
                    let charge = self
                        .owner()?
                        .reserve_snapshot_lease(encoded.len() as u64 * 3)?;
                    let owner_epoch = self.owner()?.store()?.owner_epoch();
                    self.attachments[index].snapshot_leases.insert(
                        id,
                        LeasePin {
                            payload_sha256: lease.payload_sha256.clone(),
                            owner_epoch,
                            _pin: Some(pinned.pin),
                            _charge: charge,
                        },
                    );
                    reply.snapshot_lease = Some(lease);
                }
                HostCommand::SnapshotLeaseStatus { id, payload_sha256 } => {
                    self.check_snapshot_lease(index, &id, &payload_sha256)?;
                }
                HostCommand::ReleaseSnapshot { id, payload_sha256 } => {
                    self.check_snapshot_lease(index, &id, &payload_sha256)?;
                    self.attachments[index].snapshot_leases.remove(&id);
                }
                _ => {
                    return Err(HostRefusal::new(
                        "lease_command",
                        "snapshot lease command required",
                    )
                    .into());
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            reply.error = Some(error.refusal());
        }
        reply.status = self.status();
        reply
    }
    fn check_snapshot_lease(&self, index: usize, id: &str, digest: &str) -> Result<(), HostError> {
        self.owner()?.check_snapshot_authority()?;
        let owner_epoch = self.owner()?.store()?.owner_epoch();
        if id.len() != 64
            || digest.len() != 64
            || self.attachments[index]
                .snapshot_leases
                .get(id)
                .is_none_or(|lease| {
                    lease.payload_sha256 != digest || lease.owner_epoch != owner_epoch
                })
        {
            return Err(
                HostRefusal::new("lease_revoked", "snapshot lease ended or graph changed").into(),
            );
        }
        Ok(())
    }
}

/// Pooled, bounded liveness transport. It carries no mutable store capability.
#[derive(Clone)]
pub struct SnapshotLeaseRemote {
    http: reqwest::blocking::Client,
    url: String,
    auth: String,
    project: String,
    status: Vec<u8>,
    release: Vec<u8>,
    cache: Arc<std::sync::Mutex<LeaseCache>>,
}
struct LeaseCache {
    fresh_until: std::time::Instant,
    revoked: bool,
    retry_after: std::time::Instant,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotLeaseError {
    TemporarilyUnavailable,
    Ended,
}
enum ProbeFailure {
    Busy,
    Ended,
}
impl SnapshotLeaseRemote {
    fn send(&self, body: &[u8]) -> Result<(), ProbeFailure> {
        let response = self
            .http
            .post(&self.url)
            .header("authorization", &self.auth)
            .header("x-kinewright-project", &self.project)
            .header("x-kinewright-protocol", HOST_PROTOCOL.to_string())
            .header("content-type", "application/json")
            .body(body.to_vec())
            .send()
            .map_err(|error| {
                if error.is_connect() && !error.is_timeout() {
                    ProbeFailure::Ended
                } else {
                    ProbeFailure::Busy
                }
            })?;
        if response.status() == StatusCode::NO_CONTENT {
            Ok(())
        } else if response.status() == StatusCode::TOO_MANY_REQUESTS
            || response.status().is_server_error()
        {
            Err(ProbeFailure::Busy)
        } else {
            Err(ProbeFailure::Ended)
        }
    }
    pub fn check(&self) -> Result<(), String> {
        self.send(&self.status)
            .map_err(|_| "snapshot lease unavailable".into())
    }
    /// Only a successful probe authorizes reads for 250 ms. Busy probes retain
    /// the lease for retry without extending access. Refusal or host death ends it.
    pub fn check_cached(&self) -> Result<(), SnapshotLeaseError> {
        let mut cache = self.cache.lock().map_err(|_| SnapshotLeaseError::Ended)?;
        if cache.revoked {
            return Err(SnapshotLeaseError::Ended);
        }
        if std::time::Instant::now() < cache.fresh_until {
            return Ok(());
        }
        if std::time::Instant::now() < cache.retry_after {
            return Err(SnapshotLeaseError::TemporarilyUnavailable);
        }
        let probe = self.send(&self.status);
        cache.retry_after = std::time::Instant::now() + Duration::from_millis(50);
        match probe {
            Ok(()) => {
                cache.fresh_until = std::time::Instant::now() + Duration::from_millis(250);
                Ok(())
            }
            Err(ProbeFailure::Busy) => Err(SnapshotLeaseError::TemporarilyUnavailable),
            Err(ProbeFailure::Ended) => {
                cache.revoked = true;
                Err(SnapshotLeaseError::Ended)
            }
        }
    }
    pub fn release(&self) -> Result<(), String> {
        if let Ok(mut cache) = self.cache.lock() {
            cache.revoked = true;
        }
        self.send(&self.release)
            .map_err(|_| "snapshot lease release unavailable".into())
    }
}
impl HostClient {
    pub fn snapshot_lease_remote(
        &self,
        id: &str,
        digest: &str,
    ) -> Result<SnapshotLeaseRemote, HostError> {
        let request = |command| HostRequest {
            editor: None,
            operation: None,
            protocol: HOST_PROTOCOL,
            project: self.endpoint().project.clone(),
            roots: self.bootstrap.roots.clone(),
            command,
        };
        Ok(SnapshotLeaseRemote {
            cache: Arc::new(std::sync::Mutex::new(LeaseCache {
                fresh_until: std::time::Instant::now(),
                revoked: false,
                retry_after: std::time::Instant::now(),
            })),
            http: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(2))
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .build()
                .map_err(|_| HostError::Protocol("snapshot lease transport".into()))?,
            url: format!("http://{}/snapshot-lease", self.endpoint().address),
            auth: self.bootstrap.auth.bearer(),
            project: self.endpoint().project.clone(),
            status: serde_json::to_vec(&request(HostCommand::SnapshotLeaseStatus {
                id: id.into(),
                payload_sha256: digest.into(),
            }))?,
            release: serde_json::to_vec(&request(HostCommand::ReleaseSnapshot {
                id: id.into(),
                payload_sha256: digest.into(),
            }))?,
        })
    }
}
pub(super) async fn lease_request(
    State(state): State<Arc<HttpState>>,
    request: Request,
) -> Response {
    let bearer = match check_host_headers(request.headers(), &state) {
        Ok(b) => b,
        Err(s) => return s.into_response(),
    };
    let Ok(permit) = Arc::clone(&state.lease_ingress).try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let bytes = match tokio::time::timeout(
        Duration::from_secs(15),
        to_bytes(request.into_body(), 16 << 10),
    )
    .await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
        Err(_) => return StatusCode::REQUEST_TIMEOUT.into_response(),
    };
    let Ok(request) = serde_json::from_slice::<HostRequest>(&bytes) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if !matches!(
        request.command,
        HostCommand::SnapshotLeaseStatus { .. } | HostCommand::ReleaseSnapshot { .. }
    ) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let mut owner = state.owner.lock().map_err(|_| ())?;
        let (index, _, roots) = owner
            .authorize_request(Some(&bearer), &request)
            .map_err(|_| ())?;
        let reply = owner.execute_snapshot_lease(index, request.command, &roots);
        if reply.error.is_none() {
            Ok(())
        } else {
            Err(())
        }
    })
    .await;
    if matches!(result, Ok(Ok(()))) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::FORBIDDEN
    }
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fix2_only_successful_status_extends_positive_freshness() {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for (code, delay) in [(204, 0), (503, 0), (204, 300), (204, 0)] {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut input = Vec::new();
                let mut byte = [0];
                while !input.ends_with(b"\r\n\r\n") {
                    assert_eq!(socket.read(&mut byte).unwrap(), 1);
                    input.push(byte[0]);
                }
                let mut body = [0; 2];
                socket.read_exact(&mut body).unwrap();
                std::thread::sleep(Duration::from_millis(delay));
                let _ = write!(
                    socket,
                    "HTTP/1.1 {code} Status\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
            }
        });
        let now = std::time::Instant::now();
        let remote = SnapshotLeaseRemote {
            http: reqwest::blocking::Client::builder()
                .timeout(Duration::from_millis(150))
                .no_proxy()
                .build()
                .unwrap(),
            url: format!("http://{address}/snapshot-lease"),
            auth: "test".into(),
            project: "test".into(),
            status: b"{}".to_vec(),
            release: b"{}".to_vec(),
            cache: Arc::new(std::sync::Mutex::new(LeaseCache {
                fresh_until: now,
                revoked: false,
                retry_after: now,
            })),
        };
        remote.check_cached().unwrap();
        let positive = remote.cache.lock().unwrap().fresh_until;
        std::thread::sleep(Duration::from_millis(260));
        assert_eq!(
            remote.check_cached(),
            Err(SnapshotLeaseError::TemporarilyUnavailable)
        );
        assert_eq!(
            remote.cache.lock().unwrap().fresh_until,
            positive,
            "FIX2_503_NEVER_RENEWS"
        );
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(
            remote.check_cached(),
            Err(SnapshotLeaseError::TemporarilyUnavailable)
        );
        assert_eq!(
            remote.cache.lock().unwrap().fresh_until,
            positive,
            "FIX2_TIMEOUT_NEVER_RENEWS"
        );
        std::thread::sleep(Duration::from_millis(310));
        remote.check_cached().unwrap();
        assert!(remote.cache.lock().unwrap().fresh_until > positive);
        server.join().unwrap();
    }
}
