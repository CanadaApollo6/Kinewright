//! Descriptor-only access to project-certified source bytes.
#![allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]

use std::{
    fmt,
    fmt::Write as _,
    fs::File,
    io::{self, SeekFrom},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use ffmpeg_next as ffmpeg;
use kinewright_core::{ColorDescription, MediaKind, Rational, TimeCode};
use kinewright_hash::Sha256;

pub const CHUNK_BYTES: usize = 1_048_576;
pub const CACHE_SLOTS: usize = 4;
pub const MAX_CERTIFIED_STREAMS: usize = 16;
pub const MAX_DECODER_DIMENSION: u32 = crate::decode::MAX_STILL_DIMENSION;
const MAX_AUDIO_FRAME_SAMPLES: u64 = 65_536;
const MAX_AUDIO_CHANNELS: u32 = 32;
const CODEC_BUFFER_COUNT: u64 = 24;
pub const AVIO_BUFFER_BYTES: usize = 65_536;
pub const CURSOR_BOOKKEEPING_BYTES: usize = 4_096;
pub const CURSOR_RESERVATION_BYTES: u64 =
    (CHUNK_BYTES * CACHE_SLOTS + AVIO_BUFFER_BYTES + CURSOR_BOOKKEEPING_BYTES) as u64;

pub type LifetimeGuard = Arc<dyn Send + Sync>;
pub type CursorReservation = dyn Fn(u64) -> Result<LifetimeGuard, String> + Send + Sync;

#[path = "certified_playback.rs"]
mod playback;
pub use playback::PlaybackInputs;
#[path = "owned_io_memo.rs"]
mod memo;
pub use memo::SourceAdmissionMemo;

/// The existing PF1 decoders use the same format interface for legacy media
/// and descriptor-only Studio media. The latter retains its AVIO and ledger.
pub(crate) enum DecodeInput {
    Legacy(ffmpeg::format::context::Input),
    Certified(Box<CertifiedInput>),
}
impl std::ops::Deref for DecodeInput {
    type Target = ffmpeg::format::context::Input;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Legacy(input) => input,
            Self::Certified(input) => &input.format,
        }
    }
}
impl std::ops::DerefMut for DecodeInput {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Legacy(input) => input,
            Self::Certified(input) => &mut input.format,
        }
    }
}
impl DecodeInput {
    #[cfg(test)]
    pub(crate) fn cached_chunks(&self) -> Vec<u64> {
        match self {
            Self::Legacy(_) => vec![],
            Self::Certified(input) => input
                .avio
                .cursor
                .slots
                .iter()
                .filter_map(|slot| slot.index)
                .collect(),
        }
    }
    pub(crate) fn open(
        source: &CertifiedSource,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self, kinewright_core::MediaError> {
        CertifiedInput::open(source, cancel)
            .map(|input| Self::Certified(Box::new(input)))
            .map_err(media_io_error)
    }
    pub(crate) fn check(&mut self) -> Result<(), kinewright_core::MediaError> {
        if let Self::Certified(input) = self {
            input.check().map_err(media_io_error)?;
        }
        Ok(())
    }
    pub(crate) fn video(
        &mut self,
        frame: &ffmpeg::frame::Video,
    ) -> Result<(), kinewright_core::MediaError> {
        self.check()?;
        if let Self::Certified(input) = self {
            input
                .decoder_bound
                .expect("opened decoder")
                .check_video(frame)
                .map_err(media_io_error)?;
        }
        Ok(())
    }
    pub(crate) fn audio(
        &mut self,
        frame: &ffmpeg::frame::Audio,
    ) -> Result<(), kinewright_core::MediaError> {
        self.check()?;
        if let Self::Certified(input) = self {
            input
                .decoder_bound
                .expect("opened decoder")
                .check_audio(frame)
                .map_err(media_io_error)?;
        }
        Ok(())
    }
    pub(crate) fn context(
        &mut self,
        source: &CertifiedSource,
        index: usize,
        video: bool,
    ) -> Result<ffmpeg::codec::context::Context, kinewright_core::MediaError> {
        source
            .check_selected(index, video)
            .map_err(media_io_error)?;
        let parameters = self
            .stream(index)
            .ok_or_else(|| media_io_error(decoder_error("selected stream missing")))?
            .parameters();
        let bound = bridge::decoder_bound(&parameters).map_err(media_io_error)?;
        let charge = (source.0.reserve)(bound.bytes)
            .map_err(|e| media_io_error(OwnedIoError::Capacity(e)))?;
        let context = bridge::decoder_context(parameters, bound).map_err(media_io_error)?;
        if let Self::Certified(input) = self {
            input.decoder_bound = Some(bound);
            input.probe_charges.push(charge);
        }
        Ok(context)
    }
}
#[allow(
    clippy::needless_pass_by_value,
    reason = "map_err transfers the owned decoder error"
)]
fn media_io_error(error: OwnedIoError) -> kinewright_core::MediaError {
    kinewright_core::MediaError::Backend(error.to_string())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnedIoError {
    InvalidCertificate(String),
    Capacity(String),
    Damaged(String),
    Unavailable(String),
    Cancelled,
    Decoder(String),
}

impl fmt::Display for OwnedIoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCertificate(message) => write!(f, "invalid input certificate: {message}"),
            Self::Capacity(message) => write!(f, "input cursor capacity refused: {message}"),
            Self::Damaged(message) => write!(f, "damaged certified input: {message}"),
            Self::Unavailable(message) => write!(f, "certified input unavailable: {message}"),
            Self::Cancelled => f.write_str("certified input cancelled"),
            Self::Decoder(message) => write!(f, "certified decoder: {message}"),
        }
    }
}
impl std::error::Error for OwnedIoError {}

