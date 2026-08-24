//! Deterministic CC2 colour-scope and shot-matching evidence.
//!
//! This module deliberately measures an already rendered [`RgbaImage`].  It
//! does not know how to render a project, apply a grade, or choose a preferred
//! look.  The one supported stage is [`ScopeStage::MonitoringPostComposite`],
//! which is the post-compositor monitoring image described by the CC2
//! contract.  Keeping that boundary explicit prevents a scope result from
//! being mistaken for source, pre-composite, or delivery data.
//!
//! All calculations use integer arithmetic.  Input channels are RGBA8; RGB
//! channels and Rec.709 luma are represented as 16-bit full-scale codes
//! (`0..=65_535`) in summary statistics.  Alpha-zero pixels are excluded from
//! every scope and statistic, while any non-zero alpha is included without
//! alpha weighting.  Temporal aggregation concatenates samples in ascending
//! project-frame order, so the result is independent of the caller's frame
//! ordering.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::RgbaImage;

/// The fixed full-scale used for RGB and luma percentile codes.
pub const SCOPE_SAMPLE_SCALE: u32 = 65_535;
/// The fixed full-scale used for means.
pub const SCOPE_MEAN_SCALE: u32 = 1_000_000;
/// Basis points in one whole measurement.
pub const SCOPE_BASIS_POINTS: u32 = 10_000;
/// Low clipping includes 8-bit code values 0 and 1.
pub const SCOPE_LOW_CLIP_CODE: u8 = 1;
/// High clipping includes 8-bit code values 254 and 255.
pub const SCOPE_HIGH_CLIP_CODE: u8 = 254;

/// Maximum number of bins in one configurable histogram.
pub const SCOPE_MAX_HISTOGRAM_BINS: u16 = 4_096;
/// Maximum number of horizontal samples in the luma waveform or parade.
pub const SCOPE_MAX_WAVEFORM_COLUMNS: u16 = 2_048;
/// Maximum number of rows in the luma waveform or RGB parade.
pub const SCOPE_MAX_WAVEFORM_ROWS: u16 = 1_024;
/// Maximum side length of the square vectorscope density grid.
pub const SCOPE_MAX_VECTORSCOPE_SIZE: u16 = 1_024;
/// Maximum number of explicitly identified frames in one temporal measure.
pub const SCOPE_MAX_TEMPORAL_FRAMES: usize = 64;

/// The pipeline boundary from which scope samples are taken.
///
/// CC2 currently exposes exactly one renderable stage.  The explicit name is
/// part of the evidence contract: adding a pre-grade or delivery stage later
/// requires adding a new vocabulary value and its own tests.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum ScopeStage {
    /// The managed monitor image after compositing, before any delivery codec.
    #[default]
    #[serde(
        rename = "monitoring_post_composite",
        alias = "monitoring/post-composite"
    )]
    MonitoringPostComposite,
}

impl ScopeStage {
    /// Stable wire identifier used by agent and application surfaces.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MonitoringPostComposite => "monitoring_post_composite",
        }
    }
}

/// Compatibility spelling for callers that use the roadmap's pipeline-stage
/// terminology.
pub type ScopePipelineStage = ScopeStage;

/// Normalized geometric region of interest, expressed in basis points of the
/// source raster.
///
/// The rectangle is half-open: `(x, y)` is its top-left boundary and
/// `(x + width, y + height)` is its exclusive bottom-right boundary.  All
/// values must be in `0..=10_000`, and the right/bottom boundaries must not
/// exceed `10_000`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct NormalizedRoi {
    pub x_basis_points: u32,
    pub y_basis_points: u32,
    pub width_basis_points: u32,
    pub height_basis_points: u32,
}

impl NormalizedRoi {
    /// Construct a normalized rectangle.  Structural validation is performed
    /// by [`Self::validate`] or when a measurement is requested.
    #[must_use]
    pub const fn new(
        x_basis_points: u32,
        y_basis_points: u32,
        width_basis_points: u32,
        height_basis_points: u32,
    ) -> Self {
        Self {
            x_basis_points,
            y_basis_points,
            width_basis_points,
            height_basis_points,
        }
    }

    /// The complete source raster.
    #[must_use]
    pub const fn full_frame() -> Self {
        Self::new(0, 0, SCOPE_BASIS_POINTS, SCOPE_BASIS_POINTS)
    }

    /// Validate normalized bounds before a source resolution is known.
    ///
    /// # Errors
    ///
    /// Returns a typed error for out-of-range, overflowing, or zero-area
    /// rectangles.
    pub fn validate(&self) -> Result<(), NormalizedRoiError> {
        let values = [
            ("x_basis_points", self.x_basis_points),
            ("y_basis_points", self.y_basis_points),
            ("width_basis_points", self.width_basis_points),
            ("height_basis_points", self.height_basis_points),
        ];
        if let Some((field, value)) = values.iter().find(|(_, value)| *value > SCOPE_BASIS_POINTS) {
            return Err(NormalizedRoiError::OutOfRange {
                field,
                value: *value,
            });
        }
        if self.width_basis_points == 0 || self.height_basis_points == 0 {
            return Err(NormalizedRoiError::Empty);
        }
        let right = self
            .x_basis_points
            .checked_add(self.width_basis_points)
            .ok_or(NormalizedRoiError::Overflow)?;
        let bottom = self
            .y_basis_points
            .checked_add(self.height_basis_points)
            .ok_or(NormalizedRoiError::Overflow)?;
        if right > SCOPE_BASIS_POINTS || bottom > SCOPE_BASIS_POINTS {
            return Err(NormalizedRoiError::OutsideFrame);
        }
        Ok(())
    }

    /// Convert the normalized rectangle to source pixels.
    ///
    /// Start boundaries use floor and exclusive end boundaries use ceil.  A
    /// positive normalized rectangle therefore includes every pixel touched
    /// by the geometric rectangle, including a boundary that falls between
    /// source pixels.  The resulting rectangle remains half-open.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid source dimensions, invalid normalized
    /// values, or a rectangle narrower than one source pixel.
    pub fn to_pixels(
        &self,
        source_width: u32,
        source_height: u32,
    ) -> Result<PixelRoi, NormalizedRoiError> {
        self.validate()?;
        if source_width == 0 || source_height == 0 {
            return Err(NormalizedRoiError::InvalidSourceResolution {
                width: source_width,
                height: source_height,
            });
        }
        let scale = u64::from(SCOPE_BASIS_POINTS);
        let x = u64::from(self.x_basis_points).saturating_mul(u64::from(source_width)) / scale;
        let y = u64::from(self.y_basis_points).saturating_mul(u64::from(source_height)) / scale;
        let right_numerator = u64::from(
            self.x_basis_points
                .checked_add(self.width_basis_points)
                .ok_or(NormalizedRoiError::Overflow)?,
        )
        .saturating_mul(u64::from(source_width));
        let bottom_numerator = u64::from(
            self.y_basis_points
                .checked_add(self.height_basis_points)
                .ok_or(NormalizedRoiError::Overflow)?,
        )
        .saturating_mul(u64::from(source_height));
        let right = right_numerator
            .checked_add(scale - 1)
            .ok_or(NormalizedRoiError::Overflow)?
            / scale;
        let bottom = bottom_numerator
            .checked_add(scale - 1)
            .ok_or(NormalizedRoiError::Overflow)?
            / scale;
        let x = u32::try_from(x).map_err(|_| NormalizedRoiError::Overflow)?;
        let y = u32::try_from(y).map_err(|_| NormalizedRoiError::Overflow)?;
        let right = u32::try_from(right).map_err(|_| NormalizedRoiError::Overflow)?;
        let bottom = u32::try_from(bottom).map_err(|_| NormalizedRoiError::Overflow)?;
        if right <= x || bottom <= y {
            return Err(NormalizedRoiError::EmptyAfterPixelConversion);
        }
        if right > source_width || bottom > source_height {
            return Err(NormalizedRoiError::OutsideFrame);
        }
        Ok(PixelRoi {
            x,
            y,
            width: right - x,
            height: bottom - y,
        })
    }
}

