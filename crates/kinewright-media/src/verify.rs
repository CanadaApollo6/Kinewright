//! CC6 §6: post-export delivery verification.
//!
//! Decode the file that was just written, re-render the delivery reference,
//! and compare them with named per-lane budgets. Everything here is a
//! *measurement*: nothing in this module moves, renames, or deletes the encode
//! it reads, and nothing here re-encodes.
//!
//! Three rules from the contract shape the implementation and are restated
//! where they bite:
//!
//! 1. **The crate's own bindings decoder, never the CLI** (§6.1). Production
//!    never shells out; the `ffmpeg` CLI stays test-only.
//! 2. **One seek-based decode pass** (§6.4). The *measurement* opens the output
//!    once, seeks to each sampled frame, and for that frame reads the *native*
//!    planes and converts the *same* frame through the managed scaler. There is
//!    no second decoder and no second traversal inside the comparison.
//!    The §6.2 frame-count cross-check runs on that **same** decoder and is
//!    also `O(GOP)`: [`DeliveryDecoder::presented_frame_count`] seeks once, to
//!    frame `T - 1`, and reads the tail. Verification therefore does not scale
//!    with the export's length. It cannot use the packet count instead —
//!    `ffmpeg-next` 8.0 exposes no `AV_PKT_FLAG_DISCARD` and `unsafe_code` is
//!    forbidden, so a picture an edit list trims is invisible to a demuxer-only
//!    count — but it does not need to decode the whole file to see it either.
//! 3. **Native planes for legality, the managed scaler for RGB** (§6.4). A
//!    value that has been through swscale to RGBA64 has already been clipped
//!    and matrixed and cannot show a plane excursion, so the `Y'CbCr` legal
//!    measurement reads `frame.data(plane)` directly.

use std::path::Path;
use std::sync::Arc;

use ffmpeg_next as ffmpeg;
use kinewright_core::{
    AssetId, ColorBitDepth, ColorDescription, DECODED_RANGE_EXCEPTION_BASIS_POINTS,
    DELIVERY_RGB_EXTREMES_NOTE, DeliveryBudgets, DeliveryChannelDifference, DeliveryColorError,
    DeliveryComparison, DeliveryEncodeDepth, DeliveryTagCheck, DeliveryTagSource,
    DeliveryVerification, DeliveryVerificationError, DeliveryVerificationRequest, Document,
    ExportSettings, FrameRounding, MediaError, PlaneLegalExcursion, QaSeverity, Rational, TimeCode,
    YCBCR_CHROMA_LEGAL_HIGH, YCBCR_LUMA_LEGAL_HIGH, YCBCR_LUMA_OFFSET, YCbCrLegalReport,
    YCbCrLegalSource, bt709_limited_ycbcr, delivery_tag_check, map_frames_with_rounding,
};
use kinewright_core::{ColorQcException, DeliveryColorMismatch};

use crate::{
    color_pipeline::DELIVERY_INTERMEDIATE_WHITE,
    compositor::{DeliveryFrame, GpuContext},
    decode::{
        decoder_format_name, ensure_decoder, frame_to_global_timestamp, managed_filter_graph,
        media_error, normalized_start, probe_path, stream_timestamp_to_global,
    },
    lut_store::LutLibrary,
    render::{DecodeStrategy, FrameRenderer, RenderScale},
};

/// The one denominator both sides of the §6.3 comparison divide by.
///
/// This is deliberately an alias and **not** a second copy of the number: the
/// encode side quantizes on
/// [`DELIVERY_INTERMEDIATE_WHITE`](crate::color_pipeline::DELIVERY_INTERMEDIATE_WHITE)
/// (`65_280 = 255 << 8`), and CC1 §3.1 states the same value for the decode
/// side — limited BT.709 YUV-to-RGB conversion uses `FFmpeg`'s 8-bit fixed-point
/// RGB scale even when the planes are 10 bits, so its nominal legal-white
/// denominator is `P_8`, not `P_N`. If the two ever diverged the comparison
/// would be measuring the scale difference, not the codec.
pub const DELIVERY_REFERENCE_DENOMINATOR: u16 = DELIVERY_INTERMEDIATE_WHITE;

/// The `EBU R 103` tolerance in 8-bit codes, applied symmetrically and scaled
/// by `s = 1 << (bits - 8)` at the lane depth.
///
/// `-5 %` / `+105 %` of the nominal range is `11·s` codes at both ends: 8-bit
/// luma `[5, 246]`, 8-bit chroma `[5, 251]`, 10-bit luma `[20, 984]`, 10-bit
/// chroma `[20, 1004]`.
pub const EBU_R103_TOLERANCE_CODES_8BIT: i64 = 11;

/// One decoded frame's **native** `Y'CbCr` planes, in delivery code units.
///
/// 8-bit codes are widened to `u16` **without shifting** — an 8-bit sample
/// stays `0..=255` and a 10-bit sample stays `0..=1023` — so the legal bounds
/// are `[16·s, 235·s]` and `[16·s, 240·s]` with `s = 1 << (bits - 8)` and
/// nothing is rescaled. `yuv420p10le` is little-endian 16-bit containers with
/// the top six bits zero; the reader asserts `sample <= 1023` and fails
/// [`DeliveryVerificationError::PlaneOutOfContainer`] otherwise, so a
/// byte-order mistake cannot be mistaken for a colossal excursion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativePlaneFrame {
    pub width: u32,
    pub height: u32,
    pub chroma_width: u32,
    pub chroma_height: u32,
    /// 8 or 10.
    pub bit_depth: u8,
    /// `"yuv420p"` or `"yuv420p10le"`.
    pub pixel_format: String,
    pub luma: Vec<u16>,
    pub cb: Vec<u16>,
    pub cr: Vec<u16>,
}

/// One sampled frame, read twice from the *same* decoded picture.
pub(crate) struct DecodedSample {
    /// The **decoded** frame identity of the picture these planes came from,
    /// derived from its own presentation timestamp rather than from the index
    /// that was asked for.
    ///
    /// [`DeliveryDecoder::sample`] returns the first picture at or after the
    /// requested index, so a file whose frame identities do not land where the
    /// export fps says they do answers a request for frame `n` with a
    /// *different* frame. Carrying the decoded identity is what lets the
    /// comparison refuse that rather than publish a measurement of one frame
    /// under another frame's number.
    pub(crate) at: i64,
    /// Native planes, for the §6.4 legality measurement.
    pub(crate) native: NativePlaneFrame,
    /// The same picture through the managed scaler, for the §6.3 RGB
    /// comparison. Interleaved RGBA, 16-bit, row-major, `width * height * 4`
    /// samples.
    pub(crate) rgba64: Vec<u16>,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// `s = 1 << (bits - 8)`, the delivery code scale.
const fn code_scale(bits: u8) -> i64 {
    match bits {
        10 => 4,
        _ => 1,
    }
}

/// `U = 2^bits - 1`, the delivery code ceiling.
const fn code_ceiling(bits: u8) -> i64 {
    match bits {
        10 => 1023,
        _ => 255,
    }
}

/// The delivery lane a materialized [`ColorDescription`] names.
///
/// The export gate has already accepted the description by the time
/// verification runs; this is the reverse mapping, and it refuses typed rather
/// than defaulting to eight bits, because a verification that guessed the lane
/// would compare against the wrong budgets and still report a pass.
fn delivery_depth_for(color: &ColorDescription) -> Result<DeliveryEncodeDepth, MediaError> {
    match color.bit_depth {
        ColorBitDepth::Eight | ColorBitDepth::Integer(8) => Ok(DeliveryEncodeDepth::Eight),
        ColorBitDepth::Ten | ColorBitDepth::Integer(10) => Ok(DeliveryEncodeDepth::Ten),
        ref other => Err(MediaError::DeliveryColor(
            DeliveryColorError::UnsupportedField(DeliveryColorMismatch {
                field: "bit_depth".to_owned(),
                observed: format!("{other:?}"),
                allowed: kinewright_core::DELIVERY_BIT_DEPTH_ALLOWED.to_owned(),
            }),
        )),
    }
}

/// Read one decoded picture's native planes without rescaling a single code.
pub(crate) fn native_planes(frame: &ffmpeg::frame::Video) -> Result<NativePlaneFrame, MediaError> {
    let pixel_format = decoder_format_name(frame.format());
    let bit_depth = match frame.format() {
        ffmpeg::format::Pixel::YUV420P => 8_u8,
        ffmpeg::format::Pixel::YUV420P10LE => 10,
        other => {
            return Err(MediaError::DeliveryColor(
                DeliveryColorError::PixelFormatDepthMismatch {
                    observed: decoder_format_name(other),
                    allowed: "yuv420p (8-bit) or yuv420p10le (10-bit)".to_owned(),
                },
            ));
        }
    };
    let mut planes = Vec::with_capacity(3);
    for index in 0..3 {
        planes.push(read_plane_codes(frame, index, bit_depth)?);
    }
    let cr = planes.pop().unwrap_or_default();
    let cb = planes.pop().unwrap_or_default();
    let luma = planes.pop().unwrap_or_default();
    Ok(NativePlaneFrame {
        width: frame.width(),
        height: frame.height(),
        chroma_width: frame.plane_width(1),
        chroma_height: frame.plane_height(1),
        bit_depth,
        pixel_format,
        luma,
        cb,
        cr,
    })
}

/// One plane's samples, row by row, stride honoured, container asserted.
fn read_plane_codes(
    frame: &ffmpeg::frame::Video,
    index: usize,
    bit_depth: u8,
) -> Result<Vec<u16>, MediaError> {
    let width = frame.plane_width(index) as usize;
    let height = frame.plane_height(index) as usize;
    let stride = frame.stride(index);
    let data = frame.data(index);
    let mut codes = Vec::with_capacity(width.saturating_mul(height));
    for row in 0..height {
        let start = row.saturating_mul(stride);
        if bit_depth == 8 {
            let end = start.saturating_add(width);
            codes.extend(data[start..end].iter().map(|code| u16::from(*code)));
        } else {
            let end = start.saturating_add(width.saturating_mul(2));
            for sample in data[start..end].as_chunks::<2>().0 {
                // Little-endian 16-bit containers with the top six bits zero.
                // A byte-order mistake reads as a value in the thousands, so
                // it is refused here rather than reported as an excursion.
                let code = u16::from_le_bytes([sample[0], sample[1]]);
                if i64::from(code) > code_ceiling(bit_depth) {
                    return Err(MediaError::DeliveryVerification(
                        DeliveryVerificationError::PlaneOutOfContainer {
                            observed: code.to_string(),
                            allowed: "0..=1023",
                        },
                    ));
                }
                codes.push(code);
            }
        }
    }
    Ok(codes)
}

/// Open one written encode for verification, **exactly as a player would**.
///
/// No demuxer options: the container's edit list is honoured, so what
/// verification measures is what a viewer is presented with. This used to open
/// with `ignore_editlist=1`, because the export path muxed video packets with
/// zero duration and the mov muxer then wrote an `elst` one frame shorter than
/// the track -- `FFmpeg`'s demuxer flags the last coded picture
/// `AV_PKT_FLAG_DISCARD` and a `T`-frame export presented `T - 1` frames. That
/// was a real delivery defect, not a reading problem; it is fixed in
/// `export.rs` (see [`VideoPacketDuration`](crate::export)), and reading the
/// file straight is what keeps it fixed: frame `T - 1` is sampled through the
/// normal path, so the defect cannot come back unnoticed.
fn verification_input(path: &Path) -> Result<ffmpeg::format::context::Input, MediaError> {
    ffmpeg::format::input(path).map_err(|error| {
        MediaError::Backend(format!(
            "could not open delivery output {} for verification: {error}",
            path.display()
        ))
    })
}

/// The number of frames a written encode actually **presents**, in `O(GOP)`.
///
/// Opens one [`DeliveryDecoder`] and asks it the same question
/// [`verify_delivery_output`] asks, seeded from the file's own *coded* length
/// so the caller needs nothing but a path. Kept as a free function so a fixture
/// can state a file's presented length beside `probe_path`'s statement of its
/// coded length — the two numbers an MP4 edit list can separate. The production
/// path uses the decoder it has already opened and never opens a second one.
///
/// # Errors
///
/// Returns a media error when the output cannot be probed, opened, seeked, or
/// decoded.
#[cfg(test)]
pub(crate) fn presented_frame_count(path: &Path) -> Result<u64, MediaError> {
    let asset = probe_path(path, AssetId(0))?;
    // The coded count is an upper bound on the presented count, so seeking to
    // its last frame lands at or after the last picture the file presents and
    // the tail scan still sees every one of them.
    let coded = u64::try_from(asset.duration.0).unwrap_or(0);
    DeliveryDecoder::open(path, asset.fps, &asset.color_description)?.presented_frame_count(coded)
}

/// One seek-based decode pass over a written delivery encode.
///
/// The input is opened once. `sample` seeks to the requested frame, decodes
/// forward to it, and returns that picture's native planes *and* the same
/// picture through the managed scaler. Between two sampled frames at most one
/// GOP is decoded.
pub(crate) struct DeliveryDecoder {
    path: std::path::PathBuf,
    input: ffmpeg::format::context::Input,
    decoder: ffmpeg::decoder::Video,
    graph: ffmpeg::filter::Graph,
    stream_index: usize,
    stream_time_base: ffmpeg::Rational,
    stream_start: i64,
    fps: Rational,
    pixel_format: String,
    filter_pts: i64,
}

/// How far [`DeliveryDecoder::scan_from`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// Return the first picture at or after the target and read no further —
    /// the sampling path, which needs one frame per sample.
    AtTarget,
    /// Read every remaining picture — the §6.2 cross-check, which needs the
    /// highest frame identity the file presents and nothing else.
    AtEndOfStream,
}

