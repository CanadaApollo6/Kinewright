use std::{path::Path, sync::Arc};

use ffmpeg_next as ffmpeg;
use openreel_core::{
    AssetId, FrameTexture, MediaAsset, MediaError, MediaKind, Rational, RgbaImage, TimeCode,
};

use crate::cache::FrameCache;

const AV_TIME_BASE: i64 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VideoRotation {
    None,
    Clockwise90,
    HalfTurn,
    Clockwise270,
}

impl VideoRotation {
    fn display_dimensions(self, width: u32, height: u32) -> (u32, u32) {
        match self {
            Self::Clockwise90 | Self::Clockwise270 => (height, width),
            Self::None | Self::HalfTurn => (width, height),
        }
    }

    fn encoded_dimensions(self, display_width: u32, display_height: u32) -> (u32, u32) {
        match self {
            Self::Clockwise90 | Self::Clockwise270 => (display_height, display_width),
            Self::None | Self::HalfTurn => (display_width, display_height),
        }
    }
}

pub(crate) fn probe_path(path: &Path, id: AssetId) -> Result<MediaAsset, MediaError> {
    let input = media_input(path)?;
    let video = input.streams().best(ffmpeg::media::Type::Video);
    let audio = input.streams().best(ffmpeg::media::Type::Audio);

    if let Some(stream) = &video {
        ensure_decoder(stream, "video", path)?;
    }
    if let Some(stream) = &audio {
        ensure_decoder(stream, "audio", path)?;
    }

    let kind = match (video.is_some(), audio.is_some()) {
        (true, true) => MediaKind::AudioVideo,
        (true, false) => MediaKind::Video,
        (false, true) => MediaKind::Audio,
        (false, false) => {
            return Err(MediaError::Backend(format!(
                "media {} has no decodable audio or video stream; the file may be truncated or its codecs may be unsupported",
                path.display()
            )));
        }
    };

    let (fps, resolution, duration) = if let Some(stream) = video {
        let timing =
            analyze_video_packets(path, stream.index(), normalized_start(stream.start_time()))?;
        let rate = select_video_rate(path, &stream, &timing)?;
        let rotation = stream_rotation(&stream).map_err(|error| {
            MediaError::Backend(format!(
                "could not read video orientation for {}: {error}",
                path.display()
            ))
        })?;
        let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .map_err(|error| media_error(path, "could not read video codec parameters", error))?;
        let decoder = context
            .decoder()
            .video()
            .map_err(|error| media_error(path, "could not open the video decoder", error))?;
        let stream_duration = timestamp_to_grid_ceil(stream.duration(), stream.time_base(), rate);
        let packet_duration = timing.duration.map_or(0, |duration| {
            timestamp_to_grid_ceil(duration, stream.time_base(), rate)
        });
        let container_duration = duration_us_to_frames(input.duration(), rate);
        let video_duration = stream_duration.max(packet_duration);
        let duration = TimeCode(if video_duration > 0 {
            video_duration
        } else {
            container_duration
        });
        let resolution = rotation.display_dimensions(decoder.width(), decoder.height());
        (rate, Some(resolution), duration)
    } else {
        let rate = Rational::default();
        (
            rate,
            None,
            TimeCode(duration_us_to_frames(input.duration(), rate)),
        )
    };

    if duration <= TimeCode::ZERO {
        return Err(MediaError::Backend(format!(
            "media duration is missing or zero for {}; the file may be truncated",
            path.display()
        )));
    }

    Ok(MediaAsset {
        id,
        path: path.to_path_buf(),
        name: path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("media")
            .to_owned(),
        duration,
        fps,
        kind,
        resolution,
    })
}

fn valid_rate(rate: ffmpeg::Rational) -> Option<Rational> {
    let numerator = u32::try_from(rate.numerator()).ok()?;
    let denominator = u32::try_from(rate.denominator()).ok()?;
    Rational::new(numerator, denominator).ok()
}

fn duration_us_to_frames(duration: i64, fps: Rational) -> i64 {
    if duration <= 0 {
        return 0;
    }
    mul_div_ceil(
        duration,
        i64::from(fps.numerator()),
        AV_TIME_BASE.saturating_mul(i64::from(fps.denominator())),
    )
}