/// Error produced while validating or rasterizing a normalized ROI.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NormalizedRoiError {
    #[error("ROI {field}={value} exceeds {SCOPE_BASIS_POINTS} basis points")]
    OutOfRange { field: &'static str, value: u32 },
    #[error("ROI width and height must both be positive")]
    Empty,
    #[error("ROI boundary arithmetic overflowed")]
    Overflow,
    #[error("ROI right or bottom boundary exceeds the normalized frame")]
    OutsideFrame,
    #[error("source resolution must be non-zero, got {width}x{height}")]
    InvalidSourceResolution { width: u32, height: u32 },
    #[error("ROI is non-empty geometrically but covers no source pixel")]
    EmptyAfterPixelConversion,
}

/// Pixel-space half-open ROI returned by deterministic normalized conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct PixelRoi {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl PixelRoi {
    /// Exclusive right boundary.
    #[must_use]
    pub const fn right(self) -> u32 {
        self.x.saturating_add(self.width)
    }

    /// Exclusive bottom boundary.
    #[must_use]
    pub const fn bottom(self) -> u32 {
        self.y.saturating_add(self.height)
    }

    /// Number of pixel positions in this rectangle, if representable.
    #[must_use]
    pub fn pixel_count(self) -> Option<u64> {
        u64::from(self.width).checked_mul(u64::from(self.height))
    }
}

/// Bounded raster dimensions for each generated scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ScopeResolution {
    pub histogram_bins: u16,
    pub waveform_columns: u16,
    pub waveform_rows: u16,
    pub parade_columns: u16,
    pub parade_rows: u16,
    pub vectorscope_size: u16,
}

impl Default for ScopeResolution {
    fn default() -> Self {
        Self {
            histogram_bins: 256,
            waveform_columns: 64,
            waveform_rows: 256,
            parade_columns: 64,
            parade_rows: 256,
            vectorscope_size: 256,
        }
    }
}

impl ScopeResolution {
    /// Construct and validate a complete output-resolution configuration.
    ///
    /// # Errors
    ///
    /// Returns [`ScopeError::InvalidResolution`] when a dimension is zero or
    /// exceeds the CC2 bound.
    pub fn new(
        histogram_bins: u16,
        waveform_columns: u16,
        waveform_rows: u16,
        parade_columns: u16,
        parade_rows: u16,
        vectorscope_size: u16,
    ) -> Result<Self, ScopeError> {
        let resolution = Self {
            histogram_bins,
            waveform_columns,
            waveform_rows,
            parade_columns,
            parade_rows,
            vectorscope_size,
        };
        resolution.validate()?;
        Ok(resolution)
    }

    /// Validate output dimensions against the CC2 memory bounds.
    ///
    /// # Errors
    ///
    /// Returns a typed resolution or allocation-arithmetic error when the
    /// configuration cannot be represented within the bounded grids.
    pub fn validate(&self) -> Result<(), ScopeError> {
        validate_dimension(
            "histogram_bins",
            self.histogram_bins,
            SCOPE_MAX_HISTOGRAM_BINS,
        )?;
        validate_dimension(
            "waveform_columns",
            self.waveform_columns,
            SCOPE_MAX_WAVEFORM_COLUMNS,
        )?;
        validate_dimension("waveform_rows", self.waveform_rows, SCOPE_MAX_WAVEFORM_ROWS)?;
        validate_dimension(
            "parade_columns",
            self.parade_columns,
            SCOPE_MAX_WAVEFORM_COLUMNS,
        )?;
        validate_dimension("parade_rows", self.parade_rows, SCOPE_MAX_WAVEFORM_ROWS)?;
        validate_dimension(
            "vectorscope_size",
            self.vectorscope_size,
            SCOPE_MAX_VECTORSCOPE_SIZE,
        )?;
        checked_grid_len(
            u32::from(self.waveform_columns),
            u32::from(self.waveform_rows),
            "waveform",
        )?;
        checked_grid_len(
            u32::from(self.parade_columns),
            u32::from(self.parade_rows),
            "parade",
        )?;
        checked_grid_len(
            u32::from(self.vectorscope_size),
            u32::from(self.vectorscope_size),
            "vectorscope",
        )?;
        Ok(())
    }
}

fn validate_dimension(field: &'static str, value: u16, maximum: u16) -> Result<(), ScopeError> {
    if value == 0 {
        return Err(ScopeError::InvalidResolution {
            field,
            requested: u32::from(value),
            maximum: u32::from(maximum),
        });
    }
    if value > maximum {
        return Err(ScopeError::InvalidResolution {
            field,
            requested: u32::from(value),
            maximum: u32::from(maximum),
        });
    }
    Ok(())
}

fn checked_grid_len(width: u32, height: u32, name: &'static str) -> Result<usize, ScopeError> {
    let len = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or(ScopeError::ArithmeticOverflow { operation: name })?;
    usize::try_from(len).map_err(|_| ScopeError::ArithmeticOverflow { operation: name })
}

/// A measurement request.  Requests are evidence-only and cannot mutate a
/// project or infer a correction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScopeRequest {
    pub stage: ScopeStage,
    pub roi: NormalizedRoi,
    pub resolution: ScopeResolution,
}

impl Default for ScopeRequest {
    fn default() -> Self {
        Self {
            stage: ScopeStage::MonitoringPostComposite,
            roi: NormalizedRoi::full_frame(),
            resolution: ScopeResolution::default(),
        }
    }
}

impl ScopeRequest {
    /// Validate the stage, ROI, and bounded output configuration.
    ///
    /// # Errors
    ///
    /// Returns a typed error when the stage is not renderable, the ROI is
    /// invalid, or an output dimension exceeds its bound.
    pub fn validate(&self) -> Result<(), ScopeError> {
        if self.stage != ScopeStage::MonitoringPostComposite {
            return Err(ScopeError::UnsupportedStage { stage: self.stage });
        }
        self.roi.validate().map_err(ScopeError::InvalidRoi)?;
        self.resolution.validate()
    }
}

/// A frame paired with its explicit project-frame identity.
///
/// The borrowed image keeps this input type cheap and prevents the evidence
/// engine from silently taking ownership or modifying a render result.
#[derive(Debug, Clone, Copy)]
pub struct ScopeFrame<'a> {
    pub project_frame: i64,
    pub image: &'a RgbaImage,
}

