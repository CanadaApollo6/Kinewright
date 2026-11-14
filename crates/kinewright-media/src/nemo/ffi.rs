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
    pub(super) fn load(path: &Path, threads: u8, lookahead: u8) -> Result<Self, Error> {
        let path = CString::new(path.to_str().ok_or(Error::InvalidPath)?)
            .map_err(|_| Error::InvalidPath)?;
        let (mut backend, mut model, mut streaming, mut config) = configs()?;
        backend.gpu = -1;
        backend.threads = i32::from(threads);
        model.path = path.as_ptr();
        streaming.rnnt_right_context = i32::from(lookahead);
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

#[repr(C)]
struct StreamHandle {
    _opaque: [u8; 0],
}
#[repr(C)]
struct ResultHandle {
    _opaque: [u8; 0],
}
// SAFETY: signatures match the pinned asr.h; handles stay owned by their wrappers.
unsafe extern "C" {
    fn nemo_speech_asr_streaming_recognize(
        recognizer: *mut Handle,
        options: *const RecognitionOptions,
        out: *mut *mut StreamHandle,
    ) -> i32;
    fn nemo_speech_asr_stream_push_f32(
        stream: *mut StreamHandle,
        samples: *const f32,
        count: usize,
        rate: i32,
    ) -> i32;
    fn nemo_speech_asr_stream_next(stream: *mut StreamHandle, out: *mut *mut ResultHandle) -> i32;
    fn nemo_speech_asr_stream_force_endpoint(stream: *mut StreamHandle) -> i32;
    fn nemo_speech_asr_stream_finish(stream: *mut StreamHandle) -> i32;
    fn nemo_speech_asr_stream_close(stream: *mut StreamHandle);
    fn nemo_speech_asr_result_is_final(result: *const ResultHandle) -> bool;
    fn nemo_speech_asr_result_audio_processed(result: *const ResultHandle) -> f32;
    fn nemo_speech_asr_result_transcript(result: *const ResultHandle, alt: usize) -> *const c_char;
    fn nemo_speech_asr_result_word_count(result: *const ResultHandle, alt: usize) -> usize;
    fn nemo_speech_asr_result_word_text(
        result: *const ResultHandle,
        alt: usize,
        index: usize,
    ) -> *const c_char;
    fn nemo_speech_asr_result_word_start_time(
        result: *const ResultHandle,
        alt: usize,
        index: usize,
    ) -> i32;
    fn nemo_speech_asr_result_word_end_time(
        result: *const ResultHandle,
        alt: usize,
        index: usize,
    ) -> i32;
    fn nemo_speech_asr_result_destroy(result: *mut ResultHandle);
}

pub(crate) struct Word {
    pub text: String,
    pub start_us: i64,
    pub end_us: i64,
}
pub(crate) struct Frame {
    pub final_result: bool,
    pub audio_processed_us: i64,
    pub text: String,
    pub words: Vec<Word>,
}

// The phantom borrow prevents destroying the model before its native stream.
pub(super) struct Stream<'a> {
    handle: NonNull<StreamHandle>,
    _owner: std::marker::PhantomData<&'a mut Recognizer>,
}
struct ResultOwner(NonNull<ResultHandle>);
impl Drop for ResultOwner {
    fn drop(&mut self) {
        // SAFETY: next transferred exactly one result; only this owner destroys it.
        unsafe { nemo_speech_asr_result_destroy(self.0.as_ptr()) };
    }
}
fn options(settings: super::Options) -> Result<RecognitionOptions, Error> {
    // SAFETY: the query has no input or Rust output buffer.
    let native = unsafe { nemo_speech_asr_recognition_options_size() };
    if native != size_of::<RecognitionOptions>() {
        return Err(Error::AbiMismatch {
            structure: "RecognitionOptions",
            rust: size_of::<RecognitionOptions>(),
            native,
        });
    }
    // SAFETY: the size query verified the by-value result layout before this call.
    let mut options = unsafe { nemo_speech_asr_recognition_options_default() };
    options.interim_results = true;
    options.enable_word_time_offsets = true;
    options.enable_automatic_punctuation = settings.punctuation;
    options.verbatim_transcripts = settings.verbatim;
    Ok(options)
}
impl Recognizer {
    pub(super) fn stream(&mut self, settings: super::Options) -> Result<Stream<'_>, Error> {
        let options = options(settings)?;
        let mut out = ptr::null_mut();
        // SAFETY: options and out live through the call; the resulting stream borrows this model.
        let status = unsafe {
            nemo_speech_asr_streaming_recognize(self.0.as_ptr(), &raw const options, &raw mut out)
        };
        status_result(status).map_err(Error::Native)?;
        let handle = NonNull::new(out).ok_or_else(|| {
            Error::Native(NativeError {
                status: Status::Runtime,
                message: "stream created without a handle".into(),
            })
        })?;
        Ok(Stream {
            handle,
            _owner: std::marker::PhantomData,
        })
    }
}
impl Stream<'_> {
    pub(super) fn feed(&mut self, samples: &[f32]) -> Result<(), Error> {
        // SAFETY: this stream is live; push copies the valid sample slice before returning.
        let status = unsafe {
            nemo_speech_asr_stream_push_f32(
                self.handle.as_ptr(),
                samples.as_ptr(),
                samples.len(),
                16_000,
            )
        };
        status_result(status).map_err(Error::Native)
    }
    pub(super) fn commit(&mut self) -> Result<(), Error> {
        // SAFETY: this owner retains the live stream and model.
        status_result(unsafe { nemo_speech_asr_stream_force_endpoint(self.handle.as_ptr()) })
            .map_err(Error::Native)
    }
    pub(super) fn finish(&mut self) -> Result<(), Error> {
        // SAFETY: this owner retains the live stream and model until the tail is drained.
        status_result(unsafe { nemo_speech_asr_stream_finish(self.handle.as_ptr()) })
            .map_err(Error::Native)
    }
    pub(super) fn step(&mut self) -> Result<Option<Frame>, Error> {
        let mut out = ptr::null_mut();
        // SAFETY: out is writable and the stream remains live during computation.
        let status = unsafe { nemo_speech_asr_stream_next(self.handle.as_ptr(), &raw mut out) };
        status_result(status).map_err(Error::Native)?;
        let Some(result) = NonNull::new(out).map(ResultOwner) else {
            return Ok(None);
        };
        let pointer = result.0.as_ptr();
        // SAFETY: all accessors borrow this result, whose owner lives through the copied data.
        let (final_result, processed, text, count) = unsafe {
            (
                nemo_speech_asr_result_is_final(pointer),
                nemo_speech_asr_result_audio_processed(pointer),
                result_text(nemo_speech_asr_result_transcript(pointer, 0))?,
                nemo_speech_asr_result_word_count(pointer, 0),
            )
        };
        let mut words = Vec::with_capacity(count);
        for index in 0..count {
            // SAFETY: index is in the accessor-reported top-alternative word range; data are copied before drop.
            let word = unsafe {
                Word {
                    text: result_text(nemo_speech_asr_result_word_text(pointer, 0, index))?,
                    start_us: i64::from(nemo_speech_asr_result_word_start_time(pointer, 0, index))
                        * 1000,
                    end_us: i64::from(nemo_speech_asr_result_word_end_time(pointer, 0, index))
                        * 1000,
                }
            };
            words.push(word);
        }
        Ok(Some(Frame {
            final_result,
            audio_processed_us: processed_us(processed)?,
            text,
            words,
        }))
    }
}
fn result_text(pointer: *const c_char) -> Result<String, Error> {
    if pointer.is_null() {
        return Err(Error::Native(NativeError {
            status: Status::Runtime,
            message: "missing native result text".into(),
        }));
    }
    // SAFETY: accessors returned a non-null NUL-terminated string; the result owner lives through this copy.
    Ok(unsafe { CStr::from_ptr(pointer) }
        .to_string_lossy()
        .into_owned())
}
impl Drop for Stream<'_> {
    fn drop(&mut self) {
        // SAFETY: only this owner closes this live stream, before its borrowed model can drop.
        unsafe { nemo_speech_asr_stream_close(self.handle.as_ptr()) };
    }
}
#[allow(
    clippy::cast_possible_truncation,
    reason = "the C ABI reports f32 seconds; validate range and round once into microseconds"
)]
fn processed_us(seconds: f32) -> Result<i64, Error> {
    let value = (f64::from(seconds) * 1_000_000.0).round();
    if !value.is_finite() || value < 0.0 || value >= 9_223_372_036_854_775_808.0 {
        return Err(Error::Native(NativeError {
            status: Status::Runtime,
            message: "invalid native audio progress".into(),
        }));
    }
    Ok(value as i64)
}