fn select_video_rate(
    path: &Path,
    stream: &ffmpeg::Stream<'_>,
    timing: &VideoPacketTiming,
) -> Result<Rational, MediaError> {
    let average = valid_rate(stream.avg_frame_rate());
    let nominal = valid_rate(stream.rate());
    match (average, nominal) {
        (Some(average), Some(nominal)) if average != nominal => {
            if timing.variable {
                Ok(average)
            } else {
                Ok(nominal)
            }
        }
        (Some(average), _) => Ok(average),
        (None, Some(nominal)) => Ok(nominal),
        (None, None) => Err(MediaError::Backend(format!(
            "video {} has no valid frame rate",
            path.display()
        ))),
    }
}

struct VideoPacketTiming {
    variable: bool,
    duration: Option<i64>,
}

fn analyze_video_packets(
    path: &Path,
    stream_index: usize,
    stream_start: i64,
) -> Result<VideoPacketTiming, MediaError> {
    let mut input = media_input(path)?;
    let mut reference_duration = None;
    let mut duration_variable = false;
    let mut pictures = Vec::new();
    for (stream, packet) in input.packets() {
        if stream.index() != stream_index {
            continue;
        }
        let packet_duration = packet.duration();
        if packet_duration > 0 {
            if reference_duration
                .is_some_and(|reference| timing_values_differ(reference, packet_duration))
            {
                duration_variable = true;
            }
            reference_duration.get_or_insert(packet_duration);
        }
        if let Some(pts) = packet.pts() {
            pictures.push((pts, packet_duration.max(0)));
        }
    }
    pictures.sort_unstable_by_key(|(pts, _)| *pts);
    pictures.dedup_by(|current, previous| {
        if current.0 == previous.0 {
            previous.1 = previous.1.max(current.1);
            true
        } else {
            false
        }
    });
    let mut delta = None;
    let mut delta_count = 0_u64;
    let mut pts_variable = false;
    for pair in pictures.windows(2) {
        let current = pair[1].0.saturating_sub(pair[0].0);
        if current <= 0 {
            continue;
        }
        delta_count = delta_count.saturating_add(1);
        if delta.is_some_and(|reference| timing_values_differ(reference, current)) {
            pts_variable = true;
        }
        delta.get_or_insert(current);
    }
    let duration = pictures.last().and_then(|(last_pts, last_duration)| {
        let final_interval = if *last_duration > 0 {
            *last_duration
        } else {
            delta.unwrap_or(0)
        };
        let end = last_pts.saturating_add(final_interval);
        (end > stream_start).then_some(end.saturating_sub(stream_start))
    });
    let variable = if delta_count >= 2 {
        pts_variable
    } else {
        duration_variable
    };
    Ok(VideoPacketTiming { variable, duration })
}

fn timing_values_differ(lhs: i64, rhs: i64) -> bool {
    let tolerance = lhs.abs().min(rhs.abs()).saturating_div(1_000).max(1);
    lhs.abs_diff(rhs) > u64::try_from(tolerance).unwrap_or(u64::MAX)
}

fn timestamp_to_grid_ceil(timestamp: i64, time_base: ffmpeg::Rational, fps: Rational) -> i64 {
    if timestamp <= 0 {
        return 0;
    }
    let numerator = i64::from(time_base.numerator()).saturating_mul(i64::from(fps.numerator()));
    let denominator =
        i64::from(time_base.denominator()).saturating_mul(i64::from(fps.denominator()));
    mul_div_ceil(timestamp, numerator, denominator)
}

fn mul_div_ceil(value: i64, multiplier: i64, divisor: i64) -> i64 {
    if divisor <= 0 {
        return 0;
    }
    let value = i128::from(value).saturating_mul(i128::from(multiplier));
    let divisor = i128::from(divisor);
    let rounded = value.saturating_add(divisor - 1) / divisor;
    i64::try_from(rounded).unwrap_or(i64::MAX)
}

fn frame_to_global_timestamp(frame: TimeCode, fps: Rational) -> i64 {
    if frame.0 <= 0 {
        return 0;
    }
    let numerator = i128::from(frame.0)
        .saturating_mul(i128::from(AV_TIME_BASE))
        .saturating_mul(i128::from(fps.denominator()));
    let timestamp = numerator / i128::from(fps.numerator());
    i64::try_from(timestamp).unwrap_or(i64::MAX)
}