impl<'a> ScopeFrame<'a> {
    /// Pair an image with a project frame.
    #[must_use]
    pub const fn new(project_frame: i64, image: &'a RgbaImage) -> Self {
        Self {
            project_frame,
            image,
        }
    }
}

/// Alias emphasizing that frame samples are inputs rather than stored media.
pub type ScopeFrameInput<'a> = ScopeFrame<'a>;

/// Source and sampling metadata attached to every evidence result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScopeMeasurementMetadata {
    pub stage: ScopeStage,
    pub source_resolution: ScopeRasterResolution,
    /// The source-pixel dimensions actually sampled after ROI conversion.
    pub measurement_resolution: ScopeRasterResolution,
    /// Always true for this engine: no source downsample is performed.
    pub full_resolution: bool,
    pub normalized_roi: NormalizedRoi,
    pub pixel_roi: PixelRoi,
    /// Sorted, unique project frame identities included in this aggregation.
    pub project_frames: Vec<i64>,
    pub roi_pixel_count: u64,
    pub transparent_pixel_count: u64,
    pub visible_pixel_count: u64,
}

/// A raster resolution recorded in source and measurement metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ScopeRasterResolution {
    pub width: u32,
    pub height: u32,
}

/// Fixed-point statistics for one channel.
///
/// Percentiles use 16-bit full-scale codes (`8-bit input * 257`).  `mean` is
/// a normalized mean in millionths (`0..=1_000_000`).  Percentile ranks use
/// nearest rank with `ceil(p * N / 100)`, where the first percentile is `p=1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChannelStatistics {
    pub mean: u32,
    pub first_percentile: u16,
    pub median: u16,
    pub ninety_ninth_percentile: u16,
}

impl ChannelStatistics {
    /// Alias for the first-percentile code.
    #[must_use]
    pub const fn p1(self) -> u16 {
        self.first_percentile
    }

    /// Alias for the median code.
    #[must_use]
    pub const fn p50(self) -> u16 {
        self.median
    }

    /// Alias for the ninety-ninth-percentile code.
    #[must_use]
    pub const fn p99(self) -> u16 {
        self.ninety_ninth_percentile
    }
}

/// Channel statistics for RGB plus Rec.709 luma.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScopeStatistics {
    pub red: ChannelStatistics,
    pub green: ChannelStatistics,
    pub blue: ChannelStatistics,
    pub luma: ChannelStatistics,
}

/// Clipping rates for one channel, in basis points of visible samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClippingBasisPoints {
    pub black: u32,
    pub white: u32,
}

/// Clipping rates for RGB plus luma.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScopeClipping {
    pub red: ClippingBasisPoints,
    pub green: ClippingBasisPoints,
    pub blue: ClippingBasisPoints,
    pub luma: ClippingBasisPoints,
}

/// RGB and luma histogram counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScopeHistograms {
    pub bins: u16,
    pub red: Vec<u64>,
    pub green: Vec<u64>,
    pub blue: Vec<u64>,
    pub luma: Vec<u64>,
}

/// A luma waveform density grid.  Row zero is the high-code (white) edge;
/// the final row is the low-code (black) edge.  Values are row-major counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LumaWaveform {
    pub columns: u16,
    pub rows: u16,
    pub density: Vec<u64>,
}

/// One channel of an RGB parade density grid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ParadeChannel {
    pub density: Vec<u64>,
}

/// RGB parade density grids.  Each channel uses the same configured width and
/// height and is kept separate so consumers cannot confuse channel bands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RgbParade {
    pub columns: u16,
    pub rows: u16,
    pub red: ParadeChannel,
    pub green: ParadeChannel,
    pub blue: ParadeChannel,
}

/// Vectorscope chroma density.  The square's centre is neutral chroma; row
/// zero is positive green-axis chroma and the final row is negative chroma.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VectorscopeDensity {
    pub size: u16,
    pub density: Vec<u64>,
}

/// Complete immutable scope evidence for one or more explicitly identified
/// project frames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScopeEvidence {
    pub metadata: ScopeMeasurementMetadata,
    pub statistics: ScopeStatistics,
    pub clipping: ScopeClipping,
    pub histograms: ScopeHistograms,
    pub waveform: LumaWaveform,
    pub parade: RgbParade,
    pub vectorscope: VectorscopeDensity,
}

impl ScopeEvidence {
    /// Compare this result with a reference result without mutating either.
    ///
    /// # Errors
    ///
    /// Returns [`ScopeComparisonError`] when the two results use incompatible
    /// stages, ROIs, output dimensions, or unrepresentable counts.
    pub fn compare(&self, candidate: &Self) -> Result<ScopeComparison, ScopeComparisonError> {
        compare_scope_evidence(self, candidate)
    }
}

/// Typed scope-engine failures, including hostile-input and overflow cases.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ScopeError {
    #[error("scope stage {stage:?} is not currently renderable")]
    UnsupportedStage { stage: ScopeStage },
    #[error("invalid normalized ROI: {0}")]
    InvalidRoi(#[source] NormalizedRoiError),
    #[error("invalid {field} resolution {requested}; allowed range is 1..={maximum}")]
    InvalidResolution {
        field: &'static str,
        requested: u32,
        maximum: u32,
    },
    #[error("image dimensions must be non-zero, got {width}x{height}")]
    InvalidImageDimensions { width: u32, height: u32 },
    #[error("RGBA pixel-buffer length overflow for {width}x{height}")]
    PixelBufferLengthOverflow { width: u32, height: u32 },
    #[error("RGBA pixel-buffer length mismatch: expected {expected} bytes, got {actual}")]
    PixelBufferLengthMismatch { expected: u64, actual: usize },
    #[error("no frame samples were supplied")]
    EmptyFrames,
    #[error("temporal frame count {requested} exceeds the CC2 bound of {maximum}")]
    TooManyFrames { requested: usize, maximum: usize },
    #[error("project frame {project_frame} is negative")]
    NegativeProjectFrame { project_frame: i64 },
    #[error("project frame {project_frame} occurs more than once")]
    DuplicateProjectFrame { project_frame: i64 },
    #[error(
        "frame {project_frame} has resolution {actual_width}x{actual_height}; expected {expected_width}x{expected_height}"
    )]
    FrameResolutionMismatch {
        project_frame: i64,
        expected_width: u32,
        expected_height: u32,
        actual_width: u32,
        actual_height: u32,
    },
    #[error("scope output grid arithmetic overflowed while allocating {operation}")]
    ArithmeticOverflow { operation: &'static str },
    #[error("visible sample count overflowed")]
    SampleCountOverflow,
    #[error("transparent sample count overflowed")]
    TransparentCountOverflow,
    #[error("channel sum overflowed")]
    ChannelSumOverflow,
    #[error("ROI pixel count overflowed")]
    RoiPixelCountOverflow,
    #[error("ROI contains no non-transparent pixels")]
    NoVisiblePixels,
}

/// Measure one frame using its explicit project-frame number.
///
/// # Errors
///
/// Returns [`ScopeError`] for invalid requests, malformed RGBA buffers,
/// hostile frame identities, or an ROI with no visible samples.
pub fn measure_scope(
    image: &RgbaImage,
    project_frame: i64,
    request: &ScopeRequest,
) -> Result<ScopeEvidence, ScopeError> {
    measure_scopes(&[ScopeFrame::new(project_frame, image)], request)
}

