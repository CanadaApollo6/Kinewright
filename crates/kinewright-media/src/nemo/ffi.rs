//! Hand-written subset of the pinned, patched `nemo_speech/asr.h`.
use std::{
    ffi::{CStr, CString, c_char, c_void},
    mem::{MaybeUninit, size_of},
    path::Path,
    ptr::{self, NonNull},
};

use super::{Error, NativeError, Status};

#[repr(C)]
struct BackendConfig {
    size: usize,
    gpu: i32,
    reserved: i32,
    threads: i32,
}
#[repr(C)]
struct ModelConfig {
    size: usize,
    path: *const c_char,
    name: *const c_char,
}
#[repr(C)]
struct StreamingConfig {
    size: usize,
    chunk_size: f32,
    ctc_left_padding: f32,
    ctc_right_padding: f32,
    rnnt_right_context: i32,
}
#[repr(C)]
struct RecognizerConfig {
    size: usize,
    backend: *const BackendConfig,
    model: *const ModelConfig,
    streaming: *const StreamingConfig,
    decoder: *const c_void,
    vad: *const c_void,
    endpointing: *const c_void,
    postproc: *const c_void,
    diar: *const c_void,
    batching: *const c_void,
    salm: *const c_void,
    log_status: bool,
}

#[repr(C)]
struct Handle {
    _opaque: [u8; 0],
}

// SAFETY: declarations match the pinned C header and the two repository patches.
unsafe extern "C" {
    fn nemo_speech_asr_backend_config_default(out: *mut BackendConfig, capacity: usize) -> usize;
    fn nemo_speech_asr_model_config_default(out: *mut ModelConfig, capacity: usize) -> usize;
    fn nemo_speech_asr_streaming_config_default(
        out: *mut StreamingConfig,
        capacity: usize,
    ) -> usize;
    fn nemo_speech_asr_recognizer_config_default(
        out: *mut RecognizerConfig,
        capacity: usize,
    ) -> usize;
    fn nemo_speech_asr_create(config: *const RecognizerConfig, out: *mut *mut Handle) -> i32;
    fn nemo_speech_asr_destroy(handle: *mut Handle);
    fn nemo_speech_asr_last_error() -> *const c_char;
    fn nemo_speech_asr_version() -> *const c_char;
}

// Each sized constructor refuses to write if the native size differs, before
// Rust assumes initialization. This makes ABI drift an error instead of an overrun.
macro_rules! defaults {
    ($ty:ty, $constructor:ident) => {{
        let mut value = MaybeUninit::<$ty>::uninit();
        // SAFETY: the constructor writes only to a matching-size output buffer.
        let native = unsafe { $constructor(value.as_mut_ptr(), size_of::<$ty>()) };
        if native != size_of::<$ty>() {
            return Err(Error::AbiMismatch { structure: stringify!($ty), rust: size_of::<$ty>(), native });
        }
        // SAFETY: a matching constructor initializes every field, including pointer and bool fields.
        unsafe { value.assume_init() }
    }};
}

fn configs() -> Result<
    (
        BackendConfig,
        ModelConfig,
        StreamingConfig,
        RecognizerConfig,
    ),
    Error,
> {
    Ok((
        defaults!(BackendConfig, nemo_speech_asr_backend_config_default),
        defaults!(ModelConfig, nemo_speech_asr_model_config_default),
        defaults!(StreamingConfig, nemo_speech_asr_streaming_config_default),
        defaults!(RecognizerConfig, nemo_speech_asr_recognizer_config_default),
    ))
}

#[cfg(test)]
pub(super) fn check_abi() -> Result<(), Error> {
    configs().map(|_| ())
}