pub(crate) fn stream_timestamp_to_global(timestamp: i64, time_base: ffmpeg::Rational) -> i64 {
    if timestamp <= 0 {
        return 0;
    }
    let numerator = i128::from(timestamp)
        .saturating_mul(i128::from(time_base.numerator()))
        .saturating_mul(i128::from(AV_TIME_BASE));
    let denominator = i128::from(time_base.denominator());
    i64::try_from(numerator / denominator).unwrap_or(i64::MAX)
}

pub(crate) fn media_input(path: &Path) -> Result<ffmpeg::format::context::Input, MediaError> {
    ffmpeg::format::input(path).map_err(|error| {
        MediaError::Backend(format!(
            "could not open media {}: {error}; the file may be truncated or its format may be unsupported",
            path.display()
        ))
    })
}

pub(crate) fn media_error(path: &Path, action: &str, error: impl std::fmt::Display) -> MediaError {
    MediaError::Backend(format!(
        "{action} for {}: {error}; the file may be truncated",
        path.display()
    ))
}

pub(crate) fn ensure_decoder(
    stream: &ffmpeg::Stream<'_>,
    kind: &str,
    path: &Path,
) -> Result<(), MediaError> {
    let codec = stream.parameters().id();
    if ffmpeg::decoder::find(codec).is_none() {
        return Err(MediaError::Backend(format!(
            "{kind} codec {codec:?} in {} is not supported by this FFmpeg build",
            path.display()
        )));
    }
    Ok(())
}

fn stream_rotation(stream: &ffmpeg::Stream<'_>) -> Result<VideoRotation, MediaError> {
    let side_data_rotation = stream.side_data().find_map(|side_data| {
        (side_data.kind() == ffmpeg::codec::packet::side_data::Type::DisplayMatrix)
            .then(|| rotation_from_display_matrix(side_data.data()))
    });
    if let Some(rotation) = side_data_rotation {
        return rotation;
    }
    let metadata = stream.metadata();
    let Some(value) = metadata.get("rotate") else {
        return Ok(VideoRotation::None);
    };
    let degrees = value.parse::<i32>().map_err(|_| {
        MediaError::Backend(format!("video rotation metadata {value:?} is invalid"))
    })?;
    rotation_from_degrees(degrees)
}

fn rotation_from_display_matrix(data: &[u8]) -> Result<VideoRotation, MediaError> {
    if data.len() < 36 {
        return Err(MediaError::Backend(
            "video display matrix is truncated".to_owned(),
        ));
    }
    let element = |index: usize| {
        let offset = index * 4;
        i32::from_ne_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ])
    };
    let a = i64::from(element(0));
    let b = i64::from(element(1));
    let c = i64::from(element(3));
    let d = i64::from(element(4));
    let determinant = a.saturating_mul(d).saturating_sub(b.saturating_mul(c));
    if determinant < 0 {
        return Err(MediaError::Backend(
            "reflected video display matrices are not supported".to_owned(),
        ));
    }
    if determinant == 0 {
        return Err(MediaError::Backend(
            "video display matrix is singular or invalid".to_owned(),
        ));
    }
    let dominant = a.abs().max(b.abs());
    let residual = a.abs().min(b.abs());
    if dominant == 0 || residual.saturating_mul(1_000) > dominant {
        return Err(MediaError::Backend(
            "non-right-angle video display rotation is not supported".to_owned(),
        ));
    }
    let degrees = if a.abs() >= b.abs() {
        if a >= 0 { 0 } else { 180 }
    } else if b < 0 {
        90
    } else {
        270
    };
    rotation_from_degrees(degrees)
}

fn rotation_from_degrees(degrees: i32) -> Result<VideoRotation, MediaError> {
    match degrees.rem_euclid(360) {
        0 => Ok(VideoRotation::None),
        90 => Ok(VideoRotation::Clockwise90),
        180 => Ok(VideoRotation::HalfTurn),
        270 => Ok(VideoRotation::Clockwise270),
        angle => Err(MediaError::Backend(format!(
            "video display rotation {angle} degrees is not supported; use a right-angle rotation"
        ))),
    }
}

struct PendingVideoFrame {
    first_grid_frame: i64,
    decoded: Option<ffmpeg::frame::Video>,
    texture: Option<FrameTexture>,
}

