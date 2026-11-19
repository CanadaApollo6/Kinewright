use std::{fmt, io};
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NOT_FOUND, ERROR_PATH_NOT_FOUND};

/// The code returned by Windows, or a successful call whose result was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Code {
    Win32(u32),
    HResult(i32),
    UnexpectedResult,
}

/// The part of the launcher that refused the operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Step {
    ProfileIdentity,
    RegisterProfile,
    ReadStorage,
    StorageAcl,
    StorageLimit,
    RemoveProfile,
    Environment,
    Launch,
    InspectChild,
}

/// A launch or profile error with its failing operation and native result.
#[derive(Debug)]
pub struct Error {
    pub step: Step,
    pub api: String,
    pub code: Code,
    detail: String,
}

impl Error {
    pub(super) fn new(
        step: Step,
        api: impl Into<String>,
        code: Code,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            step,
            api: api.into(),
            code,
            detail: detail.into(),
        }
    }

    pub(super) fn last(step: Step, api: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::io(step, api, &io::Error::last_os_error(), detail)
    }

    pub(super) fn io(
        step: Step,
        api: impl Into<String>,
        error: &io::Error,
        detail: impl Into<String>,
    ) -> Self {
        let code = error.raw_os_error().map_or(Code::UnexpectedResult, |code| {
            Code::Win32(code.cast_unsigned())
        });
        Self::new(step, api, code, detail)
    }

    pub(super) fn unexpected(
        step: Step,
        api: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self::new(step, api, Code::UnexpectedResult, detail)
    }

    pub(super) fn launch(error: io::Error, step: Step, api: &str) -> Self {
        if error
            .get_ref()
            .is_some_and(<dyn std::error::Error + Send + Sync>::is::<Self>)
        {
            let source = error.into_inner().expect("checked inner error");
            let mut source = *source.downcast::<Self>().expect("checked error type");
            source.step = step;
            source
        } else {
            let code = error.raw_os_error().map_or(Code::UnexpectedResult, |code| {
                Code::Win32(code.cast_unsigned())
            });
            Self::new(step, api, code, error.to_string())
        }
    }

    pub(super) fn missing(&self) -> bool {
        let code = match self.code {
            Code::Win32(code) => code,
            Code::HResult(status) if status.cast_unsigned() >> 16 == 0x8007 => {
                status.cast_unsigned() & 0xffff
            }
            _ => return false,
        };
        matches!(
            code,
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_NOT_FOUND
        )
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.step, self.api)?;
        match self.code {
            Code::Win32(code) => write!(f, " Win32 {code} ({code:#010x})")?,
            Code::HResult(code) => write!(f, " HRESULT {:#010x}", code.cast_unsigned())?,
            Code::UnexpectedResult => write!(f, " unexpected result")?,
        }
        write!(f, ": {}", self.detail)
    }
}

impl std::error::Error for Error {}