/// Measure one or more frames into deterministic, immutable evidence.
///
/// Frames may be supplied in any order; they are sorted by project frame and
/// duplicate or negative identities are rejected.  All frames must have the
/// same source resolution.  Samples are concatenated rather than averaging
/// per-frame summaries, so frames with more non-transparent pixels contribute
/// proportionally to the aggregate.
///
/// # Errors
///
/// Returns [`ScopeError`] for invalid requests, malformed RGBA buffers,
/// hostile frame identities, mismatched frame dimensions, overflow, or an ROI
/// with no visible samples.
#[allow(clippy::too_many_lines)]
pub fn measure_scopes(
    frames: &[ScopeFrame<'_>],
    request: &ScopeRequest,
) -> Result<ScopeEvidence, ScopeError> {
    request.validate()?;
    if frames.is_empty() {
        return Err(ScopeError::EmptyFrames);
    }
    if frames.len() > SCOPE_MAX_TEMPORAL_FRAMES {
        return Err(ScopeError::TooManyFrames {
            requested: frames.len(),
            maximum: SCOPE_MAX_TEMPORAL_FRAMES,
        });
    }

    let mut ordered = frames.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|frame| frame.project_frame);
    for pair in ordered.windows(2) {
        if pair[0].project_frame == pair[1].project_frame {
            return Err(ScopeError::DuplicateProjectFrame {
                project_frame: pair[0].project_frame,
            });
        }
    }
    if let Some(frame) = ordered.iter().find(|frame| frame.project_frame < 0) {
        return Err(ScopeError::NegativeProjectFrame {
            project_frame: frame.project_frame,
        });
    }

    let first = ordered[0].image;
    validate_image(first)?;
    let pixel_roi = request
        .roi
        .to_pixels(first.width, first.height)
        .map_err(ScopeError::InvalidRoi)?;
    let roi_pixel_count = pixel_roi
        .pixel_count()
        .ok_or(ScopeError::RoiPixelCountOverflow)?;

    for frame in &ordered {
        validate_image(frame.image)?;
        if frame.image.width != first.width || frame.image.height != first.height {
            return Err(ScopeError::FrameResolutionMismatch {
                project_frame: frame.project_frame,
                expected_width: first.width,
                expected_height: first.height,
                actual_width: frame.image.width,
                actual_height: frame.image.height,
            });
        }
    }

    let config = request.resolution;
    let mut accumulator = Accumulator::new(config)?;
    for frame in &ordered {
        accumulator.add_frame(frame.image, pixel_roi)?;
    }
    if accumulator.visible_pixel_count == 0 {
        return Err(ScopeError::NoVisiblePixels);
    }

    let project_frames = ordered.iter().map(|frame| frame.project_frame).collect();
    let transparent_pixel_count = accumulator.transparent_pixel_count;
    let visible_pixel_count = accumulator.visible_pixel_count;
    Ok(accumulator.finish(ScopeMeasurementMetadata {
        stage: request.stage,
        source_resolution: ScopeRasterResolution {
            width: first.width,
            height: first.height,
        },
        measurement_resolution: ScopeRasterResolution {
            width: pixel_roi.width,
            height: pixel_roi.height,
        },
        full_resolution: true,
        normalized_roi: request.roi,
        pixel_roi,
        project_frames,
        roi_pixel_count: roi_pixel_count
            .checked_mul(
                u64::try_from(ordered.len()).map_err(|_| ScopeError::RoiPixelCountOverflow)?,
            )
            .ok_or(ScopeError::RoiPixelCountOverflow)?,
        transparent_pixel_count,
        visible_pixel_count,
    }))
}

fn validate_image(image: &RgbaImage) -> Result<(), ScopeError> {
    if image.width == 0 || image.height == 0 {
        return Err(ScopeError::InvalidImageDimensions {
            width: image.width,
            height: image.height,
        });
    }
    let pixel_count = u64::from(image.width)
        .checked_mul(u64::from(image.height))
        .ok_or(ScopeError::PixelBufferLengthOverflow {
            width: image.width,
            height: image.height,
        })?;
    let expected = pixel_count
        .checked_mul(4)
        .ok_or(ScopeError::PixelBufferLengthOverflow {
            width: image.width,
            height: image.height,
        })?;
    if usize::try_from(expected).ok() != Some(image.pixels.len()) {
        return Err(ScopeError::PixelBufferLengthMismatch {
            expected,
            actual: image.pixels.len(),
        });
    }
    Ok(())
}

struct Accumulator {
    resolution: ScopeResolution,
    histograms: ScopeHistograms,
    exact: [[u64; 256]; 4],
    sums: [u128; 4],
    clipping_black: [u64; 4],
    clipping_white: [u64; 4],
    waveform: Vec<u64>,
    parade_red: Vec<u64>,
    parade_green: Vec<u64>,
    parade_blue: Vec<u64>,
    vectorscope: Vec<u64>,
    transparent_pixel_count: u64,
    visible_pixel_count: u64,
}

impl Accumulator {
    fn new(resolution: ScopeResolution) -> Result<Self, ScopeError> {
        let histogram_len = usize::from(resolution.histogram_bins);
        let waveform_len = checked_grid_len(
            u32::from(resolution.waveform_columns),
            u32::from(resolution.waveform_rows),
            "waveform",
        )?;
        let parade_len = checked_grid_len(
            u32::from(resolution.parade_columns),
            u32::from(resolution.parade_rows),
            "parade",
        )?;
        let vectorscope_len = checked_grid_len(
            u32::from(resolution.vectorscope_size),
            u32::from(resolution.vectorscope_size),
            "vectorscope",
        )?;
        Ok(Self {
            resolution,
            histograms: ScopeHistograms {
                bins: resolution.histogram_bins,
                red: vec![0; histogram_len],
                green: vec![0; histogram_len],
                blue: vec![0; histogram_len],
                luma: vec![0; histogram_len],
            },
            exact: [[0; 256]; 4],
            sums: [0; 4],
            clipping_black: [0; 4],
            clipping_white: [0; 4],
            waveform: vec![0; waveform_len],
            parade_red: vec![0; parade_len],
            parade_green: vec![0; parade_len],
            parade_blue: vec![0; parade_len],
            vectorscope: vec![0; vectorscope_len],
            transparent_pixel_count: 0,
            visible_pixel_count: 0,
        })
    }