pub(crate) struct VideoDecoder {
    path: std::path::PathBuf,
    input: ffmpeg::format::context::Input,
    decoder: ffmpeg::decoder::Video,
    scaler: ffmpeg::software::scaling::Context,
    stream_index: usize,
    stream_time_base: ffmpeg::Rational,
    stream_start: i64,
    fps: Rational,
    rotation: VideoRotation,
    scaled_width: u32,
    scaled_height: u32,
    fallback_index: i64,
    pending: Option<PendingVideoFrame>,
    lookahead: Option<PendingVideoFrame>,
    continuation_at: Option<TimeCode>,
    eof_sent: bool,
    seek_count: u64,
}

impl VideoDecoder {
    pub(crate) fn open(path: &Path, fps: Rational) -> Result<Self, MediaError> {
        Self::open_scaled(path, fps, None)
    }

    pub(crate) fn open_scaled(
        path: &Path,
        fps: Rational,
        max_width: Option<u32>,
    ) -> Result<Self, MediaError> {
        let input = media_input(path)?;
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or_else(|| {
                MediaError::Backend(format!("media {} has no video stream", path.display()))
            })?;
        let stream_index = stream.index();
        let stream_time_base = stream.time_base();
        let stream_start = normalized_start(stream.start_time());
        ensure_decoder(&stream, "video", path)?;
        let rotation = stream_rotation(&stream).map_err(|error| {
            MediaError::Backend(format!(
                "could not read video orientation for {}: {error}",
                path.display()
            ))
        })?;
        let mut context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .map_err(|error| media_error(path, "could not read video codec parameters", error))?;
        context.set_threading(ffmpeg::codec::threading::Config {
            kind: ffmpeg::codec::threading::Type::Frame,
            count: std::thread::available_parallelism()
                .map_or(1, std::num::NonZeroUsize::get)
                .min(16),
        });
        let decoder = context
            .decoder()
            .video()
            .map_err(|error| media_error(path, "could not open the video decoder", error))?;
        let source_width = decoder.width();
        let source_height = decoder.height();
        if source_width == 0 || source_height == 0 {
            return Err(MediaError::Backend(format!(
                "video {} reports an invalid zero-sized frame",
                path.display()
            )));
        }
        let display_source = rotation.display_dimensions(source_width, source_height);
        let display_width = max_width
            .unwrap_or(display_source.0)
            .min(display_source.0)
            .max(1);
        let display_height = u32::try_from(
            u64::from(display_source.1).saturating_mul(u64::from(display_width))
                / u64::from(display_source.0.max(1)),
        )
        .unwrap_or(display_source.1)
        .max(1);
        let (scaled_width, scaled_height) =
            rotation.encoded_dimensions(display_width, display_height);
        let scaler = ffmpeg::software::scaling::Context::get(
            decoder.format(),
            source_width,
            source_height,
            ffmpeg::format::Pixel::RGBA,
            scaled_width,
            scaled_height,
            ffmpeg::software::scaling::Flags::BILINEAR,
        )
        .map_err(|error| media_error(path, "could not create video pixel converter", error))?;

        Ok(Self {
            path: path.to_path_buf(),
            input,
            decoder,
            scaler,
            stream_index,
            stream_time_base,
            stream_start,
            fps,
            rotation,
            scaled_width,
            scaled_height,
            fallback_index: 0,
            pending: None,
            lookahead: None,
            continuation_at: None,
            eof_sent: false,
            seek_count: 0,
        })
    }

    pub(crate) fn decode_window(
        &mut self,
        start: TimeCode,
        end: TimeCode,
        cache: &mut FrameCache,
    ) -> Result<(), MediaError> {
        let timestamp = frame_to_global_timestamp(start, self.fps).saturating_add(
            stream_timestamp_to_global(self.stream_start, self.stream_time_base),
        );
        self.input
            .seek(timestamp, ..timestamp)
            .map_err(|error| media_error(&self.path, "video seek failed", error))?;
        self.decoder.flush();
        self.fallback_index = start.0;
        self.pending = None;
        self.lookahead = None;
        self.continuation_at = None;
        self.eof_sent = false;
        self.seek_count = self.seek_count.saturating_add(1);

        self.decode_from_cursor(start, end, cache)
    }