/// What one [`DeliveryDecoder::scan_from`] found.
struct Scan {
    /// The picture the scan stopped on and its own decoded frame identity,
    /// present only under [`Stop::AtTarget`].
    picture: Option<(i64, ffmpeg::frame::Video)>,
    /// The highest frame identity decoded during the scan, `None` when the
    /// scan decoded nothing.
    highest: Option<i64>,
}

impl DeliveryDecoder {
    /// Open one encode for verification.
    ///
    /// `description` is the **probed** description of the file, so the managed
    /// scaler is configured from what the file says about itself rather than
    /// from what the export intended.
    pub(crate) fn open(
        path: &Path,
        fps: Rational,
        description: &ColorDescription,
    ) -> Result<Self, MediaError> {
        let input = verification_input(path)?;
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or_else(|| {
                MediaError::Backend(format!(
                    "delivery output {} has no video stream",
                    path.display()
                ))
            })?;
        let stream_index = stream.index();
        let stream_time_base = stream.time_base();
        let stream_start = normalized_start(stream.start_time());
        ensure_decoder(&stream, "video", path)?;
        let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .map_err(|error| media_error(path, "could not read video codec parameters", error))?;
        let decoder = context
            .decoder()
            .video()
            .map_err(|error| media_error(path, "could not open the video decoder", error))?;
        // §5.5, normative: the managed decoder's `flags=bicubic` with explicit
        // matrix and range is the measured-best configuration, and
        // `full_chroma_int` must never be added — it interpolates chroma
        // across the 4:2:0 edge instead of replicating it and costs 63 codes
        // on max and 5 dB. This reuses the ingest decoder's own graph rather
        // than building a second one.
        let graph = managed_filter_graph(
            path,
            &decoder,
            stream_time_base,
            decoder.width(),
            decoder.height(),
            description,
        )?;
        let pixel_format = decoder_format_name(decoder.format());
        Ok(Self {
            path: path.to_path_buf(),
            input,
            decoder,
            graph,
            stream_index,
            stream_time_base,
            stream_start,
            fps,
            pixel_format,
            filter_pts: 0,
        })
    }

    /// The negotiated decoder pixel format, e.g. `"yuv420p"`.
    pub(crate) fn pixel_format(&self) -> &str {
        &self.pixel_format
    }

    /// Seek to `index` and read that frame twice: native planes, then managed
    /// scaler.
    pub(crate) fn sample(&mut self, index: u64) -> Result<DecodedSample, MediaError> {
        let (at, frame) = self.decode_frame_at(index)?;
        let native = native_planes(&frame)?;
        // Some containers leave `AVCodecContext::pix_fmt` unresolved until the
        // first decoded frame, so the negotiated format is taken from the
        // picture rather than trusted from the opened decoder.
        self.pixel_format.clone_from(&native.pixel_format);
        let rgba64 = self.scale_to_rgba64(frame)?;
        Ok(DecodedSample {
            at,
            width: native.width,
            height: native.height,
            native,
            rgba64,
        })
    }

    /// The §6.2 cross-check, in `O(GOP)`: how many frames this encode
    /// **presents**.
    ///
    /// `expected_frames` is `T`, and the scan is seeded at `T - 1` — the last
    /// frame the document implies — so the answer is *"frame `T - 1` decodes
    /// and presents, and seeking past it yields no further picture"* rather
    /// than a straight decode of the whole file. One seek, one GOP, plus
    /// whatever tail follows the last frame; verification no longer scales
    /// with the export's length.
    ///
    /// The count is derived from the **highest presented frame identity**
    /// `h`, as `h + 1`:
    ///
    /// * `h == T - 1` — every implied frame is presented, the check passes;
    /// * `h < T - 1` — the file presents `h + 1` frames and `T - 1` is missing
    ///   (the `elst` defect: the last coded picture is flagged
    ///   `AV_PKT_FLAG_DISCARD` and never reaches the decoder's output);
    /// * `h >= T` — the file presents a frame the document does not imply.
    ///
    /// An empty tail (`h` absent) reports `0`. This shares the `T - 1`
    /// assumption every `O(GOP)` check must make — that the frames before the
    /// seek point are contiguous — which is exactly the trade E21 asks for.
    fn presented_frame_count(&mut self, expected_frames: u64) -> Result<u64, MediaError> {
        let seed = expected_frames.saturating_sub(1);
        let highest = self.scan_from(seed, Stop::AtEndOfStream)?.highest;
        Ok(highest.map_or(0, |index| {
            u64::try_from(index.saturating_add(1)).unwrap_or(0)
        }))
    }

    /// Seek to the keyframe at or before `index` and decode forward to it,
    /// returning the picture **and its own decoded frame identity**.
    ///
    /// Packets are pulled lazily, so between two sampled frames at most one
    /// GOP is decoded and the file is never traversed a second time.
    fn decode_frame_at(&mut self, index: u64) -> Result<(i64, ffmpeg::frame::Video), MediaError> {
        let path = self.path.clone();
        self.scan_from(index, Stop::AtTarget)?
            .picture
            .ok_or_else(|| {
                MediaError::DeliveryVerification(DeliveryVerificationError::FrameCountMismatch {
                    observed: format!("frame {index} is not present in {}", path.display()),
                    allowed: format!("a decodable frame at index {index}"),
                })
            })
    }

    /// Seek to the keyframe at or before `index` and decode forward, stopping
    /// as `stop` says.
    ///
    /// The seek is the reason both callers are `O(GOP)`: the demuxer restarts
    /// at the keyframe covering `index`, not at the head of the file.
    fn scan_from(&mut self, index: u64, stop: Stop) -> Result<Scan, MediaError> {
        let target = TimeCode(i64::try_from(index).unwrap_or(i64::MAX));
        let timestamp = frame_to_global_timestamp(target, self.fps).saturating_add(
            stream_timestamp_to_global(self.stream_start, self.stream_time_base),
        );
        if self.input.seek(timestamp, ..timestamp).is_err() {
            // A target past the last coded picture — which is precisely what a
            // cross-check against a document claiming one frame too many asks
            // for — can fail to resolve to a keyframe. Rewinding is correct
            // rather than fatal: the answer is still the file's own presented
            // length, and this branch is unreachable for a well-formed encode
            // whose implied frame count is right.
            self.input
                .seek(0, ..)
                .map_err(|error| media_error(&self.path, "delivery output seek failed", error))?;
        }
        self.decoder.flush();

        // Disjoint field borrows: the packet iterator holds the input while
        // the decoder is fed, which one `&mut self` method call could not do.
        let Self {
            path,
            input,
            decoder,
            stream_index,
            stream_time_base,
            stream_start,
            fps,
            ..
        } = self;
        let mut scan = Scan {
            picture: None,
            highest: None,
        };
        let mut decoded = ffmpeg::frame::Video::empty();
        for (stream, packet) in input.packets() {
            if stream.index() != *stream_index {
                continue;
            }
            decoder
                .send_packet(&packet)
                .map_err(|error| media_error(path, "delivery output decode failed", error))?;
            while decoder.receive_frame(&mut decoded).is_ok() {
                let at = frame_index(&decoded, *stream_time_base, *stream_start, *fps);
                if at == i64::MAX {
                    continue;
                }
                scan.highest = Some(scan.highest.map_or(at, |highest: i64| highest.max(at)));
                if stop == Stop::AtTarget && at >= target.0 {
                    scan.picture = Some((at, decoded));
                    return Ok(scan);
                }
            }
        }
        decoder
            .send_eof()
            .map_err(|error| media_error(path, "delivery output decode failed", error))?;
        while decoder.receive_frame(&mut decoded).is_ok() {
            let at = frame_index(&decoded, *stream_time_base, *stream_start, *fps);
            if at == i64::MAX {
                continue;
            }
            scan.highest = Some(scan.highest.map_or(at, |highest: i64| highest.max(at)));
            if stop == Stop::AtTarget && at >= target.0 {
                scan.picture = Some((at, decoded));
                return Ok(scan);
            }
        }
        Ok(scan)
    }

    /// Push one decoded picture through the managed scaler and read RGBA64.
    fn scale_to_rgba64(&mut self, mut frame: ffmpeg::frame::Video) -> Result<Vec<u16>, MediaError> {
        // Seeking makes the decoded presentation timestamps go backwards,
        // which a `buffer` source rejects. The filter graph is a colour
        // conversion with no temporal state, so the submission order is
        // restamped monotonically and the frame identity stays with the
        // caller.
        self.filter_pts = self.filter_pts.saturating_add(1);
        frame.set_pts(Some(self.filter_pts));
        {
            let mut source_context = self.graph.get("source").ok_or_else(|| {
                MediaError::Backend("managed verification source filter disappeared".to_owned())
            })?;
            source_context.source().add(&frame).map_err(|error| {
                MediaError::Backend(format!(
                    "managed verification source submission failed: {error}"
                ))
            })?;
        }
        let mut converted = ffmpeg::frame::Video::empty();
        {
            let mut sink_context = self.graph.get("sink").ok_or_else(|| {
                MediaError::Backend("managed verification sink filter disappeared".to_owned())
            })?;
            sink_context.sink().frame(&mut converted).map_err(|error| {
                MediaError::Backend(format!(
                    "managed BT.709 limited-range verification decode failed: {error}"
                ))
            })?;
        }
        let width = converted.width() as usize;
        let height = converted.height() as usize;
        let stride = converted.stride(0);
        let data = converted.data(0);
        let mut samples = Vec::with_capacity(width.saturating_mul(height).saturating_mul(4));
        for row in 0..height {
            let start = row.saturating_mul(stride);
            let end = start.saturating_add(width.saturating_mul(8));
            for sample in data[start..end].as_chunks::<2>().0 {
                samples.push(u16::from_le_bytes([sample[0], sample[1]]));
            }
        }
        Ok(samples)
    }
}

/// The output frame identity of one decoded picture.
fn frame_index(
    frame: &ffmpeg::frame::Video,
    stream_time_base: ffmpeg::Rational,
    stream_start: i64,
    fps: Rational,
) -> i64 {
    let Some(pts) = frame.pts().or_else(|| frame.timestamp()) else {
        return i64::MAX;
    };
    let pts = pts.saturating_sub(stream_start);
    let numerator = i128::from(pts)
        .saturating_mul(i128::from(stream_time_base.numerator()))
        .saturating_mul(i128::from(fps.numerator()));
    let denominator =
        i128::from(stream_time_base.denominator()).saturating_mul(i128::from(fps.denominator()));
    if denominator <= 0 {
        return i64::MAX;
    }
    // Round to nearest so a one-tick muxer rounding cannot shift a frame
    // identity by a whole frame.
    i64::try_from((numerator.saturating_mul(2) + denominator) / (denominator * 2))
        .unwrap_or(i64::MAX)
}

/// A bounded histogram of absolute code differences.
///
/// The population is millions of samples and the alphabet is at most 1 024
/// codes, so the accumulator is a histogram rather than a sorted vector: the
/// percentile and mean are exact, the memory is constant, and §10.3's
/// row-major ordering cannot change the result because a histogram has no
/// summation order at all.
#[derive(Debug, Clone)]
struct DifferenceHistogram {
    counts: Vec<u64>,
    total: u64,
    sum: u128,
    sum_squares: u128,
}

impl DifferenceHistogram {
    fn new(bits: u8) -> Self {
        let span = usize::try_from(code_ceiling(bits))
            .unwrap_or(1023)
            .saturating_add(1);
        Self {
            counts: vec![0; span],
            total: 0,
            sum: 0,
            sum_squares: 0,
        }
    }

