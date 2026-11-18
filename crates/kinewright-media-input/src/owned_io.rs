//! Audited descriptor AVIO FFI, extracted from kinewright-media's `owned_io`.
#![allow(clippy::missing_errors_doc)]
use ffmpeg_next::{self as ffmpeg, ffi};
use std::{
    ffi::{CStr, c_char, c_int, c_void},
    io::SeekFrom,
    panic::{AssertUnwindSafe, catch_unwind},
    ptr::{self, NonNull},
};

const AVIO_BUFFER_BYTES: usize = 65_536;

pub struct OpenPolicy {
    pub demuxers: &'static CStr,
    pub protocols: &'static CStr,
}
impl Default for OpenPolicy {
    fn default() -> Self {
        Self {
            demuxers: crate::codec_policy::FORMATS.as_c_str(),
            protocols: c"none",
        }
    }
}
/// Host verification and helper read-only IO provide different cursors while
/// sharing the same native open, nested-input refusal, and policy options.
pub trait Cursor: Send {
    type Error;
    fn read(&mut self, output: &mut [u8]) -> Result<usize, Self::Error>;
    fn seek(&mut self, position: SeekFrom) -> Result<u64, Self::Error>;
    fn size(&mut self) -> Result<u64, Self::Error>;
    fn check(&mut self) -> Result<(), Self::Error>;
    fn temporarily_unavailable(error: &Self::Error) -> bool;
    fn capacity(&mut self, message: &str) -> Self::Error;
    fn decoder(&mut self, message: String) -> Self::Error;
    fn unavailable(&mut self, message: &str) -> Self::Error;
    fn invalid(&mut self, message: &str) -> Self::Error {
        self.unavailable(message)
    }
    fn allocation_exceeded(&mut self, message: &str) -> Self::Error {
        self.capacity(message)
    }
    fn deny_secondary(&mut self) {
        let _ = self.unavailable("secondary input open denied");
    }
    fn policy(&self) -> OpenPolicy {
        OpenPolicy::default()
    }
}