    /// Continue directly from the prior window when it ended immediately
    /// before `start`. A discontinuity falls back to the deterministic seek
    /// path used by scrubbing.
    pub(crate) fn decode_window_sequential(
        &mut self,
        start: TimeCode,
        end: TimeCode,
        cache: &mut FrameCache,
    ) -> Result<(), MediaError> {
        if self.continuation_at != Some(start) {
            return self.decode_window(start, end, cache);
        }
        self.decode_from_cursor(start, end, cache)
    }

    #[cfg(test)]
    pub(crate) fn seek_count(&self) -> u64 {
        self.seek_count
    }

    fn decode_from_cursor(
        &mut self,
        start: TimeCode,
        end: TimeCode,
        cache: &mut FrameCache,
    ) -> Result<(), MediaError> {
        if end < start {
            self.continuation_at = Some(start);
            return Ok(());
        }

        if self.apply_lookahead(start, end, cache)? {
            self.continuation_at = Some(TimeCode(end.0.saturating_add(1)));
            return Ok(());
        }

        loop {
            if self.receive_frames(start, end, cache)? {
                self.continuation_at = Some(TimeCode(end.0.saturating_add(1)));
                return Ok(());
            }
            if self.eof_sent {
                self.cache_pending_until(end.0.saturating_add(1), start, end, cache)?;
                self.continuation_at = Some(TimeCode(end.0.saturating_add(1)));
                return Ok(());
            }
            let next = self
                .input
                .packets()
                .next()
                .map(|(stream, packet)| (stream.index(), packet));
            let Some((stream_index, packet)) = next else {
                self.decoder.send_eof().map_err(|error| {
                    media_error(&self.path, "video decoder flush failed", error)
                })?;
                self.eof_sent = true;
                continue;
            };
            if stream_index != self.stream_index {
                continue;
            }
            self.decoder
                .send_packet(&packet)
                .map_err(|error| media_error(&self.path, "video decode failed", error))?;
        }
    }

    fn apply_lookahead(
        &mut self,
        start: TimeCode,
        end: TimeCode,
        cache: &mut FrameCache,
    ) -> Result<bool, MediaError> {
        let Some(next) = self.lookahead.take() else {
            return Ok(false);
        };
        self.accept_frame(next, start, end, cache)
    }

