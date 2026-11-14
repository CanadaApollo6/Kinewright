//! CPU-only NeMo-Speech.cpp ownership boundary. Callers must verify weights
//! before using this low-level loader; recognizer/model.rs owns verification.
use std::{fmt, fs, path::Path};

#[allow(
    unsafe_code,
    reason = "the hand-written NeMo C ABI is confined to this module"
)]
mod ffi;

/// Identifies the pinned source, patches, compiler and CPU build settings.
pub const BUILD_KEY: &str = env!("KINEWRIGHT_NEMO_BUILD_KEY");

/// Status codes of the C ABI, including an unexpected value from a changed ABI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    InvalidArgument,
    OutOfMemory,
    Runtime,
    Cancelled,
    ResourceExhausted,
    Unavailable,
    Unknown(i32),
}

/// A failed C call. The thread-local error is copied before another C call.
#[derive(Debug, PartialEq, Eq)]
pub struct NativeError {
    pub status: Status,
    pub message: String,
}

/// Failures before or during recogniser creation.
#[derive(Debug)]
pub enum Error {
    UnsupportedCpu,
    MissingModel,
    EmptyModel,
    ModelIo(std::io::Error),
    InvalidPath,
    InvalidModel(NativeError),
    Native(NativeError),
    AbiMismatch {
        structure: &'static str,
        rust: usize,
        native: usize,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedCpu => f.write_str("NeMo requires AVX2, FMA, F16C, BMI2 and SSE4.2"),
            Self::MissingModel => f.write_str("NeMo model file is missing"),
            Self::EmptyModel => f.write_str("NeMo model file is empty"),
            Self::ModelIo(error) => write!(f, "could not read NeMo model: {error}"),
            Self::InvalidPath => f.write_str("NeMo model path must be UTF-8 without NUL"),
            Self::InvalidModel(error) => write!(f, "invalid NeMo model: {}", error.message),
            Self::Native(error) => write!(f, "NeMo {:?}: {}", error.status, error.message),
            Self::AbiMismatch {
                structure,
                rust,
                native,
            } => write!(
                f,
                "NeMo ABI mismatch for {structure}: Rust {rust}, library {native}"
            ),
        }
    }
}
impl std::error::Error for Error {}

/// One CPU recogniser. Its native handle is released on drop.
/// No callbacks cross the C boundary.
pub struct Recognizer {
    native: ffi::Recognizer,
}

impl Recognizer {
    /// Load a model with four CPU threads (or the machine's lower capacity),
    /// explicit lookahead 13, and recogniser status lines disabled.
    ///
    /// # Errors
    /// Returns a typed CPU, ABI, file, path or native model error. This low-level
    /// operation does not replace the model's size and SHA-256 verification.
    pub fn load(model: &Path) -> Result<Self, Error> {
        Self::load_config(model, CPU_THREADS, 13)
    }

    pub(crate) fn load_config(model: &Path, threads: u8, lookahead: u8) -> Result<Self, Error> {
        check_cpu()?;
        let metadata = fs::metadata(model).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::MissingModel
            } else {
                Error::ModelIo(error)
            }
        })?;
        if !metadata.is_file() {
            return Err(Error::InvalidPath);
        }
        if metadata.len() == 0 {
            return Err(Error::EmptyModel);
        }
        Ok(Self {
            native: ffi::Recognizer::load(model, cpu_threads(threads), lookahead)?,
        })
    }
}

pub(crate) const CPU_THREADS: u8 = 4;
fn cpu_threads(requested: u8) -> u8 {
    let requested = requested.max(1);
    let available = std::thread::available_parallelism()
        .map_or(usize::from(CPU_THREADS), std::num::NonZeroUsize::get);
    u8::try_from(available.min(usize::from(requested))).unwrap_or(requested)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Options {
    pub punctuation: bool,
    pub verbatim: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            punctuation: true,
            verbatim: true,
        }
    }
}

fn check_cpu() -> Result<(), Error> {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx2")
        && std::is_x86_feature_detected!("fma")
        && std::is_x86_feature_detected!("f16c")
        && std::is_x86_feature_detected!("bmi2")
        && std::is_x86_feature_detected!("sse4.2")
    {
        return Ok(());
    }
    Err(Error::UnsupportedCpu)
}

/// Version reported by the linked runtime.
#[must_use]
pub fn runtime_version() -> String {
    ffi::runtime_version()
}

pub(crate) use ffi::Frame;
#[cfg(test)]
pub(crate) use ffi::Word;
pub(crate) struct Stream<'a> {
    native: ffi::Stream<'a>,
}
impl Recognizer {
    pub(crate) fn stream(&mut self, options: Options) -> Result<Stream<'_>, Error> {
        Ok(Stream {
            native: self.native.stream(options)?,
        })
    }
}
impl Stream<'_> {
    pub(crate) fn feed(&mut self, samples: &[f32]) -> Result<(), Error> {
        self.native.feed(samples)
    }
    pub(crate) fn step(&mut self) -> Result<Option<Frame>, Error> {
        self.native.step()
    }
    pub(crate) fn commit(&mut self) -> Result<(), Error> {
        self.native.commit()
    }
    pub(crate) fn finish(&mut self) -> Result<(), Error> {
        self.native.finish()
    }
}
#[cfg(windows)]
pub(crate) fn file_change_time(file: &std::fs::File) -> std::io::Result<i64> {
    ffi::file_change_time(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDirectory;

    #[test]
    fn dq1_struct_sizes_match_library_defaults() {
        ffi::check_abi().unwrap();
        assert_eq!(runtime_version(), "nemo-speech-asr 0.3.0");
        ffi::check_options_size();
    }

    #[test]
    fn dq1_missing_model_is_typed() {
        let dir = TempDirectory::new("dq1-missing");
        assert!(matches!(
            Recognizer::load(&dir.path("missing.gguf")),
            Err(Error::MissingModel)
        ));
    }

    #[test]
    fn dq1_empty_model_is_typed() {
        let dir = TempDirectory::new("dq1-empty");
        let path = dir.path("empty.gguf");
        fs::write(&path, []).unwrap();
        assert!(matches!(Recognizer::load(&path), Err(Error::EmptyModel)));
    }

    #[test]
    fn dq1_garbage_model_is_typed_by_real_library() {
        let dir = TempDirectory::new("dq1-garbage");
        let path = dir.path("garbage.gguf");
        fs::write(&path, b"not a GGUF model").unwrap();
        let error = Recognizer::load(&path)
            .err()
            .expect("garbage must fail in the native loader");
        let Error::InvalidModel(error) = error else {
            panic!("garbage must reach the native model loader, got {error}");
        };
        assert_eq!(error.status, Status::Runtime);
        assert!(
            error.message.contains("Failed to load GGML file"),
            "{error:?}"
        );
    }
}