    fn push(&mut self, difference: i64) {
        let bounded = usize::try_from(difference.unsigned_abs()).unwrap_or(usize::MAX);
        let index = bounded.min(self.counts.len().saturating_sub(1));
        self.counts[index] = self.counts[index].saturating_add(1);
        self.total = self.total.saturating_add(1);
        let value = u128::from(difference.unsigned_abs());
        self.sum = self.sum.saturating_add(value);
        self.sum_squares = self.sum_squares.saturating_add(value.saturating_mul(value));
    }

    fn maximum(&self) -> u32 {
        for (code, count) in self.counts.iter().enumerate().rev() {
            if *count > 0 {
                return u32::try_from(code).unwrap_or(u32::MAX);
            }
        }
        0
    }

    /// §10.4's nearest-rank convention: element `min(n - 1, ceil(0.99·n) - 1)`.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]
    fn p99(&self) -> u32 {
        if self.total == 0 {
            return 0;
        }
        let rank = ((self.total as f64) * 0.99).ceil() as u64;
        let rank = rank.max(1).min(self.total).saturating_sub(1);
        let mut seen = 0_u64;
        for (code, count) in self.counts.iter().enumerate() {
            seen = seen.saturating_add(*count);
            if seen > rank {
                return u32::try_from(code).unwrap_or(u32::MAX);
            }
        }
        self.maximum()
    }

    #[allow(clippy::cast_precision_loss)]
    fn mean(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.sum as f64 / self.total as f64
    }

    /// One channel's reported statistics, with the **mean** divided by
    /// `mean_scale`.
    ///
    /// §6.3 words the two RGB numbers in different units and this is where the
    /// difference is applied, once:
    ///
    /// * `maximum_code_diff` and `p99_code_diff_millionths` are always in
    ///   **lane code units** (`U = 2^bits - 1`) — they are *reported, never
    ///   gated*, and reporting them in the lane's own alphabet is what makes
    ///   them evidence about that lane's decode.
    /// * `mean_code_diff_millionths` for the RGB channels is
    ///   **8-bit-equivalent** (`d8 = d / s`, `s = 2^(bits - 8)`), because that
    ///   is the unit §6.3 gates `combined` in and the unit §11.2.11 compares
    ///   the two lanes in. PSNR is already defined on the 8-bit-equivalent MSE
    ///   for exactly the same reason.
    /// * The **luma** plane passes `mean_scale = 1`: §6.3(a) gates all three of
    ///   its terms in luma code units at the lane depth, so nothing about it is
    ///   rescaled.
    ///
    /// `mean(d8) == mean(d) / s` exactly, so scaling the mean is the same
    /// measurement as accumulating `d8` and costs no precision.
    fn difference(&self, mean_scale: i64) -> DeliveryChannelDifference {
        DeliveryChannelDifference {
            maximum_code_diff: self.maximum(),
            p99_code_diff_millionths: millionths(f64::from(self.p99())),
            #[allow(clippy::cast_precision_loss)]
            mean_code_diff_millionths: millionths(self.mean() / mean_scale.max(1) as f64),
        }
    }
}

/// §10.1: every float is reported in millionths, half away from zero.
#[allow(clippy::cast_possible_truncation)]
fn millionths(value: f64) -> i64 {
    (value * 1_000_000.0).round() as i64
}

/// Running legality accumulator for one native plane.
#[derive(Debug, Clone, Copy)]
struct PlaneLegalAccumulator {
    below: u64,
    above: u64,
    total: u64,
    minimum: i64,
    maximum: i64,
    low: i64,
    high: i64,
}

impl PlaneLegalAccumulator {
    const fn new(low: i64, high: i64) -> Self {
        Self {
            below: 0,
            above: 0,
            total: 0,
            minimum: i64::MAX,
            maximum: i64::MIN,
            low,
            high,
        }
    }

    fn push(&mut self, code: i64) {
        self.total = self.total.saturating_add(1);
        // Strict comparisons: a sample sitting exactly on a bound — 75 % blue
        // is `Cb = 240.0` exactly — is legal, not an excursion.
        if code < self.low {
            self.below = self.below.saturating_add(1);
        } else if code > self.high {
            self.above = self.above.saturating_add(1);
        }
        self.minimum = self.minimum.min(code);
        self.maximum = self.maximum.max(code);
    }

    /// This plane's excursion report.
    ///
    /// A plane that saw **no** sample reports core's empty interval
    /// (`minimum > maximum`, [`PlaneLegalExcursion::samples_seen`] `== false`)
    /// rather than `0..=0`. Zero is a code a real sample can land on, so a
    /// fabricated `0..=0` beside `below_count == 0` is indistinguishable from
    /// a plane whose every sample really was black — a legible-looking pair
    /// nothing measured. The unseen pair is not representable by any sample
    /// set, so the caller can tell the two apart.
    fn excursion(&self) -> PlaneLegalExcursion {
        let (minimum_code_hundredths, maximum_code_hundredths) = if self.total == 0 {
            (
                PlaneLegalExcursion::UNSEEN_MINIMUM_CODE_HUNDREDTHS,
                PlaneLegalExcursion::UNSEEN_MAXIMUM_CODE_HUNDREDTHS,
            )
        } else {
            (
                self.minimum.saturating_mul(100),
                self.maximum.saturating_mul(100),
            )
        };
        PlaneLegalExcursion {
            below_count: self.below,
            above_count: self.above,
            below_basis_points: basis_points(self.below, self.total),
            above_basis_points: basis_points(self.above, self.total),
            minimum_code_hundredths,
            maximum_code_hundredths,
        }
    }

    /// The §6.4 (b) rule: any sample outside the `EBU R 103` box.
    const fn outside_r103(&self, tolerance: i64) -> bool {
        if self.total == 0 {
            return false;
        }
        self.minimum < self.low.saturating_sub(tolerance)
            || self.maximum > self.high.saturating_add(tolerance)
    }
}

/// §10.1: integer-floor basis points, `0` for an empty population.
fn basis_points(value: u64, count: u64) -> u32 {
    if count == 0 {
        return 0;
    }
    u32::try_from(value.saturating_mul(10_000) / count).unwrap_or(u32::MAX)
}

/// Decode a finished delivery encode and compare it against a freshly
/// rendered delivery reference (CC6 §6).
///
/// # Errors
///
/// Returns a media error when the request is not a well-formed question about
/// this lane (an out-of-range `frame_count`, or budgets belonging to the other
/// delivery lane), when the output cannot be decoded or probed, when a sampled
/// reference render is not full-resolution, when a native sample does not fit
/// its declared container, or when the decoded frame count — or a decoded
/// frame's own identity — disagrees with what the document implies.
pub(crate) fn verify_delivery_output(
    gpu: &GpuContext,
    library: Arc<LutLibrary>,
    document: &Document,
    path: &Path,
    settings: &ExportSettings,
    request: &DeliveryVerificationRequest,
) -> Result<DeliveryVerification, MediaError> {
    // 0. The request itself, before anything is opened or measured.
    let depth = requested_lane(settings, request)?;
    let bits = depth.bits();

    // 1. Probe the written file. The probe is the only thing that reads the
    //    tags, and it also states the file's own frame count, so no extra
    //    traversal is needed to cross-check `T`.
    let probed_asset = probe_path(path, AssetId(0))?;
    let probed = probed_asset.color_description.clone();

    // 2. `T` is the frame count the document implies at the export fps — the
    //    same mapping the exporter itself used.
    let expected_frames = implied_frame_count(document, settings)?;

    // 3. One decoder, opened once, for both the cross-check and the
    //    comparison.
    let mut decoder = DeliveryDecoder::open(path, settings.fps, &probed)?;
    let expected_pixel_format = depth.pixel_format();

    // 4. Cross-check `T` against the number of frames the file actually
    //    **presents** (E21: `O(GOP)`, not a straight decode — verification
    //    must not scale with the export's length). Not `probe_path`'s
    //    duration either: that is the coded packet count, and a container
    //    whose edit list trims a coded picture would pass it while a viewer
    //    was shown one frame fewer. Verification never silently samples a
    //    shorter file, and never accepts a file that codes a frame it does not
    //    present.
    let presented_frames = decoder.presented_frame_count(expected_frames)?;
    if presented_frames != expected_frames {
        return Err(MediaError::DeliveryVerification(
            DeliveryVerificationError::FrameCountMismatch {
                observed: presented_frames.to_string(),
                allowed: expected_frames.to_string(),
            },
        ));
    }

    // 5. The §6.2 closed-form integer sample. No clock, no adaptive stride.
    let samples = sampled_frames(request, expected_frames)?;

    // The reference renderer is isolated from the playback worker for
    // `monitor_proof_for_document`'s reasons: a verification must not evict or
    // reuse a proxy cache, and it must not touch transport state.
    let mut renderer = FrameRenderer::new(gpu.clone());
    renderer.set_lut_library(library);

    let measured = measure_samples(
        gpu,
        &mut renderer,
        &mut decoder,
        document,
        settings,
        bits,
        &samples,
    )?;

    // The negotiated pixel format is read from the decoded pictures, not from
    // the opened decoder, because some containers leave it unresolved until
    // the first frame. A lane whose file does not carry the declared depth is
    // refused typed rather than compared against the wrong budgets.
    let decoded_pixel_format = decoder.pixel_format().to_owned();
    if decoded_pixel_format != expected_pixel_format {
        return Err(MediaError::DeliveryColor(
            DeliveryColorError::PixelFormatDepthMismatch {
                observed: decoded_pixel_format,
                allowed: expected_pixel_format.to_owned(),
            },
        ));
    }

    let scale = code_scale(bits);
    let budgets = request.budgets;
    // §6.3, normative: the luma plane is gated in **lane** code units, while
    // the RGB mean is gated 8-bit-equivalent. The RGB maxima and P99s stay in
    // lane code units and are reported, never gated.
    let luma = measured.luma.difference(1);
    let [red, green, blue] = [0, 1, 2].map(|index| measured.channels[index].difference(scale));
    let combined = measured.combined.difference(scale);
    let psnr_db_hundredths = psnr_hundredths(&measured.combined, scale);
    let within_budgets = within_budgets(&luma, &combined, psnr_db_hundredths, &budgets);

    let comparison = DeliveryComparison {
        frames: measured.frames.clone(),
        luma,
        red,
        green,
        blue,
        combined,
        psnr_db_hundredths,
        decoded_ycbcr: YCbCrLegalReport {
            bit_depth: bits,
            luma: measured.luma_legal.excursion(),
            cb: measured.blue_difference_legal.excursion(),
            cr: measured.red_difference_legal.excursion(),
            source: YCbCrLegalSource::DecodedNativePlanes,
        },
        rgb_extremes_note: DELIVERY_RGB_EXTREMES_NOTE.to_owned(),
        budgets,
        within_budgets,
    };

    let tags = delivery_tag_check(
        &request.expected_delivery,
        &probed,
        DeliveryTagSource::ProbedOutputFile,
    );

    let tolerance = EBU_R103_TOLERANCE_CODES_8BIT.saturating_mul(scale);
    let mut exceptions = budget_exceptions(&comparison);
    exceptions.extend(range_exceptions(
        &[
            ("luma", measured.luma_legal),
            ("cb", measured.blue_difference_legal),
            ("cr", measured.red_difference_legal),
        ],
        tolerance,
    ));
    exceptions.extend(tag_exceptions(&tags));
    sort_exceptions(&mut exceptions);
    let technical_pass = !exceptions
        .iter()
        .any(|exception| exception.severity == QaSeverity::Error);

    Ok(DeliveryVerification {
        output_path: path.to_path_buf(),
        delivery_bit_depth: depth,
        probed,
        tags,
        decoded_pixel_format,
        comparison,
        exceptions,
        technical_pass,
    })
}

/// The delivery lane one request is a well-formed question about.
///
/// Both refusals here are about the *request*, not about the file, so they are
/// made before anything is opened and a malformed request never costs a decode.
///
/// # Errors
///
/// Returns [`DeliveryVerificationError::FrameCountOutOfRange`] for a
/// `frame_count` outside `1..=16` — the refusal core documents every caller
/// must make, because `sample_frames` clamps so that it can stay total and a
/// clamped sample is a *different* measurement reported under the number that
/// was asked for — and
/// [`DeliveryVerificationError::BudgetLaneMismatch`] when the request carries
/// the other lane's budgets, which would still produce a `within_budgets`
/// verdict and publish it as a pass against a gate nobody chose (§6.3: a
/// caller may not invent a looser set).
fn requested_lane(
    settings: &ExportSettings,
    request: &DeliveryVerificationRequest,
) -> Result<DeliveryEncodeDepth, MediaError> {
    request.validate().map_err(MediaError::from)?;
    let depth = delivery_depth_for(&settings.delivery_color)?;
    let lane_budgets = DeliveryBudgets::for_depth(depth);
    if request.budgets != lane_budgets {
        return Err(MediaError::from(
            DeliveryVerificationError::BudgetLaneMismatch {
                observed: format!("{:?} for the {} lane", request.budgets, depth.as_str()),
                allowed: format!("{lane_budgets:?}"),
            },
        ));
    }
    Ok(depth)
}