    fn receive_frames(
        &mut self,
        start: TimeCode,
        end: TimeCode,
        cache: &mut FrameCache,
    ) -> Result<bool, MediaError> {
        loop {
            let mut decoded = ffmpeg::frame::Video::empty();
            if self.decoder.receive_frame(&mut decoded).is_err() {
                break;
            }
            let first_grid_frame = decoded.timestamp().map_or_else(
                || {
                    let index = self.fallback_index;
                    self.fallback_index = self.fallback_index.saturating_add(1);
                    index
                },
                |pts| {
                    timestamp_to_grid_ceil(
                        pts.saturating_sub(self.stream_start),
                        self.stream_time_base,
                        self.fps,
                    )
                },
            );
            let next = PendingVideoFrame {
                first_grid_frame,
                decoded: Some(decoded),
                texture: None,
            };
            self.fallback_index = first_grid_frame.saturating_add(1);
            if self.accept_frame(next, start, end, cache)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn accept_frame(
        &mut self,
        next: PendingVideoFrame,
        start: TimeCode,
        end: TimeCode,
        cache: &mut FrameCache,
    ) -> Result<bool, MediaError> {
        if self.pending.is_some() {
            self.cache_pending_until(next.first_grid_frame, start, end, cache)?;
        }
        if next.first_grid_frame > end.0 && self.pending.is_some() {
            self.lookahead = Some(next);
            return Ok(true);
        }
        self.pending = Some(next);
        Ok(false)
    }

    fn cache_pending_until(
        &mut self,
        next_grid_frame: i64,
        start: TimeCode,
        end: TimeCode,
        cache: &mut FrameCache,
    ) -> Result<(), MediaError> {
        let Some(mut pending) = self.pending.take() else {
            return Ok(());
        };
        let first = pending.first_grid_frame.max(start.0);
        let last = next_grid_frame.saturating_sub(1).min(end.0);
        if first <= last {
            let texture = if let Some(texture) = &pending.texture {
                texture.clone()
            } else {
                let decoded = pending.decoded.as_ref().ok_or_else(|| {
                    MediaError::Backend("pending video frame has no pixels".to_owned())
                })?;
                self.convert(decoded)?
            };
            pending.decoded = None;
            pending.texture = Some(texture.clone());
            for index in first..=last {
                cache.insert(TimeCode(index), texture.clone());
            }
        }
        self.pending = Some(pending);
        Ok(())
    }

    fn convert(&mut self, decoded: &ffmpeg::frame::Video) -> Result<FrameTexture, MediaError> {
        let mut rgba = ffmpeg::frame::Video::empty();
        self.scaler
            .run(decoded, &mut rgba)
            .map_err(|error| media_error(&self.path, "video pixel conversion failed", error))?;
        let row_bytes = usize::try_from(self.scaled_width)
            .unwrap_or_default()
            .saturating_mul(4);
        let height = usize::try_from(self.scaled_height).unwrap_or_default();
        let mut pixels = Vec::with_capacity(row_bytes.saturating_mul(height));
        let plane = rgba.data(0);
        let stride = rgba.stride(0);
        for row in 0..height {
            let start = row.saturating_mul(stride);
            let end = start.saturating_add(row_bytes);
            let source = plane.get(start..end).ok_or_else(|| {
                MediaError::Backend("decoded RGBA frame has an invalid stride".to_owned())
            })?;
            pixels.extend_from_slice(source);
        }
        let (width, height, pixels) =
            rotate_rgba(self.rotation, self.scaled_width, self.scaled_height, pixels)?;
        Ok(FrameTexture {
            width,
            height,
            rgba: Arc::new(pixels),
        })
    }
}

fn rotate_rgba(
    rotation: VideoRotation,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
) -> Result<(u32, u32, Vec<u8>), MediaError> {
    if rotation == VideoRotation::None {
        return Ok((width, height, pixels));
    }
    let expected = usize::try_from(width)
        .unwrap_or_default()
        .checked_mul(usize::try_from(height).unwrap_or_default())
        .and_then(|value| value.checked_mul(4))
        .ok_or_else(|| MediaError::Backend("rotated frame is too large".to_owned()))?;
    if pixels.len() != expected {
        return Err(MediaError::Backend(
            "decoded frame size does not match its dimensions".to_owned(),
        ));
    }
    let (output_width, output_height) = rotation.display_dimensions(width, height);
    let mut rotated = vec![0_u8; expected];
    for output_y in 0..output_height {
        for output_x in 0..output_width {
            let (source_x, source_y) = match rotation {
                VideoRotation::Clockwise90 => (output_y, height - 1 - output_x),
                VideoRotation::HalfTurn => (width - 1 - output_x, height - 1 - output_y),
                VideoRotation::Clockwise270 => (width - 1 - output_y, output_x),
                VideoRotation::None => (output_x, output_y),
            };
            let source = usize::try_from(source_y)
                .unwrap_or_default()
                .saturating_mul(usize::try_from(width).unwrap_or_default())
                .saturating_add(usize::try_from(source_x).unwrap_or_default())
                .saturating_mul(4);
            let destination = usize::try_from(output_y)
                .unwrap_or_default()
                .saturating_mul(usize::try_from(output_width).unwrap_or_default())
                .saturating_add(usize::try_from(output_x).unwrap_or_default())
                .saturating_mul(4);
            rotated[destination..destination + 4].copy_from_slice(&pixels[source..source + 4]);
        }
    }
    Ok((output_width, output_height, rotated))
}

pub(crate) fn thumbnail(
    path: &Path,
    fps: Rational,
    at: TimeCode,
    max_width: u32,
) -> Result<RgbaImage, MediaError> {
    let mut decoder = VideoDecoder::open_scaled(path, fps, Some(max_width.max(1)))?;
    let mut cache = FrameCache::new(2);
    decoder.decode_window(at, at, &mut cache)?;
    let frame = cache
        .frame_at_or_before(at)
        .ok_or_else(|| MediaError::Backend(format!("no video frame decoded at {at}")))?;
    Ok(RgbaImage {
        width: frame.width,
        height: frame.height,
        pixels: (*frame.rgba).clone(),
    })
}

fn normalized_start(start: i64) -> i64 {
    if start < -1_000_000_000_000 { 0 } else { start }
}

pub(crate) fn backend(error: impl std::fmt::Display) -> MediaError {
    MediaError::Backend(error.to_string())
}
