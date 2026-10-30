//! Additive C8 observations. The absent old host is pending, never a behavioral red.
use crate::writer_host::{
    HostAuth, HostClient, HostCommand, HostConfig, HostOrigin, SessionRoots, WriterHost,
};
use kinewright_core::{Core, Document};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn observe(root: &Path, revised: bool) -> Result<Value, String> {
    let core = Core::spawn(Document::default()).map_err(|e| e.to_string())?;
    let local_control = core.export_history().map_err(|e| e.to_string())?.document
        == std::sync::Arc::new(Document::default());
    if !revised {
        return Ok(json!({"control":local_control,"surface":"pending_baseline_witness"}));
    }
    let host = WriterHost::start(HostConfig {
        scheduler_settings: None,
        project: "s0-c8-host-auth-control".into(),
        path: None,
        roots: SessionRoots {
            read: vec![root.to_path_buf()],
            write: vec![root.to_path_buf()],
        },
        origin: HostOrigin::Headless,
    })
    .map_err(|e| e.to_string())?;
    let client = host.client();
    let control = client
        .request(HostCommand::Status)
        .map_err(|e| e.to_string())?
        .error
        .is_none();
    let wrong = HostClient::connect(
        client.endpoint().clone(),
        HostAuth::generate().map_err(|e| e.to_string())?,
        client.bootstrap().roots,
    )
    .map_err(|e| e.to_string())?;
    let refused = wrong.request(HostCommand::ReadHistory).is_err();
    host.shutdown().map_err(|e| e.to_string())?;
    Ok(json!({"control":local_control && control,"auth_precedes_read":control && refused}))
}