/// §6.2's sample set, refused typed when it is empty.
///
/// [`DeliveryVerificationRequest::sample_frames`] is total: it answers `[]` for
/// a document that implies no frames at all. Measuring that would produce a
/// report whose every accumulator saw nothing — an empty `frames` list, zero
/// differences, and three planes that never had a sample — and `within_budgets`
/// would be `true` because nothing exceeded anything. That is a pass nobody
/// measured, so the empty sample set is a refusal instead.
///
/// `FrameCountMismatch` rather than a new variant: the field is `frame_count`,
/// the recovery action ("verify the export wrote every frame the document
/// implies") is the right one, and a `delivery_verification_no_samples` code
/// would split one question — *does this file have the frames the document
/// claims?* — across two codes that agent and app surfaces would have to learn
/// separately.
fn sampled_frames(
    request: &DeliveryVerificationRequest,
    expected_frames: u64,
) -> Result<Vec<u64>, MediaError> {
    let samples = request.sample_frames(expected_frames);
    if samples.is_empty() {
        return Err(MediaError::from(
            DeliveryVerificationError::FrameCountMismatch {
                observed: "0 sampled frames".to_owned(),
                allowed: "at least one sampled frame".to_owned(),
            },
        ));
    }
    Ok(samples)
}

/// `T`: the frame count the document implies at the export fps.
///
/// This is the exporter's own mapping, not a second one, so a mismatch against
/// the written file is evidence about the file rather than about two different
/// opinions of how long the document is.
fn implied_frame_count(document: &Document, settings: &ExportSettings) -> Result<u64, MediaError> {
    let frames = map_frames_with_rounding(
        document.duration,
        document.fps,
        settings.fps,
        FrameRounding::Ceil,
    )
    .map_err(|error| MediaError::Backend(error.to_string()))?;
    u64::try_from(frames.0)
        .map_err(|_| MediaError::Backend("export frame count is invalid".to_owned()))
}

/// Every accumulator one verification fills, over every sampled frame.
struct SampleMeasurements {
    /// Project frame identities, in sample order.
    frames: Vec<i64>,
    luma: DifferenceHistogram,
    channels: [DifferenceHistogram; 3],
    combined: DifferenceHistogram,
    luma_legal: PlaneLegalAccumulator,
    /// `Cb`, the blue-difference chroma plane.
    blue_difference_legal: PlaneLegalAccumulator,
    /// `Cr`, the red-difference chroma plane.
    red_difference_legal: PlaneLegalAccumulator,
}

/// Decode, re-render, and compare every sampled frame in one pass.
fn measure_samples(
    gpu: &GpuContext,
    renderer: &mut FrameRenderer,
    decoder: &mut DeliveryDecoder,
    document: &Document,
    settings: &ExportSettings,
    bits: u8,
    samples: &[u64],
) -> Result<SampleMeasurements, MediaError> {
    let scale = code_scale(bits);
    let ceiling = code_ceiling(bits);
    let luma_low = i64::from(YCBCR_LUMA_OFFSET) * scale;
    let luma_high = i64::from(YCBCR_LUMA_LEGAL_HIGH) * scale;
    // Both planes share the same floor, `16·s`; only the ceiling differs.
    let chroma_low = luma_low;
    let chroma_high = i64::from(YCBCR_CHROMA_LEGAL_HIGH) * scale;
    // §5.7: the delivery reference scale is bound **once**, here, so the
    // render and the claim it produces cannot drift apart. `delivery_reference`
    // takes it as an argument rather than binding it internally so the refusal
    // has a reachable failing case (rule 11.0.5): a caller that hands it a
    // proxy scale is refused typed instead of silently measuring a proxy.
    let reference_scale = RenderScale::FullResolution;
    let mut measured = SampleMeasurements {
        frames: Vec::with_capacity(samples.len()),
        luma: DifferenceHistogram::new(bits),
        channels: [
            DifferenceHistogram::new(bits),
            DifferenceHistogram::new(bits),
            DifferenceHistogram::new(bits),
        ],
        combined: DifferenceHistogram::new(bits),
        luma_legal: PlaneLegalAccumulator::new(luma_low, luma_high),
        blue_difference_legal: PlaneLegalAccumulator::new(chroma_low, chroma_high),
        red_difference_legal: PlaneLegalAccumulator::new(chroma_low, chroma_high),
    };

    for output_frame in samples.iter().copied() {
        let sample = decoder.sample(output_frame)?;
        // §6.2 asks for frame `n`; the decoder answers with the first picture
        // at or after `n`. When the file's own frame identities do not land
        // where the export fps says they do, those two are different frames,
        // and comparing the picture that came back against the reference for
        // the frame that was asked for would publish a measurement of one
        // frame under another frame's number. Refuse typed instead.
        let requested = i64::try_from(output_frame).unwrap_or(i64::MAX);
        if sample.at != requested {
            return Err(MediaError::from(
                DeliveryVerificationError::FrameCountMismatch {
                    observed: format!(
                        "decoded frame {} for requested frame {output_frame}",
                        sample.at
                    ),
                    allowed: format!("decoded frame {output_frame}"),
                },
            ));
        }
        // The **decoded** identity, not the requested one: the two are equal
        // by the refusal above, and recording the one the picture carried is
        // what makes `DeliveryComparison.frames` a statement about the file.
        let output_at = TimeCode(sample.at);
        // The exact frame mapping the exporter used, so the reference is the
        // same project frame the encode carried.
        let project_at =
            map_frames_with_rounding(output_at, settings.fps, document.fps, FrameRounding::Floor)
                .map_err(|error| MediaError::Backend(error.to_string()))?;
        let project_at = TimeCode(project_at.0.min(document.duration.0.saturating_sub(1)));
        measured.frames.push(project_at.0);

        let reference = delivery_reference(
            gpu,
            renderer,
            document,
            settings,
            project_at,
            reference_scale,
        )?;
        if (reference.width, reference.height) != (sample.width, sample.height) {
            return Err(MediaError::Backend(format!(
                "delivery verification raster mismatch: decoded {}x{}, reference {}x{}",
                sample.width, sample.height, reference.width, reference.height
            )));
        }

        accumulate_frame(&reference, &sample, bits, ceiling, &mut measured);
        for code in &sample.native.luma {
            measured.luma_legal.push(i64::from(*code));
        }
        for code in &sample.native.cb {
            measured.blue_difference_legal.push(i64::from(*code));
        }
        for code in &sample.native.cr {
            measured.red_difference_legal.push(i64::from(*code));
        }
    }
    Ok(measured)
}

/// Render one full-resolution delivery reference, claim derived not asserted.
///
/// §5.7: no preview, proxy, thumbnail, or cached raster may be labelled a
/// delivery reference. The `full_resolution` claim comes from the *existing*
/// derivation — the scale that was requested AND the raster that came back —
/// so this does not invent a second claim.
fn delivery_reference(
    gpu: &GpuContext,
    renderer: &mut FrameRenderer,
    document: &Document,
    settings: &ExportSettings,
    project_at: TimeCode,
    scale: RenderScale,
) -> Result<DeliveryFrame, MediaError> {
    let reference = renderer.render_delivery(
        document,
        project_at,
        settings.resolution,
        scale,
        DecodeStrategy::Seek,
    )?;
    let claim = gpu.monitor_proof_metadata_for(
        scale,
        (reference.width, reference.height),
        settings.resolution,
    );
    if claim.full_resolution {
        return Ok(reference);
    }
    Err(MediaError::DeliveryVerification(
        DeliveryVerificationError::NotFullResolution {
            observed: format!(
                "{}x{} at {scale:?} for a {}x{} delivery raster",
                reference.width, reference.height, settings.resolution.0, settings.resolution.1
            ),
            allowed: "a full-resolution delivery render at the export raster",
        },
    ))
}

/// Compare one sampled frame, reference against decode, in delivery code units.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn accumulate_frame(
    reference: &DeliveryFrame,
    sample: &DecodedSample,
    bits: u8,
    ceiling: i64,
    measured: &mut SampleMeasurements,
) {
    let denominator = f64::from(DELIVERY_REFERENCE_DENOMINATOR);
    let unit = ceiling as f64;
    let width = reference.width as usize;
    let height = reference.height as usize;
    for row in 0..height {
        for column in 0..width {
            let pixel = row.saturating_mul(width).saturating_add(column);
            let reference_base = pixel.saturating_mul(8);
            let mut encoded = [0.0_f64; 3];
            for (channel, value) in encoded.iter_mut().enumerate() {
                let offset = reference_base.saturating_add(channel.saturating_mul(2));
                let code = u16::from_le_bytes([
                    reference.rgba64le[offset],
                    reference.rgba64le[offset.saturating_add(1)],
                ]);
                *value = f64::from(code) / denominator;
            }
            // §6.3, normative: both sides go through the *same* denominator.
            // `ref_code = round(U · v16 / 65280)` and
            // `dec_code = round(U · C_rgba64 / 65280)`.
            for (channel, encoded) in encoded.iter().enumerate() {
                let reference_code = (encoded * unit).round() as i64;
                let decoded_offset = pixel.saturating_mul(4).saturating_add(channel);
                let decoded_code = ((f64::from(sample.rgba64[decoded_offset]) / denominator) * unit)
                    .round() as i64;
                let difference = reference_code - decoded_code;
                measured.channels[channel].push(difference);
                measured.combined.push(difference);
            }
            // §6.3(a): the reference Y' plane through the §3.4 matrix at the
            // lane depth, against the decoded *native* Y plane. This term
            // carries no chroma decimation error at all, which is why it — and
            // not the RGB maximum — is the gate.
            let reference_luma = bt709_limited_ycbcr(
                [
                    encoded[0].clamp(0.0, 1.0),
                    encoded[1].clamp(0.0, 1.0),
                    encoded[2].clamp(0.0, 1.0),
                ],
                bits,
            )[0]
            .round() as i64;
            let decoded_luma = i64::from(sample.native.luma[pixel]);
            measured.luma.push(reference_luma - decoded_luma);
        }
    }
}

/// §6.3's PSNR, on the 8-bit-equivalent MSE so lanes are comparable.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn psnr_hundredths(combined: &DifferenceHistogram, scale: i64) -> Option<i32> {
    if combined.total == 0 {
        return None;
    }
    let scale = scale.max(1) as f64;
    let mean_squares = combined.sum_squares as f64 / combined.total as f64;
    let mse8 = mean_squares / (scale * scale);
    if mse8 <= 0.0 {
        // `Option` for the degenerate case, following `AudioLoudness`'s
        // precedent, not a sentinel.
        return None;
    }
    let psnr = 10.0 * (255.0_f64 * 255.0 / mse8).log10();
    Some((psnr * 100.0).round() as i32)
}

/// The gated set of §6.3: the luma plane, the RGB mean, and PSNR.
fn within_budgets(
    luma: &DeliveryChannelDifference,
    combined: &DeliveryChannelDifference,
    psnr_db_hundredths: Option<i32>,
    budgets: &DeliveryBudgets,
) -> bool {
    let psnr_ok = psnr_db_hundredths.is_none_or(|psnr| psnr >= budgets.psnr_floor_db_hundredths);
    luma.maximum_code_diff <= budgets.luma_max_code
        && luma.p99_code_diff_millionths <= budgets.luma_p99_code_millionths
        && luma.mean_code_diff_millionths <= budgets.luma_mean_code_millionths
        && combined.mean_code_diff_millionths <= budgets.rgb_mean_code_millionths
        && psnr_ok
}

