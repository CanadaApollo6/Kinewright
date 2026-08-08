use std::{path::Path, sync::Arc};

use ffmpeg_next as ffmpeg;
use openreel_core::{
    AssetId, FrameTexture, MediaAsset, MediaError, MediaKind, Rational, RgbaImage, TimeCode,
};

use crate::cache::FrameCache;

const AV_TIME_BASE: i64 = 1_000_000;

pub(crate) fn probe_path(path: &Path, id: AssetId) -> Result<MediaAsset, MediaError> {
    let input = ffmpeg::format::input(path).map_err(backend)?;
    let video = input.streams().best(ffmpeg::media::Type::Video);
    let audio = input.streams().best(ffmpeg::media::Type::Audio);

    let kind = match (video.is_some(), audio.is_some()) {
        (true, true) => MediaKind::AudioVideo,
        (true, false) => MediaKind::Video,
        (false, true) => MediaKind::Audio,
        (false, false) => {
            return Err(MediaError::Backend(
                "the file has no decodable audio or video stream".to_owned(),
            ));
        }
    };

    let (fps, resolution, duration) = if let Some(stream) = video {
        let rate = valid_rate(stream.avg_frame_rate())
            .or_else(|| valid_rate(stream.rate()))
            .ok_or_else(|| MediaError::Backend("video has no valid frame rate".to_owned()))?;
        let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .map_err(backend)?;
        let decoder = context.decoder().video().map_err(backend)?;
        let duration = if stream.frames() > 0 {
            TimeCode(stream.frames())
        } else if stream.duration() > 0 {
            TimeCode(timestamp_to_frames(
                stream.duration(),
                stream.time_base(),
                rate,
            ))
        } else {
            TimeCode(duration_us_to_frames(input.duration(), rate))
        };
        (rate, Some((decoder.width(), decoder.height())), duration)
    } else {
        let rate = Rational::default();
        (rate, None, TimeCode(duration_us_to_frames(input.duration(), rate)))
    };

    if duration <= TimeCode::ZERO {
        return Err(MediaError::Backend(
            "media duration is missing or zero".to_owned(),
        ));
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
    mul_div_round(
        duration,
        i64::from(fps.numerator()),
        AV_TIME_BASE.saturating_mul(i64::from(fps.denominator())),
    )
}

fn timestamp_to_frames(timestamp: i64, time_base: ffmpeg::Rational, fps: Rational) -> i64 {
    let numerator = i64::from(time_base.numerator())
        .saturating_mul(i64::from(fps.numerator()));
    let denominator = i64::from(time_base.denominator())
        .saturating_mul(i64::from(fps.denominator()));
    mul_div_round(timestamp, numerator, denominator)
}

fn mul_div_round(value: i64, multiplier: i64, divisor: i64) -> i64 {
    if divisor <= 0 {
        return 0;
    }
    let value = i128::from(value).saturating_mul(i128::from(multiplier));
    let divisor = i128::from(divisor);
    let rounded = value.saturating_add(divisor / 2) / divisor;
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

pub(crate) struct VideoDecoder {
    input: ffmpeg::format::context::Input,
    decoder: ffmpeg::decoder::Video,
    scaler: ffmpeg::software::scaling::Context,
    stream_index: usize,
    stream_time_base: ffmpeg::Rational,
    stream_start: i64,
    fps: Rational,
    output_width: u32,
    output_height: u32,
    fallback_index: i64,
}

impl VideoDecoder {
    pub(crate) fn open(path: &Path, fps: Rational) -> Result<Self, MediaError> {
        Self::open_scaled(path, fps, None)
    }

    fn open_scaled(path: &Path, fps: Rational, max_width: Option<u32>) -> Result<Self, MediaError> {
        let input = ffmpeg::format::input(path).map_err(backend)?;
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or_else(|| MediaError::Backend("media has no video stream".to_owned()))?;
        let stream_index = stream.index();
        let stream_time_base = stream.time_base();
        let stream_start = normalized_start(stream.start_time());
        let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .map_err(backend)?;
        let decoder = context.decoder().video().map_err(backend)?;
        let source_width = decoder.width();
        let source_height = decoder.height();
        let output_width = max_width.unwrap_or(source_width).min(source_width).max(1);
        let output_height = u32::try_from(
            u64::from(source_height)
                .saturating_mul(u64::from(output_width))
                / u64::from(source_width.max(1)),
        )
        .unwrap_or(source_height)
        .max(1);
        let scaler = ffmpeg::software::scaling::Context::get(
            decoder.format(),
            source_width,
            source_height,
            ffmpeg::format::Pixel::RGBA,
            output_width,
            output_height,
            ffmpeg::software::scaling::Flags::BILINEAR,
        )
        .map_err(backend)?;

        Ok(Self {
            input,
            decoder,
            scaler,
            stream_index,
            stream_time_base,
            stream_start,
            fps,
            output_width,
            output_height,
            fallback_index: 0,
        })
    }

    pub(crate) fn decode_window(
        &mut self,
        start: TimeCode,
        end: TimeCode,
        cache: &mut FrameCache,
    ) -> Result<(), MediaError> {
        let timestamp = frame_to_global_timestamp(start, self.fps);
        self.input.seek(timestamp, ..timestamp).map_err(backend)?;
        self.decoder.flush();
        self.fallback_index = start.0;

        let mut reached_end = false;
        while !reached_end {
            let next = self
                .input
                .packets()
                .next()
                .map(|(stream, packet)| (stream.index(), packet));
            let Some((stream_index, packet)) = next else {
                self.decoder.send_eof().map_err(backend)?;
                self.receive_frames(start, end, cache)?;
                break;
            };
            if stream_index != self.stream_index {
                continue;
            }
            self.decoder.send_packet(&packet).map_err(backend)?;
            reached_end = self.receive_frames(start, end, cache)?;
        }
        Ok(())
    }

    fn receive_frames(
        &mut self,
        start: TimeCode,
        end: TimeCode,
        cache: &mut FrameCache,
    ) -> Result<bool, MediaError> {
        let mut decoded = ffmpeg::frame::Video::empty();
        while self.decoder.receive_frame(&mut decoded).is_ok() {
            let index = decoded.timestamp().map_or_else(
                || {
                    let index = self.fallback_index;
                    self.fallback_index = self.fallback_index.saturating_add(1);
                    index
                },
                |pts| {
                    timestamp_to_frames(
                        pts.saturating_sub(self.stream_start),
                        self.stream_time_base,
                        self.fps,
                    )
                },
            );
            if index < start.0 {
                continue;
            }
            let frame = self.convert(&decoded)?;
            cache.insert(TimeCode(index), frame);
            if index >= end.0 {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn convert(&mut self, decoded: &ffmpeg::frame::Video) -> Result<FrameTexture, MediaError> {
        let mut rgba = ffmpeg::frame::Video::empty();
        self.scaler.run(decoded, &mut rgba).map_err(backend)?;
        let row_bytes = usize::try_from(self.output_width)
            .unwrap_or_default()
            .saturating_mul(4);
        let height = usize::try_from(self.output_height).unwrap_or_default();
        let mut pixels = Vec::with_capacity(row_bytes.saturating_mul(height));
        let plane = rgba.data(0);
        let stride = rgba.stride(0);
        for row in 0..height {
            let start = row.saturating_mul(stride);
            let end = start.saturating_add(row_bytes);
            pixels.extend_from_slice(&plane[start..end]);
        }
        Ok(FrameTexture {
            width: self.output_width,
            height: self.output_height,
            rgba: Arc::new(pixels),
        })
    }
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
    if start < -1_000_000_000_000 {
        0
    } else {
        start
    }
}

pub(crate) fn backend(error: impl std::fmt::Display) -> MediaError {
    MediaError::Backend(error.to_string())
}