#[cfg(any(test, feature = "test-util"))]
thread_local! {
    static STREAM_INFO_PROBE_CALLS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Count `FFmpeg` stream-info probe entries on this thread during an operation.
#[cfg(any(test, feature = "test-util"))]
pub fn observe_stream_info_probes<T>(operation: impl FnOnce() -> T) -> (T, u64) {
    let before = STREAM_INFO_PROBE_CALLS.get();
    let result = operation();
    (result, STREAM_INFO_PROBE_CALLS.get() - before)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CertifiedIdentity {
    pub source_sha256: [u8; 32],
    pub source_length: u64,
    pub table_sha256: [u8; 32],
}

struct Source {
    file: File,
    verified_identity: OnceLock<memo::FileIdentity>,
    failure: Mutex<Option<OwnedIoError>>,
    identity: CertifiedIdentity,
    chunks: Arc<[[u8; 32]]>,
    _lifetime: LifetimeGuard,
    reserve: Arc<CursorReservation>,
    #[cfg(any(test, feature = "test-util"))]
    accepted_output_bytes: std::sync::atomic::AtomicU64,
    #[cfg(test)]
    hooks: TestHooks,
}

/// The project owner certifies the descriptor, table object and complete binding.
/// This port retains that authority; it cannot discover a replacement by pathname.
#[derive(Clone)]
pub struct CertifiedSource(Arc<Source>, Option<SelectedStreams>);

#[derive(Clone, Copy)]
struct SelectedStreams {
    video: Option<usize>,
    audio: Option<usize>,
}

impl CertifiedSource {
    pub(crate) fn still_orientation(
        &self,
    ) -> Result<Option<crate::still_orientation::StillOrientation>, OwnedIoError> {
        let mut cursor = self.cursor(Arc::new(AtomicBool::new(false)))?;
        let mut header = vec![0_u8; AVIO_BUFFER_BYTES];
        let count = cursor.read(&mut header)?;
        Ok(crate::still_orientation::still_bytes_orientation(
            &header[..count],
        ))
    }
    /// Admit a descriptor by checking its complete fingerprint and every chunk.
    /// Project admission additionally binds these identities to its manifest.
    /// No public unchecked constructor can manufacture a certified source.
    #[allow(clippy::too_many_arguments)]
    pub fn verify(
        file: File,
        source_sha256: [u8; 32],
        source_length: u64,
        table_sha256: [u8; 32],
        chunk_sha256: Arc<[[u8; 32]]>,
        lifetime: LifetimeGuard,
        reserve_cursor: Arc<CursorReservation>,
    ) -> Result<Self, OwnedIoError> {
        let source = Self::new(
            file,
            source_sha256,
            source_length,
            table_sha256,
            chunk_sha256,
            lifetime,
            reserve_cursor,
        )?;
        let mut cursor = source.cursor(Arc::new(AtomicBool::new(false)))?;
        let mut digest = Sha256::new();
        let mut buffer = vec![0_u8; AVIO_BUFFER_BYTES];
        loop {
            let count = cursor.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        if digest.finalize() != source_sha256 {
            return Err(OwnedIoError::InvalidCertificate(
                "complete source digest".into(),
            ));
        }
        memo::mark_verified(&source)?;
        Ok(source)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        file: File,
        source_sha256: [u8; 32],
        source_length: u64,
        table_sha256: [u8; 32],
        chunk_sha256: Arc<[[u8; 32]]>,
        lifetime: LifetimeGuard,
        reserve_cursor: Arc<CursorReservation>,
    ) -> Result<Self, OwnedIoError> {
        let count = source_length.div_ceil(CHUNK_BYTES as u64);
        if usize::try_from(count).ok() != Some(chunk_sha256.len())
            || source_length > i64::MAX as u64
        {
            return Err(OwnedIoError::InvalidCertificate(
                "chunk count or source length".into(),
            ));
        }
        let metadata = file.metadata().map_err(unavailable)?;
        if !metadata.is_file() || metadata.len() != source_length {
            return Err(OwnedIoError::Damaged(
                "descriptor length differs from certificate".into(),
            ));
        }
        Ok(Self(
            Arc::new(Source {
                file,
                verified_identity: OnceLock::new(),
                failure: Mutex::new(None),
                identity: CertifiedIdentity {
                    source_sha256,
                    source_length,
                    table_sha256,
                },
                chunks: chunk_sha256,
                _lifetime: lifetime,
                reserve: reserve_cursor,
                #[cfg(any(test, feature = "test-util"))]
                accepted_output_bytes: std::sync::atomic::AtomicU64::new(0),
                #[cfg(test)]
                hooks: TestHooks::default(),
            }),
            None,
        ))
    }

    pub fn with_selected_streams(
        mut self,
        video: Option<usize>,
        audio: Option<usize>,
    ) -> Result<Self, OwnedIoError> {
        if video.is_none() && audio.is_none() {
            return Err(OwnedIoError::InvalidCertificate(
                "no selected streams".into(),
            ));
        }
        if let Some(allowed) = self.1
            && (video.is_some_and(|index| Some(index) != allowed.video)
                || audio.is_some_and(|index| Some(index) != allowed.audio))
        {
            return Err(OwnedIoError::InvalidCertificate(
                "cannot widen selected streams".into(),
            ));
        }
        self.1 = Some(SelectedStreams { video, audio });
        Ok(self)
    }

    fn check_selected(&self, index: usize, video: bool) -> Result<(), OwnedIoError> {
        if self.1.is_some_and(|selection| {
            (if video {
                selection.video
            } else {
                selection.audio
            }) != Some(index)
        }) {
            return Err(OwnedIoError::InvalidCertificate(
                "stream is outside certified interpretation".into(),
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn identity(&self) -> CertifiedIdentity {
        self.0.identity
    }

    /// First-plane bytes in decoded frames accepted across this source's decoders.
    /// Zero means no decoded output was accepted, including through nested inputs.
    #[cfg(any(test, feature = "test-util"))]
    #[must_use]
    pub fn accepted_output_bytes(&self) -> u64 {
        self.0.accepted_output_bytes.load(Ordering::Acquire)
    }

    pub fn cursor(&self, cancel: Arc<AtomicBool>) -> Result<VerifiedCursor, OwnedIoError> {
        let charge = (self.0.reserve)(CURSOR_RESERVATION_BYTES).map_err(OwnedIoError::Capacity)?;
        let mut cursor = VerifiedCursor {
            source: Arc::clone(&self.0),
            slots: std::array::from_fn(|_| Slot {
                index: None,
                bytes: Vec::new(),
            }),
            next_slot: 0,
            position: 0,
            failure: None,
            cancel,
            _charge: charge,
            #[cfg(any(test, feature = "test-util"))]
            delivered_bytes: 0,
        };
        cursor.check()?;
        for slot in &mut cursor.slots {
            slot.bytes
                .try_reserve_exact(CHUNK_BYTES)
                .map_err(|error| OwnedIoError::Capacity(error.to_string()))?;
            slot.bytes.resize(CHUNK_BYTES, 0);
        }
        Ok(cursor)
    }

    #[allow(clippy::too_many_lines)]
    pub fn probe(&self, cancel: Arc<AtomicBool>) -> Result<CertifiedProbe, OwnedIoError> {
        let mut input = CertifiedInput::open(self, cancel)?;
        let streams = input
            .format
            .streams()
            .map(|stream| CertifiedStream {
                index: stream.index(),
                kind: match stream.parameters().medium() {
                    ffmpeg::media::Type::Video => StreamKind::Video,
                    ffmpeg::media::Type::Audio => StreamKind::Audio,
                    _ => StreamKind::Other,
                },
                time_base: (
                    stream.time_base().numerator(),
                    stream.time_base().denominator(),
                ),
                timestamp_origin: normalized_origin(stream.start_time()),
                duration: stream.duration(),
                average_frame_rate: (
                    stream.avg_frame_rate().numerator(),
                    stream.avg_frame_rate().denominator(),
                ),
            })
            .collect();
        let duration_microseconds = input.format.duration();
        let video_index = self.1.map_or_else(
            || {
                input
                    .format
                    .streams()
                    .best(ffmpeg::media::Type::Video)
                    .map(|stream| stream.index())
            },
            |selection| selection.video,
        );
        let audio_index = self.1.map_or_else(
            || {
                input
                    .format
                    .streams()
                    .best(ffmpeg::media::Type::Audio)
                    .map(|stream| stream.index())
            },
            |selection| selection.audio,
        );
        for (index, medium) in [
            (video_index, ffmpeg::media::Type::Video),
            (audio_index, ffmpeg::media::Type::Audio),
        ] {
            if let Some(index) = index {
                let stream = input.format.stream(index).ok_or_else(|| {
                    OwnedIoError::InvalidCertificate("missing selected stream".into())
                })?;
                if stream.parameters().medium() != medium {
                    return Err(OwnedIoError::InvalidCertificate(
                        "selected stream kind".into(),
                    ));
                }
            }
        }
        let kind = match (video_index, audio_index) {
            (Some(_), Some(_)) => MediaKind::AudioVideo,
            (Some(_), None) => MediaKind::Video,
            (None, Some(_)) => MediaKind::Audio,
            (None, None) => return Err(decoder_error("no decodable audio or video streams")),
        };
        let mut result = CertifiedProbe {
            streams,
            duration_microseconds,
            video_stream: video_index,
            audio_stream: audio_index,
            duration: TimeCode::ZERO,
            fps: Rational::default(),
            kind,
            resolution: None,
            color_description: ColorDescription::unknown(),
        };
        if let Some(index) = video_index {
            let stream = input.format.stream(index).ok_or_else(|| {
                OwnedIoError::InvalidCertificate("missing selected video stream".into())
            })?;
            let time_base = stream.time_base();
            let stream_duration = stream.duration();
            let mut rotation = crate::decode::stream_rotation(&stream).map_err(decoder_error)?;
            let rate = [stream.avg_frame_rate(), stream.rate()]
                .into_iter()
                .find_map(|rate| {
                    let numerator = u32::try_from(rate.numerator()).ok()?;
                    let denominator = u32::try_from(rate.denominator()).ok()?;
                    Rational::new(numerator, denominator).ok()
                });
            self.check_selected(index, true)?;
            let parameters = stream.parameters();
            if parameters.medium() != ffmpeg::media::Type::Video {
                return Err(OwnedIoError::InvalidCertificate(
                    "selected video kind".into(),
                ));
            }
            let bound = bridge::decoder_bound(&parameters)?;
            let charge = (self.0.reserve)(bound.bytes).map_err(OwnedIoError::Capacity)?;
            let context = bridge::decoder_context(parameters, bound)?;
            let decoder = context.decoder().video().map_err(decoder_error)?;
            let still = audio_index.is_none() && crate::decode::is_still_image_codec(decoder.id());
            let mut video = CertifiedVideoDecoder {
                decoder,
                input,
                stream_index: index,
                eof: false,
                bound,
                charge,
                source: self.clone(),
            };
            let frame = video
                .next_frame()?
                .ok_or_else(|| decoder_error("video has no decoded frame"))?;
            if still {
                let mut cursor = self.cursor(video.input.avio.cancel())?;
                let _header_charge =
                    (self.0.reserve)(AVIO_BUFFER_BYTES as u64).map_err(OwnedIoError::Capacity)?;
                let mut header = Vec::new();
                header
                    .try_reserve_exact(AVIO_BUFFER_BYTES)
                    .map_err(|error| OwnedIoError::Capacity(error.to_string()))?;
                header.resize(AVIO_BUFFER_BYTES, 0);
                let count = cursor.read(&mut header)?;
                if let Some(orientation) =
                    crate::still_orientation::still_bytes_orientation(&header[..count])
                {
                    rotation = orientation.rotation;
                }
                result.kind = MediaKind::Image;
                result.duration = TimeCode(1);
            } else {
                result.fps = rate.ok_or_else(|| decoder_error("video frame rate is unknown"))?;
                let frames =
                    crate::decode::timestamp_to_grid_ceil(stream_duration, time_base, result.fps);
                result.duration = TimeCode(if frames > 0 {
                    frames
                } else {
                    crate::decode::duration_us_to_frames(duration_microseconds, result.fps)
                });
            }
            result.resolution = Some(rotation.display_dimensions(frame.width(), frame.height()));
            result.color_description =
                crate::decode::color_description_from_decoder(&video.decoder, Some(frame.format()));
            video.input.check()?;
        } else {
            result.duration = TimeCode(crate::decode::duration_us_to_frames(
                duration_microseconds,
                result.fps,
            ));
            if let Some(index) = audio_index {
                let stream = input.format.stream(index).ok_or_else(|| {
                    OwnedIoError::InvalidCertificate("missing selected audio stream".into())
                })?;
                self.check_selected(index, false)?;
                let parameters = stream.parameters();
                if parameters.medium() != ffmpeg::media::Type::Audio {
                    return Err(OwnedIoError::InvalidCertificate(
                        "selected audio kind".into(),
                    ));
                }
                let bound = bridge::decoder_bound(&parameters)?;
                let _charge = (self.0.reserve)(bound.bytes).map_err(OwnedIoError::Capacity)?;
                let context = bridge::decoder_context(parameters, bound)?;
                let _decoder = context.decoder().audio().map_err(decoder_error)?;
            }
            input.check()?;
        }
        if result.duration <= TimeCode::ZERO
            || result.resolution.is_some_and(|(w, h)| w == 0 || h == 0)
        {
            return Err(decoder_error("missing or zero media duration/dimensions"));
        }
        Ok(result)
    }

    pub fn probe_asset(
        &self,
        id: kinewright_core::AssetId,
        locator: &std::path::Path,
        cancel: Arc<AtomicBool>,
    ) -> Result<kinewright_core::MediaAsset, OwnedIoError> {
        let probe = self.probe(cancel)?;
        let mut sha256 = String::with_capacity(64);
        for byte in self.0.identity.source_sha256 {
            write!(sha256, "{byte:02x}").expect("writing to a String cannot fail");
        }
        Ok(kinewright_core::MediaAsset {
            id,
            path: locator.to_path_buf(),
            name: locator
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("media")
                .to_owned(),
            duration: probe.duration,
            fps: probe.fps,
            kind: probe.kind,
            resolution: probe.resolution,
            source_fingerprint: kinewright_core::MediaSourceFingerprint {
                content_sha256: Some(sha256),
                byte_len: Some(self.0.identity.source_length),
            },
            color_description: probe.color_description,
            assumed_from: None,
        })
    }

    pub fn open_video(
        &self,
        stream_index: usize,
        cancel: Arc<AtomicBool>,
    ) -> Result<CertifiedVideoDecoder, OwnedIoError> {
        self.check_selected(stream_index, true)?;
        let mut input = CertifiedInput::open(self, cancel)?;
        let stream = input
            .format
            .stream(stream_index)
            .ok_or_else(|| decoder_error("missing selected video stream"))?;
        if stream.parameters().medium() != ffmpeg::media::Type::Video {
            return Err(decoder_error("selected stream is not video"));
        }
        let parameters = stream.parameters();
        let bound = bridge::decoder_bound(&parameters)?;
        let charge = (self.0.reserve)(bound.bytes).map_err(OwnedIoError::Capacity)?;
        let context = bridge::decoder_context(parameters, bound)?;
        let decoder = context.decoder().video().map_err(decoder_error)?;
        input.check()?;
        Ok(CertifiedVideoDecoder {
            decoder,
            input,
            stream_index,
            eof: false,
            bound,
            charge,
            source: self.clone(),
        })
    }

    pub fn open_audio(
        &self,
        stream_index: usize,
        cancel: Arc<AtomicBool>,
    ) -> Result<CertifiedAudioDecoder, OwnedIoError> {
        self.check_selected(stream_index, false)?;
        let mut input = CertifiedInput::open(self, cancel)?;
        let stream = input
            .format
            .stream(stream_index)
            .ok_or_else(|| decoder_error("missing selected audio stream"))?;
        if stream.parameters().medium() != ffmpeg::media::Type::Audio {
            return Err(decoder_error("selected stream is not audio"));
        }
        let parameters = stream.parameters();
        let bound = bridge::decoder_bound(&parameters)?;
        let charge = (self.0.reserve)(bound.bytes).map_err(OwnedIoError::Capacity)?;
        let context = bridge::decoder_context(parameters, bound)?;
        let decoder = context.decoder().audio().map_err(decoder_error)?;
        input.check()?;
        Ok(CertifiedAudioDecoder {
            decoder,
            input,
            stream_index,
            eof: false,
            bound,
            charge,
            source: self.clone(),
        })
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "map_err adapters consume owned I/O errors"
)]
fn unavailable(error: io::Error) -> OwnedIoError {
    OwnedIoError::Unavailable(error.to_string())
}
fn decoder_error(error: impl fmt::Display) -> OwnedIoError {
    OwnedIoError::Decoder(error.to_string())
}
fn normalized_origin(value: i64) -> i64 {
    if value == i64::MIN { 0 } else { value }
}

struct Slot {
    index: Option<u64>,
    bytes: Vec<u8>,
}

/// Four private slots, scoped to one complete source/table identity and logical cursor.
pub struct VerifiedCursor {
    source: Arc<Source>,
    slots: [Slot; CACHE_SLOTS],
    next_slot: usize,
    position: u64,
    failure: Option<OwnedIoError>,
    cancel: Arc<AtomicBool>,
    _charge: LifetimeGuard,
    #[cfg(any(test, feature = "test-util"))]
    delivered_bytes: u64,
}
const _: () = assert!(std::mem::size_of::<VerifiedCursor>() <= CURSOR_BOOKKEEPING_BYTES);

impl VerifiedCursor {
    fn fail<T>(&mut self, error: OwnedIoError) -> Result<T, OwnedIoError> {
        if matches!(
            error,
            OwnedIoError::Damaged(_) | OwnedIoError::Unavailable(_)
        ) {
            self.source
                .failure
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get_or_insert_with(|| error.clone());
        }
        Err(self.failure.get_or_insert(error).clone())
    }

    fn check(&mut self) -> Result<(), OwnedIoError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        if let Some(error) = self
            .source
            .failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            self.failure = Some(error.clone());
            return Err(error);
        }
        if self.cancel.load(Ordering::Acquire) {
            return self.fail(OwnedIoError::Cancelled);
        }
        let metadata = match self.source.file.metadata() {
            Ok(metadata) => metadata,
            Err(error) => return self.fail(unavailable(error)),
        };
        if metadata.len() != self.source.identity.source_length {
            return self.fail(OwnedIoError::Damaged("descriptor length changed".into()));
        }
        Ok(())
    }

    pub fn size(&mut self) -> Result<u64, OwnedIoError> {
        self.check()?;
        Ok(self.source.identity.source_length)
    }

    pub fn seek(&mut self, seek: SeekFrom) -> Result<u64, OwnedIoError> {
        self.check()?;
        let position = match seek {
            SeekFrom::Start(position) => i128::from(position),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
            SeekFrom::End(offset) => {
                i128::from(self.source.identity.source_length) + i128::from(offset)
            }
        };
        if !(0..=i128::from(self.source.identity.source_length)).contains(&position) {
            return self.fail(OwnedIoError::Damaged(
                "seek outside certified source".into(),
            ));
        }
        self.position = u64::try_from(position).expect("bounded certified position");
        Ok(self.position)
    }

    pub fn read(&mut self, output: &mut [u8]) -> Result<usize, OwnedIoError> {
        self.check()?;
        if output.is_empty() || self.position == self.source.identity.source_length {
            return Ok(0);
        }
        let index = self.position / CHUNK_BYTES as u64;
        let slot = if let Some(slot) = self.slots.iter().position(|slot| slot.index == Some(index))
        {
            slot
        } else {
            self.fill(index)?
        };
        self.check()?;
        let offset = usize::try_from(self.position % CHUNK_BYTES as u64).expect("chunk offset");
        let remaining = usize::try_from(
            (self.source.identity.source_length - self.position).min(CHUNK_BYTES as u64),
        )
        .expect("bounded remaining bytes");
        let count = output.len().min(CHUNK_BYTES - offset).min(remaining);
        output[..count].copy_from_slice(&self.slots[slot].bytes[offset..offset + count]);
        self.check()?;
        self.position += count as u64;
        Ok(count)
    }

    fn fill(&mut self, index: u64) -> Result<usize, OwnedIoError> {
        self.check()?;
        let slot_index = self.next_slot;
        self.next_slot = (self.next_slot + 1) % CACHE_SLOTS;
        let slot = &mut self.slots[slot_index];
        slot.index = None;
        let start = index * CHUNK_BYTES as u64;
        let length =
            usize::try_from((self.source.identity.source_length - start).min(CHUNK_BYTES as u64))
                .expect("chunk length");
        let mut filled = 0;
        while filled < length {
            let result = {
                #[cfg(test)]
                if self.source.hooks.short_read.swap(false, Ordering::AcqRel) {
                    Ok(0)
                } else {
                    positioned_read(
                        &self.source.file,
                        &mut slot.bytes[filled..length],
                        start + filled as u64,
                    )
                }
                #[cfg(not(test))]
                positioned_read(
                    &self.source.file,
                    &mut slot.bytes[filled..length],
                    start + filled as u64,
                )
            };
            match result {
                Ok(0) => {
                    return self.fail(OwnedIoError::Damaged(
                        "premature short descriptor read".into(),
                    ));
                }
                Ok(count) => filled += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return self.fail(unavailable(error)),
            }
        }
        #[cfg(test)]
        {
            self.source.hooks.readable.store(
                self.slots
                    .iter()
                    .filter(|slot| slot.index.is_some())
                    .count(),
                Ordering::Release,
            );
            self.source.hooks.before_verify(index);
        }
        let mut digest = Sha256::new();
        digest.update(&self.slots[slot_index].bytes[..length]);
        if digest.finalize()
            != self.source.chunks[usize::try_from(index).expect("certified chunk index")]
        {
            return self.fail(OwnedIoError::Damaged(format!(
                "chunk {index} digest differs from certificate"
            )));
        }
        self.check()?;
        self.slots[slot_index].index = Some(index);
        Ok(slot_index)
    }
}

#[cfg(unix)]
fn positioned_read(file: &File, output: &mut [u8], offset: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, output, offset)
}
#[cfg(windows)]
fn positioned_read(file: &File, output: &mut [u8], offset: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, output, offset)
}
#[cfg(not(any(unix, windows)))]
fn positioned_read(_file: &File, _output: &mut [u8], _offset: u64) -> io::Result<usize> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "certified descriptor positioning requires Unix or Windows",
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamKind {
    Video,
    Audio,
    Other,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertifiedStream {
    pub index: usize,
    pub kind: StreamKind,
    pub time_base: (i32, i32),
    pub timestamp_origin: i64,
    pub duration: i64,
    pub average_frame_rate: (i32, i32),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertifiedProbe {
    pub streams: Vec<CertifiedStream>,
    pub duration_microseconds: i64,
    pub video_stream: Option<usize>,
    pub audio_stream: Option<usize>,
    pub duration: TimeCode,
    pub fps: Rational,
    pub kind: MediaKind,
    pub resolution: Option<(u32, u32)>,
    pub color_description: ColorDescription,
}

pub(crate) struct CertifiedInput {
    format: ffmpeg::format::context::Input,
    avio: bridge::Avio,
    probe_charges: Vec<LifetimeGuard>,
    decoder_bound: Option<DecoderBound>,
}
impl CertifiedInput {
    fn open(source: &CertifiedSource, cancel: Arc<AtomicBool>) -> Result<Self, OwnedIoError> {
        crate::initialize_ffmpeg().map_err(decoder_error)?;
        let (format, avio) = bridge::Avio::open(source.cursor(cancel)?)?;
        let mut input = Self {
            format,
            avio,
            probe_charges: Vec::new(),
            decoder_bound: None,
        };
        if input.format.nb_streams() as usize > MAX_CERTIFIED_STREAMS {
            return Err(OwnedIoError::Capacity("too many certified streams".into()));
        }
        // Every probe decoder needs a reservation and options before FFmpeg can open it.
        let still_pipe = input.format.nb_streams() == 1
            && matches!(input.format.format().name(), "png_pipe" | "jpeg_pipe");
        if still_pipe {
            let mut cursor = source.cursor(input.avio.cancel())?;
            let mut header = vec![0_u8; AVIO_BUFFER_BYTES];
            let count = cursor.read(&mut header)?;
            let dimensions = still_dimensions(&header[..count])
                .ok_or_else(|| decoder_error("unsupported or incomplete still header"))?;
            bridge::set_still_dimensions(&mut input.format, dimensions)?;
        }
        if input.format.nb_streams() == 0
            || (bridge::dynamic_stream_table(&input.format) && !still_pipe)
        {
            return Err(decoder_error(
                "incomplete or dynamic stream table unsupported",
            ));
        }
        let mut bounds = Vec::new();
        let mut stream_kinds = Vec::new();
        for stream in input.format.streams() {
            stream_kinds.push(stream.parameters().medium());
            if matches!(
                stream.parameters().medium(),
                ffmpeg::media::Type::Video | ffmpeg::media::Type::Audio
            ) {
                let bound = bridge::decoder_bound(&stream.parameters())?;
                input
                    .probe_charges
                    .push((source.0.reserve)(bound.bytes).map_err(OwnedIoError::Capacity)?);
                bounds.push(Some(bound));
            } else {
                return Err(decoder_error("non-audio/video stream kind unsupported"));
            }
        }
        let probed = bridge::find_stream_info(&mut input.format, &bounds);
        input.check()?;
        if input.format.nb_streams() as usize != bounds.len()
            || input
                .format
                .streams()
                .zip(stream_kinds)
                .any(|(stream, kind)| stream.parameters().medium() != kind)
        {
            return Err(decoder_error("changed stream table unsupported"));
        }
        probed?;
        Ok(input)
    }
    fn check(&mut self) -> Result<(), OwnedIoError> {
        self.avio.check()
    }
    fn packet(&mut self, stream_index: usize) -> Result<Option<ffmpeg::Packet>, OwnedIoError> {
        loop {
            self.check()?;
            let mut packet = ffmpeg::Packet::empty();
            let result = packet.read(&mut self.format);
            self.check()?;
            match result {
                Ok(()) if packet.stream() == stream_index => return Ok(Some(packet)),
                Ok(()) => {}
                Err(ffmpeg::Error::Eof) => return Ok(None),
                Err(error) => return Err(decoder_error(error)),
            }
        }
    }
    fn seek(&mut self, timestamp: i64) -> Result<(), OwnedIoError> {
        self.check()?;
        let result = self.format.seek(timestamp, ..timestamp);
        self.check()?;
        result.map_err(decoder_error)
    }
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

#[derive(Clone, Copy)]
struct DecoderBound {
    frame_bytes: u64,
    bytes: u64,
    pixels: i64,
    allocation_pixels: i64,
    channels: u32,
}
impl DecoderBound {
    fn check_video(self, frame: &ffmpeg::frame::Video) -> Result<(), OwnedIoError> {
        if frame.width() == 0
            || frame.height() == 0
            || frame.width() > MAX_DECODER_DIMENSION
            || frame.height() > MAX_DECODER_DIMENSION
            || u64::from(frame.width()) * u64::from(frame.height())
                > u64::try_from(self.pixels).expect("positive admitted pixel bound")
        {
            return Err(OwnedIoError::Capacity(
                "decoded dimensions exceed reserved bound".into(),
            ));
        }
        Ok(())
    }
    fn check_audio(self, frame: &ffmpeg::frame::Audio) -> Result<(), OwnedIoError> {
        if u32::from(frame.channels()) > self.channels
            || frame.samples() as u64 > MAX_AUDIO_FRAME_SAMPLES
        {
            return Err(OwnedIoError::Capacity(
                "decoded audio exceeds reserved bound".into(),
            ));
        }
        Ok(())
    }
}

pub struct CertifiedVideoFrame {
    frame: ffmpeg::frame::Video,
    _charge: LifetimeGuard,
    _frame_charge: LifetimeGuard,
    _source: CertifiedSource,
}
impl std::ops::Deref for CertifiedVideoFrame {
    type Target = ffmpeg::frame::Video;
    fn deref(&self) -> &Self::Target {
        &self.frame
    }
}
pub struct CertifiedAudioFrame {
    frame: ffmpeg::frame::Audio,
    _charge: LifetimeGuard,
    _frame_charge: LifetimeGuard,
    _source: CertifiedSource,
}
impl std::ops::Deref for CertifiedAudioFrame {
    type Target = ffmpeg::frame::Audio;
    fn deref(&self) -> &Self::Target {
        &self.frame
    }
}

/// Decoder fields drop before format, then AVIO, then the cursor's descriptor/pins/charges.
pub struct CertifiedVideoDecoder {
    decoder: ffmpeg::decoder::Video,
    input: CertifiedInput,
    stream_index: usize,
    eof: bool,
    bound: DecoderBound,
    charge: LifetimeGuard,
    source: CertifiedSource,
}
impl CertifiedVideoDecoder {
    #[cfg(any(test, feature = "test-util"))]
    #[must_use]
    pub fn cached_chunk_indices(&self) -> [Option<u64>; CACHE_SLOTS] {
        self.input
            .avio
            .cursor
            .slots
            .each_ref()
            .map(|slot| slot.index)
    }
    #[cfg(any(test, feature = "test-util"))]
    #[must_use]
    pub fn delivered_bytes(&self) -> u64 {
        self.input.avio.cursor.delivered_bytes
    }

    pub fn next_frame(&mut self) -> Result<Option<CertifiedVideoFrame>, OwnedIoError> {
        loop {
            self.input.check()?;
            let frame_charge =
                (self.source.0.reserve)(self.bound.frame_bytes).map_err(OwnedIoError::Capacity)?;
            let mut frame = ffmpeg::frame::Video::empty();
            let result = self.decoder.receive_frame(&mut frame);
            self.input.check()?;
            match result {
                Ok(()) => {
                    self.bound.check_video(&frame)?;
                    #[cfg(any(test, feature = "test-util"))]
                    self.source
                        .0
                        .accepted_output_bytes
                        .fetch_add(frame.data(0).len() as u64, Ordering::AcqRel);
                    return Ok(Some(CertifiedVideoFrame {
                        frame,
                        _charge: Arc::clone(&self.charge),
                        _frame_charge: frame_charge,
                        _source: self.source.clone(),
                    }));
                }
                Err(ffmpeg::Error::Eof) => return Ok(None),
                Err(ffmpeg::Error::Other {
                    errno: ffmpeg::error::EAGAIN,
                }) if !self.eof => {}
                Err(error) => return Err(decoder_error(error)),
            }
            if let Some(packet) = self.input.packet(self.stream_index)? {
                self.decoder.send_packet(&packet).map_err(decoder_error)?;
            } else {
                self.decoder.send_eof().map_err(decoder_error)?;
                self.eof = true;
            }
            self.input.check()?;
        }
    }
    pub fn seek(&mut self, timestamp_microseconds: i64) -> Result<(), OwnedIoError> {
        self.input.seek(timestamp_microseconds)?;
        self.decoder.flush();
        self.eof = false;
        self.input.check()
    }
}

pub struct CertifiedAudioDecoder {
    decoder: ffmpeg::decoder::Audio,
    input: CertifiedInput,
    stream_index: usize,
    eof: bool,
    bound: DecoderBound,
    charge: LifetimeGuard,
    source: CertifiedSource,
}
impl CertifiedAudioDecoder {
    #[cfg(any(test, feature = "test-util"))]
    #[must_use]
    pub fn cached_chunk_indices(&self) -> [Option<u64>; CACHE_SLOTS] {
        self.input
            .avio
            .cursor
            .slots
            .each_ref()
            .map(|slot| slot.index)
    }
    #[cfg(any(test, feature = "test-util"))]
    #[must_use]
    pub fn delivered_bytes(&self) -> u64 {
        self.input.avio.cursor.delivered_bytes
    }

    pub fn next_frame(&mut self) -> Result<Option<CertifiedAudioFrame>, OwnedIoError> {
        loop {
            self.input.check()?;
            let frame_charge =
                (self.source.0.reserve)(self.bound.frame_bytes).map_err(OwnedIoError::Capacity)?;
            let mut frame = ffmpeg::frame::Audio::empty();
            let result = self.decoder.receive_frame(&mut frame);
            self.input.check()?;
            match result {
                Ok(()) => {
                    self.bound.check_audio(&frame)?;
                    #[cfg(any(test, feature = "test-util"))]
                    self.source
                        .0
                        .accepted_output_bytes
                        .fetch_add(frame.data(0).len() as u64, Ordering::AcqRel);
                    return Ok(Some(CertifiedAudioFrame {
                        frame,
                        _charge: Arc::clone(&self.charge),
                        _frame_charge: frame_charge,
                        _source: self.source.clone(),
                    }));
                }
                Err(ffmpeg::Error::Eof) => return Ok(None),
                Err(ffmpeg::Error::Other {
                    errno: ffmpeg::error::EAGAIN,
                }) if !self.eof => {}
                Err(error) => return Err(decoder_error(error)),
            }
            if let Some(packet) = self.input.packet(self.stream_index)? {
                self.decoder.send_packet(&packet).map_err(decoder_error)?;
            } else {
                self.decoder.send_eof().map_err(decoder_error)?;
                self.eof = true;
            }
            self.input.check()?;
        }
    }
    pub fn seek(&mut self, timestamp_microseconds: i64) -> Result<(), OwnedIoError> {
        self.input.seek(timestamp_microseconds)?;
        self.decoder.flush();
        self.eof = false;
        self.input.check()
    }
}

#[allow(unsafe_code)]
mod bridge {
    use super::{
        AVIO_BUFFER_BYTES, Arc, AtomicBool, CHUNK_BYTES, CODEC_BUFFER_COUNT, DecoderBound,
        MAX_AUDIO_CHANNELS, MAX_AUDIO_FRAME_SAMPLES, MAX_CERTIFIED_STREAMS, MAX_DECODER_DIMENSION,
        OwnedIoError, SeekFrom, VerifiedCursor, decoder_error, ffmpeg,
    };
    #[cfg(test)]
    use super::{CertifiedInput, Ordering};
    use ffmpeg::ffi;
    use std::{
        ffi::{CStr, c_char, c_int, c_void},
        panic::{AssertUnwindSafe, catch_unwind},
        ptr::{self, NonNull},
    };

    const SINGLE_OBJECT_DEMUXERS: &CStr = c"avi,mov,matroska,webm,wav,mp3,flac,ogg,aac,nut,mpegts,mpeg,mpegvideo,image2,png_pipe,jpeg_pipe,webp_pipe,bmp_pipe,tiff_pipe";

    pub(super) fn set_still_dimensions(
        input: &mut ffmpeg::format::context::Input,
        (width, height): (u32, u32),
    ) -> Result<(), OwnedIoError> {
        if width == 0
            || height == 0
            || width > MAX_DECODER_DIMENSION
            || height > MAX_DECODER_DIMENSION
        {
            return Err(OwnedIoError::Capacity("still dimensions".into()));
        }
        let mut parameters = input
            .stream(0)
            .ok_or_else(|| decoder_error("still stream missing"))?
            .parameters();
        unsafe {
            (*parameters.as_mut_ptr()).width = i32::try_from(width).expect("bounded width");
            (*parameters.as_mut_ptr()).height = i32::try_from(height).expect("bounded height");
        }
        Ok(())
    }

    pub(super) fn dynamic_stream_table(input: &ffmpeg::format::context::Input) -> bool {
        unsafe { (*input.as_ptr()).ctx_flags & ffi::AVFMTCTX_NOHEADER != 0 }
    }

    pub(super) fn decoder_bound(
        parameters: &ffmpeg::codec::Parameters,
    ) -> Result<DecoderBound, OwnedIoError> {
        let parameters = unsafe { &*parameters.as_ptr() };
        let (pixels, channels, frame_bytes) =
            if parameters.codec_type == ffi::AVMediaType::AVMEDIA_TYPE_VIDEO {
                let width = u32::try_from(parameters.width)
                    .map_err(|_| OwnedIoError::Capacity("invalid decoder width".into()))?;
                let height = u32::try_from(parameters.height)
                    .map_err(|_| OwnedIoError::Capacity("invalid decoder height".into()))?;
                if width == 0
                    || height == 0
                    || width > MAX_DECODER_DIMENSION
                    || height > MAX_DECODER_DIMENSION
                {
                    return Err(OwnedIoError::Capacity(
                        "unsupported decoder dimensions".into(),
                    ));
                }
                (
                    i64::from(width) * i64::from(height),
                    0,
                    (u64::from(width).div_ceil(128) * 128 + 256)
                        * (u64::from(height).div_ceil(128) * 128 + 256)
                        * 16,
                )
            } else {
                let channels = u32::try_from(parameters.ch_layout.nb_channels)
                    .map_err(|_| OwnedIoError::Capacity("invalid audio channels".into()))?;
                if channels == 0
                    || channels > MAX_AUDIO_CHANNELS
                    || parameters.sample_rate <= 0
                    || parameters.sample_rate > 384_000
                {
                    return Err(OwnedIoError::Capacity(
                        "unsupported audio layout/rate".into(),
                    ));
                }
                (
                    0,
                    channels,
                    MAX_AUDIO_FRAME_SAMPLES * u64::from(channels) * 8,
                )
            };
        Ok(DecoderBound {
            frame_bytes,
            bytes: frame_bytes
                .checked_mul(CODEC_BUFFER_COUNT)
                .and_then(|bytes| bytes.checked_add(CHUNK_BYTES as u64))
                .ok_or_else(|| OwnedIoError::Capacity("decoder bound overflow".into()))?,
            pixels,
            allocation_pixels: if pixels > 0 {
                i64::try_from(frame_bytes / 16).expect("bounded padded raster")
            } else {
                0
            },
            channels,
        })
    }
    pub(super) fn decoder_context(
        parameters: ffmpeg::codec::Parameters,
        bound: DecoderBound,
    ) -> Result<ffmpeg::codec::context::Context, OwnedIoError> {
        let mut context =
            ffmpeg::codec::context::Context::from_parameters(parameters).map_err(decoder_error)?;
        context.set_threading(ffmpeg::codec::threading::Config {
            count: 1,
            kind: ffmpeg::codec::threading::Type::None,
        });
        unsafe {
            (*context.as_mut_ptr()).max_pixels = bound.allocation_pixels;
            (*context.as_mut_ptr()).max_samples =
                i64::try_from(MAX_AUDIO_FRAME_SAMPLES).expect("bounded audio samples");
        }
        Ok(context)
    }
    pub(super) fn find_stream_info(
        input: &mut ffmpeg::format::context::Input,
        bounds: &[Option<DecoderBound>],
    ) -> Result<(), OwnedIoError> {
        let mut options = vec![ptr::null_mut(); bounds.len()];
        unsafe {
            // options.c refuses new streams before allocating their decoder context.
            (*input.as_mut_ptr()).max_streams =
                i32::try_from(bounds.len()).expect("bounded reserved stream count");
            for (option, bound) in options.iter_mut().zip(bounds) {
                ffi::av_dict_set(option, c"threads".as_ptr(), c"1".as_ptr(), 0);
                if let Some(bound) = bound {
                    ffi::av_dict_set_int(
                        option,
                        c"max_pixels".as_ptr(),
                        bound.allocation_pixels,
                        0,
                    );
                    ffi::av_dict_set_int(
                        option,
                        c"max_samples".as_ptr(),
                        i64::try_from(MAX_AUDIO_FRAME_SAMPLES).expect("bounded audio samples"),
                        0,
                    );
                }
            }
            #[cfg(any(test, feature = "test-util"))]
            super::STREAM_INFO_PROBE_CALLS.with(|calls| calls.set(calls.get().saturating_add(1)));
            let result = ffi::avformat_find_stream_info(input.as_mut_ptr(), options.as_mut_ptr());
            for option in &mut options {
                ffi::av_dict_free(option);
            }
            if result < 0 {
                return Err(decoder_error(ffmpeg::Error::from(result)));
            }
        }
        Ok(())
    }

    pub(super) struct Avio {
        context: NonNull<ffi::AVIOContext>,
        pub(super) cursor: Box<VerifiedCursor>,
    }
    // FFmpeg calls these callbacks synchronously. Moving the owner cannot overlap calls.
    unsafe impl Send for Avio {}

    impl Avio {
        #[allow(clippy::too_many_lines)]
        pub(super) fn open(
            cursor: VerifiedCursor,
        ) -> Result<(ffmpeg::format::context::Input, Self), OwnedIoError> {
            let mut cursor = Box::new(cursor);
            unsafe {
                let buffer = ffi::av_malloc(AVIO_BUFFER_BYTES).cast::<u8>();
                if buffer.is_null() {
                    return Err(OwnedIoError::Capacity("AVIO buffer allocation".into()));
                }
                let context = ffi::avio_alloc_context(
                    buffer,
                    c_int::try_from(AVIO_BUFFER_BYTES).expect("bounded AVIO buffer"),
                    0,
                    ptr::from_mut(cursor.as_mut()).cast(),
                    Some(read),
                    None,
                    Some(seek),
                );
                let Some(context) = NonNull::new(context) else {
                    ffi::av_free(buffer.cast());
                    return Err(OwnedIoError::Capacity("AVIO context allocation".into()));
                };
                let mut avio = Self { context, cursor };
                let mut format = ffi::avformat_alloc_context();
                if format.is_null() {
                    return Err(OwnedIoError::Capacity("format allocation".into()));
                }
                (*format).pb = context.as_ptr();
                (*format).opaque = ptr::from_mut(avio.cursor.as_mut()).cast();
                (*format).flags |= ffi::AVFMT_FLAG_CUSTOM_IO;
                (*format).io_open = Some(deny_open);
                (*format).interrupt_callback.callback = Some(interrupt);
                (*format).interrupt_callback.opaque = ptr::from_mut(avio.cursor.as_mut()).cast();
                (*format).max_streams =
                    i32::try_from(MAX_CERTIFIED_STREAMS).expect("bounded stream count");
                (*format).probesize = 32_768;
                (*format).format_probesize = 32_768;
                let formats = SINGLE_OBJECT_DEMUXERS;
                let protocols = c"none";
                #[cfg(test)]
                let (formats, protocols) = match avio.cursor.source.hooks.open_policy {
                    super::TestOpenPolicy::Production => (formats, protocols),
                    super::TestOpenPolicy::ProtocolGuard => (c"concat,hls,wav,mpegts", protocols),
                    super::TestOpenPolicy::FormatGuard => (formats, c"file"),
                    super::TestOpenPolicy::NestedOutputControl => (c"concat,wav", c"file"),
                };
                let mut options = ptr::null_mut();
                // Concat inherits this through ff_copy_whiteblacklists.
                let protocol_option = ffi::av_dict_set(
                    &raw mut options,
                    c"protocol_whitelist".as_ptr(),
                    protocols.as_ptr(),
                    0,
                );
                // C0 nested-format guard. Demuxer rejection precedes read_header/nested opens.
                let format_option = ffi::av_dict_set(
                    &raw mut options,
                    c"format_whitelist".as_ptr(),
                    formats.as_ptr(),
                    0,
                );
                if protocol_option < 0 || format_option < 0 {
                    ffi::av_dict_free(&raw mut options);
                    ffi::avformat_free_context(format);
                    return Err(OwnedIoError::Capacity("format policy allocation".into()));
                }
                // Some demuxers create contexts without copying our io_open callback.
                let opened = ffi::avformat_open_input(
                    &raw mut format,
                    c"file:certified-owned-input".as_ptr(),
                    ptr::null(),
                    &raw mut options,
                );
                ffi::av_dict_free(&raw mut options);
                if opened < 0 {
                    avio.check()?;
                    return Err(decoder_error(ffmpeg::Error::from(opened)));
                }
                let input = ffmpeg::format::context::Input::wrap(format);
                avio.check()?;
                Ok((input, avio))
            }
        }
        pub(super) fn cancel(&self) -> Arc<AtomicBool> {
            Arc::clone(&self.cursor.cancel)
        }
        pub(super) fn check(&mut self) -> Result<(), OwnedIoError> {
            self.cursor.check()?;
            let buffer_size = unsafe { self.context.as_ref().buffer_size };
            if buffer_size < 0
                || usize::try_from(buffer_size).expect("nonnegative AVIO buffer")
                    > AVIO_BUFFER_BYTES
            {
                return self.cursor.fail(OwnedIoError::Capacity(
                    "format exceeded bounded AVIO buffer".into(),
                ));
            }
            Ok(())
        }
        #[cfg(test)]
        pub(super) fn buffer_size(&self) -> usize {
            usize::try_from(unsafe { self.context.as_ref().buffer_size }).unwrap()
        }
    }
    impl Drop for Avio {
        fn drop(&mut self) {
            // The format has closed first. FFmpeg may replace the buffer while probing.
            unsafe {
                let mut context = self.context.as_ptr();
                ffi::av_free((*context).buffer.cast());
                (*context).buffer = ptr::null_mut();
                ffi::avio_context_free(&raw mut context);
            }
        }
    }

    unsafe extern "C" fn deny_open(
        format: *mut ffi::AVFormatContext,
        _io: *mut *mut ffi::AVIOContext,
        _url: *const c_char,
        _flags: c_int,
        _options: *mut *mut ffi::AVDictionary,
    ) -> c_int {
        let cursor = unsafe { &mut *(*format).opaque.cast::<VerifiedCursor>() };
        #[cfg(test)]
        cursor
            .source
            .hooks
            .nested_open_calls
            .fetch_add(1, Ordering::AcqRel);
        let _ = cursor.fail::<()>(OwnedIoError::Unavailable(
            "secondary input open denied".into(),
        ));
        ffi::AVERROR(ffmpeg::error::EACCES)
    }
    #[cfg(test)]
    pub(super) fn callback_read(cursor: &mut VerifiedCursor, buffer: &mut [u8]) -> i32 {
        unsafe {
            read(
                std::ptr::from_mut(cursor).cast(),
                buffer.as_mut_ptr(),
                i32::try_from(buffer.len()).unwrap(),
            )
        }
    }
    #[cfg(test)]
    pub(super) fn append_stream(input: &mut ffmpeg::format::context::Input) -> bool {
        unsafe { !ffi::avformat_new_stream(input.as_mut_ptr(), ptr::null()).is_null() }
    }
    #[cfg(test)]
    pub(super) fn secondary_open(input: &mut CertifiedInput, url: &CStr) -> i32 {
        unsafe {
            let format = input.format.as_mut_ptr();
            let nested = ffi::avformat_alloc_context();
            assert!(!nested.is_null());
            (*nested).opaque = (*format).opaque;
            let mut secondary = std::ptr::null_mut();
            let result = (*format).io_open.unwrap()(
                nested,
                &raw mut secondary,
                url.as_ptr(),
                ffi::AVIO_FLAG_READ,
                std::ptr::null_mut(),
            );
            if !secondary.is_null() {
                ffi::avio_closep(&raw mut secondary);
            }
            ffi::avformat_free_context(nested);
            result
        }
    }
    unsafe extern "C" fn interrupt(opaque: *mut c_void) -> c_int {
        // The Box stays alive until format and AVIO teardown have completed.
        let cursor = unsafe { &mut *opaque.cast::<VerifiedCursor>() };
        i32::from(cursor.check().is_err())
    }
    unsafe extern "C" fn read(opaque: *mut c_void, buffer: *mut u8, length: c_int) -> c_int {
        let cursor = unsafe { &mut *opaque.cast::<VerifiedCursor>() };
        let result = catch_unwind(AssertUnwindSafe(|| {
            if length <= 0 {
                return cursor.fail(OwnedIoError::Damaged("invalid AVIO read size".into()));
            }
            let output = unsafe {
                std::slice::from_raw_parts_mut(
                    buffer,
                    usize::try_from(length).expect("positive FFmpeg buffer length"),
                )
            };
            cursor.read(output)
        }));
        match result {
            Ok(Ok(0)) => ffi::AVERROR_EOF,
            Ok(Ok(count)) => {
                #[cfg(any(test, feature = "test-util"))]
                {
                    cursor.delivered_bytes = cursor.delivered_bytes.saturating_add(count as u64);
                }
                #[cfg(test)]
                cursor
                    .source
                    .hooks
                    .delivered
                    .fetch_add(count, Ordering::Relaxed);
                c_int::try_from(count).expect("read cannot exceed FFmpeg c_int request")
            }
            Ok(Err(_)) => ffi::AVERROR(ffmpeg::error::EIO),
            Err(_) => {
                let _ = cursor.fail::<()>(OwnedIoError::Unavailable("panic in AVIO read".into()));
                ffi::AVERROR(ffmpeg::error::EIO)
            }
        }
    }
    unsafe extern "C" fn seek(opaque: *mut c_void, offset: i64, whence: c_int) -> i64 {
        let cursor = unsafe { &mut *opaque.cast::<VerifiedCursor>() };
        let result = catch_unwind(AssertUnwindSafe(|| {
            if whence & ffi::AVSEEK_SIZE != 0 {
                return cursor.size();
            }
            let position = match whence & !ffi::AVSEEK_FORCE {
                0 if offset >= 0 => {
                    SeekFrom::Start(u64::try_from(offset).expect("nonnegative offset"))
                }
                1 => SeekFrom::Current(offset),
                2 => SeekFrom::End(offset),
                _ => return cursor.fail(OwnedIoError::Damaged("invalid AVIO seek".into())),
            };
            cursor.seek(position)
        }));
        match result {
            Ok(Ok(position)) => i64::try_from(position).expect("certificate fits i64"),
            Ok(Err(_)) => i64::from(ffi::AVERROR(ffmpeg::error::EIO)),
            Err(_) => {
                let _ = cursor.fail::<()>(OwnedIoError::Unavailable("panic in AVIO seek".into()));
                i64::from(ffi::AVERROR(ffmpeg::error::EIO))
            }
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Default)]
enum TestOpenPolicy {
    #[default]
    Production,
    ProtocolGuard,
    FormatGuard,
    NestedOutputControl,
}
#[cfg(test)]
#[derive(Default)]
struct TestHooks {
    open_policy: TestOpenPolicy,
    nested_open_calls: std::sync::atomic::AtomicUsize,
    short_read: AtomicBool,
    verification: std::sync::Mutex<Option<(u64, Arc<std::sync::Barrier>)>>,
    delivered: std::sync::atomic::AtomicUsize,
    readable: std::sync::atomic::AtomicUsize,
}
#[cfg(test)]
impl TestHooks {
    fn before_verify(&self, index: u64) {
        let barrier = self
            .verification
            .lock()
            .expect("verification hook")
            .as_ref()
            .filter(|(wanted, _)| *wanted == index)
            .map(|(_, barrier)| Arc::clone(barrier));
        if let Some(barrier) = barrier {
            barrier.wait();
            barrier.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TempDirectory, run_ffmpeg};
    use std::{
        fs::{self, OpenOptions},
        io::{Seek, Write},
        path::{Path, PathBuf},
        sync::{Condvar, Mutex, atomic::AtomicUsize},
        thread,
        time::Duration,
    };

    fn running() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }
    fn digest(bytes: &[u8]) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(bytes);
        hash.finalize()
    }
    fn source(path: &Path) -> CertifiedSource {
        source_guarded(path, Arc::new(()), Arc::new(|_| Ok(Arc::new(()))))
    }
    fn source_guarded(
        path: &Path,
        guard: LifetimeGuard,
        reserve: Arc<CursorReservation>,
    ) -> CertifiedSource {
        let bytes = fs::read(path).unwrap();
        let chunks: Arc<[[u8; 32]]> = bytes
            .chunks(CHUNK_BYTES)
            .map(digest)
            .collect::<Vec<_>>()
            .into();
        let table: Vec<u8> = chunks.iter().flatten().copied().collect();
        CertifiedSource::new(
            File::open(path).unwrap(),
            digest(&bytes),
            bytes.len() as u64,
            digest(&table),
            chunks,
            guard,
            reserve,
        )
        .unwrap()
    }
    fn bytes_fixture(bytes: &[u8]) -> (TempDirectory, PathBuf, CertifiedSource) {
        let directory = TempDirectory::new("kinewright-owned-io");
        let path = directory.path("source.bin");
        fs::write(&path, bytes).unwrap();
        let source = source(&path);
        (directory, path, source)
    }
    fn media_fixture(color: &str) -> (TempDirectory, PathBuf, CertifiedSource) {
        let directory = TempDirectory::new("kinewright-owned-decoder");
        let path = directory.path("source.avi");
        run_ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                &format!("color=c={color}:s=320x240:r=5:d=12"),
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=12",
                "-c:v",
                "rawvideo",
                "-pix_fmt",
                "yuv420p",
                "-threads",
                "1",
                "-c:a",
                "pcm_s16le",
                "-color_primaries",
                "bt709",
                "-color_trc",
                "bt709",
                "-colorspace",
                "bt709",
                "-shortest",
            ],
            &path,
        );
        let source = source(&path);
        assert!(source.identity().source_length > (CHUNK_BYTES * 6) as u64);
        (directory, path, source)
    }
    fn wav_fixture(directory: &TempDirectory) -> PathBuf {
        let path = directory.path("uncertified.wav");
        run_ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=911:sample_rate=48000:duration=1",
                "-c:a",
                "pcm_s16le",
                "-threads",
                "1",
            ],
            &path,
        );
        path
    }
    fn nested_fixture(hls: bool, policy: TestOpenPolicy) -> (TempDirectory, CertifiedSource) {
        // Relative safe names reproduce resolution against FFmpeg's inert file: label.
        let cwd = std::env::current_dir().unwrap();
        let directory = TempDirectory::new_in(&cwd, "kinewright-owned-nested");
        let wav = wav_fixture(&directory);
        let relative = wav
            .strip_prefix(cwd)
            .unwrap()
            .to_str()
            .unwrap()
            .replace('\\', "/");
        let playlist = if hls {
            format!(
                "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:1,\n{relative}\n#EXT-X-ENDLIST\n"
            )
        } else {
            format!("ffconcat version 1.0\nfile '{relative}'\nduration 1.0\n")
        };
        let path = directory.path("certified-playlist.bin");
        fs::write(&path, playlist).unwrap();
        let mut source = source(&path);
        Arc::get_mut(&mut source.0).unwrap().hooks.open_policy = policy;
        (directory, source)
    }
    fn read_nested_audio(source: &CertifiedSource) -> Result<(), OwnedIoError> {
        let probe = source.probe(running())?;
        let index = probe
            .audio_stream
            .ok_or_else(|| decoder_error("no nested audio"))?;
        let mut audio = source.open_audio(index, running())?;
        while audio.next_frame()?.is_some() {}
        Ok(())
    }
    fn overwrite(writer: &mut File, offset: u64, bytes: &[u8]) {
        writer.seek(SeekFrom::Start(offset)).unwrap();
        writer.write_all(bytes).unwrap();
        writer.flush().unwrap();
    }
    fn assert_damaged<T>(result: Result<T, OwnedIoError>) {
        assert!(
            matches!(result, Err(OwnedIoError::Damaged(_))),
            "C0_DAMAGED_INPUT_REFUSED got {:?}",
            result.err()
        );
    }

    #[test]
    fn s3_certified_cursor_bounds_independent_offsets_final_chunk_and_exact_eof() {
        let bytes: Vec<u8> = (0..CHUNK_BYTES * 5 + 29)
            .map(|index| u8::try_from(index % 251).unwrap())
            .collect();
        let (_directory, _path, source) = bytes_fixture(&bytes);
        let mut first = source.cursor(running()).unwrap();
        let mut second = source.cursor(running()).unwrap();
        let mut output = vec![0; CHUNK_BYTES * 2];
        assert_eq!(first.read(&mut output).unwrap(), CHUNK_BYTES);
        assert_eq!(&output[..CHUNK_BYTES], &bytes[..CHUNK_BYTES]);
        assert_eq!(
            second.seek(SeekFrom::End(-29)).unwrap(),
            (bytes.len() - 29) as u64
        );
        assert_eq!(second.read(&mut output).unwrap(), 29);
        assert_eq!(&output[..29], &bytes[bytes.len() - 29..]);
        assert_eq!(second.read(&mut output).unwrap(), 0);
        assert_eq!(first.position, CHUNK_BYTES as u64);
        for index in 1..6 {
            first
                .seek(SeekFrom::Start((index * CHUNK_BYTES) as u64))
                .unwrap();
            let count = first.read(&mut output).unwrap();
            let end = ((index + 1) * CHUNK_BYTES).min(bytes.len());
            assert_eq!(&output[..count], &bytes[index * CHUNK_BYTES..end]);
            assert!(
                first
                    .slots
                    .iter()
                    .filter(|slot| slot.index.is_some())
                    .count()
                    <= CACHE_SLOTS
            );
            assert_eq!(
                first
                    .slots
                    .iter()
                    .map(|slot| slot.bytes.capacity())
                    .sum::<usize>(),
                CACHE_SLOTS * CHUNK_BYTES
            );
        }
        assert_damaged(first.seek(SeekFrom::End(1)));
        assert_damaged(first.size());
    }

    #[test]
    fn s3_certified_cache_is_private_and_evict_refetch_verifies_again() {
        let bytes = vec![19; CHUNK_BYTES * 5];
        let (_directory, path, source) = bytes_fixture(&bytes);
        let mut writer = OpenOptions::new().write(true).open(path).unwrap();
        let mut cursor = source.cursor(running()).unwrap();
        let mut output = [0_u8; 1];
        cursor.read(&mut output).unwrap();
        overwrite(&mut writer, 0, &[37]);
        cursor.seek(SeekFrom::Start(0)).unwrap();
        cursor.read(&mut output).unwrap();
        assert_eq!(output, [19], "cache hit serves the verified private copy");
        for index in 1..5 {
            cursor
                .seek(SeekFrom::Start((index * CHUNK_BYTES) as u64))
                .unwrap();
            cursor.read(&mut output).unwrap();
        }
        assert!(!cursor.slots.iter().any(|slot| slot.index == Some(0)));
        cursor.seek(SeekFrom::Start(0)).unwrap();
        assert_damaged(cursor.read(&mut output));
        assert_damaged(cursor.seek(SeekFrom::End(0)));
        assert_damaged(cursor.read(&mut output));
    }

    #[test]
    fn s3_certified_size_changes_refuse_cache_hits_seek_and_size_queries() {
        for change in [-1_i64, 1] {
            let (_directory, path, source) = bytes_fixture(&vec![7; CHUNK_BYTES + 7]);
            let writer = OpenOptions::new().write(true).open(path).unwrap();
            let mut read_cursor = source.cursor(running()).unwrap();
            let mut seek_cursor = source.cursor(running()).unwrap();
            let mut size_cursor = source.cursor(running()).unwrap();
            let mut output = [0; 1];
            read_cursor.read(&mut output).unwrap();
            let length = source.identity().source_length;
            writer
                .set_len(length.checked_add_signed(change).unwrap())
                .unwrap();
            assert_damaged(read_cursor.read(&mut output));
            assert_damaged(seek_cursor.seek(SeekFrom::Start(0)));
            assert_damaged(size_cursor.size());
            writer.set_len(length).unwrap();
            assert_damaged(read_cursor.read(&mut output));
            assert_damaged(seek_cursor.seek(SeekFrom::Start(0)));
            assert_damaged(size_cursor.size());
        }
    }

    #[test]
    fn s3_certified_short_read_with_unchanged_descriptor_length_refuses() {
        let (_directory, _path, source) = bytes_fixture(&vec![0; CHUNK_BYTES]);
        let mut cursor = source.cursor(running()).unwrap();
        source.0.hooks.short_read.store(true, Ordering::Release);
        let mut output = [23_u8; 32];
        assert!(
            bridge::callback_read(&mut cursor, &mut output) < 0,
            "C0_SHORT_READ_REFUSED"
        );
        assert_eq!(source.0.file.metadata().unwrap().len(), CHUNK_BYTES as u64);
        assert_eq!(output, [23; 32]);
        assert_eq!(source.0.hooks.delivered.load(Ordering::Acquire), 0);
        assert_damaged(cursor.read(&mut output));
    }

    #[test]
    fn s3_certified_avio_exposes_zero_unverified_scratch_bytes_at_barrier() {
        let (_directory, path, source) = bytes_fixture(&vec![17; CHUNK_BYTES]);
        let mut writer = OpenOptions::new().write(true).open(path).unwrap();
        overwrite(&mut writer, 0, &[18]);
        let barrier = Arc::new(std::sync::Barrier::new(2));
        *source.0.hooks.verification.lock().unwrap() = Some((0, Arc::clone(&barrier)));
        let mut cursor = source.cursor(running()).unwrap();
        let worker = thread::spawn(move || {
            let mut output = [71; 32];
            let status = bridge::callback_read(&mut cursor, &mut output);
            (
                status,
                output,
                cursor
                    .slots
                    .iter()
                    .filter(|slot| slot.index.is_some())
                    .count(),
            )
        });
        barrier.wait();
        let exposed = source.0.hooks.delivered.load(Ordering::Acquire);
        let readable = source.0.hooks.readable.load(Ordering::Acquire);
        barrier.wait();
        let (status, output, readable_after) = worker.join().unwrap();
        assert_eq!(exposed, 0);
        assert_eq!(readable, 0, "C0_VERIFIED_ONLY_PUBLICATION");
        assert!(status < 0, "C0_BARRIER_READ_REFUSED");
        assert_eq!(output, [71; 32]);
        assert_eq!(readable_after, 0);
    }

    #[test]
    fn s3_certified_size_change_during_fill_refuses_before_publication() {
        let (_directory, path, source) = bytes_fixture(&vec![43; CHUNK_BYTES]);
        let writer = OpenOptions::new().write(true).open(path).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        *source.0.hooks.verification.lock().unwrap() = Some((0, Arc::clone(&barrier)));
        let mut cursor = source.cursor(running()).unwrap();
        let worker = thread::spawn(move || {
            let mut output = [71; 32];
            let status = bridge::callback_read(&mut cursor, &mut output);
            (status, output)
        });
        barrier.wait();
        writer.set_len(CHUNK_BYTES as u64 + 1).unwrap();
        barrier.wait();
        let (status, output) = worker.join().unwrap();
        assert!(status < 0, "C0_SIZE_DURING_FILL_REFUSED");
        assert_eq!(output, [71; 32]);
        assert_eq!(source.0.hooks.delivered.load(Ordering::Acquire), 0);
    }

    #[test]
    fn s3_certified_cancel_is_latched_across_read_seek_size_and_reopen() {
        let (_directory, _path, source) = bytes_fixture(&vec![61; CHUNK_BYTES]);
        let cancel = running();
        let mut cursor = source.cursor(Arc::clone(&cancel)).unwrap();
        cursor.read(&mut [0]).unwrap();
        cancel.store(true, Ordering::Release);
        assert_eq!(cursor.read(&mut [0]), Err(OwnedIoError::Cancelled));
        cancel.store(false, Ordering::Release);
        assert_eq!(
            cursor.seek(SeekFrom::Start(0)),
            Err(OwnedIoError::Cancelled)
        );
        assert_eq!(cursor.size(), Err(OwnedIoError::Cancelled));
        assert_eq!(source.cursor(running()).unwrap().read(&mut [0]), Ok(1));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn s3_certified_probe_video_audio_seek_reopen_and_landmarks() {
        for (color, expected_y) in [("red", 63_u8), ("blue", 32)] {
            let (_directory, path, source) = media_fixture(color);
            let cancel = running();
            let probe = source.probe(Arc::clone(&cancel)).unwrap();
            assert_eq!(probe.kind, MediaKind::AudioVideo);
            assert_eq!(probe.fps, Rational::new(5, 1).unwrap());
            assert_eq!(probe.resolution, Some((320, 240)));
            assert_eq!(probe.duration, TimeCode(60));
            assert!(!matches!(
                probe.color_description.bit_depth,
                kinewright_core::ColorBitDepth::Unknown
            ));
            let video_index = probe.video_stream.unwrap();
            let audio_index = probe.audio_stream.unwrap();
            assert!(
                probe
                    .streams
                    .iter()
                    .all(|stream| stream.time_base.0 > 0 && stream.time_base.1 > 0)
            );
            let asset = source
                .probe_asset(
                    kinewright_core::AssetId(1),
                    Path::new("missing/provenance.avi"),
                    running(),
                )
                .unwrap();
            assert_eq!(asset.path, Path::new("missing/provenance.avi"));
            assert_eq!(asset.name, "provenance.avi");
            assert_eq!(
                asset.source_fingerprint.byte_len,
                Some(fs::metadata(&path).unwrap().len())
            );
            assert_eq!(
                asset.source_fingerprint.content_sha256.unwrap(),
                kinewright_hash::sha256_bytes(&fs::read(&path).unwrap())
            );
            assert_eq!(asset.color_description, probe.color_description);
            let mut video = source.open_video(video_index, Arc::clone(&cancel)).unwrap();
            let first = video.next_frame().unwrap().unwrap();
            assert_eq!((first.width(), first.height()), (320, 240));
            assert!(
                first.data(0)[0].abs_diff(expected_y) <= 1,
                "{color} expected {expected_y}, actual {}, format {:?}",
                first.data(0)[0],
                first.format()
            );
            assert!(video.input.avio.buffer_size() <= AVIO_BUFFER_BYTES);
            video.seek(6_000_000).unwrap();
            assert!(video.input.avio.buffer_size() <= AVIO_BUFFER_BYTES);
            assert!(video.next_frame().unwrap().unwrap().data(0)[0].abs_diff(expected_y) <= 1);
            let mut audio = source.open_audio(audio_index, Arc::clone(&cancel)).unwrap();
            let first_audio = audio.next_frame().unwrap().unwrap();
            assert_eq!(first_audio.rate(), 48_000);
            assert_eq!(first_audio.channels(), 1);
            assert!(first_audio.data(0).iter().any(|byte| *byte != 0));
            audio.seek(6_000_000).unwrap();
            assert!(audio.input.avio.buffer_size() <= AVIO_BUFFER_BYTES);
            assert!(audio.next_frame().unwrap().unwrap().samples() > 0);
            let mut reopened = source.open_video(video_index, cancel).unwrap();
            assert!(reopened.next_frame().unwrap().unwrap().data(0)[0].abs_diff(expected_y) <= 1);
        }
    }

    struct Gate {
        entered: Mutex<bool>,
        changed: Condvar,
        released: AtomicBool,
    }
    impl Gate {
        fn new() -> Self {
            Self {
                entered: Mutex::new(false),
                changed: Condvar::new(),
                released: AtomicBool::new(false),
            }
        }
        fn hold(&self) {
            *self.entered.lock().unwrap() = true;
            self.changed.notify_all();
            let entered = self.entered.lock().unwrap();
            let (_entered, result) = self
                .changed
                .wait_timeout_while(entered, Duration::from_secs(30), |_| {
                    !self.released.load(Ordering::Acquire)
                })
                .unwrap();
            assert!(!result.timed_out(), "tamper barrier release");
        }
        fn wait(&self) {
            let entered = self.entered.lock().unwrap();
            let (_entered, result) = self
                .changed
                .wait_timeout_while(entered, Duration::from_secs(30), |entered| !*entered)
                .unwrap();
            assert!(!result.timed_out(), "decoder reached tamper barrier");
        }
        fn release(&self) {
            self.released.store(true, Ordering::Release);
            self.changed.notify_all();
        }
    }

    fn packet_chunk(path: &Path, kind: ffmpeg::media::Type) -> (usize, u64) {
        let mut input = crate::decode::media_input(path).unwrap();
        let stream = input.streams().best(kind).unwrap();
        let index = stream.index();
        let time_base = stream.time_base();
        for (stream, packet) in input.packets() {
            if stream.index() == index
                && packet.pts().is_some_and(|pts| {
                    i128::from(pts) * i128::from(time_base.numerator())
                        >= 6 * i128::from(time_base.denominator())
                })
            {
                return (
                    index,
                    u64::try_from(packet.position()).unwrap() / CHUNK_BYTES as u64,
                );
            }
        }
        panic!("fixture lacks seek target");
    }

    #[test]
    fn s3_certified_uncached_video_seek_tamper_after_open_is_latched() {
        let (_directory, path, _original) = media_fixture("red");
        let mut writer = OpenOptions::new().write(true).open(&path).unwrap();
        let source = source_guarded(&path, Arc::new(()), Arc::new(|_| Ok(Arc::new(()))));
        let (index, chunk) = packet_chunk(&path, ffmpeg::media::Type::Video);
        let mut decoder = source.open_video(index, running()).unwrap();
        assert!(decoder.next_frame().unwrap().is_some());
        assert!(
            !decoder
                .input
                .avio
                .cursor
                .slots
                .iter()
                .any(|slot| slot.index == Some(chunk)),
            "target chunk must be uncached"
        );
        let gate = Arc::new(Gate::new());
        let worker_gate = Arc::clone(&gate);
        let worker = thread::spawn(move || {
            worker_gate.hold();
            let first = decoder
                .seek(6_000_000)
                .and_then(|()| decoder.next_frame().map(|frame| frame.is_some()));
            let second = decoder.next_frame();
            (first, second.map(|frame| frame.is_some()), decoder.seek(0))
        });
        gate.wait();
        overwrite(
            &mut writer,
            chunk * CHUNK_BYTES as u64,
            &vec![0xff; CHUNK_BYTES],
        );
        gate.release();
        let (first, second, seek) = worker.join().unwrap();
        assert_damaged(first);
        assert_damaged(second);
        assert_damaged(seek);
        assert_damaged(source.open_video(index, running()));
    }

    #[test]
    fn s3_certified_uncached_audio_seek_tamper_after_open_is_latched() {
        let (_directory, path, source) = media_fixture("blue");
        let mut writer = OpenOptions::new().write(true).open(&path).unwrap();
        let (index, chunk) = packet_chunk(&path, ffmpeg::media::Type::Audio);
        let mut decoder = source.open_audio(index, running()).unwrap();
        assert!(decoder.next_frame().unwrap().is_some());
        assert!(
            !decoder
                .input
                .avio
                .cursor
                .slots
                .iter()
                .any(|slot| slot.index == Some(chunk)),
            "target chunk must be uncached"
        );
        let gate = Arc::new(Gate::new());
        let worker_gate = Arc::clone(&gate);
        let worker = thread::spawn(move || {
            worker_gate.hold();
            let first = decoder
                .seek(6_000_000)
                .and_then(|()| decoder.next_frame().map(|frame| frame.is_some()));
            (first, decoder.next_frame().map(|frame| frame.is_some()))
        });
        gate.wait();
        overwrite(
            &mut writer,
            chunk * CHUNK_BYTES as u64,
            &vec![0xff; CHUNK_BYTES],
        );
        gate.release();
        let (first, second) = worker.join().unwrap();
        assert_damaged(first);
        assert_damaged(second);
    }

    #[test]
    fn s3_certified_latched_error_rejects_already_buffered_video_and_audio() {
        let (_directory, path, source) = media_fixture("red");
        let probe = source.probe(running()).unwrap();
        let mut video = source
            .open_video(probe.video_stream.unwrap(), running())
            .unwrap();
        let packet = video.input.packet(video.stream_index).unwrap().unwrap();
        video.decoder.send_packet(&packet).unwrap();
        let mut audio = source
            .open_audio(probe.audio_stream.unwrap(), running())
            .unwrap();
        let packet = audio.input.packet(audio.stream_index).unwrap().unwrap();
        audio.decoder.send_packet(&packet).unwrap();
        let mut writer = OpenOptions::new().write(true).open(&path).unwrap();
        let chunk = 3_u64;
        assert!(
            !video
                .input
                .avio
                .cursor
                .slots
                .iter()
                .any(|slot| slot.index == Some(chunk))
        );
        assert!(
            !audio
                .input
                .avio
                .cursor
                .slots
                .iter()
                .any(|slot| slot.index == Some(chunk))
        );
        overwrite(&mut writer, chunk * CHUNK_BYTES as u64, &[27]);
        for input in [&mut video.input, &mut audio.input] {
            let seek = input
                .avio
                .cursor
                .seek(SeekFrom::Start(chunk * CHUNK_BYTES as u64));
            if seek.is_ok() {
                assert!(bridge::callback_read(&mut input.avio.cursor, &mut [0; 32]) < 0);
            } else {
                assert_damaged(seek);
            }
        }
        assert_damaged(video.next_frame());
        assert_damaged(audio.next_frame());
    }

    #[test]
    fn s3_certified_secondary_open_is_denied_and_latched() {
        let (directory, _path, source) = media_fixture("red");
        let wav = wav_fixture(&directory);
        let url = std::ffi::CString::new(wav.to_str().unwrap()).unwrap();
        let mut input = CertifiedInput::open(&source, running()).unwrap();
        assert!(
            bridge::secondary_open(&mut input, &url) < 0,
            "C0_SECONDARY_OPEN_DENIED"
        );
        assert!(matches!(input.check(), Err(OwnedIoError::Unavailable(_))));
    }

    #[test]
    fn s3_certified_wav_control_accepts_only_descriptor_bytes() {
        let directory = TempDirectory::new("kinewright-owned-wav");
        let path = wav_fixture(&directory);
        let source = source(&path);
        fs::rename(&path, directory.path("renamed.wav")).unwrap();
        let probe = source.probe(running()).unwrap();
        assert_eq!(probe.kind, MediaKind::Audio);
        let mut audio = source
            .open_audio(probe.audio_stream.unwrap(), running())
            .unwrap();
        let mut samples = 0;
        while let Some(frame) = audio.next_frame().unwrap() {
            samples += frame.samples();
        }
        assert_eq!(samples, 48_000);
        assert!(source.accepted_output_bytes() >= 96_000);
    }

    #[test]
    fn s3_certified_mpegps_refuses_incomplete_table_before_probe() {
        let directory = TempDirectory::new("kinewright-owned-mpegps");
        let path = directory.path("source.mpg");
        run_ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=64x48:rate=25:duration=1",
                "-c:v",
                "mpeg2video",
                "-g",
                "1",
                "-threads",
                "1",
                "-f",
                "mpeg",
            ],
            &path,
        );
        let source = source(&path);
        let (format, avio) = bridge::Avio::open(source.cursor(running()).unwrap()).unwrap();
        let header = CertifiedInput {
            format,
            avio,
            probe_charges: Vec::new(),
            decoder_bound: None,
        };
        assert_eq!(
            header.format.nb_streams(),
            0,
            "MPEG-PS opens without streams"
        );
        assert!(bridge::dynamic_stream_table(&header.format));
        drop(header);
        let (result, calls) = observe_stream_info_probes(|| source.probe(running()));
        assert_eq!(calls, 0, "C0_INCOMPLETE_PROBE_NOT_ENTERED");
        assert!(
            matches!(result, Err(OwnedIoError::Decoder(ref message))
                if message == "incomplete or dynamic stream table unsupported"),
            "C0_INCOMPLETE_TABLE_REFUSED: {result:?}",
        );
        assert_eq!(source.accepted_output_bytes(), 0);
    }

    #[test]
    fn s3_certified_subtitle_refuses_unsupported_kind_before_probe() {
        let directory = TempDirectory::new("kinewright-owned-subtitle");
        let subtitle = directory.path("caption.srt");
        fs::write(
            &subtitle,
            "1\n00:00:00,000 --> 00:00:01,000\nUnbounded subtitle decoder control\n",
        )
        .unwrap();
        let path = directory.path("source.mkv");
        run_ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                "sine=sample_rate=48000:duration=1",
                "-i",
                subtitle.to_str().unwrap(),
                "-map",
                "0:a",
                "-map",
                "1:s",
                "-c:a",
                "pcm_s16le",
                "-c:s",
                "srt",
                "-threads",
                "1",
                "-t",
                "1",
            ],
            &path,
        );
        let source = source(&path);
        let (format, avio) = bridge::Avio::open(source.cursor(running()).unwrap()).unwrap();
        let header = CertifiedInput {
            format,
            avio,
            probe_charges: Vec::new(),
            decoder_bound: None,
        };
        assert_eq!(
            header.format.nb_streams(),
            2,
            "subtitle fixture has a complete header"
        );
        assert!(!bridge::dynamic_stream_table(&header.format));
        assert_eq!(
            header.format.stream(1).unwrap().parameters().medium(),
            ffmpeg::media::Type::Subtitle
        );
        drop(header);
        let (result, calls) = observe_stream_info_probes(|| source.probe(running()));
        assert_eq!(calls, 0, "C0_NON_AV_PROBE_NOT_ENTERED");
        assert!(
            matches!(result, Err(OwnedIoError::Decoder(ref message))
                if message == "non-audio/video stream kind unsupported"),
            "C0_NON_AV_TABLE_REFUSED: {result:?}",
        );
        assert_eq!(source.accepted_output_bytes(), 0);
        let wav = self::source(&wav_fixture(&directory));
        let (control, calls) = observe_stream_info_probes(|| wav.probe(running()));
        assert!(control.is_ok(), "supported WAV probe control: {control:?}");
        assert_eq!(calls, 1, "probe observer sees a supported production probe");
    }

    #[test]
    fn s3_certified_probe_stream_limit_refuses_before_new_stream_allocation() {
        let directory = TempDirectory::new("kinewright-owned-probe-limit");
        let source = source(&wav_fixture(&directory));
        let (format, avio) = bridge::Avio::open(source.cursor(running()).unwrap()).unwrap();
        let mut header = CertifiedInput {
            format,
            avio,
            probe_charges: Vec::new(),
            decoder_bound: None,
        };
        assert_eq!(header.format.nb_streams(), 1);
        assert!(
            bridge::append_stream(&mut header.format),
            "header limit control permits a second stream"
        );
        drop(header);
        let mut bounded = CertifiedInput::open(&source, running()).unwrap();
        assert!(
            !bridge::append_stream(&mut bounded.format),
            "C0_PROBE_NEW_STREAM_ALLOCATION_REFUSED",
        );
        assert_eq!(bounded.format.nb_streams(), 1);
    }

    #[test]
    fn s3_certified_concat_counterfactual_accepts_uncertified_pcm() {
        let (_directory, source) = nested_fixture(false, TestOpenPolicy::NestedOutputControl);
        read_nested_audio(&source)
            .expect("unrestricted concat fixture must decode its uncertified WAV");
        assert!(
            source.accepted_output_bytes() >= 96_000,
            "counterfactual accepts the one-second WAV PCM"
        );
        assert_eq!(source.0.hooks.nested_open_calls.load(Ordering::Acquire), 0);
    }

    #[test]
    fn s3_certified_ffconcat_and_hls_refuse_uncertified_nested_output() {
        for hls in [false, true] {
            let (_directory, source) = nested_fixture(hls, TestOpenPolicy::Production);
            let result = read_nested_audio(&source);
            assert_eq!(source.accepted_output_bytes(), 0, "C0_NESTED_OUTPUT_ZERO");
            assert!(
                matches!(result, Err(OwnedIoError::Decoder(_))),
                "C0_PLAYLIST_REFUSED"
            );
            assert_eq!(source.0.hooks.nested_open_calls.load(Ordering::Acquire), 0);
        }
    }

    #[test]
    fn s3_certified_nested_protocol_policy_is_inherited_by_concat() {
        let (_directory, source) = nested_fixture(false, TestOpenPolicy::ProtocolGuard);
        let result = read_nested_audio(&source);
        assert_eq!(source.accepted_output_bytes(), 0, "C0_PROTOCOL_OUTPUT_ZERO");
        assert!(
            matches!(result, Err(OwnedIoError::Decoder(_))),
            "C0_PROTOCOL_REFUSED"
        );
        // Concat does not call our callback. The inherited whitelist is the guard here.
        assert_eq!(source.0.hooks.nested_open_calls.load(Ordering::Acquire), 0);
    }

    #[test]
    fn s3_certified_nested_format_policy_precedes_concat_and_hls_opens() {
        for hls in [false, true] {
            let (_directory, source) = nested_fixture(hls, TestOpenPolicy::FormatGuard);
            let result = read_nested_audio(&source);
            assert_eq!(source.accepted_output_bytes(), 0, "C0_FORMAT_OUTPUT_ZERO");
            assert_eq!(
                source.0.hooks.nested_open_calls.load(Ordering::Acquire),
                0,
                "C0_FORMAT_NO_SECONDARY_ATTEMPT",
            );
            assert!(
                matches!(result, Err(OwnedIoError::Decoder(_))),
                "C0_FORMAT_REFUSED"
            );
        }
    }

    #[test]
    fn s3_certified_hls_format_policy_precedes_secondary_open() {
        let (_directory, source) = nested_fixture(true, TestOpenPolicy::FormatGuard);
        let result = read_nested_audio(&source);
        assert_eq!(source.accepted_output_bytes(), 0);
        assert_eq!(
            source.0.hooks.nested_open_calls.load(Ordering::Acquire),
            0,
            "C0_HLS_NO_SECONDARY_ATTEMPT",
        );
        assert!(
            matches!(result, Err(OwnedIoError::Decoder(_))),
            "C0_HLS_FORMAT_REFUSED"
        );
    }

    struct DropFlag(Arc<AtomicBool>);
    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    struct Charge {
        used: Arc<AtomicUsize>,
        bytes: usize,
    }
    impl Drop for Charge {
        fn drop(&mut self) {
            self.used.fetch_sub(self.bytes, Ordering::AcqRel);
        }
    }

    #[test]
    fn s3_certified_cursor_capacity_and_decoder_pin_charge_lifetime() {
        let (_directory, path, _source) = media_fixture("red");
        let dropped = Arc::new(AtomicBool::new(false));
        let lifetime: LifetimeGuard = Arc::new(DropFlag(Arc::clone(&dropped)));
        let weak = Arc::downgrade(&lifetime);
        let used = Arc::new(AtomicUsize::new(0));
        let ledger = Arc::clone(&used);
        let reserve: Arc<CursorReservation> = Arc::new(move |bytes| {
            let bytes = usize::try_from(bytes).unwrap();
            let mut used = ledger.load(Ordering::Acquire);
            loop {
                if bytes == usize::try_from(CURSOR_RESERVATION_BYTES).unwrap() && used != 0 {
                    return Err("one cursor budget".into());
                }
                let next = used.checked_add(bytes).ok_or("cursor budget overflow")?;
                match ledger.compare_exchange_weak(used, next, Ordering::AcqRel, Ordering::Acquire)
                {
                    Ok(_) => break,
                    Err(current) => used = current,
                }
            }
            Ok(Arc::new(Charge {
                used: Arc::clone(&ledger),
                bytes,
            }))
        });
        let source = source_guarded(&path, lifetime, reserve);
        let mut decoder = source.open_video(0, running()).unwrap();
        assert!(used.load(Ordering::Acquire) > usize::try_from(CURSOR_RESERVATION_BYTES).unwrap());
        assert!(matches!(
            source.open_audio(1, running()),
            Err(OwnedIoError::Capacity(_))
        ));
        drop(source);
        assert!(weak.upgrade().is_some(), "C0_SOURCE_AUTHORITY_RETAINED");
        assert!(!dropped.load(Ordering::Acquire));
        let frame = decoder.next_frame().unwrap().unwrap();
        drop(decoder);
        assert!(
            weak.upgrade().is_some(),
            "a retained frame keeps source authority"
        );
        assert!(
            used.load(Ordering::Acquire) > 0,
            "a retained frame keeps decoder and frame charges"
        );
        drop(frame);
        assert!(weak.upgrade().is_none());
        assert!(dropped.load(Ordering::Acquire));
        assert_eq!(used.load(Ordering::Acquire), 0);
    }
    #[test]
    fn s3_certified_selected_streams_cannot_be_opened_or_widened() {
        let (_directory, _path, source) = media_fixture("red");
        let probe = source.probe(running()).unwrap();
        let index = probe.video_stream.unwrap();
        let audio_only = source
            .clone()
            .with_selected_streams(None, probe.audio_stream)
            .unwrap();
        assert!(
            matches!(
                audio_only.open_video(index, running()),
                Err(OwnedIoError::InvalidCertificate(_))
            ),
            "C0_SELECTED_VIDEO_REFUSED"
        );
        let source = source.with_selected_streams(Some(index), None).unwrap();
        assert!(
            source
                .open_video(index, running())
                .unwrap()
                .next_frame()
                .unwrap()
                .is_some()
        );
        assert!(
            matches!(
                source.open_audio(probe.audio_stream.unwrap(), running()),
                Err(OwnedIoError::InvalidCertificate(_))
            ),
            "C0_SELECTED_AUDIO_REFUSED"
        );
        assert!(matches!(
            source.open_video(probe.audio_stream.unwrap(), running()),
            Err(OwnedIoError::InvalidCertificate(_))
        ));
        assert!(
            source
                .clone()
                .with_selected_streams(Some(index), probe.audio_stream)
                .is_err()
        );
        let narrowed = source.probe(running()).unwrap();
        assert_eq!(narrowed.kind, MediaKind::Video);
        assert!(narrowed.audio_stream.is_none());
    }
    #[test]
    fn s3_certified_probe_rejects_missing_and_wrong_kind_selected_streams() {
        let (_directory, _path, source) = media_fixture("red");
        let probe = source.probe(running()).unwrap();
        let video = probe.video_stream.unwrap();
        let audio = probe.audio_stream.unwrap();
        for (video, audio) in [
            (Some(usize::MAX), None),
            (None, Some(usize::MAX)),
            (Some(video), Some(usize::MAX)),
            (Some(audio), None),
            (None, Some(video)),
            (Some(video), Some(video)),
        ] {
            let narrowed = source.clone().with_selected_streams(video, audio).unwrap();
            assert!(
                matches!(
                    narrowed.probe(running()),
                    Err(OwnedIoError::InvalidCertificate(_))
                ),
                "C0_SELECTED_STREAM_TYPED_REFUSAL video={video:?} audio={audio:?}",
            );
        }
        for (video, audio, kind) in [
            (Some(video), Some(audio), MediaKind::AudioVideo),
            (Some(video), None, MediaKind::Video),
            (None, Some(audio), MediaKind::Audio),
        ] {
            assert_eq!(
                source
                    .clone()
                    .with_selected_streams(video, audio)
                    .unwrap()
                    .probe(running())
                    .unwrap()
                    .kind,
                kind,
            );
        }
    }
    #[test]
    fn s3_certified_cursor_charge_survives_source_until_final_cursor_close() {
        let (_directory, path, _source) = bytes_fixture(&[5; 32]);
        let used = Arc::new(AtomicUsize::new(0));
        let ledger = Arc::clone(&used);
        let source = source_guarded(
            &path,
            Arc::new(()),
            Arc::new(move |bytes| {
                let bytes = usize::try_from(bytes).unwrap();
                ledger.fetch_add(bytes, Ordering::AcqRel);
                Ok(Arc::new(Charge {
                    used: Arc::clone(&ledger),
                    bytes,
                }))
            }),
        );
        let mut cursor = source.cursor(running()).unwrap();
        assert_eq!(
            used.load(Ordering::Acquire),
            usize::try_from(CURSOR_RESERVATION_BYTES).unwrap(),
            "C0_CURSOR_CHARGE_RETAINED",
        );
        drop(source);
        assert_eq!(cursor.read(&mut [0; 32]), Ok(32));
        assert_eq!(
            used.load(Ordering::Acquire),
            usize::try_from(CURSOR_RESERVATION_BYTES).unwrap()
        );
        drop(cursor);
        assert_eq!(used.load(Ordering::Acquire), 0);
    }

    #[test]
    fn s3_certified_decoder_and_output_frame_preflight_refuse_before_use() {
        let (_directory, path, _source) = media_fixture("red");
        for refuse_call in [3, 4] {
            let calls = Arc::new(AtomicUsize::new(0));
            let counted = Arc::clone(&calls);
            let source = source_guarded(
                &path,
                Arc::new(()),
                Arc::new(move |_| {
                    if counted.fetch_add(1, Ordering::AcqRel) == refuse_call {
                        return Err("decoder/frame allocation refusal".into());
                    }
                    Ok(Arc::new(()))
                }),
            );
            let opened = source.open_video(0, running());
            if refuse_call == 3 {
                assert!(
                    matches!(opened, Err(OwnedIoError::Capacity(_))),
                    "C0_DECODER_ALLOCATION_PREFLIGHT"
                );
            } else {
                let mut decoder = opened.unwrap();
                assert!(
                    matches!(decoder.next_frame(), Err(OwnedIoError::Capacity(_))),
                    "C0_FRAME_ALLOCATION_PREFLIGHT"
                );
            }
        }
    }
    #[test]
    fn s3_certified_inert_label_has_no_provenance_fallback() {
        let (directory, path, source) = media_fixture("red");
        fs::rename(&path, directory.path("renamed.avi")).unwrap();
        assert!(!path.exists());
        let asset = source
            .probe_asset(kinewright_core::AssetId(7), &path, running())
            .unwrap();
        assert_eq!(asset.path, path);
        let mut decoder = source.open_video(0, running()).unwrap();
        let frame = decoder.next_frame().unwrap().unwrap();
        assert!((60..=65).contains(&frame.data(0)[0]));
        assert!(decoder.input.avio.buffer_size() <= AVIO_BUFFER_BYTES);
        assert!(decoder.delivered_bytes() > 0);
        assert!(bridge::secondary_open(&mut decoder.input, c"file:forbidden") < 0);
        assert!(matches!(
            decoder.next_frame(),
            Err(OwnedIoError::Unavailable(_))
        ));
    }
    mod pf1 {
        include!("certified_playback_tests.rs");
    }
}
