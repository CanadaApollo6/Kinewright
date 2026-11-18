//! Audited `FFmpeg` FFI. The only input capability is the retained source descriptor.
#![allow(unsafe_code)]
use ffmpeg_next::{self as ffmpeg, ffi};
use kinewright_job_sys::media_boundary::MediaFile;
use kinewright_media_ipc::MediaSandboxError as Error;
use std::{
    io::SeekFrom,
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const IO_LIMIT: u64 = 64 * 1024 * 1024;
struct Cursor {
    file: MediaFile,
    length: u64,
    read: u64,
    position: u64,
    cancel: Arc<AtomicBool>,
    failure: Option<Error>,
}
pub struct Input {
    pub format: ffmpeg::format::context::Input,
    avio: Avio,
    pub header: Vec<u8>,
}
type Avio = kinewright_media_input::owned_io::Avio<Cursor>;
impl Input {
    pub fn open(
        file: MediaFile,
        length: u64,
        budget: u64,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self, Error> {
        if length > i64::MAX.cast_unsigned() {
            return Err(Error::UnsupportedMedia);
        }
        let mut header = vec![0; 65_536];
        let count = file
            .read_at(&mut header, 0)
            .map_err(|_| Error::UnsupportedMedia)?;
        header.truncate(count);
        let cursor = Cursor {
            file,
            length,
            read: count as u64,
            position: 0,
            cancel,
            failure: None,
        };
        // SAFETY: Input declares format before AVIO, so native input teardown
        // finishes before its stable cursor and buffer are released.
        let (mut format, avio) = unsafe { Avio::open(cursor) }?;
        // SAFETY: format is the exclusive owner of its live AVFormatContext.
        unsafe {
            (*format.as_mut_ptr()).max_analyze_duration = 5_000_000;
            let still_codec = match format.format().name() {
                "png_pipe" => Some(ffmpeg::codec::Id::PNG),
                "jpeg_pipe" => Some(ffmpeg::codec::Id::MJPEG),
                _ => None,
            };
            if let Some(codec) = still_codec {
                if format.nb_streams() != 1 {
                    return Err(Error::UnsupportedMedia);
                }
                let mut parameters = format
                    .stream(0)
                    .ok_or(Error::UnsupportedMedia)?
                    .parameters();
                if parameters.id() != codec {
                    return Err(Error::UnsupportedMedia);
                }
                let (w, h) = still_dimensions(&header).ok_or(Error::UnsupportedMedia)?;
                (*parameters.as_mut_ptr()).width =
                    i32::try_from(w).map_err(|_| Error::UnsupportedMedia)?;
                (*parameters.as_mut_ptr()).height =
                    i32::try_from(h).map_err(|_| Error::UnsupportedMedia)?;
            } else if format.nb_streams() == 0
                || (*format.as_ptr()).ctx_flags & ffi::AVFMTCTX_NOHEADER != 0
            {
                return Err(Error::UnsupportedMedia);
            }
            (*format.as_mut_ptr()).max_streams =
                i32::try_from(format.nb_streams()).map_err(|_| Error::UnsupportedMedia)?;
            let input = Self {
                format,
                avio,
                header,
            };
            for stream in input.format.streams() {
                allocation(&stream.parameters(), budget, Metadata::Initial)?;
            }
            Ok(input)
        }
    }
    pub fn find_stream_info(&mut self, budget: u64) -> Result<(), Error> {
        let mut options = vec![ptr::null_mut(); self.format.nb_streams() as usize];
        // SAFETY: options has one writable dictionary slot per bounded stream;
        // FFmpeg cannot add streams beyond the frozen max_streams limit.
        unsafe {
            for (option, stream) in options.iter_mut().zip(self.format.streams()) {
                let bound = allocation(&stream.parameters(), budget, Metadata::Initial)?;
                ffi::av_dict_set(option, c"threads".as_ptr(), c"1".as_ptr(), 0);
                ffi::av_dict_set_int(option, c"max_pixels".as_ptr(), bound.native_pixels, 0);
                ffi::av_dict_set_int(option, c"max_samples".as_ptr(), 65_536, 0);
            }
            let result =
                ffi::avformat_find_stream_info(self.format.as_mut_ptr(), options.as_mut_ptr());
            for option in &mut options {
                ffi::av_dict_free(option);
            }
            if result < 0 {
                return Err(Error::UnsupportedMedia);
            }
        }
        self.avio.check()?;
        for stream in self.format.streams() {
            allocation(&stream.parameters(), budget, Metadata::Resolved)?;
        }
        Ok(())
    }
    pub fn check(&mut self) -> Result<(), Error> {
        self.avio.check()
    }
    pub fn decoder_context(
        parameters: ffmpeg::codec::Parameters,
        budget: u64,
    ) -> Result<ffmpeg::codec::context::Context, Error> {
        let bound = allocation(&parameters, budget, Metadata::Resolved)?;
        let mut context = ffmpeg::codec::context::Context::from_parameters(parameters)
            .map_err(|_| Error::UnsupportedMedia)?;
        context.set_threading(ffmpeg::codec::threading::Config {
            count: 1,
            kind: ffmpeg::codec::threading::Type::None,
        });
        // SAFETY: context owns a live AVCodecContext; these limits precede decoder open.
        unsafe {
            (*context.as_mut_ptr()).max_pixels = bound.native_pixels;
            (*context.as_mut_ptr()).max_samples = 65_536;
        }
        Ok(context)
    }
    pub fn packet(&mut self, index: usize) -> Result<Option<ffmpeg::Packet>, Error> {
        loop {
            let mut packet = ffmpeg::Packet::empty();
            self.avio.check()?;
            let result = packet.read(&mut self.format);
            self.avio.check()?;
            match result {
                Ok(()) if packet.stream() == index => return Ok(Some(packet)),
                Ok(()) => {}
                Err(ffmpeg::Error::Eof) => return Ok(None),
                Err(_) => return Err(Error::UnsupportedMedia),
            }
        }
    }
    pub fn check_frame(
        &self,
        index: usize,
        decoder: &ffmpeg::decoder::Video,
        frame: &ffmpeg::frame::Video,
        budget: u64,
    ) -> Result<(), Error> {
        let stream = self.format.stream(index).ok_or(Error::UnsupportedMedia)?;
        let bound = allocation(&stream.parameters(), budget, Metadata::Resolved)?;
        // SAFETY: decoder owns a live context throughout this post-decode check.
        let references = unsafe { (*decoder.as_ptr()).refs };
        if u64::try_from(references).is_ok_and(|references| references > bound.references)
            || u64::from(frame.width()) * u64::from(frame.height()) > bound.pixels.cast_unsigned()
            || frame.width() != decoder.width()
            || frame.height() != decoder.height()
        {
            return Err(Error::UnsupportedMedia);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Eq, PartialEq)]
enum Metadata {
    Initial,
    Resolved,
}

struct Bound {
    /// The declared picture, checked against each decoded frame.
    pixels: i64,
    /// `FFmpeg`'s `max_pixels`: the reserved picture envelope. Decoders check
    /// it against their padded allocation, not the declared picture: the
    /// stride-aligned width (`STRIDE_ALIGN`, up to 64), and for MPEG-4 Part 2
    /// and H.263 a 16-pixel edge on every side. The hard budget already
    /// reserves this envelope per frame, so it admits no extra memory.
    native_pixels: i64,
    references: u64,
}
#[allow(clippy::too_many_lines, reason = "audited codec allocation table")]
fn allocation(
    parameters: &ffmpeg::codec::Parameters,
    budget: u64,
    metadata: Metadata,
) -> Result<Bound, Error> {
    if !kinewright_media_input::codec_policy::allows(parameters.id()) {
        return Err(Error::UnsupportedMedia);
    }
    // SAFETY: the parameters owner keeps this AVCodecParameters alive throughout inspection.
    let p = unsafe { &*parameters.as_ptr() };
    if p.codec_type == ffi::AVMediaType::AVMEDIA_TYPE_AUDIO {
        // MP3 and ADTS headers leave these unset until stream-info runs. That
        // bounded native parse still has max_samples and the hard process budget;
        // unset values never escape the resolved-metadata check below.
        let minimum = i32::from(metadata == Metadata::Resolved);
        if !(minimum..=32).contains(&p.ch_layout.nb_channels)
            || !(minimum..=384_000).contains(&p.sample_rate)
        {
            return Err(Error::UnsupportedMedia);
        }
        return Ok(Bound {
            pixels: 0,
            native_pixels: 0,
            references: 0,
        });
    }
    if p.codec_type != ffi::AVMediaType::AVMEDIA_TYPE_VIDEO
        || !(1..=8192).contains(&p.width)
        || !(1..=8192).contains(&p.height)
    {
        return Err(Error::UnsupportedMedia);
    }
    let pixels = i64::from(p.width) * i64::from(p.height);
    if pixels.cast_unsigned() * 8 > 128 * 1024 * 1024 {
        return Err(Error::UnsupportedMedia);
    }
    // Headers cannot increase the hard native reservation. Unknown pixel formats
    // use the largest supported packed sample footprint until stream-info resolves it.
    let mut bytes_per_pixel = match p.codec_id {
        ffi::AVCodecID::AV_CODEC_ID_H264
        | ffi::AVCodecID::AV_CODEC_ID_HEVC
        | ffi::AVCodecID::AV_CODEC_ID_AV1
        | ffi::AVCodecID::AV_CODEC_ID_VP9
        | ffi::AVCodecID::AV_CODEC_ID_VP8 => 8,
        _ => 16,
    };
    if let Some(bytes) = storage_bytes(p.format) {
        bytes_per_pixel = bytes;
    }
    let dpb = match p.codec_id {
        ffi::AVCodecID::AV_CODEC_ID_H264 => {
            let max_mbs = match p.level {
                10 => 396,
                11 => 900,
                12 | 13 | 20 => 2376,
                21 => 4752,
                22 | 30 => 8100,
                31 => 18_000,
                32 => 20_480,
                40 | 41 => 32_768,
                42 => 34_816,
                50 => 110_400,
                51 | 52 => 184_320,
                _ => usize::MAX,
            };
            let picture_mbs = u64::try_from(p.width).unwrap_or(8192).div_ceil(16)
                * u64::try_from(p.height).unwrap_or(8192).div_ceil(16);
            let references = (max_mbs as u64 / picture_mbs).min(16);
            if references == 0 {
                return Err(Error::UnsupportedMedia);
            }
            references
        }
        ffi::AVCodecID::AV_CODEC_ID_HEVC => {
            let max_luma = match p.level {
                30 => 36_864,
                60 => 122_880,
                63 => 245_760,
                90 => 552_960,
                93 => 983_040,
                120 | 123 => 2_228_224,
                150 | 153 | 156 => 8_912_896,
                180 | 183 | 186 => 35_651_584,
                _ => i64::MAX,
            };
            if pixels <= max_luma / 4 {
                16
            } else if pixels <= max_luma / 2 {
                12
            } else if pixels <= max_luma / 4 * 3 {
                8
            } else {
                6
            }
        }
        ffi::AVCodecID::AV_CODEC_ID_AV1 | ffi::AVCodecID::AV_CODEC_ID_VP9 => 8,
        _ => 1,
    };
    // The reserved picture envelope: 128-aligned with 256 padding per axis.
    let envelope = (u64::try_from(p.width)
        .map_err(|_| Error::UnsupportedMedia)?
        .div_ceil(128)
        * 128
        + 256)
        * (u64::try_from(p.height)
            .map_err(|_| Error::UnsupportedMedia)?
            .div_ceil(128)
            * 128
            + 256);
    let frame = envelope * bytes_per_pixel;
    // One decoder thread, no scaler, one accepted frame plus native scratch.
    if frame
        .checked_mul(dpb + 3)
        .is_none_or(|bytes| bytes > budget)
    {
        return Err(Error::ResourceLimit);
    }
    Ok(Bound {
        pixels,
        native_pixels: i64::try_from(envelope).map_err(|_| Error::UnsupportedMedia)?,
        references: dpb,
    })
}
fn still_dimensions(header: &[u8]) -> Option<(u32, u32)> {
    if header.starts_with(b"\x89PNG\r\n\x1a\n") && header.get(12..16)? == b"IHDR" {
        return Some((
            u32::from_be_bytes(header.get(16..20)?.try_into().ok()?),
            u32::from_be_bytes(header.get(20..24)?.try_into().ok()?),
        ));
    }
    if !header.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let mut offset = 2;
    while header.get(offset) == Some(&0xff) {
        while header.get(offset) == Some(&0xff) {
            offset += 1;
        }
        let marker = *header.get(offset)?;
        offset += 1;
        let length = usize::from(u16::from_be_bytes(
            header.get(offset..offset + 2)?.try_into().ok()?,
        ));
        if length < 2 {
            return None;
        }
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            let height = u16::from_be_bytes(header.get(offset + 3..offset + 5)?.try_into().ok()?);
            let width = u16::from_be_bytes(header.get(offset + 5..offset + 7)?.try_into().ok()?);
            return Some((u32::from(width), u32::from(height)));
        }
        offset = offset.checked_add(length)?;
    }
    None
}

impl kinewright_media_input::owned_io::Cursor for Cursor {
    type Error = Error;
    fn read(&mut self, output: &mut [u8]) -> Result<usize, Self::Error> {
        self.check()?;
        if self.read >= IO_LIMIT {
            self.failure = Some(Error::ResourceLimit);
            return Err(Error::ResourceLimit);
        }
        let remaining = usize::try_from(IO_LIMIT - self.read).map_err(|_| Error::ResourceLimit)?;
        let length = output.len().min(remaining);
        let count = self
            .file
            .read_at(&mut output[..length], self.position)
            .map_err(|_| Error::UnsupportedMedia)?;
        self.read += count as u64;
        self.position = self
            .position
            .checked_add(count as u64)
            .ok_or(Error::UnsupportedMedia)?;
        Ok(count)
    }
    fn seek(&mut self, position: SeekFrom) -> Result<u64, Self::Error> {
        self.check()?;
        let next = match position {
            SeekFrom::Start(position) => i128::from(position),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.length) + i128::from(offset),
        };
        if next < 0 || next > i128::from(self.length) {
            return Err(Error::UnsupportedMedia);
        }
        self.position = u64::try_from(next).map_err(|_| Error::UnsupportedMedia)?;
        Ok(self.position)
    }
    fn size(&mut self) -> Result<u64, Self::Error> {
        Ok(self.length)
    }
    fn check(&mut self) -> Result<(), Self::Error> {
        if self.cancel.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        self.failure.map_or(Ok(()), Err)
    }
    fn temporarily_unavailable(_error: &Self::Error) -> bool {
        false
    }
    fn capacity(&mut self, _message: &str) -> Self::Error {
        Error::ResourceLimit
    }
    fn decoder(&mut self, _message: String) -> Self::Error {
        Error::UnsupportedMedia
    }
    fn unavailable(&mut self, _message: &str) -> Self::Error {
        self.failure = Some(Error::UnsupportedMedia);
        Error::UnsupportedMedia
    }
}

fn storage_bytes(format: i32) -> Option<u64> {
    if format < 0 {
        return None;
    }
    // SAFETY: this walks FFmpeg's immutable descriptor table, never constructs
    // a Rust enum from an untrusted numeric pixel-format header.
    unsafe {
        let mut descriptor = ptr::null();
        for _ in 0..512 {
            descriptor = ffi::av_pix_fmt_desc_next(descriptor);
            if descriptor.is_null() {
                return None;
            }
            if ffi::av_pix_fmt_desc_get_id(descriptor) as i32 == format {
                return u64::try_from(ffi::av_get_padded_bits_per_pixel(descriptor))
                    .ok()
                    .filter(|bits| *bits > 0)
                    .map(|bits| bits.div_ceil(8));
            }
        }
    }
    None
}