    #[allow(clippy::too_many_lines)]
    fn add_frame(&mut self, image: &RgbaImage, roi: PixelRoi) -> Result<(), ScopeError> {
        let image_width =
            usize::try_from(image.width).map_err(|_| ScopeError::ArithmeticOverflow {
                operation: "image width",
            })?;
        let roi_right = roi.right();
        let roi_bottom = roi.bottom();
        for y in roi.y..roi_bottom {
            let row_start = usize::try_from(y)
                .ok()
                .and_then(|row| row.checked_mul(image_width))
                .and_then(|pixel| pixel.checked_mul(4))
                .ok_or(ScopeError::ArithmeticOverflow {
                    operation: "image row offset",
                })?;
            for x in roi.x..roi_right {
                let pixel_offset = usize::try_from(x)
                    .ok()
                    .and_then(|column| column.checked_mul(4))
                    .and_then(|offset| row_start.checked_add(offset))
                    .ok_or(ScopeError::ArithmeticOverflow {
                        operation: "image pixel offset",
                    })?;
                let pixel = image.pixels.get(pixel_offset..pixel_offset + 4).ok_or(
                    ScopeError::PixelBufferLengthMismatch {
                        expected: u64::from(image.width)
                            .saturating_mul(u64::from(image.height))
                            .saturating_mul(4),
                        actual: image.pixels.len(),
                    },
                )?;
                let [red, green, blue, alpha] = [pixel[0], pixel[1], pixel[2], pixel[3]];
                if alpha == 0 {
                    self.transparent_pixel_count = self
                        .transparent_pixel_count
                        .checked_add(1)
                        .ok_or(ScopeError::TransparentCountOverflow)?;
                    continue;
                }
                let luma = luma_code(red, green, blue);
                let values = [red, green, blue, luma];
                self.visible_pixel_count = self
                    .visible_pixel_count
                    .checked_add(1)
                    .ok_or(ScopeError::SampleCountOverflow)?;
                for (channel, value) in values.into_iter().enumerate() {
                    let scaled = u16::from(value) * 257;
                    self.sums[channel] = self.sums[channel]
                        .checked_add(u128::from(scaled))
                        .ok_or(ScopeError::ChannelSumOverflow)?;
                    self.exact[channel][usize::from(value)] = self.exact[channel]
                        [usize::from(value)]
                    .checked_add(1)
                    .ok_or(ScopeError::SampleCountOverflow)?;
                    if value <= SCOPE_LOW_CLIP_CODE {
                        self.clipping_black[channel] = self.clipping_black[channel]
                            .checked_add(1)
                            .ok_or(ScopeError::SampleCountOverflow)?;
                    }
                    if value >= SCOPE_HIGH_CLIP_CODE {
                        self.clipping_white[channel] = self.clipping_white[channel]
                            .checked_add(1)
                            .ok_or(ScopeError::SampleCountOverflow)?;
                    }
                    let histogram = match channel {
                        0 => &mut self.histograms.red,
                        1 => &mut self.histograms.green,
                        2 => &mut self.histograms.blue,
                        _ => &mut self.histograms.luma,
                    };
                    let bin =
                        usize::from(value) * usize::from(self.resolution.histogram_bins) / 256;
                    histogram[bin] = histogram[bin]
                        .checked_add(1)
                        .ok_or(ScopeError::SampleCountOverflow)?;
                }

                let relative_x = x - roi.x;
                let waveform_column =
                    scaled_index(relative_x, roi.width, self.resolution.waveform_columns);
                let waveform_row = inverted_code_index(luma, self.resolution.waveform_rows);
                increment_grid(
                    &mut self.waveform,
                    waveform_column,
                    waveform_row,
                    self.resolution.waveform_columns,
                )?;
                let parade_column =
                    scaled_index(relative_x, roi.width, self.resolution.parade_columns);
                let parade_rows = self.resolution.parade_rows;
                let parade_red_row = inverted_code_index(red, parade_rows);
                let parade_green_row = inverted_code_index(green, parade_rows);
                let parade_blue_row = inverted_code_index(blue, parade_rows);
                increment_grid(
                    &mut self.parade_red,
                    parade_column,
                    parade_red_row,
                    self.resolution.parade_columns,
                )?;
                increment_grid(
                    &mut self.parade_green,
                    parade_column,
                    parade_green_row,
                    self.resolution.parade_columns,
                )?;
                increment_grid(
                    &mut self.parade_blue,
                    parade_column,
                    parade_blue_row,
                    self.resolution.parade_columns,
                )?;

                let (u, v) = vectorscope_coordinates(red, green, blue);
                let vector_x = signed_chroma_index(u, self.resolution.vectorscope_size);
                let vector_y = signed_chroma_index(-v, self.resolution.vectorscope_size);
                increment_grid(
                    &mut self.vectorscope,
                    vector_x,
                    vector_y,
                    self.resolution.vectorscope_size,
                )?;
            }
        }
        Ok(())
    }

    fn finish(self, metadata: ScopeMeasurementMetadata) -> ScopeEvidence {
        let statistics = ScopeStatistics {
            red: self.channel_statistics(0),
            green: self.channel_statistics(1),
            blue: self.channel_statistics(2),
            luma: self.channel_statistics(3),
        };
        let clipping = ScopeClipping {
            red: clipping(
                self.clipping_black[0],
                self.clipping_white[0],
                self.visible_pixel_count,
            ),
            green: clipping(
                self.clipping_black[1],
                self.clipping_white[1],
                self.visible_pixel_count,
            ),
            blue: clipping(
                self.clipping_black[2],
                self.clipping_white[2],
                self.visible_pixel_count,
            ),
            luma: clipping(
                self.clipping_black[3],
                self.clipping_white[3],
                self.visible_pixel_count,
            ),
        };
        ScopeEvidence {
            metadata,
            statistics,
            clipping,
            histograms: self.histograms,
            waveform: LumaWaveform {
                columns: self.resolution.waveform_columns,
                rows: self.resolution.waveform_rows,
                density: self.waveform,
            },
            parade: RgbParade {
                columns: self.resolution.parade_columns,
                rows: self.resolution.parade_rows,
                red: ParadeChannel {
                    density: self.parade_red,
                },
                green: ParadeChannel {
                    density: self.parade_green,
                },
                blue: ParadeChannel {
                    density: self.parade_blue,
                },
            },
            vectorscope: VectorscopeDensity {
                size: self.resolution.vectorscope_size,
                density: self.vectorscope,
            },
        }
    }

    fn channel_statistics(&self, channel: usize) -> ChannelStatistics {
        let count = self.visible_pixel_count;
        let denominator = u128::from(count) * u128::from(SCOPE_SAMPLE_SCALE);
        let mean =
            (self.sums[channel] * u128::from(SCOPE_MEAN_SCALE) + denominator / 2) / denominator;
        ChannelStatistics {
            mean: u32::try_from(mean).unwrap_or(u32::MAX),
            first_percentile: percentile_code(&self.exact[channel], count, 1),
            median: percentile_code(&self.exact[channel], count, 50),
            ninety_ninth_percentile: percentile_code(&self.exact[channel], count, 99),
        }
    }
}

fn luma_code(red: u8, green: u8, blue: u8) -> u8 {
    let weighted = 54_u32 * u32::from(red) + 183_u32 * u32::from(green) + 19_u32 * u32::from(blue);
    u8::try_from(weighted / 256).unwrap_or(u8::MAX)
}

fn percentile_code(histogram: &[u64; 256], count: u64, percentile: u64) -> u16 {
    let rank = (u128::from(count) * u128::from(percentile)).div_ceil(100);
    let mut cumulative = 0_u128;
    for (value, frequency) in histogram.iter().copied().enumerate() {
        cumulative += u128::from(frequency);
        if cumulative >= rank {
            return u16::try_from(value * 257).unwrap_or(u16::MAX);
        }
    }
    u16::MAX
}

fn clipping(black: u64, white: u64, count: u64) -> ClippingBasisPoints {
    let basis_points = |value: u64| {
        u32::try_from((u128::from(value) * u128::from(SCOPE_BASIS_POINTS)) / u128::from(count))
            .unwrap_or(u32::MAX)
    };
    ClippingBasisPoints {
        black: basis_points(black),
        white: basis_points(white),
    }
}