pub(super) struct Recognizer(NonNull<Handle>);
impl Recognizer {
    pub(super) fn load(path: &Path) -> Result<Self, Error> {
        let path = CString::new(path.to_str().ok_or(Error::InvalidPath)?)
            .map_err(|_| Error::InvalidPath)?;
        let (mut backend, mut model, mut streaming, mut config) = configs()?;
        backend.gpu = -1;
        backend.threads = i32::try_from(
            std::thread::available_parallelism()
                .map_or(4, std::num::NonZeroUsize::get)
                .min(4),
        )
        .unwrap_or(4);
        model.path = path.as_ptr();
        streaming.rnnt_right_context = 13;
        config.backend = &raw const backend;
        config.model = &raw const model;
        config.streaming = &raw const streaming;
        config.log_status = false;
        let mut out = ptr::null_mut();
        // SAFETY: all config pointers stay live during create, which copies their data; out is writable.
        let status = unsafe { nemo_speech_asr_create(&raw const config, &raw mut out) };
        if let Err(error) = status_result(status) {
            return Err(if error.status == Status::Runtime {
                Error::InvalidModel(error)
            } else {
                Error::Native(error)
            });
        }
        NonNull::new(out).map(Self).ok_or_else(|| {
            Error::Native(NativeError {
                status: Status::Runtime,
                message: "create succeeded without a recognizer".into(),
            })
        })
    }
}
impl Drop for Recognizer {
    fn drop(&mut self) {
        // SAFETY: this owner holds the successful create handle and destroys it exactly once.
        unsafe { nemo_speech_asr_destroy(self.0.as_ptr()) };
    }
}

fn status_result(code: i32) -> Result<(), NativeError> {
    let status = match code {
        0 => return Ok(()),
        1 => Status::InvalidArgument,
        2 => Status::OutOfMemory,
        3 => Status::Runtime,
        4 => Status::Cancelled,
        5 => Status::ResourceExhausted,
        6 => Status::Unavailable,
        other => Status::Unknown(other),
    };
    // SAFETY: the error pointer is thread-local and remains valid until the next C call on this thread.
    let message = unsafe { CStr::from_ptr(nemo_speech_asr_last_error()) }
        .to_string_lossy()
        .into_owned();
    Err(NativeError { status, message })
}

pub(super) fn runtime_version() -> String {
    // SAFETY: the library returns a static, NUL-terminated version string.
    unsafe { CStr::from_ptr(nemo_speech_asr_version()) }
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
#[repr(C)]
struct RecognitionOptions {
    size: usize,
    request_id: *const c_char,
    language_code: *const c_char,
    interim_results: bool,
    enable_word_time_offsets: bool,
    enable_automatic_punctuation: bool,
    verbatim_transcripts: bool,
    profanity_filter: bool,
    stop_history_eou_ms: i32,
    speech_contexts: *const c_void,
    speech_context_count: usize,
    max_alternatives: i32,
    enable_speaker_diarization: bool,
    max_speaker_count: i32,
}
#[cfg(test)]
// SAFETY: signature is the library's original options constructor from asr.h.
unsafe extern "C" {
    fn nemo_speech_asr_recognition_options_size() -> usize;
    fn nemo_speech_asr_recognition_options_default() -> RecognitionOptions;
}
#[cfg(test)]
pub(super) fn check_options_size() {
    // SAFETY: this size probe has no input and allocates no Rust output buffer.
    let native = unsafe { nemo_speech_asr_recognition_options_size() };
    assert_eq!(native, size_of::<RecognitionOptions>());
    // SAFETY: this declaration matches the pinned library's by-value options type.
    let options = unsafe { nemo_speech_asr_recognition_options_default() };
    assert_eq!(options.size, size_of::<RecognitionOptions>());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dq1_all_status_codes_map_to_typed_errors() {
        assert_eq!(status_result(0), Ok(()));
        for (code, expected) in [
            (1, Status::InvalidArgument),
            (2, Status::OutOfMemory),
            (3, Status::Runtime),
            (4, Status::Cancelled),
            (5, Status::ResourceExhausted),
            (6, Status::Unavailable),
            (7, Status::Unknown(7)),
        ] {
            assert_eq!(status_result(code).unwrap_err().status, expected);
        }
    }

    #[test]
    fn dq1_sized_default_refuses_to_write_a_mismatched_buffer() {
        let mut value = BackendConfig {
            size: 99,
            gpu: 42,
            reserved: 123,
            threads: 77,
        };
        // SAFETY: the sized constructor must not write when capacity differs from sizeof(config).
        let native = unsafe {
            nemo_speech_asr_backend_config_default(&raw mut value, size_of::<BackendConfig>() - 1)
        };
        assert_eq!(native, size_of::<BackendConfig>());
        assert_eq!(
            (value.size, value.gpu, value.reserved, value.threads),
            (99, 42, 123, 77)
        );
    }
}