pub struct Avio<C: Cursor> {
    context: NonNull<ffi::AVIOContext>,
    // FFmpeg retains pointers to this allocation across calls.
    cursor: NonNull<C>,
}
// SAFETY: each AVIO has one native caller at a time. The stable cursor is
// Send; moving the owner never overlaps callbacks or changes their addresses.
unsafe impl<C: Cursor> Send for Avio<C> {}
impl<C: Cursor> Avio<C> {
    /// # Safety
    /// Drop the returned format before this AVIO and keep its cursor allocation
    /// stable until all synchronous native callbacks and format teardown finish.
    /// Serialize all format/AVIO operations and cursor borrows; no callback may
    /// overlap another operation on this AVIO or its cursor.
    pub unsafe fn open(cursor: C) -> Result<(ffmpeg::format::context::Input, Self), C::Error> {
        let mut avio = Self::allocate(cursor)?;
        // SAFETY: the stable cursor and native buffer outlive the format
        // under the caller's documented ownership obligation.
        unsafe {
            let context = avio.context;
            let mut format = ffi::avformat_alloc_context();
            if format.is_null() {
                return Err(avio.cursor_mut().capacity("format allocation"));
            }
            (*format).pb = context.as_ptr();
            (*format).opaque = avio.cursor.as_ptr().cast();
            (*format).flags |= ffi::AVFMT_FLAG_CUSTOM_IO;
            (*format).io_open = Some(deny_open::<C>);
            (*format).interrupt_callback.callback = Some(interrupt::<C>);
            (*format).interrupt_callback.opaque = avio.cursor.as_ptr().cast();
            (*format).max_streams = 16;
            (*format).probesize = 32_768;
            (*format).format_probesize = 32_768;
            let policy = avio.cursor().policy();
            let mut options = ptr::null_mut();
            let protocol = ffi::av_dict_set(
                &raw mut options,
                c"protocol_whitelist".as_ptr(),
                policy.protocols.as_ptr(),
                0,
            );
            let demuxer = ffi::av_dict_set(
                &raw mut options,
                c"format_whitelist".as_ptr(),
                policy.demuxers.as_ptr(),
                0,
            );
            let codec = ffi::av_dict_set(
                &raw mut options,
                c"codec_whitelist".as_ptr(),
                crate::codec_policy::CODECS.as_ptr(),
                0,
            );
            if protocol < 0 || demuxer < 0 || codec < 0 {
                ffi::av_dict_free(&raw mut options);
                ffi::avformat_free_context(format);
                return Err(avio.cursor_mut().capacity("format policy allocation"));
            }
            let opened = ffi::avformat_open_input(
                &raw mut format,
                c"file:certified-owned-input".as_ptr(),
                ptr::null(),
                &raw mut options,
            );
            ffi::av_dict_free(&raw mut options);
            if opened < 0 {
                avio.check()?;
                return Err(avio
                    .cursor_mut()
                    .decoder(ffmpeg::Error::from(opened).to_string()));
            }
            let input = ffmpeg::format::context::Input::wrap(format);
            avio.check()?;
            Ok((input, avio))
        }
    }
    fn allocate(mut cursor: C) -> Result<Self, C::Error> {
        // SAFETY: the new native buffer and stable cursor are exclusively
        // owned here; each callback borrows them synchronously until teardown.
        unsafe {
            let buffer = ffi::av_malloc(AVIO_BUFFER_BYTES).cast::<u8>();
            if buffer.is_null() {
                return Err(cursor.capacity("AVIO buffer allocation"));
            }
            let cursor = NonNull::new(Box::into_raw(Box::new(cursor)))
                .expect("Box::into_raw never returns null");
            let context = ffi::avio_alloc_context(
                buffer,
                65_536,
                0,
                cursor.as_ptr().cast(),
                Some(read::<C>),
                None,
                Some(seek::<C>),
            );
            let Some(context) = NonNull::new(context) else {
                ffi::av_free(buffer.cast());
                let mut cursor = Box::from_raw(cursor.as_ptr());
                return Err(cursor.capacity("AVIO context allocation"));
            };
            Ok(Self { context, cursor })
        }
    }
    #[cfg(feature = "test-util")]
    pub fn test_cursor(cursor: C) -> Result<Self, C::Error> {
        Self::allocate(cursor)
    }
    #[cfg(feature = "test-util")]
    pub fn test_read(&mut self, output: &mut [u8]) -> i32 {
        let Ok(length) = i32::try_from(output.len()) else {
            return ffi::AVERROR(ffmpeg::error::EIO);
        };
        // SAFETY: this owner keeps the standalone AVIO context live; output is
        // writable for exactly the native read length and never aliases the cursor.
        unsafe { ffi::avio_read(self.context.as_ptr(), output.as_mut_ptr(), length) }
    }
    #[must_use]
    pub fn cursor(&self) -> &C {
        // SAFETY: the native callers are idle while Rust borrows the cursor.
        unsafe { self.cursor.as_ref() }
    }
    pub fn cursor_mut(&mut self) -> &mut C {
        // SAFETY: native callers are idle and this borrow is exclusive.
        unsafe { self.cursor.as_mut() }
    }
    pub fn check(&mut self) -> Result<(), C::Error> {
        self.cursor_mut().check()?;
        // SAFETY: context is owned, live, and not concurrently used by FFmpeg.
        unsafe {
            if self.context.as_ref().error == ffi::AVERROR(ffmpeg::error::EAGAIN) {
                self.context.as_mut().error = 0;
                self.context.as_mut().eof_reached = 0;
            }
            let size = self.context.as_ref().buffer_size;
            if size < 0 || usize::try_from(size).unwrap_or(usize::MAX) > AVIO_BUFFER_BYTES {
                return Err(self
                    .cursor_mut()
                    .allocation_exceeded("format exceeded bounded AVIO buffer"));
            }
        }
        Ok(())
    }
    #[cfg(feature = "test-util")]
    #[must_use]
    pub fn buffer_size(&self) -> usize {
        // SAFETY: the context remains live while this owner is borrowed.
        usize::try_from(unsafe { self.context.as_ref().buffer_size }).unwrap_or(0)
    }
}
impl<C: Cursor> Drop for Avio<C> {
    fn drop(&mut self) {
        // SAFETY: callers drop the format first; its custom-IO flag prevents
        // closing this context. The current buffer belongs to FFmpeg, which may
        // have replaced the original allocation during synchronous reads.
        unsafe {
            ffi::av_free(self.context.as_ref().buffer.cast());
            self.context.as_mut().buffer = ptr::null_mut();
            let mut context = self.context.as_ptr();
            ffi::avio_context_free(&raw mut context);
            drop(Box::from_raw(self.cursor.as_ptr()));
        }
    }
}
unsafe extern "C" fn deny_open<C: Cursor>(
    format: *mut ffi::AVFormatContext,
    _: *mut *mut ffi::AVIOContext,
    _: *const c_char,
    _: c_int,
    _: *mut *mut ffi::AVDictionary,
) -> c_int {
    // SAFETY: FFmpeg passes a live context. A nested context may have no
    // copied opaque pointer. When present, it refers to our live cursor.
    if let Some(mut cursor) =
        (unsafe { format.as_ref() }).and_then(|format| NonNull::new(format.opaque.cast::<C>()))
    {
        unsafe { cursor.as_mut() }.deny_secondary();
    }
    ffi::AVERROR(ffmpeg::error::EACCES)
}
unsafe extern "C" fn interrupt<C: Cursor>(opaque: *mut c_void) -> c_int {
    // SAFETY: opaque is the registered stable cursor, called synchronously.
    let cursor = unsafe { &mut *opaque.cast::<C>() };
    match cursor.check() {
        Ok(()) => 0,
        Err(error) => i32::from(!C::temporarily_unavailable(&error)),
    }
}
unsafe extern "C" fn read<C: Cursor>(opaque: *mut c_void, buffer: *mut u8, length: c_int) -> c_int {
    // SAFETY: opaque is registered by open and remains live through format drop.
    let cursor = unsafe { &mut *opaque.cast::<C>() };
    if length <= 0 || buffer.is_null() {
        let _ = cursor.invalid("invalid AVIO read size");
        return ffi::AVERROR(ffmpeg::error::EIO);
    }
    let maximum = usize::try_from(length).unwrap_or(0);
    let result = catch_unwind(AssertUnwindSafe(|| {
        let length =
            usize::try_from(length).map_err(|_| cursor.invalid("invalid AVIO read size"))?;
        // SAFETY: FFmpeg supplies a writable native buffer with the stated length.
        let output = unsafe { std::slice::from_raw_parts_mut(buffer, length) };
        cursor.read(output)
    }));
    match result {
        Ok(Ok(0)) => ffi::AVERROR_EOF,
        Ok(Ok(count)) if count <= maximum => {
            i32::try_from(count).unwrap_or(ffi::AVERROR(ffmpeg::error::EIO))
        }
        Ok(Err(error)) if C::temporarily_unavailable(&error) => ffi::AVERROR(ffmpeg::error::EAGAIN),
        Ok(Ok(_) | Err(_)) => ffi::AVERROR(ffmpeg::error::EIO),
        Err(_) => {
            let _ = cursor.unavailable("panic in AVIO read");
            ffi::AVERROR(ffmpeg::error::EIO)
        }
    }
}
unsafe extern "C" fn seek<C: Cursor>(opaque: *mut c_void, offset: i64, whence: c_int) -> i64 {
    // SAFETY: opaque is registered by open and remains live through format drop.
    let cursor = unsafe { &mut *opaque.cast::<C>() };
    let result = catch_unwind(AssertUnwindSafe(|| {
        if whence & ffi::AVSEEK_SIZE != 0 {
            return cursor.size();
        }
        let position = match whence & !ffi::AVSEEK_FORCE {
            0 if offset >= 0 => SeekFrom::Start(offset.cast_unsigned()),
            1 => SeekFrom::Current(offset),
            2 => SeekFrom::End(offset),
            _ => return Err(cursor.invalid("invalid AVIO seek")),
        };
        cursor.seek(position)
    }));
    match result {
        Ok(Ok(position)) => {
            i64::try_from(position).unwrap_or(i64::from(ffi::AVERROR(ffmpeg::error::EIO)))
        }
        Ok(Err(error)) if C::temporarily_unavailable(&error) => {
            i64::from(ffi::AVERROR(ffmpeg::error::EAGAIN))
        }
        Ok(Err(_)) => i64::from(ffi::AVERROR(ffmpeg::error::EIO)),
        Err(_) => {
            let _ = cursor.unavailable("panic in AVIO seek");
            i64::from(ffi::AVERROR(ffmpeg::error::EIO))
        }
    }
}

#[cfg(feature = "test-util")]
pub fn test_callback_read<C: Cursor>(cursor: &mut C, buffer: &mut [u8]) -> i32 {
    let Ok(length) = i32::try_from(buffer.len()) else {
        return ffi::AVERROR(ffmpeg::error::EIO);
    };
    // SAFETY: cursor and output are exclusively borrowed and valid for the
    // synchronous callback, just as in the registered native AVIO.
    unsafe { read::<C>(ptr::from_mut(cursor).cast(), buffer.as_mut_ptr(), length) }
}