fn scaled_index(position: u32, extent: u32, bins: u16) -> usize {
    usize::try_from(
        (u64::from(position) * u64::from(bins) / u64::from(extent)).min(u64::from(bins) - 1),
    )
    .unwrap_or(usize::MAX)
}

fn inverted_code_index(value: u8, rows: u16) -> usize {
    if rows == 1 {
        return 0;
    }
    (u64::from(255_u8.saturating_sub(value)) * u64::from(rows - 1) / 255) as usize
}

fn signed_chroma_index(value: i32, size: u16) -> usize {
    if size == 1 {
        return 0;
    }
    let value = value.clamp(-255, 255) + 255;
    (u64::try_from(value).unwrap_or(0) * u64::from(size - 1) / 510) as usize
}

fn vectorscope_coordinates(red: u8, green: u8, blue: u8) -> (i32, i32) {
    // U is blue-minus-red and V is green-minus the red/blue midpoint.  Both
    // are integer chroma axes centred at zero; the exact mapping is part of
    // the CC2 brief and intentionally avoids a platform-dependent float.
    let u = i32::from(blue) - i32::from(red);
    let v = 2 * i32::from(green) - i32::from(red) - i32::from(blue);
    (u, v)
}

fn increment_grid(grid: &mut [u64], x: usize, y: usize, width: u16) -> Result<(), ScopeError> {
    let index = y
        .checked_mul(usize::from(width))
        .and_then(|row| row.checked_add(x))
        .ok_or(ScopeError::ArithmeticOverflow {
            operation: "scope grid index",
        })?;
    let cell = grid.get_mut(index).ok_or(ScopeError::ArithmeticOverflow {
        operation: "scope grid index",
    })?;
    *cell = cell.checked_add(1).ok_or(ScopeError::SampleCountOverflow)?;
    Ok(())
}