#[cfg(windows)]
pub(super) fn file_change_time(file: &std::fs::File) -> std::io::Result<i64> {
    use std::os::windows::io::AsRawHandle as _;
    #[repr(C)]
    struct BasicInfo {
        created: i64,
        accessed: i64,
        written: i64,
        changed: i64,
        attributes: u32,
    }
    #[link(name = "kernel32")]
    // SAFETY: signature and FileBasicInfo layout match the Win64 SDK.
    unsafe extern "system" {
        fn GetFileInformationByHandleEx(
            handle: *mut c_void,
            class: i32,
            info: *mut c_void,
            bytes: u32,
        ) -> i32;
    }
    let mut info = MaybeUninit::<BasicInfo>::uninit();
    // SAFETY: the file handle is retained and the fixed-size buffer matches FILE_BASIC_INFO.
    let result = unsafe {
        GetFileInformationByHandleEx(file.as_raw_handle(), 0, info.as_mut_ptr().cast(), 40)
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the successful SDK call initialized every BasicInfo field.
    Ok(unsafe { info.assume_init() }.changed)
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
    fn dq1_native_options_honour_the_identity_settings() {
        for value in [false, true] {
            let actual = options(super::super::Options {
                punctuation: value,
                verbatim: value,
            })
            .unwrap();
            assert_eq!(
                actual.enable_automatic_punctuation, value,
                "DQ1_NATIVE_PUNCTUATION"
            );
            assert_eq!(actual.verbatim_transcripts, value, "DQ1_NATIVE_VERBATIM");
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
