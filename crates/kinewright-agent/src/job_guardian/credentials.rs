//! Shared mode-only inspection, including the final provider spawn boundary.
use serde::de::{IgnoredAny, MapAccess, Visitor};
use std::{
    fmt,
    io::{self, Read},
    os::unix::fs::MetadataExt,
    path::Path,
};
struct AuthFields {
    mode: bool,
    api_key: bool,
}
impl<'de> serde::Deserialize<'de> for AuthFields {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Fields;
        impl<'de> Visitor<'de> for Fields {
            type Value = AuthFields;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("auth object")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut mode = None;
                let mut api_key = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "auth_mode" if mode.is_none() => {
                            let value = map.next_value::<Option<String>>()?;
                            mode = Some(matches!(value.as_deref(), Some("chatgpt")));
                        }
                        "OPENAI_API_KEY" if api_key.is_none() => {
                            api_key = Some(map.next_value::<Option<IgnoredAny>>()?.is_some());
                        }
                        "auth_mode" | "OPENAI_API_KEY" => {
                            return Err(serde::de::Error::custom("duplicate auth field"));
                        }
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(AuthFields {
                    mode: mode.unwrap_or(false),
                    api_key: api_key.unwrap_or(false),
                })
            }
        }
        deserializer.deserialize_map(Fields)
    }
}

pub(crate) fn inspect_subscription(
    path: &Path,
    identity: Option<[u64; 2]>,
) -> io::Result<[u64; 2]> {
    let file = kinewright_job_sys::pin_credential_file(path, identity)?;
    if file.metadata()?.len() > 64 * 1024 {
        return Err(io::Error::other("Sign in: credential file capacity"));
    }
    let metadata = file.metadata()?;
    // IgnoredAny skips all token values. Errors deliberately discard parser input/location details.
    let mut parser = serde_json::Deserializer::from_reader(file.take(64 * 1024 + 1));
    let fields: AuthFields = serde::Deserialize::deserialize(&mut parser)
        .map_err(|_| io::Error::other("Sign in: credential file unreadable"))?;
    parser
        .end()
        .map_err(|_| io::Error::other("Sign in: credential file unreadable"))?;
    if !fields.mode || fields.api_key {
        return Err(io::Error::other(
            "Sign in: subscription authentication required",
        ));
    }
    Ok([metadata.dev(), metadata.ino()])
}