/// The §6.3 gated numbers, as `decoded_difference_over_budget` exceptions.
fn budget_exceptions(comparison: &DeliveryComparison) -> Vec<ColorQcException> {
    let mut exceptions = Vec::new();
    let budgets = &comparison.budgets;
    for (field, observed, allowed) in [
        (
            "luma.maximum_code_diff",
            i64::from(comparison.luma.maximum_code_diff),
            i64::from(budgets.luma_max_code),
        ),
        (
            "luma.p99_code_diff_millionths",
            comparison.luma.p99_code_diff_millionths,
            budgets.luma_p99_code_millionths,
        ),
        (
            "luma.mean_code_diff_millionths",
            comparison.luma.mean_code_diff_millionths,
            budgets.luma_mean_code_millionths,
        ),
        (
            "combined.mean_code_diff_millionths",
            comparison.combined.mean_code_diff_millionths,
            budgets.rgb_mean_code_millionths,
        ),
    ] {
        if observed > allowed {
            exceptions.push(ColorQcException {
                code: "decoded_difference_over_budget".to_owned(),
                severity: QaSeverity::Error,
                message: format!(
                    "The decoded delivery output differs from the full-resolution reference by {observed} on {field}, over the lane budget of {allowed}. This is a codec measurement, not a grade judgement, and it never moves the file it measured."
                ),
                field: Some(field.to_owned()),
                observed: Some(observed.to_string()),
                allowed: Some(format!("<= {allowed}")),
                clip: None,
                effect: None,
            });
        }
    }
    if let Some(psnr) = comparison.psnr_db_hundredths
        && psnr < budgets.psnr_floor_db_hundredths
    {
        exceptions.push(ColorQcException {
            code: "decoded_difference_over_budget".to_owned(),
            severity: QaSeverity::Error,
            message: format!(
                "The decoded delivery output measures {psnr} hundredths of a dB PSNR against the full-resolution reference, below the lane floor of {}.",
                budgets.psnr_floor_db_hundredths
            ),
            field: Some("psnr_db_hundredths".to_owned()),
            observed: Some(psnr.to_string()),
            allowed: Some(format!(">= {}", budgets.psnr_floor_db_hundredths)),
            clip: None,
            effect: None,
        });
    }
    exceptions
}

/// The §6.4 `decoded_range_excursion` rule (EBU R 103).
///
/// Strict-box excursion counts and extremes are **always reported**; the
/// Warning is raised only when the excursion rate exceeds
/// [`DECODED_RANGE_EXCEPTION_BASIS_POINTS`] or a sample lies outside the
/// `EBU R 103` box. A hard zero-excursion gate is refused because it is not
/// achievable: legitimate content sitting exactly on a bound crosses it by one
/// code under any ringing at all.
fn range_exceptions(
    planes: &[(&str, PlaneLegalAccumulator)],
    tolerance: i64,
) -> Vec<ColorQcException> {
    let mut exceptions = Vec::new();
    for (name, plane) in planes {
        // §6.4 (a)'s rate is core's own accessor over the *combined* count, so
        // the gate and the fixture prediction cannot drift apart and neither
        // can silently become `max(below, above)`.
        let rate = plane.excursion().excursion_basis_points(plane.total);
        let outside = plane.outside_r103(tolerance);
        if rate <= DECODED_RANGE_EXCEPTION_BASIS_POINTS && !outside {
            continue;
        }
        let reason = if outside {
            "outside the EBU R 103 box"
        } else {
            "outside the strict legal box"
        };
        exceptions.push(ColorQcException {
            code: "decoded_range_excursion".to_owned(),
            severity: QaSeverity::Warning,
            message: format!(
                "{} of {} decoded {name} samples ({rate} basis points) are {reason}. Every remaining excursion after a legal encode is a codec artefact, which is why this is reported rather than gated at zero.",
                plane.below.saturating_add(plane.above),
                plane.total
            ),
            field: Some(format!("decoded_ycbcr.{name}")),
            observed: Some(format!(
                "{rate} basis points, codes {}..={}",
                plane.minimum, plane.maximum
            )),
            allowed: Some(format!(
                "<= {DECODED_RANGE_EXCEPTION_BASIS_POINTS} basis points and codes {}..={}",
                plane.low.saturating_sub(tolerance),
                plane.high.saturating_add(tolerance)
            )),
            clip: None,
            effect: None,
        });
    }
    exceptions
}

/// The §3.6 post-export tag verdict, as exceptions.
fn tag_exceptions(tags: &DeliveryTagCheck) -> Vec<ColorQcException> {
    let mut exceptions = Vec::new();
    for mismatch in &tags.mismatches {
        exceptions.push(ColorQcException {
            code: "delivery_tag_mismatch".to_owned(),
            severity: QaSeverity::Error,
            message: format!(
                "Delivery tag {} is {}, expected {}. A mis-tagged file is never a creative choice: it will be misinterpreted by every downstream tool.",
                mismatch.field, mismatch.observed, mismatch.allowed
            ),
            field: Some(mismatch.field.clone()),
            observed: Some(mismatch.observed.clone()),
            allowed: Some(mismatch.allowed.clone()),
            clip: None,
            effect: None,
        });
    }
    for entry in &tags.not_representable {
        exceptions.push(ColorQcException {
            code: "delivery_tag_not_representable".to_owned(),
            severity: QaSeverity::Info,
            message: format!(
                "Delivery tag {} cannot be carried by this container, so it is reported rather than compared: {}",
                entry.field, entry.reason
            ),
            field: Some(entry.field.clone()),
            observed: Some("not_representable".to_owned()),
            allowed: Some(entry.expected.clone()),
            clip: None,
            effect: None,
        });
    }
    exceptions
}

/// §10.6: `(severity desc, code asc, tiebreak asc)`.
fn sort_exceptions(exceptions: &mut [ColorQcException]) {
    exceptions.sort_by(|left, right| {
        severity_rank(left.severity)
            .cmp(&severity_rank(right.severity))
            .then_with(|| left.code.cmp(&right.code))
            .then_with(|| left.field.cmp(&right.field))
            .then_with(|| left.observed.cmp(&right.observed))
    });
}

