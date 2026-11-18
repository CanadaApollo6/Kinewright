use std::fmt;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalCode {
    NotFound,
    RemotePath,
    UnsupportedContainer,
    UnsupportedFormatVersion,
    TooLarge,
    TooDeep,
    TooManyItems,
    TooManyTracks,
    Malformed,
    DtdNotAllowed,
    AmbiguousTimeline,
    ProjectRateRequired,
    UnsupportedRate,
    Unrepresentable,
    NothingToImport,
    OutsideRoots,
    LoweringFailed,
    Cancelled,
    IoTimeout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Refusal {
    pub code: RefusalCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub timelines: Vec<String>,
}

impl Refusal {
    #[must_use]
    pub fn new(code: RefusalCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            at: None,
            timelines: Vec::new(),
        }
    }

    #[must_use]
    pub fn at(mut self, at: impl Into<String>) -> Self {
        self.at = Some(at.into());
        self
    }

    pub(crate) fn arithmetic() -> Self {
        Self::new(
            RefusalCode::Unrepresentable,
            "An exact time exceeds the representable range.",
        )
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(at) = &self.at {
            write!(formatter, "{at}: ")?;
        }
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Refusal {}