/// One signed metric with both recorded endpoints retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SignedDelta {
    pub reference: i64,
    pub candidate: i64,
    /// `candidate - reference`; positive means the candidate is higher.
    pub delta: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChannelStatisticsDelta {
    pub mean: SignedDelta,
    pub first_percentile: SignedDelta,
    pub median: SignedDelta,
    pub ninety_ninth_percentile: SignedDelta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScopeStatisticsDelta {
    pub red: ChannelStatisticsDelta,
    pub green: ChannelStatisticsDelta,
    pub blue: ChannelStatisticsDelta,
    pub luma: ChannelStatisticsDelta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClippingDelta {
    pub black: SignedDelta,
    pub white: SignedDelta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScopeClippingDelta {
    pub red: ClippingDelta,
    pub green: ClippingDelta,
    pub blue: ClippingDelta,
    pub luma: ClippingDelta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScopeHistogramDelta {
    pub bins: u16,
    pub red: Vec<i64>,
    pub green: Vec<i64>,
    pub blue: Vec<i64>,
    pub luma: Vec<i64>,
}

/// Signed row-major density difference for a scope grid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScopeGridDelta {
    pub width: u16,
    pub height: u16,
    pub values: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RgbParadeDelta {
    pub columns: u16,
    pub rows: u16,
    pub red: Vec<i64>,
    pub green: Vec<i64>,
    pub blue: Vec<i64>,
}

/// Reference-vs-candidate evidence with signed, recorded deltas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScopeComparison {
    pub stage: ScopeStage,
    pub reference_frames: Vec<i64>,
    pub candidate_frames: Vec<i64>,
    pub roi: NormalizedRoi,
    pub reference_visible_pixel_count: u64,
    pub candidate_visible_pixel_count: u64,
    pub roi_pixel_count: SignedDelta,
    pub transparent_pixel_count: SignedDelta,
    pub visible_pixel_count: SignedDelta,
    pub statistics: ScopeStatisticsDelta,
    pub clipping: ScopeClippingDelta,
    pub histograms: ScopeHistogramDelta,
    pub waveform: ScopeGridDelta,
    pub parade: RgbParadeDelta,
    pub vectorscope: ScopeGridDelta,
}

/// Errors comparing evidence that was measured under incompatible contracts.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ScopeComparisonError {
    #[error("scope stages differ: reference {reference:?}, candidate {candidate:?}")]
    StageMismatch {
        reference: ScopeStage,
        candidate: ScopeStage,
    },
    #[error("scope ROIs differ")]
    RoiMismatch,
    #[error("scope output dimensions differ for {scope}")]
    ResolutionMismatch { scope: &'static str },
    #[error("signed delta overflowed for {metric}")]
    DeltaOverflow { metric: &'static str },
}

/// Compare reference and candidate evidence.  No correction or grade is
/// generated; this function only records candidate-minus-reference values.
///
/// # Errors
///
/// Returns [`ScopeComparisonError`] when stage, ROI, or output dimensions do
/// not match, or a signed delta cannot represent an endpoint.
#[allow(clippy::too_many_lines)]
pub fn compare_scope_evidence(
    reference: &ScopeEvidence,
    candidate: &ScopeEvidence,
) -> Result<ScopeComparison, ScopeComparisonError> {
    if reference.metadata.stage != candidate.metadata.stage {
        return Err(ScopeComparisonError::StageMismatch {
            reference: reference.metadata.stage,
            candidate: candidate.metadata.stage,
        });
    }
    if reference.metadata.normalized_roi != candidate.metadata.normalized_roi {
        return Err(ScopeComparisonError::RoiMismatch);
    }
    if reference.histograms.bins != candidate.histograms.bins
        || reference.histograms.red.len() != candidate.histograms.red.len()
        || reference.histograms.green.len() != candidate.histograms.green.len()
        || reference.histograms.blue.len() != candidate.histograms.blue.len()
        || reference.histograms.luma.len() != candidate.histograms.luma.len()
    {
        return Err(ScopeComparisonError::ResolutionMismatch { scope: "histogram" });
    }
    if reference.waveform.columns != candidate.waveform.columns
        || reference.waveform.rows != candidate.waveform.rows
        || reference.waveform.density.len() != candidate.waveform.density.len()
    {
        return Err(ScopeComparisonError::ResolutionMismatch { scope: "waveform" });
    }
    if reference.parade.columns != candidate.parade.columns
        || reference.parade.rows != candidate.parade.rows
        || reference.parade.red.density.len() != candidate.parade.red.density.len()
        || reference.parade.green.density.len() != candidate.parade.green.density.len()
        || reference.parade.blue.density.len() != candidate.parade.blue.density.len()
    {
        return Err(ScopeComparisonError::ResolutionMismatch { scope: "parade" });
    }
    if reference.vectorscope.size != candidate.vectorscope.size
        || reference.vectorscope.density.len() != candidate.vectorscope.density.len()
    {
        return Err(ScopeComparisonError::ResolutionMismatch {
            scope: "vectorscope",
        });
    }

    Ok(ScopeComparison {
        stage: reference.metadata.stage,
        reference_frames: reference.metadata.project_frames.clone(),
        candidate_frames: candidate.metadata.project_frames.clone(),
        roi: reference.metadata.normalized_roi,
        reference_visible_pixel_count: reference.metadata.visible_pixel_count,
        candidate_visible_pixel_count: candidate.metadata.visible_pixel_count,
        roi_pixel_count: signed(
            reference.metadata.roi_pixel_count,
            candidate.metadata.roi_pixel_count,
            "roi_pixel_count",
        )?,
        transparent_pixel_count: signed(
            reference.metadata.transparent_pixel_count,
            candidate.metadata.transparent_pixel_count,
            "transparent_pixel_count",
        )?,
        visible_pixel_count: signed(
            reference.metadata.visible_pixel_count,
            candidate.metadata.visible_pixel_count,
            "visible_pixel_count",
        )?,
        statistics: ScopeStatisticsDelta {
            red: channel_statistics_delta(reference.statistics.red, candidate.statistics.red)?,
            green: channel_statistics_delta(
                reference.statistics.green,
                candidate.statistics.green,
            )?,
            blue: channel_statistics_delta(reference.statistics.blue, candidate.statistics.blue)?,
            luma: channel_statistics_delta(reference.statistics.luma, candidate.statistics.luma)?,
        },
        clipping: ScopeClippingDelta {
            red: clipping_delta(reference.clipping.red, candidate.clipping.red)?,
            green: clipping_delta(reference.clipping.green, candidate.clipping.green)?,
            blue: clipping_delta(reference.clipping.blue, candidate.clipping.blue)?,
            luma: clipping_delta(reference.clipping.luma, candidate.clipping.luma)?,
        },
        histograms: ScopeHistogramDelta {
            bins: reference.histograms.bins,
            red: vector_delta(
                &reference.histograms.red,
                &candidate.histograms.red,
                "red histogram",
            )?,
            green: vector_delta(
                &reference.histograms.green,
                &candidate.histograms.green,
                "green histogram",
            )?,
            blue: vector_delta(
                &reference.histograms.blue,
                &candidate.histograms.blue,
                "blue histogram",
            )?,
            luma: vector_delta(
                &reference.histograms.luma,
                &candidate.histograms.luma,
                "luma histogram",
            )?,
        },
        waveform: grid_delta(
            reference.waveform.columns,
            reference.waveform.rows,
            &reference.waveform.density,
            &candidate.waveform.density,
            "waveform",
        )?,
        parade: RgbParadeDelta {
            columns: reference.parade.columns,
            rows: reference.parade.rows,
            red: vector_delta(
                &reference.parade.red.density,
                &candidate.parade.red.density,
                "red parade",
            )?,
            green: vector_delta(
                &reference.parade.green.density,
                &candidate.parade.green.density,
                "green parade",
            )?,
            blue: vector_delta(
                &reference.parade.blue.density,
                &candidate.parade.blue.density,
                "blue parade",
            )?,
        },
        vectorscope: grid_delta(
            reference.vectorscope.size,
            reference.vectorscope.size,
            &reference.vectorscope.density,
            &candidate.vectorscope.density,
            "vectorscope",
        )?,
    })
}

/// Short alias for application and agent call sites.
///
/// # Errors
///
/// Forwards compatibility and overflow errors from
/// [`compare_scope_evidence`].
pub fn compare_scopes(
    reference: &ScopeEvidence,
    candidate: &ScopeEvidence,
) -> Result<ScopeComparison, ScopeComparisonError> {
    compare_scope_evidence(reference, candidate)
}

fn signed(
    reference: u64,
    candidate: u64,
    metric: &'static str,
) -> Result<SignedDelta, ScopeComparisonError> {
    let reference =
        i64::try_from(reference).map_err(|_| ScopeComparisonError::DeltaOverflow { metric })?;
    let candidate =
        i64::try_from(candidate).map_err(|_| ScopeComparisonError::DeltaOverflow { metric })?;
    let delta = candidate
        .checked_sub(reference)
        .ok_or(ScopeComparisonError::DeltaOverflow { metric })?;
    Ok(SignedDelta {
        reference,
        candidate,
        delta,
    })
}

fn signed_u32(
    reference: u32,
    candidate: u32,
    metric: &'static str,
) -> Result<SignedDelta, ScopeComparisonError> {
    signed(u64::from(reference), u64::from(candidate), metric)
}

fn signed_u16(
    reference: u16,
    candidate: u16,
    metric: &'static str,
) -> Result<SignedDelta, ScopeComparisonError> {
    signed(u64::from(reference), u64::from(candidate), metric)
}

fn channel_statistics_delta(
    reference: ChannelStatistics,
    candidate: ChannelStatistics,
) -> Result<ChannelStatisticsDelta, ScopeComparisonError> {
    Ok(ChannelStatisticsDelta {
        mean: signed_u32(reference.mean, candidate.mean, "mean")?,
        first_percentile: signed_u16(
            reference.first_percentile,
            candidate.first_percentile,
            "first_percentile",
        )?,
        median: signed_u16(reference.median, candidate.median, "median")?,
        ninety_ninth_percentile: signed_u16(
            reference.ninety_ninth_percentile,
            candidate.ninety_ninth_percentile,
            "ninety_ninth_percentile",
        )?,
    })
}

fn clipping_delta(
    reference: ClippingBasisPoints,
    candidate: ClippingBasisPoints,
) -> Result<ClippingDelta, ScopeComparisonError> {
    Ok(ClippingDelta {
        black: signed_u32(reference.black, candidate.black, "black_clipping")?,
        white: signed_u32(reference.white, candidate.white, "white_clipping")?,
    })
}

fn vector_delta(
    reference: &[u64],
    candidate: &[u64],
    metric: &'static str,
) -> Result<Vec<i64>, ScopeComparisonError> {
    if reference.len() != candidate.len() {
        return Err(ScopeComparisonError::ResolutionMismatch { scope: metric });
    }
    reference
        .iter()
        .copied()
        .zip(candidate.iter().copied())
        .map(|(reference, candidate)| signed(reference, candidate, metric).map(|value| value.delta))
        .collect()
}

fn grid_delta(
    width: u16,
    height: u16,
    reference: &[u64],
    candidate: &[u64],
    metric: &'static str,
) -> Result<ScopeGridDelta, ScopeComparisonError> {
    Ok(ScopeGridDelta {
        width,
        height,
        values: vector_delta(reference, candidate, metric)?,
    })
}

impl fmt::Display for ScopeStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32, pixels: &[[u8; 4]]) -> RgbaImage {
        assert_eq!(pixels.len(), usize::try_from(width * height).unwrap());
        RgbaImage {
            width,
            height,
            pixels: pixels.iter().flatten().copied().collect(),
        }
    }

    fn tiny_resolution() -> ScopeResolution {
        ScopeResolution::new(4, 4, 4, 4, 4, 4).unwrap()
    }

    fn request() -> ScopeRequest {
        ScopeRequest {
            stage: ScopeStage::MonitoringPostComposite,
            roi: NormalizedRoi::full_frame(),
            resolution: tiny_resolution(),
        }
    }

    #[test]
    fn primaries_and_transparent_pixels_are_measured_deterministically() {
        let frame = image(
            4,
            1,
            &[
                [255, 0, 0, 255],
                [0, 255, 0, 255],
                [0, 0, 255, 255],
                [255, 255, 255, 0],
            ],
        );
        let evidence = measure_scope(&frame, 10, &request()).unwrap();
        assert_eq!(evidence.metadata.project_frames, vec![10]);
        assert_eq!(evidence.metadata.roi_pixel_count, 4);
        assert_eq!(evidence.metadata.transparent_pixel_count, 1);
        assert_eq!(evidence.metadata.visible_pixel_count, 3);
        assert_eq!(evidence.statistics.red.first_percentile, 0);
        assert_eq!(evidence.statistics.red.ninety_ninth_percentile, 65_535);
        assert_eq!(evidence.histograms.red.iter().sum::<u64>(), 3);
        assert_eq!(evidence.histograms.green.iter().sum::<u64>(), 3);
        assert_eq!(evidence.histograms.blue.iter().sum::<u64>(), 3);
        assert_eq!(evidence.histograms.luma.iter().sum::<u64>(), 3);
        assert_eq!(evidence.clipping.red.white, 3_333);
        assert_eq!(evidence.clipping.luma.black, 0);
        assert_eq!(evidence.vectorscope.density.iter().sum::<u64>(), 3);
    }

    #[test]
    fn monotone_ramp_populates_histogram_and_waveform_in_code_order() {
        let frame = image(
            4,
            1,
            &[
                [0, 0, 0, 255],
                [64, 64, 64, 255],
                [128, 128, 128, 255],
                [255, 255, 255, 255],
            ],
        );
        let evidence = measure_scope(&frame, 3, &request()).unwrap();
        assert_eq!(evidence.statistics.luma.first_percentile, 0);
        assert_eq!(evidence.statistics.luma.median, 64 * 257);
        assert_eq!(evidence.statistics.luma.ninety_ninth_percentile, 65_535);
        assert_eq!(evidence.histograms.luma, vec![1, 1, 1, 1]);
        let occupied_rows = evidence
            .waveform
            .density
            .chunks(usize::from(evidence.waveform.columns))
            .enumerate()
            .filter(|(_, row)| row.iter().any(|count| *count != 0))
            .map(|(row, _)| row)
            .collect::<Vec<_>>();
        assert!(occupied_rows.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(occupied_rows.len(), 4);
    }

    #[test]
    fn normalized_roi_uses_floor_start_and_ceil_end() {
        let roi = NormalizedRoi::new(2_500, 0, 2_500, 10_000);
        assert_eq!(
            roi.to_pixels(4, 1).unwrap(),
            PixelRoi {
                x: 1,
                y: 0,
                width: 1,
                height: 1
            }
        );
        let boundary = NormalizedRoi::new(2_501, 0, 2_499, 10_000);
        assert_eq!(
            boundary.to_pixels(4, 1).unwrap(),
            PixelRoi {
                x: 1,
                y: 0,
                width: 1,
                height: 1
            }
        );
        assert_eq!(
            NormalizedRoi::new(0, 0, 0, 10_000).validate(),
            Err(NormalizedRoiError::Empty)
        );
    }

    #[test]
    fn temporal_frames_sort_by_project_identity_and_concatenate_samples() {
        let dark = image(1, 1, &[[0, 0, 0, 255]]);
        let bright = image(1, 1, &[[255, 255, 255, 255]]);
        let frames = [ScopeFrame::new(20, &bright), ScopeFrame::new(4, &dark)];
        let evidence = measure_scopes(&frames, &request()).unwrap();
        assert_eq!(evidence.metadata.project_frames, vec![4, 20]);
        assert_eq!(evidence.statistics.luma.median, 0);
        assert_eq!(evidence.statistics.luma.ninety_ninth_percentile, 65_535);
        assert_eq!(evidence.histograms.luma.iter().sum::<u64>(), 2);
    }

    #[test]
    fn comparison_records_candidate_minus_reference_sign() {
        let dark = image(1, 1, &[[32, 32, 32, 255]]);
        let bright = image(1, 1, &[[96, 96, 96, 255]]);
        let reference = measure_scope(&dark, 0, &request()).unwrap();
        let candidate = measure_scope(&bright, 1, &request()).unwrap();
        let comparison = compare_scopes(&reference, &candidate).unwrap();
        assert!(comparison.statistics.luma.mean.delta > 0);
        assert_eq!(
            comparison.statistics.luma.mean.delta,
            comparison.statistics.luma.mean.candidate - comparison.statistics.luma.mean.reference
        );
        assert_eq!(comparison.roi_pixel_count.delta, 0);
        assert_eq!(comparison.transparent_pixel_count.delta, 0);
        assert_eq!(comparison.reference_frames, vec![0]);
        assert_eq!(comparison.candidate_frames, vec![1]);
    }

    #[test]
    fn hostile_images_and_empty_measurements_fail_closed() {
        let malformed = RgbaImage {
            width: 2,
            height: 2,
            pixels: vec![0; 3],
        };
        assert!(matches!(
            measure_scope(&malformed, 0, &request()),
            Err(ScopeError::PixelBufferLengthMismatch { .. })
        ));
        let empty = RgbaImage {
            width: 1,
            height: 1,
            pixels: vec![[0, 0, 0, 0]].into_iter().flatten().collect(),
        };
        assert_eq!(
            measure_scope(&empty, 0, &request()),
            Err(ScopeError::NoVisiblePixels)
        );
        assert_eq!(
            measure_scopes(&[], &request()),
            Err(ScopeError::EmptyFrames)
        );
        assert!(matches!(
            measure_scope(&image(1, 1, &[[0, 0, 0, 255]]), -1, &request()),
            Err(ScopeError::NegativeProjectFrame { .. })
        ));
    }

    #[test]
    fn bounds_reject_zero_and_oversized_scope_grids() {
        let mut resolution = tiny_resolution();
        resolution.waveform_columns = 0;
        assert!(matches!(
            resolution.validate(),
            Err(ScopeError::InvalidResolution {
                field: "waveform_columns",
                ..
            })
        ));
        resolution.waveform_columns = SCOPE_MAX_WAVEFORM_COLUMNS + 1;
        assert!(matches!(
            resolution.validate(),
            Err(ScopeError::InvalidResolution {
                field: "waveform_columns",
                ..
            })
        ));
    }

    #[test]
    fn temporal_frame_bound_rejects_hostile_request_size() {
        let frame = image(1, 1, &[[1, 2, 3, 255]]);
        let frames = (0..=SCOPE_MAX_TEMPORAL_FRAMES)
            .map(|project_frame| ScopeFrame::new(i64::try_from(project_frame).unwrap(), &frame))
            .collect::<Vec<_>>();
        assert_eq!(
            measure_scopes(&frames, &request()),
            Err(ScopeError::TooManyFrames {
                requested: SCOPE_MAX_TEMPORAL_FRAMES + 1,
                maximum: SCOPE_MAX_TEMPORAL_FRAMES,
            })
        );
    }

    #[test]
    fn public_evidence_round_trips_through_json() {
        let frame = image(2, 1, &[[10, 20, 30, 255], [200, 210, 220, 255]]);
        let evidence = measure_scope(&frame, 7, &request()).unwrap();
        let wire = serde_json::to_string(&evidence).unwrap();
        let decoded: ScopeEvidence = serde_json::from_str(&wire).unwrap();
        assert_eq!(decoded, evidence);
    }
}
