//! A private bootstrap capability reissues only the original Editor attachment.
use super::*;

#[derive(Clone)]
pub struct HostIssuer {
    client: HostClient,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReissuedEditor {
    bootstrap: HostBootstrap,
    manual_client: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reattach {}
impl std::fmt::Debug for HostIssuer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("HostIssuer([REDACTED])")
    }
}
impl HostIssuer {
    pub(super) fn new(
        endpoint: HostEndpoint,
        auth: HostAuth,
        roots: SessionRoots,
    ) -> Result<Self, HostError> {
        Ok(Self {
            client: HostClient::connect(endpoint, auth, roots)?,
        })
    }
    /// This capability is never part of an agent bootstrap or project record.
    pub fn reattach_editor(&self) -> Result<HostClient, HostError> {
        let private: ReissuedEditor = serde_json::from_slice(&self.client.post("issuer", b"{}")?)?;
        let bootstrap = private.bootstrap;
        let mut client = HostClient::connect(bootstrap.endpoint, bootstrap.auth, bootstrap.roots)?;
        client.editor = private.manual_client;
        Ok(client)
    }
}
pub(super) async fn issuer_request(
    State(state): State<Arc<HttpState>>,
    request: Request,
) -> Response {
    let credential = request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok());
    if !state
        .issuer
        .as_ref()
        .is_some_and(|issuer| issuer.accepts(credential))
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if request
        .headers()
        .get("x-kinewright-project")
        .and_then(|v| v.to_str().ok())
        != Some(state.bootstrap.endpoint.project.as_str())
        || request
            .headers()
            .get("x-kinewright-protocol")
            .and_then(|v| v.to_str().ok())
            != Some("1")
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(permit) = Arc::clone(&state.ingress).try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let Ok(body) = to_bytes(request.into_body(), 32).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    if serde_json::from_slice::<Reattach>(&body).is_err() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let reply = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let mut owner = state.owner.lock().map_err(|_| ())?;
        let roots = owner.admitted_roots();
        let auth = HostAuth::generate().map_err(|_| ())?;
        owner
            .register_attachment(
                auth.clone(),
                "private-bootstrap".into(),
                AttachmentRole::Editor,
                roots.clone(),
                true,
            )
            .map_err(|_| ())?;
        let manual_client = owner
            .manual
            .as_ref()
            .map(|lease| lease.client.clone())
            .or_else(|| {
                owner
                    .foreground_owner
                    .as_ref()
                    .map(|held| held.client.clone())
            });
        Ok::<_, ()>(ReissuedEditor {
            manual_client,
            bootstrap: HostBootstrap {
                endpoint: state.bootstrap.endpoint.clone(),
                auth,
                roots,
            },
        })
    })
    .await;
    match reply {
        Ok(Ok(private)) => json_response(&private),
        _ => StatusCode::FORBIDDEN.into_response(),
    }
}
