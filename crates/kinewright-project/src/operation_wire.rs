//! Encoded JSON crosses tagged Serde envelopes as text. The exact inner bytes
//! remain the identity and validation input; no typed payload is built here.
use serde::{Deserialize, Deserializer, Serializer};
use serde_json::value::RawValue;

pub(crate) fn serialize<S: Serializer>(value: &RawValue, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(value.get())
}
pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Box<RawValue>, D::Error> {
    RawValue::from_string(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
}
