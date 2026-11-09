//! Arm-specific measurement observations. No surface translation or dispatch.
use crate::orchestration::orchestration_records::Role;
use serde_json::{Value, json};
use std::io;
pub(crate) fn arm() -> &'static str {
    "candidate"
}
/// B3's only surface-specific expressions are the two calls below: observe
/// that arm's actually served definitions and its actual provider prefixes.
pub(crate) fn roles() -> io::Result<Value> {
    let mut surfaces = serde_json::Map::new();
    for role in [
        Role::Director,
        Role::Editor,
        Role::Reviewer,
        Role::Motion,
        Role::Audio,
        Role::Colour,
    ] {
        let tools = crate::server::task_role_tools(role).map_err(io::Error::other)?;
        let prefix = super::task_context::application_prefix(role).map_err(io::Error::other)?;
        surfaces.insert(
            serde_json::to_value(role)?.as_str().unwrap().into(),
            json!({"tools":tools,"prefix":String::from_utf8(prefix).map_err(io::Error::other)?}),
        );
    }
    Ok(Value::Object(surfaces))
}