/// `Error` first, then `Warning`, then `Info`: severity descending.
const fn severity_rank(severity: QaSeverity) -> u8 {
    match severity {
        QaSeverity::Error => 0,
        QaSeverity::Warning => 1,
        QaSeverity::Info => 2,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::process::{Command as ProcessCommand, Stdio};
    use std::sync::Arc;

    use kinewright_core::{
        Analysis, AssetId, ColorBitDepth, ColorContext, ColorMatrix, ColorPrimaries, ColorRange,
        ColorTransfer, DeliveryEncodeDepth, DeliveryVerificationRequest, Document,
        ExportCancellation, ExportSettings, QaSeverity, TimeCode,
    };

    use super::*;
    use crate::{
        cc1_fixtures::{fallback_gpu, simple_document},
        color_pipeline::{PrimaryCorrection, PrimaryParameter},
        initialize_ffmpeg,
        test_support::{TempDirectory, ffmpeg_executable},
    };

    /// The bar luma codes written into the CC6 verification source raster,
    /// transcribed independently from CC1 §9's delivery source.
    const CC6_BAR_CODES: [u8; 5] = [16, 64, 128, 192, 235];
    const CC6_SOURCE_SIZE: (u32, u32) = (64, 32);
    const CC6_SOURCE_FRAMES: u32 = 12;
    const CC6_SOURCE_FPS: u32 = 25;

    // -----------------------------------------------------------------------
    // Source generation, through the pinned CLI, test-only.
    // -----------------------------------------------------------------------

    /// Write raw `yuv420p` planes to a tagged, lossless FFV1 file.
    ///
    /// The CLI is used here and only here: production never shells out
    /// (§6.1), and a *hand-built* file is the only way to put a deliberately
    /// illegal code into a plane, which is what §11.2.12's failing direction
    /// needs.
    fn write_ffv1_yuv420p(
        directory: &TempDirectory,
        name: &str,
        size: (u32, u32),
        ten_bit: bool,
        planes: &[u8],
    ) -> std::path::PathBuf {
        let path = directory.path(name);
        let pixel_format = if ten_bit { "yuv420p10le" } else { "yuv420p" };
        let mut command = ProcessCommand::new(ffmpeg_executable());
        command
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "rawvideo",
                "-pix_fmt",
                pixel_format,
                "-s",
                &format!("{}x{}", size.0, size.1),
                "-r",
                "1",
                "-i",
                "pipe:0",
                "-vf",
                "setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
                "-frames:v",
                "1",
                "-c:v",
                "ffv1",
                "-level",
                "3",
                "-g",
                "1",
                "-pix_fmt",
                pixel_format,
                "-color_primaries",
                "bt709",
                "-color_trc",
                "bt709",
                "-colorspace",
                "bt709",
                "-color_range",
                "tv",
            ])
            .arg(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("the pinned FFmpeg CLI should start");
        child
            .stdin
            .take()
            .expect("raw plane stdin")
            .write_all(planes)
            .expect("write the raw planes");
        let output = child.wait_with_output().expect("FFmpeg process");
        assert!(
            output.status.success(),
            "native-plane fixture generation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        path
    }

    /// `yuv420p` (or `yuv420p10le`) planes filled with one luma and one chroma
    /// pair, with an optional patch of a second luma / `Cb` in the top-left
    /// quarter.
    fn planar_frame(
        size: (u32, u32),
        ten_bit: bool,
        base: (u16, u16, u16),
        patch: Option<(u16, u16)>,
    ) -> Vec<u8> {
        let (width, height) = size;
        let chroma = (width / 2, height / 2);
        let mut bytes = Vec::new();
        let push = |code: u16, bytes: &mut Vec<u8>| {
            if ten_bit {
                bytes.extend_from_slice(&code.to_le_bytes());
            } else {
                bytes.push(u8::try_from(code).expect("an 8-bit code"));
            }
        };
        for y in 0..height {
            for x in 0..width {
                let inside = x < width / 2 && y < height / 2;
                let code = match (inside, patch) {
                    (true, Some((luma, _))) => luma,
                    _ => base.0,
                };
                push(code, &mut bytes);
            }
        }
        for y in 0..chroma.1 {
            for x in 0..chroma.0 {
                let inside = x < chroma.0 / 2 && y < chroma.1 / 2;
                let code = match (inside, patch) {
                    (true, Some((_, cb))) => cb,
                    _ => base.1,
                };
                push(code, &mut bytes);
            }
        }
        for _ in 0..chroma.0 * chroma.1 {
            push(base.2, &mut bytes);
        }
        bytes
    }

    /// Read frame 0's native planes through the production reader.
    fn read_native_frame(path: &std::path::Path) -> NativePlaneFrame {
        let asset = probe_path(path, AssetId(1)).expect("the fixture file should probe");
        let mut decoder = DeliveryDecoder::open(path, asset.fps, &asset.color_description)
            .expect("the fixture file should open for verification");
        decoder.sample(0).expect("frame 0 should decode").native
    }

    // -----------------------------------------------------------------------
    // §11.2.12: decoded native planes.
    // -----------------------------------------------------------------------

    #[test]
    #[allow(clippy::too_many_lines)]
    fn cc6_decoded_native_planes_report_ycbcr_excursions_in_delivery_code_units() {
        initialize_ffmpeg().expect("FFmpeg must initialize for the native-plane fixture");
        let directory = TempDirectory::new("cc6-native-planes");
        let size = (32_u32, 16_u32);

        for (label, ten_bit, base, illegal, expect_above_luma, expect_below_cb) in [
            // 8-bit: Y = 250 is above both the strict box (235) and the
            // EBU R 103 box (246); Cb = 5 is below the strict box (16) and
            // exactly on the R 103 floor, so it is a strict-box excursion and
            // not an R 103 one.
            (
                "eight_bit",
                false,
                (128_u16, 128, 128),
                (250_u16, 5_u16),
                250_i64,
                5_i64,
            ),
            // 10-bit: the same picture on the lane's own scale, with Cb = 16
            // taken *below* the R 103 floor of 20 so the ten-bit box is
            // exercised on both sides.
            (
                "ten_bit",
                true,
                (512_u16, 512, 512),
                (1000_u16, 16_u16),
                1000,
                16,
            ),
        ] {
            let scale = if ten_bit { 4_i64 } else { 1 };
            let illegal_planes = planar_frame(size, ten_bit, base, Some(illegal));
            let path = write_ffv1_yuv420p(
                &directory,
                &format!("illegal-{label}.mkv"),
                size,
                ten_bit,
                &illegal_planes,
            );
            let native = read_native_frame(&path);

            // Plane dimensions are `(w, h)` and `(w/2, h/2)`, and every code
            // stays inside its container.
            assert_eq!((native.width, native.height), size, "{label}");
            assert_eq!(
                (native.chroma_width, native.chroma_height),
                (size.0 / 2, size.1 / 2),
                "{label}"
            );
            assert_eq!(native.luma.len(), (size.0 * size.1) as usize, "{label}");
            assert_eq!(
                native.cb.len(),
                (size.0 / 2 * (size.1 / 2)) as usize,
                "{label}"
            );
            assert_eq!(native.bit_depth, if ten_bit { 10 } else { 8 }, "{label}");
            assert_eq!(
                native.pixel_format,
                if ten_bit { "yuv420p10le" } else { "yuv420p" },
                "{label}"
            );
            let ceiling = code_ceiling(native.bit_depth);
            for code in native.luma.iter().chain(&native.cb).chain(&native.cr) {
                assert!(
                    i64::from(*code) <= ceiling,
                    "{label}: native sample {code} escaped its container"
                );
            }

            let mut luma = PlaneLegalAccumulator::new(16 * scale, 235 * scale);
            let mut blue = PlaneLegalAccumulator::new(16 * scale, 240 * scale);
            let mut red = PlaneLegalAccumulator::new(16 * scale, 240 * scale);
            for code in &native.luma {
                luma.push(i64::from(*code));
            }
            for code in &native.cb {
                blue.push(i64::from(*code));
            }
            for code in &native.cr {
                red.push(i64::from(*code));
            }
            // The excursions are reported in delivery code units at the lane
            // depth, not rescaled to a common scale.
            assert_eq!(luma.maximum, expect_above_luma, "{label}");
            assert!(luma.excursion().above_count > 0, "{label}");
            assert_eq!(blue.minimum, expect_below_cb, "{label}");
            assert!(blue.excursion().below_count > 0, "{label}");
            assert_eq!(red.excursion().below_count, 0, "{label}");
            assert_eq!(red.excursion().above_count, 0, "{label}");

            let tolerance = EBU_R103_TOLERANCE_CODES_8BIT * scale;
            assert!(
                luma.outside_r103(tolerance),
                "{label}: Y = {expect_above_luma} must be outside the EBU R 103 box"
            );
            let exceptions =
                range_exceptions(&[("luma", luma), ("cb", blue), ("cr", red)], tolerance);
            assert!(
                exceptions
                    .iter()
                    .any(|exception| exception.code == "decoded_range_excursion"
                        && exception.severity == QaSeverity::Warning),
                "{label}: an illegal plane must raise decoded_range_excursion: {exceptions:?}"
            );
            println!(
                "CC6_NATIVE_PLANES lane={label} luma_above_bp={} luma_codes={}..={} cb_below_bp={} cb_codes={}..={}",
                luma.excursion().above_basis_points,
                luma.minimum,
                luma.maximum,
                blue.excursion().below_basis_points,
                blue.minimum,
                blue.maximum
            );

            // Passing direction: a legal file of the same shape reports no
            // excursion at all, so the reader is known to be able to say "none".
            let legal_planes = planar_frame(
                size,
                ten_bit,
                base,
                Some((235 * u16::try_from(scale).unwrap(), base.1)),
            );
            let legal_path = write_ffv1_yuv420p(
                &directory,
                &format!("legal-{label}.mkv"),
                size,
                ten_bit,
                &legal_planes,
            );
            let legal = read_native_frame(&legal_path);
            let mut legal_luma = PlaneLegalAccumulator::new(16 * scale, 235 * scale);
            for code in &legal.luma {
                legal_luma.push(i64::from(*code));
            }
            assert_eq!(legal_luma.excursion().above_count, 0, "{label}");
            assert_eq!(legal_luma.excursion().below_count, 0, "{label}");
            assert!(!legal_luma.outside_r103(tolerance), "{label}");
            assert!(
                range_exceptions(&[("luma", legal_luma)], tolerance).is_empty(),
                "{label}: a legal plane raises nothing"
            );
        }
    }

    // -----------------------------------------------------------------------
    // §11.2.15: the typed verification refusals, each with a passing neighbour.
    // -----------------------------------------------------------------------

    #[test]
    fn cc6_delivery_verification_plane_out_of_container_is_typed() {
        initialize_ffmpeg().expect("FFmpeg must initialize");
        let mut frame = ffmpeg::frame::Video::new(ffmpeg::format::Pixel::YUV420P10LE, 2, 2);
        for plane in 0..3 {
            let stride = frame.stride(plane);
            let width = frame.plane_width(plane) as usize;
            let height = frame.plane_height(plane) as usize;
            let data = frame.data_mut(plane);
            for row in 0..height {
                for column in 0..width {
                    let offset = row * stride + column * 2;
                    data[offset..offset + 2].copy_from_slice(&512_u16.to_le_bytes());
                }
            }
        }
        // Passing direction, one step away: the largest code that fits.
        let stride = frame.stride(0);
        frame.data_mut(0)[..2].copy_from_slice(&1023_u16.to_le_bytes());
        let native = native_planes(&frame).expect("1023 fits a ten-bit container");
        assert_eq!(native.luma[0], 1023);

        // Failing direction: one code past the container. A byte-order mistake
        // must not be mistaken for a colossal excursion.
        let _ = stride;
        frame.data_mut(0)[..2].copy_from_slice(&1024_u16.to_le_bytes());
        let error = native_planes(&frame).expect_err("1024 does not fit a ten-bit container");
        let MediaError::DeliveryVerification(error) = error else {
            panic!("a container overflow is a typed verification refusal: {error:?}");
        };
        assert_eq!(error.code(), "delivery_verification_plane_out_of_container");
        assert_eq!(error.field(), "native_plane_sample");
        assert_eq!(error.observed(), "1024");
        assert_eq!(error.allowed_values(), "0..=1023");
        assert!(!error.recovery_action().is_empty());
    }

    // -----------------------------------------------------------------------
    // The end-to-end 8-bit export, verified through the production surface.
    // -----------------------------------------------------------------------

    /// The CC6 verification source: five neutral grey bars plus one moving
    /// white square, tagged BT.709 limited, lossless FFV1.
    ///
    /// Deliberately chroma-neutral: the RGB **mean** is a gated number
    /// (§6.3(b)), and a hard saturated edge would load it with 4:2:0 chroma
    /// decimation, which §6.3(c) states is evidence and not a gate. The
    /// saturated-edge source belongs to the exit-gate fixture, which reports
    /// the extremes rather than gating them.
    fn generate_verification_source(directory: &TempDirectory) -> std::path::PathBuf {
        let (width, height) = CC6_SOURCE_SIZE;
        let mut input = Vec::new();
        for frame in 0..CC6_SOURCE_FRAMES {
            let square_x = 4 * frame;
            for y in 0..height {
                for x in 0..width {
                    let inside = (square_x..square_x + 8).contains(&x) && (12..20).contains(&y);
                    let bar = (x * 5 / width).min(4) as usize;
                    input.push(if inside { 235 } else { CC6_BAR_CODES[bar] });
                }
            }
            // yuv444p with neutral chroma on both planes.
            input.extend(std::iter::repeat_n(128_u8, (width * height * 2) as usize));
        }
        let path = directory.path("cc6-verification-source.mkv");
        let mut command = ProcessCommand::new(ffmpeg_executable());
        command
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "yuv444p",
                "-s",
                &format!("{width}x{height}"),
                "-r",
                &CC6_SOURCE_FPS.to_string(),
                "-i",
                "pipe:0",
                "-vf",
                "setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
                "-c:v",
                "ffv1",
                "-level",
                "3",
                "-g",
                "1",
                "-pix_fmt",
                "yuv444p",
                "-color_primaries",
                "bt709",
                "-color_trc",
                "bt709",
                "-colorspace",
                "bt709",
                "-color_range",
                "tv",
            ])
            .arg(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("the pinned FFmpeg CLI should start");
        child
            .stdin
            .take()
            .expect("verification source stdin")
            .write_all(&input)
            .expect("write the verification source");
        let output = child.wait_with_output().expect("FFmpeg process");
        assert!(
            output.status.success(),
            "verification source generation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        path
    }

    fn verification_document(source: &std::path::Path) -> Document {
        let asset = probe_path(source, AssetId(2)).expect("the verification source should probe");
        assert_eq!(asset.color_description.primaries, ColorPrimaries::Bt709);
        assert_eq!(asset.color_description.transfer, ColorTransfer::Bt709);
        assert_eq!(asset.color_description.matrix, ColorMatrix::Bt709);
        assert_eq!(asset.color_description.range, ColorRange::Limited);
        assert_eq!(asset.color_description.bit_depth, ColorBitDepth::Eight);
        assert_eq!(asset.duration, TimeCode(i64::from(CC6_SOURCE_FRAMES)));
        let mut document = simple_document(asset, CC6_SOURCE_SIZE);
        // A managed primary node, so the encode is a product of the pipeline
        // rather than a copy of the source.
        let correction = PrimaryCorrection {
            exposure_milli_stops: 100,
            ..PrimaryCorrection::default()
        };
        document.tracks[0].clips[0].effects = vec![kinewright_core::Effect {
            id: kinewright_core::EffectId(90),
            name: "primary_correction".to_owned(),
            parameters: PrimaryParameter::ALL
                .into_iter()
                .map(|parameter| {
                    (
                        parameter.name().to_owned(),
                        kinewright_core::ParamValue::Integer(correction.parameter(parameter)),
                    )
                })
                .collect(),
            keyframes: std::collections::BTreeMap::new(),
        }];
        document
            .validate()
            .expect("the verification document should validate");
        document
    }

    fn verification_settings(document: &Document) -> ExportSettings {
        ExportSettings {
            fps: document.fps,
            resolution: document.resolution,
            delivery_color: ColorContext::sdr_rec709().delivery,
            video_codec: "libx264".to_owned(),
            audio_codec: "aac".to_owned(),
            video_bitrate: 20_000_000,
            audio_bitrate: 192_000,
            cancellation: ExportCancellation::default(),
        }
    }

    #[test]
    #[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
    fn cc6_eight_bit_export_verifies_end_to_end_through_the_production_surface() {
        initialize_ffmpeg().expect("FFmpeg must initialize for the verification fixture");
        let directory = TempDirectory::new("cc6-verify-end-to-end");
        let source = generate_verification_source(&directory);
        let document = verification_document(&source);
        let settings = verification_settings(&document);
        let gpu = fallback_gpu();
        let output = directory.path("cc6-verified-export.mp4");
        let (progress_tx, _progress_rx) = crossbeam_channel::unbounded();
        crate::export::export_document(&document, &output, &settings, &progress_tx, gpu.context())
            .expect("the production export path should write an 8-bit H.264 delivery");

        let engine = crate::engine::FfmpegMediaEngine::new_with_gpu(gpu.context())
            .expect("the production media engine should start");
        let document = Arc::new(document);
        let request = DeliveryVerificationRequest::new(
            DeliveryEncodeDepth::Eight,
            settings.delivery_color.clone(),
        );
        let verification = engine
            .verify_delivery_output(Arc::clone(&document), &output, &settings, request.clone())
            .expect("the written export should verify");

        assert_eq!(verification.delivery_bit_depth, DeliveryEncodeDepth::Eight);
        assert_eq!(verification.decoded_pixel_format, "yuv420p");
        assert_eq!(verification.output_path, output);
        assert_eq!(verification.probed.primaries, ColorPrimaries::Bt709);
        assert_eq!(verification.probed.range, ColorRange::Limited);
        assert_eq!(verification.probed.bit_depth, ColorBitDepth::Eight);
        assert_eq!(verification.tags.tag_source, "probed_output_file");
        assert!(
            verification.tags.conforming,
            "a probed managed export must carry conforming tags: {:?}",
            verification.tags.mismatches
        );
        assert_eq!(
            verification.tags.not_representable.len(),
            1,
            "H.264 cannot carry a white point"
        );

        // §6.2 on T = 12 with n = 5: floor(i·11/4) = 0, 2, 5, 8, 11.
        assert_eq!(
            request.sample_frames(u64::from(CC6_SOURCE_FRAMES)),
            vec![0, 2, 5, 8, 11]
        );
        assert_eq!(verification.comparison.frames, vec![0, 2, 5, 8, 11]);

        let comparison = &verification.comparison;
        assert_eq!(comparison.rgb_extremes_note, DELIVERY_RGB_EXTREMES_NOTE);
        assert_eq!(
            comparison.decoded_ycbcr.source,
            YCbCrLegalSource::DecodedNativePlanes
        );
        assert_eq!(comparison.decoded_ycbcr.bit_depth, 8);

        // Non-vacuity: a measurement of exactly zero would prove the codec was
        // never exercised, not that it is perfect.
        assert!(
            comparison.combined.mean_code_diff_millionths > 0,
            "the source does not exercise the codec"
        );

        println!(
            "CC6_VERIFY_8BIT luma_max={} luma_p99_millionths={} luma_mean_millionths={} rgb_mean_millionths={} psnr_hundredths={:?} rgb_max={} rgb_p99_millionths={}",
            comparison.luma.maximum_code_diff,
            comparison.luma.p99_code_diff_millionths,
            comparison.luma.mean_code_diff_millionths,
            comparison.combined.mean_code_diff_millionths,
            comparison.psnr_db_hundredths,
            comparison.combined.maximum_code_diff,
            comparison.combined.p99_code_diff_millionths,
        );
        println!(
            "CC6_VERIFY_8BIT_LEGAL luma={}..={} cb={}..={} cr={}..={} luma_excursion_bp={}/{} cb_excursion_bp={}/{}",
            comparison.decoded_ycbcr.luma.minimum_code_hundredths / 100,
            comparison.decoded_ycbcr.luma.maximum_code_hundredths / 100,
            comparison.decoded_ycbcr.cb.minimum_code_hundredths / 100,
            comparison.decoded_ycbcr.cb.maximum_code_hundredths / 100,
            comparison.decoded_ycbcr.cr.minimum_code_hundredths / 100,
            comparison.decoded_ycbcr.cr.maximum_code_hundredths / 100,
            comparison.decoded_ycbcr.luma.below_basis_points,
            comparison.decoded_ycbcr.luma.above_basis_points,
            comparison.decoded_ycbcr.cb.below_basis_points,
            comparison.decoded_ycbcr.cb.above_basis_points,
        );

        // Rule 11.0.5: a budget no measurement approaches proves nothing, so
        // the margin is measured, printed, and asserted rather than assumed.
        let budgets = comparison.budgets;
        let margin = |observed: i64, allowed: i64| {
            if observed == 0 {
                f64::INFINITY
            } else {
                allowed as f64 / observed as f64
            }
        };
        let luma_max_margin = margin(
            i64::from(comparison.luma.maximum_code_diff),
            i64::from(budgets.luma_max_code),
        );
        let luma_p99_margin = margin(
            comparison.luma.p99_code_diff_millionths,
            budgets.luma_p99_code_millionths,
        );
        let luma_mean_margin = margin(
            comparison.luma.mean_code_diff_millionths,
            budgets.luma_mean_code_millionths,
        );
        let rgb_mean_margin = margin(
            comparison.combined.mean_code_diff_millionths,
            budgets.rgb_mean_code_millionths,
        );
        println!(
            "CC6_VERIFY_8BIT_MARGIN luma_max={luma_max_margin:.3}x luma_p99={luma_p99_margin:.3}x luma_mean={luma_mean_margin:.3}x rgb_mean={rgb_mean_margin:.3}x psnr_headroom_hundredths={}",
            comparison
                .psnr_db_hundredths
                .map_or(0, |psnr| psnr - budgets.psnr_floor_db_hundredths)
        );
        for (name, measured) in [
            ("luma_max", luma_max_margin),
            ("luma_p99", luma_p99_margin),
            ("luma_mean", luma_mean_margin),
            ("rgb_mean", rgb_mean_margin),
        ] {
            assert!(
                measured >= 2.0,
                "the {name} budget must keep at least a 2x margin on this source; measured {measured}x"
            );
        }

        assert!(
            comparison.within_budgets,
            "the 8-bit lane must land inside its budgets: {comparison:?}"
        );
        assert!(u64::from(comparison.luma.maximum_code_diff) <= u64::from(budgets.luma_max_code));
        assert!(comparison.luma.p99_code_diff_millionths <= budgets.luma_p99_code_millionths);
        assert!(comparison.luma.mean_code_diff_millionths <= budgets.luma_mean_code_millionths);
        assert!(comparison.combined.mean_code_diff_millionths <= budgets.rgb_mean_code_millionths);
        let psnr = comparison
            .psnr_db_hundredths
            .expect("a lossy encode has a finite MSE");
        assert!(psnr >= budgets.psnr_floor_db_hundredths, "PSNR {psnr}");
        assert!(
            verification.technical_pass,
            "no Error-severity exception: {:?}",
            verification.exceptions
        );

        // Evidence: the export's coded length and its *presented* length, the
        // two numbers the MP4 edit list can separate. They were once one frame
        // apart -- the muxer's negative-DTS `elst` trimmed the final coded
        // picture from presentation -- and this line records that they now
        // agree, at the same time as `presented_frame_count` gates it inside
        // `verify_delivery_output`.
        let coded = crate::decode::probe_path(&output, AssetId(9))
            .expect("the export re-probes")
            .duration;
        let presented = presented_frame_count(&output).expect("the export decodes");
        println!(
            "CC6_VERIFY_8BIT_TIMELINE implied_frames={CC6_SOURCE_FRAMES} coded_frames={} presented_frames={presented} sampled_frames={} last_sampled_frame={:?}",
            coded.0,
            comparison.frames.len(),
            comparison.frames.last()
        );
        assert_eq!(
            presented,
            u64::from(CC6_SOURCE_FRAMES),
            "every coded picture of a delivery export must also be presented"
        );
        assert_eq!(coded.0, i64::from(CC6_SOURCE_FRAMES));

        // §11.2.15's frame-count refusal, and its passing neighbour one step
        // away: the same file against a document that claims one more frame.
        let mut longer = (*document).clone();
        longer.duration = TimeCode(longer.duration.0 + 1);
        longer.tracks[0].clips[0].source_range = TimeCode::ZERO..longer.duration;
        let error = engine
            .verify_delivery_output(
                Arc::new(longer),
                &output,
                &settings,
                DeliveryVerificationRequest::new(
                    DeliveryEncodeDepth::Eight,
                    settings.delivery_color.clone(),
                ),
            )
            .expect_err("a shorter file is refused, never silently sampled");
        let MediaError::DeliveryVerification(error) = error else {
            panic!("a frame-count disagreement is a typed refusal: {error:?}");
        };
        assert_eq!(error.code(), "delivery_verification_frame_count_mismatch");
        assert_eq!(error.field(), "frame_count");
        assert_eq!(error.observed(), CC6_SOURCE_FRAMES.to_string());
        assert_eq!(error.allowed_values(), (CC6_SOURCE_FRAMES + 1).to_string());

        // §11.2.15's full-resolution refusal, and its passing neighbour: the
        // same reference render at a proxy scale is refused typed, while the
        // full-resolution one succeeds.
        let mut renderer = FrameRenderer::new(gpu.context());
        let full = delivery_reference(
            &gpu.context(),
            &mut renderer,
            &document,
            &settings,
            TimeCode::ZERO,
            RenderScale::FullResolution,
        )
        .expect("a full-resolution delivery reference is accepted");
        assert_eq!((full.width, full.height), settings.resolution);
        let error = delivery_reference(
            &gpu.context(),
            &mut renderer,
            &document,
            &settings,
            TimeCode::ZERO,
            RenderScale::Proxy { max_width: 16 },
        )
        .expect_err("a proxy raster may never be labelled a delivery reference");
        let MediaError::DeliveryVerification(error) = error else {
            panic!("a proxy reference is a typed refusal: {error:?}");
        };
        assert_eq!(error.code(), "delivery_verification_not_full_resolution");
        assert_eq!(error.field(), "full_resolution");
        assert!(error.observed().contains("Proxy"));
        assert!(!error.allowed_values().is_empty());
    }

    /// The failing direction of §6's frame-count cross-check: a file that codes
    /// `T` pictures but presents `T - 1`.
    ///
    /// The export is re-run through the test-only path that muxes video
    /// packets with the zero duration libavcodec leaves on them -- the
    /// pre-CC6 behaviour. The mov muxer then computes the track duration from
    /// the last packet's `pts + 0`, so the `elst` it writes for libx264's
    /// negative DTS is one frame short and `FFmpeg`'s demuxer flags the final
    /// coded picture `AV_PKT_FLAG_DISCARD`.
    ///
    /// Verification must refuse that file **typed**, reporting the *presented*
    /// count. A cross-check against the coded count -- which is what
    /// `probe_path` reports, and what this test asserts is unchanged -- would
    /// pass it and ship a delivery missing its last frame.
    #[test]
    fn cc6_verification_refuses_an_export_whose_edit_list_drops_the_last_frame() {
        initialize_ffmpeg().expect("FFmpeg must initialize for the verification fixture");
        let directory = TempDirectory::new("cc6-verify-edit-list-defect");
        let source = generate_verification_source(&directory);
        let document = verification_document(&source);
        let settings = verification_settings(&document);
        let gpu = fallback_gpu();
        let output = directory.path("cc6-edit-list-defect.mp4");
        let (progress_tx, _progress_rx) = crossbeam_channel::unbounded();
        crate::export::export_document_with_zero_packet_durations(
            &document,
            &output,
            &settings,
            &progress_tx,
            gpu.context(),
        )
        .expect("the test-only defective export should still write a file");

        // The file is not truncated: every picture is coded, and the crate's
        // reader of a file's *coded* length says so.
        assert_eq!(
            probe_path(&output, AssetId(11))
                .expect("the defective export probes")
                .duration,
            TimeCode(i64::from(CC6_SOURCE_FRAMES)),
            "the defect is a presentation loss, not a missing packet"
        );
        // ... and yet one fewer is presented.
        assert_eq!(
            presented_frame_count(&output).expect("the defective export decodes"),
            u64::from(CC6_SOURCE_FRAMES - 1),
            "this fixture only tests anything if the edit list really trims a frame"
        );

        let engine = crate::engine::FfmpegMediaEngine::new_with_gpu(gpu.context())
            .expect("the production media engine should start");
        let error = engine
            .verify_delivery_output(
                Arc::new(document),
                &output,
                &settings,
                DeliveryVerificationRequest::new(
                    DeliveryEncodeDepth::Eight,
                    settings.delivery_color.clone(),
                ),
            )
            .expect_err("a file that does not present every frame is refused");
        let MediaError::DeliveryVerification(error) = error else {
            panic!("a presented-frame shortfall is a typed refusal: {error:?}");
        };
        assert_eq!(error.code(), "delivery_verification_frame_count_mismatch");
        assert_eq!(error.field(), "frame_count");
        assert_eq!(error.observed(), (CC6_SOURCE_FRAMES - 1).to_string());
        assert_eq!(error.allowed_values(), CC6_SOURCE_FRAMES.to_string());
        assert!(!error.recovery_action().is_empty());
    }

    /// §6.2/§6.3: the picture that comes back must be the frame that was asked
    /// for, and the comparison records the identity the *picture* carried.
    ///
    /// [`DeliveryDecoder::sample`] returns the first picture at or after the
    /// requested index, which is right for a file whose frame identities land
    /// where the export fps says they do and wrong for one whose do not. The
    /// overshoot is manufactured the only way a well-formed encode allows: the
    /// same file, read at **twice** its own frame rate, so frame `n` of the
    /// file computes as identity `2n` and a request for frame 1 is answered by
    /// frame 2. Without the refusal that picture would be compared against the
    /// reference render of frame 1 and published under frame 1's number.
    #[test]
    fn cc6_a_decoded_frame_that_is_not_the_requested_frame_is_refused() {
        initialize_ffmpeg().expect("FFmpeg must initialize for the verification fixture");
        let directory = TempDirectory::new("cc6-verify-decoded-identity");
        let source = generate_verification_source(&directory);
        let document = verification_document(&source);
        let settings = verification_settings(&document);
        let gpu = fallback_gpu();
        let output = directory.path("cc6-decoded-identity.mp4");
        let (progress_tx, _progress_rx) = crossbeam_channel::unbounded();
        crate::export::export_document(&document, &output, &settings, &progress_tx, gpu.context())
            .expect("the production export path should write an 8-bit H.264 delivery");
        let probed = probe_path(&output, AssetId(12))
            .expect("the export re-probes")
            .color_description;

        let mut renderer = FrameRenderer::new(gpu.context());
        let context = gpu.context();

        // Passing direction: read at the file's own rate, frame 1 is frame 1,
        // and the comparison records that identity.
        let mut honest = DeliveryDecoder::open(&output, settings.fps, &probed)
            .expect("the export opens for verification");
        assert_eq!(
            honest.sample(1).expect("frame 1 decodes").at,
            1,
            "at the file's own rate the decoder answers the frame that was asked for"
        );
        let mut honest = DeliveryDecoder::open(&output, settings.fps, &probed)
            .expect("the export opens for verification");
        let measured = measure_samples(
            &context,
            &mut renderer,
            &mut honest,
            &document,
            &settings,
            8,
            &[1],
        )
        .expect("a frame that is the frame that was asked for is measured");
        assert_eq!(
            measured.frames,
            vec![1],
            "the comparison records the decoded frame identity"
        );

        // Failing direction: the same file at twice its rate.
        let doubled = Rational::new(settings.fps.numerator() * 2, settings.fps.denominator())
            .expect("a doubled frame rate");
        let mut overshooting = DeliveryDecoder::open(&output, doubled, &probed)
            .expect("the export opens for verification");
        assert_eq!(
            overshooting.sample(1).expect("a picture decodes").at,
            2,
            "this fixture only tests anything if the doubled rate really overshoots"
        );
        let mut overshooting = DeliveryDecoder::open(&output, doubled, &probed)
            .expect("the export opens for verification");
        let error = verification_error(
            measure_samples(
                &context,
                &mut renderer,
                &mut overshooting,
                &document,
                &settings,
                8,
                &[1],
            )
            // `err()` rather than `expect_err`: the Ok side is a histogram of
            // millions of samples and has no business in a panic message.
            .err()
            .expect("a picture that is not the requested frame is never compared"),
        );
        assert_eq!(error.code(), "delivery_verification_frame_count_mismatch");
        assert_eq!(error.field(), "frame_count");
        assert_eq!(error.observed(), "decoded frame 2 for requested frame 1");
        assert_eq!(error.allowed_values(), "decoded frame 1");
        assert!(!error.recovery_action().is_empty());
    }

    // -----------------------------------------------------------------------
    // The request-shaped refusals: neither of these opens the file at all, so
    // they are asserted against a path that does not exist. A version that
    // decoded first and validated afterwards would fail here with a decode
    // error instead of the typed refusal.
    // -----------------------------------------------------------------------

    /// A document and settings for one lane, with no source and no file.
    fn lane_settings(depth: DeliveryEncodeDepth) -> (Arc<Document>, ExportSettings) {
        let document = Document::default();
        let delivery_color = kinewright_core::delivery_color_for_depth(&document, depth);
        let settings = ExportSettings {
            fps: document.fps,
            resolution: document.resolution,
            delivery_color,
            video_codec: "libx264".to_owned(),
            audio_codec: "aac".to_owned(),
            video_bitrate: 20_000_000,
            audio_bitrate: 192_000,
            cancellation: ExportCancellation::default(),
        };
        (Arc::new(document), settings)
    }

    /// The typed verification refusal behind a [`MediaError`].
    fn verification_error(error: MediaError) -> kinewright_core::DeliveryVerificationError {
        match error {
            MediaError::DeliveryVerification(typed) => typed,
            other => panic!("a verification refusal must be typed, not a string: {other:?}"),
        }
    }

    /// §6.4: a plane that saw no sample has no extremes, and §6.2's empty
    /// sample set is refused rather than measured.
    #[test]
    fn cc6_an_unseen_plane_reports_the_empty_interval_and_an_empty_sample_set_is_refused() {
        // (a) An accumulator nothing was pushed into reports core's empty
        //     interval, not `0..=0`. `0` is a code a real sample can land on,
        //     so a fabricated `0..=0` beside `below_count == 0` would be
        //     indistinguishable from a plane whose every sample was black.
        let unseen = PlaneLegalAccumulator::new(16, 235).excursion();
        assert!(
            !unseen.samples_seen(),
            "a plane that saw nothing must not claim extremes: {unseen:?}"
        );
        assert_eq!(
            unseen.minimum_code_hundredths,
            PlaneLegalExcursion::UNSEEN_MINIMUM_CODE_HUNDREDTHS
        );
        assert_eq!(
            unseen.maximum_code_hundredths,
            PlaneLegalExcursion::UNSEEN_MAXIMUM_CODE_HUNDREDTHS
        );
        assert!(unseen.minimum_code_hundredths > unseen.maximum_code_hundredths);
        assert_eq!(unseen.below_count, 0);
        assert_eq!(unseen.above_count, 0);
        assert_eq!(unseen.excursion_basis_points(0), 0);

        // Passing direction, one sample away: a single legal sample gives the
        // plane a real, seen extreme pair on that code.
        let mut seen = PlaneLegalAccumulator::new(16, 235);
        seen.push(0);
        let seen = seen.excursion();
        assert!(seen.samples_seen());
        assert_eq!(seen.minimum_code_hundredths, 0);
        assert_eq!(seen.maximum_code_hundredths, 0);
        assert_eq!(seen.below_count, 1, "code 0 is below the 16 floor");
        assert_eq!(seen.excursion_basis_points(1), 10_000);

        // (b) The sample set behind those accumulators. `sample_frames` is
        //     total and answers `[]` for a document that implies no frames;
        //     measuring that would publish `within_budgets == true` because
        //     nothing exceeded anything.
        let (_, settings) = lane_settings(DeliveryEncodeDepth::Eight);
        let request = DeliveryVerificationRequest::new(
            DeliveryEncodeDepth::Eight,
            settings.delivery_color.clone(),
        );
        assert!(request.sample_frames(0).is_empty());
        let error = verification_error(
            sampled_frames(&request, 0).expect_err("an empty sample set is never measured"),
        );
        assert_eq!(error.code(), "delivery_verification_frame_count_mismatch");
        assert_eq!(error.field(), "frame_count");
        assert_eq!(error.observed(), "0 sampled frames");
        assert_eq!(error.allowed_values(), "at least one sampled frame");
        assert!(!error.recovery_action().is_empty());
        // Passing direction, one frame away.
        assert_eq!(
            sampled_frames(&request, 1).expect("one implied frame samples frame 0"),
            vec![0]
        );
    }

    /// §6.3: a request may not carry the other lane's budgets.
    #[test]
    fn cc6_verification_refuses_budgets_from_the_other_delivery_lane() {
        initialize_ffmpeg().expect("FFmpeg must initialize");
        let gpu = fallback_gpu();
        let engine = crate::engine::FfmpegMediaEngine::new_with_gpu(gpu.context())
            .expect("the production media engine should start");
        let directory = TempDirectory::new("cc6-verify-budget-lane");
        let absent = directory.path("never-written.mp4");

        // Both directions: 8-bit settings with the 10-bit lane's budgets, and
        // 10-bit settings with the 8-bit lane's.
        for (settings_depth, budget_depth) in [
            (DeliveryEncodeDepth::Eight, DeliveryEncodeDepth::Ten),
            (DeliveryEncodeDepth::Ten, DeliveryEncodeDepth::Eight),
        ] {
            let (document, settings) = lane_settings(settings_depth);
            let mut request =
                DeliveryVerificationRequest::new(settings_depth, settings.delivery_color.clone());
            request.budgets = DeliveryBudgets::for_depth(budget_depth);
            assert_ne!(request.budgets, DeliveryBudgets::for_depth(settings_depth));
            let error = verification_error(
                engine
                    .verify_delivery_output(
                        Arc::clone(&document),
                        &absent,
                        &settings,
                        request.clone(),
                    )
                    .expect_err("a lane may not be measured against another lane's budgets"),
            );
            assert_eq!(
                error.code(),
                "delivery_verification_budget_lane_mismatch",
                "{settings_depth:?} settings with {budget_depth:?} budgets"
            );
            assert_eq!(error.field(), "budgets");
            assert!(
                error.observed().contains(settings_depth.as_str()),
                "the refusal must name the lane whose budgets were expected: {}",
                error.observed()
            );
            assert!(
                error.allowed_values().contains(
                    &DeliveryBudgets::for_depth(settings_depth)
                        .luma_max_code
                        .to_string()
                ),
                "the refusal must state the budgets the lane names: {}",
                error.allowed_values()
            );
            assert!(!error.recovery_action().is_empty());

            // Passing direction, one field away: the matching budgets get past
            // this refusal and fail on the *file*, which does not exist.
            let matching =
                DeliveryVerificationRequest::new(settings_depth, settings.delivery_color.clone());
            let error = engine
                .verify_delivery_output(Arc::clone(&document), &absent, &settings, matching)
                .expect_err("the file was never written");
            assert!(
                !matches!(
                    error,
                    MediaError::DeliveryVerification(
                        kinewright_core::DeliveryVerificationError::BudgetLaneMismatch { .. }
                    )
                ),
                "matching budgets must not raise the lane refusal: {error:?}"
            );
        }
    }

    /// §6.2: `frame_count` is validated before anything is sampled, so a
    /// clamped sample is never published under the number that was asked for.
    #[test]
    fn cc6_verification_refuses_a_frame_count_the_sampler_would_have_clamped() {
        initialize_ffmpeg().expect("FFmpeg must initialize");
        let gpu = fallback_gpu();
        let engine = crate::engine::FfmpegMediaEngine::new_with_gpu(gpu.context())
            .expect("the production media engine should start");
        let directory = TempDirectory::new("cc6-verify-frame-count");
        let absent = directory.path("never-written.mp4");
        let (document, settings) = lane_settings(DeliveryEncodeDepth::Eight);

        for frame_count in [0_u8, kinewright_core::DELIVERY_VERIFICATION_MAX_FRAMES + 1] {
            let mut request = DeliveryVerificationRequest::new(
                DeliveryEncodeDepth::Eight,
                settings.delivery_color.clone(),
            );
            request.frame_count = frame_count;
            // The clamp this refusal exists to keep from being published: the
            // sampler would have answered a *different* question.
            assert!(!request.sample_frames(60).is_empty());
            let error = verification_error(
                engine
                    .verify_delivery_output(Arc::clone(&document), &absent, &settings, request)
                    .expect_err("a frame_count outside 1..=16 is refused, never clamped"),
            );
            // Rule 11.0.4: code, field, observed, allowed.
            assert_eq!(
                error.code(),
                "delivery_verification_frame_count_out_of_range",
                "frame_count {frame_count}"
            );
            assert_eq!(error.field(), "frame_count");
            assert_eq!(error.observed(), frame_count.to_string());
            assert_eq!(error.allowed_values(), "1..=16");
            assert!(!error.recovery_action().is_empty());
        }

        // Passing direction, one step either side of the range.
        for frame_count in [1_u8, kinewright_core::DELIVERY_VERIFICATION_MAX_FRAMES] {
            let mut request = DeliveryVerificationRequest::new(
                DeliveryEncodeDepth::Eight,
                settings.delivery_color.clone(),
            );
            request.frame_count = frame_count;
            let error = engine
                .verify_delivery_output(Arc::clone(&document), &absent, &settings, request)
                .expect_err("the file was never written");
            assert!(
                !matches!(
                    error,
                    MediaError::DeliveryVerification(
                        kinewright_core::DeliveryVerificationError::FrameCountOutOfRange { .. }
                    )
                ),
                "frame_count {frame_count} is inside the range: {error:?}"
            );
        }
    }

    #[test]
    fn cc6_delivery_reference_denominator_is_the_delivery_intermediate_white() {
        // Not a second copy of 65_280: the encode side quantizes on this very
        // constant, and §6.3 requires both sides of the comparison to divide
        // by it.
        assert_eq!(
            DELIVERY_REFERENCE_DENOMINATOR,
            crate::color_pipeline::DELIVERY_INTERMEDIATE_WHITE
        );
        assert_eq!(u32::from(DELIVERY_REFERENCE_DENOMINATOR), 255_u32 << 8);
        // The `EBU R 103` tolerance is -5 %/+105 % of the nominal range, i.e.
        // 11 codes at 8 bits and 44 at 10.
        assert_eq!(EBU_R103_TOLERANCE_CODES_8BIT, 11);
        assert_eq!(16 - EBU_R103_TOLERANCE_CODES_8BIT, 5);
        assert_eq!(235 + EBU_R103_TOLERANCE_CODES_8BIT, 246);
        assert_eq!(240 + EBU_R103_TOLERANCE_CODES_8BIT, 251);
        let ten = EBU_R103_TOLERANCE_CODES_8BIT * code_scale(10);
        assert_eq!(16 * code_scale(10) - ten, 20);
        assert_eq!(235 * code_scale(10) + ten, 984);
        assert_eq!(240 * code_scale(10) + ten, 1004);
    }

    #[test]
    fn cc6_delivery_budgets_are_distinct_from_the_compositor_gate() {
        use crate::cc1_fixtures::{MONITOR_CPU_GPU_MAX, MONITOR_CPU_GPU_MEAN, MONITOR_CPU_GPU_P99};

        // CC1's compositor tolerances must never be silently substituted for a
        // codec budget: they are flat-field numbers and would fail instantly on
        // any raster carrying a saturated edge.
        for depth in DeliveryEncodeDepth::ALL {
            let budgets = DeliveryBudgets::for_depth(depth);
            assert_ne!(budgets.luma_max_code, u32::from(MONITOR_CPU_GPU_MAX));
            assert_ne!(
                budgets.luma_p99_code_millionths,
                millionths(MONITOR_CPU_GPU_P99)
            );
            assert_ne!(
                budgets.luma_mean_code_millionths,
                millionths(MONITOR_CPU_GPU_MEAN)
            );
        }
    }
}
