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

use crate::{ClipId, EffectId, RgbaImage};

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
///
/// The engine measures 8-bit input, so at most 256 bins can ever be
/// non-empty.  Allowing more would silently produce permanently dead bins and
/// break the CC2 guarantee that code 255 lands in the final bin, so a larger
/// request is a typed [`ScopeError::InvalidResolution`] instead.
pub const SCOPE_MAX_HISTOGRAM_BINS: u16 = 256;
/// Maximum number of horizontal samples in the luma waveform or parade.
pub const SCOPE_MAX_WAVEFORM_COLUMNS: u16 = 2_048;
/// Maximum number of rows in the luma waveform or RGB parade.
///
/// Rows map from 8-bit codes, so at most 256 rows can ever be non-empty.
/// Allowing more would silently produce permanently dead rows and break the
/// CC2 guarantee that each code lands in its own row, so a larger request is a
/// typed [`ScopeError::InvalidResolution`] instead.  Columns are unaffected:
/// they map from ROI width, not from codes.
pub const SCOPE_MAX_WAVEFORM_ROWS: u16 = 256;
/// Maximum side length of the square vectorscope density grid.
///
/// Both chroma axes carry the 511 representable values `-255..=255`, so at
/// most 511 cells per axis can ever be non-empty.  Allowing more would
/// silently produce permanently dead cells, so a larger request is a typed
/// [`ScopeError::InvalidResolution`] instead.
pub const SCOPE_MAX_VECTORSCOPE_SIZE: u16 = 511;
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

/// The pinned matte-scoping threshold token: a pixel is in the measured
/// population when its coverage is greater than zero.
///
/// `m > 0` is the set the correction touched at all, which is exactly the set
/// the CC5 containment gate measures.  A `m >= 0.5` threshold would silently
/// discard half of every feather band and make a scope disagree with that
/// gate, so the threshold is a constant rather than a parameter.
pub const MATTE_SCOPE_THRESHOLD: &str = "coverage_greater_than_zero";

/// The matte a measurement was scoped to.
///
/// A matte-scoped measurement is still taken at
/// [`ScopeStage::MonitoringPostComposite`]; only the measured *region*
/// changes.  `ScopeStage` is deliberately not extended: a matte is not a
/// pipeline boundary, and a new stage value would make stage equality in
/// [`compare_scope_evidence`] mean two different things at once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MatteRegionDescription {
    /// The clip carrying the matte-scoping colour node.
    pub clip: ClipId,
    /// The matte-carrying colour node's effect identity.
    pub effect: EffectId,
    /// Always [`MATTE_SCOPE_THRESHOLD`].
    pub threshold: String,
    /// Coverage pixels above the threshold in the scoping matte.
    pub covered_pixel_count: u64,
}

impl MatteRegionDescription {
    /// Describe a matte region with the pinned [`MATTE_SCOPE_THRESHOLD`].
    #[must_use]
    pub fn new(clip: ClipId, effect: EffectId, covered_pixel_count: u64) -> Self {
        Self {
            clip,
            effect,
            threshold: MATTE_SCOPE_THRESHOLD.to_owned(),
            covered_pixel_count,
        }
    }
}

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
        // Defence in depth: unreachable given a validated ROI.  `validate`
        // guarantees `width_basis_points >= 1`, and the ceil applied to the
        // exclusive end therefore always lands at least one pixel past the
        // floored start, so `right >= x + 1`.  Retained so a future change to
        // the rounding rules fails with a typed error instead of allocating a
        // zero-area measurement.
        if right <= x || bottom <= y {
            return Err(NormalizedRoiError::EmptyAfterPixelConversion);
        }
        // Defence in depth: unreachable given a validated ROI.  `validate`
        // guarantees `x + width <= 10_000`, so the ceil of
        // `(x + width) * source_width / 10_000` is at most `source_width`.
        // Retained so a rounding or bounds change cannot produce an ROI that
        // reads outside the source raster.
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
        // Defence in depth: unreachable while `ScopeStage` has exactly one
        // variant.  Retained so that adding a pre-grade, effect-scoped, or
        // delivery stage fails closed here instead of silently falling back to
        // monitoring evidence, which the CC2 contract forbids.
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
    /// The matte this measurement was scoped to, when it was matte-scoped.
    ///
    /// Absent from serialized evidence when unset, and defaulted on read, so
    /// evidence recorded before matte scoping existed still loads and still
    /// compares as unscoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub matte_region: Option<MatteRegionDescription>,
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
    /// Returns [`ScopeComparisonError`] when either result is internally
    /// inconsistent or when the two use incompatible stages, ROIs, output
    /// dimensions, or unrepresentable counts.
    pub fn compare(&self, candidate: &Self) -> Result<ScopeComparison, ScopeComparisonError> {
        compare_scope_evidence(self, candidate)
    }

    /// Check that this evidence is internally consistent.
    ///
    /// Every histogram and density vector must contain exactly the number of
    /// cells its own declared dimensions describe.  Freshly measured evidence
    /// always satisfies this, but deserialized evidence is caller-supplied
    /// data: a truncated grid would otherwise compare "successfully" and
    /// produce a delta whose `values` length contradicts its `width` and
    /// `height`.
    ///
    /// # Errors
    ///
    /// Returns [`ScopeComparisonError::MalformedEvidence`] naming the first
    /// field whose length does not match its declared dimensions.
    pub fn validate_shape(&self) -> Result<(), ScopeComparisonError> {
        self.validate_shape_as("evidence")
    }

    fn validate_shape_as(&self, which: &'static str) -> Result<(), ScopeComparisonError> {
        let bins = u64::from(self.histograms.bins);
        let waveform_cells = u64::from(self.waveform.columns) * u64::from(self.waveform.rows);
        let parade_cells = u64::from(self.parade.columns) * u64::from(self.parade.rows);
        let vectorscope_cells = u64::from(self.vectorscope.size) * u64::from(self.vectorscope.size);
        let checks = [
            ("histograms.red", bins, self.histograms.red.len()),
            ("histograms.green", bins, self.histograms.green.len()),
            ("histograms.blue", bins, self.histograms.blue.len()),
            ("histograms.luma", bins, self.histograms.luma.len()),
            (
                "waveform.density",
                waveform_cells,
                self.waveform.density.len(),
            ),
            (
                "parade.red.density",
                parade_cells,
                self.parade.red.density.len(),
            ),
            (
                "parade.green.density",
                parade_cells,
                self.parade.green.density.len(),
            ),
            (
                "parade.blue.density",
                parade_cells,
                self.parade.blue.density.len(),
            ),
            (
                "vectorscope.density",
                vectorscope_cells,
                self.vectorscope.density.len(),
            ),
        ];
        for (field, expected, actual) in checks {
            // A length that cannot be represented as `u64` is by definition
            // not equal to the declared cell count, so saturating is safe.
            let actual = u64::try_from(actual).unwrap_or(u64::MAX);
            if actual != expected {
                return Err(ScopeComparisonError::MalformedEvidence {
                    which,
                    field,
                    expected,
                    actual,
                });
            }
        }
        Ok(())
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
    #[error("scope grid index arithmetic overflowed for the {grid} grid")]
    GridIndexOverflow { grid: &'static str },
    #[error("scope grid index {index} is outside the {length}-cell {grid} grid")]
    GridIndexOutOfRange {
        grid: &'static str,
        index: usize,
        length: usize,
    },
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
    #[error(
        "matte coverage raster {}x{} does not match the measured raster {}x{}",
        observed.width,
        observed.height,
        allowed.width,
        allowed.height
    )]
    MatteRegionRasterMismatch {
        observed: ScopeRasterResolution,
        allowed: ScopeRasterResolution,
    },
}

/// Build the analysis-only frame copy a matte-scoped measurement measures.
///
/// The returned image carries the source RGB unchanged and
/// `A = 255 if m > 0 else 0`, where `m` is the coverage code of a matte proof
/// raster (`R = G = B = round(255 · m)`; the red channel is read).  Because the
/// CC2 engine already excludes alpha-zero pixels from every scope and
/// statistic, handing it this copy measures exactly the covered set with no
/// change to the engine, the document, the render, or the layer's own alpha.
///
/// The threshold is the pinned [`MATTE_SCOPE_THRESHOLD`]: a pixel the
/// correction touched at all is in the population.
///
/// # Errors
///
/// Returns [`ScopeError::MatteRegionRasterMismatch`] when the coverage raster
/// does not have the measured frame's dimensions, or the usual image errors
/// for zero dimensions and a pixel buffer whose length contradicts them.
pub fn matte_scoped_frame(rgba: &RgbaImage, coverage: &RgbaImage) -> Result<RgbaImage, ScopeError> {
    validate_image(rgba)?;
    validate_image(coverage)?;
    if rgba.width != coverage.width || rgba.height != coverage.height {
        return Err(ScopeError::MatteRegionRasterMismatch {
            observed: ScopeRasterResolution {
                width: coverage.width,
                height: coverage.height,
            },
            allowed: ScopeRasterResolution {
                width: rgba.width,
                height: rgba.height,
            },
        });
    }
    let mut pixels = rgba.pixels.clone();
    let (scoped, _) = pixels.as_chunks_mut::<4>();
    let (samples, _) = coverage.pixels.as_chunks::<4>();
    for (pixel, sample) in scoped.iter_mut().zip(samples) {
        // The coverage raster is grey, so its red channel is `round(255 * m)`.
        pixel[3] = u8::from(sample[0] > 0) * 255;
    }
    Ok(RgbaImage {
        width: rgba.width,
        height: rgba.height,
        pixels,
    })
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

    // `first` was already validated above so the ROI could be rasterized
    // against its dimensions; only the remaining frames still need checking.
    for frame in ordered.iter().skip(1) {
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
        // The engine measures whatever raster it is handed.  Matte scoping is
        // applied by the caller through `matte_scoped_frame`, which is what
        // keeps the CC2 engine unchanged, so the caller also records the
        // region description on the result.
        matte_region: None,
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
                let pixel_end =
                    pixel_offset
                        .checked_add(4)
                        .ok_or(ScopeError::ArithmeticOverflow {
                            operation: "image pixel offset",
                        })?;
                let pixel = image.pixels.get(pixel_offset..pixel_end).ok_or(
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
                    // Defence in depth: unreachable because a `u128` sum of
                    // 16-bit codes cannot overflow before the visible-sample
                    // counter (a `u64`) does.  Retained so a narrower
                    // accumulator type can never wrap a published statistic.
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
                    "waveform",
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
                    "parade red",
                    parade_column,
                    parade_red_row,
                    self.resolution.parade_columns,
                )?;
                increment_grid(
                    &mut self.parade_green,
                    "parade green",
                    parade_column,
                    parade_green_row,
                    self.resolution.parade_columns,
                )?;
                increment_grid(
                    &mut self.parade_blue,
                    "parade blue",
                    parade_column,
                    parade_blue_row,
                    self.resolution.parade_columns,
                )?;

                let (u, v) = vectorscope_coordinates(red, green, blue);
                let vector_x = signed_chroma_index(u, self.resolution.vectorscope_size);
                let vector_y = signed_chroma_index(-v, self.resolution.vectorscope_size);
                increment_grid(
                    &mut self.vectorscope,
                    "vectorscope",
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
    debug_assert!(count > 0, "percentiles require at least one visible sample");
    let rank = (u128::from(count) * u128::from(percentile)).div_ceil(100);
    let mut cumulative = 0_u128;
    for (value, frequency) in histogram.iter().copied().enumerate() {
        cumulative += u128::from(frequency);
        if cumulative >= rank {
            // `value` indexes a `[_; 256]`, so `value * 257 <= 65_535`.
            return u16::try_from(value * 257).unwrap_or(u16::MAX);
        }
    }
    // Unreachable: `histogram` is built from exactly the `count` visible
    // samples the caller measured, and `measure_scopes` rejects a measurement
    // with no visible pixels, so `cumulative` reaches `count` on the final
    // iteration.  Percentile ranks are `ceil(count * p / 100)` with `p <= 99`,
    // hence `rank <= count`.  Falling through and returning `u16::MAX` would
    // manufacture a full-white percentile that no pixel produced, so this
    // panics rather than publishing a fabricated statistic.
    unreachable!(
        "percentile rank {rank} exceeds the {count} samples used to build the percentile histogram"
    );
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

/// Map an 8-bit code to a waveform or parade row.
///
/// The 256 possible codes are bucketed uniformly rather than anchored to the
/// endpoints: row zero holds the brightest `256 / rows` codes and the final
/// row the darkest.  An endpoint-anchored mapping would give the black row a
/// single code while every other row received four or five, so a flat ramp
/// looked artificially sparse at the bottom of the scope.  With `rows == 256`
/// this reduces exactly to the identity inversion `255 - value`.
fn inverted_code_index(value: u8, rows: u16) -> usize {
    // `inverted <= 255`, so the quotient is always strictly below `rows`.
    let inverted = usize::from(u8::MAX - value);
    inverted * usize::from(rows) / 256
}

/// Map a signed chroma axis value to a vectorscope column or row.
///
/// The 511 representable axis values (`-255..=255`) are bucketed uniformly
/// across `size` cells.  Neutral chroma therefore lands at `255 * size / 511`,
/// which is the exact centre for an odd `size` and the cell just below centre
/// for an even one, and no end bucket is stretched to cover a single value.
/// U increases left to right and V is negated by the caller so it increases
/// bottom to top.
fn signed_chroma_index(value: i32, size: u16) -> usize {
    // `clamped + 255` is always in `0..=510`, so `unsigned_abs` is exact.
    let shifted = (value.clamp(-255, 255) + 255).unsigned_abs();
    // The quotient is always below `size` (at most 511).  The fallback is
    // unreachable on any supported target, and an out-of-range index would
    // still be rejected by `increment_grid` rather than wrapping.
    usize::try_from(shifted * u32::from(size) / 511).unwrap_or(usize::MAX)
}

fn vectorscope_coordinates(red: u8, green: u8, blue: u8) -> (i32, i32) {
    // U is blue-minus-red and V is green-minus the red/blue midpoint.  Both
    // are integer chroma axes centred at zero; the exact mapping is part of
    // the CC2 brief and intentionally avoids a platform-dependent float.
    let u = i32::from(blue) - i32::from(red);
    let v = 2 * i32::from(green) - i32::from(red) - i32::from(blue);
    (u, v)
}

fn increment_grid(
    grid: &mut [u64],
    name: &'static str,
    x: usize,
    y: usize,
    width: u16,
) -> Result<(), ScopeError> {
    // A failure here is an index computation, not an allocation, so it gets
    // its own variant rather than borrowing the allocation-sizing wording.
    let index = y
        .checked_mul(usize::from(width))
        .and_then(|row| row.checked_add(x))
        .ok_or(ScopeError::GridIndexOverflow { grid: name })?;
    let length = grid.len();
    // Defence in depth: unreachable because both coordinates come from the
    // bounded column/row mappings for this exact grid.  Retained, and reported
    // as an out-of-range index rather than as arithmetic overflow, so a future
    // mapping change is diagnosed accurately instead of being mislabelled.
    let cell = grid.get_mut(index).ok_or(ScopeError::GridIndexOutOfRange {
        grid: name,
        index,
        length,
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
    /// Signed change in the matte-covered population (candidate − reference)
    /// when both sides were measured under a matte region; `None` otherwise.
    /// The count is reported rather than compared because a qualifier matte's
    /// coverage depends on the colour entering the node (CC5 §4.3).
    #[serde(default)]
    pub matte_covered_pixel_delta: Option<SignedDelta>,
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
    #[error("scope matte regions differ: reference {reference:?}, candidate {candidate:?}")]
    MatteRegionMismatch {
        reference: Option<MatteRegionDescription>,
        candidate: Option<MatteRegionDescription>,
    },
    #[error("scope output dimensions differ for {scope}")]
    ResolutionMismatch { scope: &'static str },
    #[error("signed delta overflowed for {metric}")]
    DeltaOverflow { metric: &'static str },
    #[error("{which} evidence field {field} declares {expected} cells but contains {actual}")]
    MalformedEvidence {
        which: &'static str,
        field: &'static str,
        expected: u64,
        actual: u64,
    },
}

/// Compare reference and candidate evidence.  No correction or grade is
/// generated; this function only records candidate-minus-reference values.
///
/// # Errors
///
/// Returns [`ScopeComparisonError`] when either input is internally
/// inconsistent, when stage, ROI, or output dimensions do not match, or when a
/// signed delta cannot represent an endpoint.
#[allow(clippy::too_many_lines)]
pub fn compare_scope_evidence(
    reference: &ScopeEvidence,
    candidate: &ScopeEvidence,
) -> Result<ScopeComparison, ScopeComparisonError> {
    // Compare only self-consistent evidence.  Agreeing on declared dimensions
    // is not enough: either side may have been deserialized with a grid whose
    // length contradicts those dimensions.
    reference.validate_shape_as("reference")?;
    candidate.validate_shape_as("candidate")?;
    if reference.metadata.stage != candidate.metadata.stage {
        return Err(ScopeComparisonError::StageMismatch {
            reference: reference.metadata.stage,
            candidate: candidate.metadata.stage,
        });
    }
    if reference.metadata.normalized_roi != candidate.metadata.normalized_roi {
        return Err(ScopeComparisonError::RoiMismatch);
    }
    // A matte-scoped measurement covers a different population than an
    // unscoped one, exactly as a different ROI does, so the two must not be
    // differenced.  Both sides must be unscoped or name the same matte.
    // The matte region is compared on what was *requested* (clip, effect,
    // threshold), not on the measured covered population: a qualifier matte's
    // coverage is a function of the colour entering the node, so a before/after
    // pair legitimately differs in count. The count difference is reported as
    // a signed delta rather than refusing the comparison.
    let same_region = match (
        &reference.metadata.matte_region,
        &candidate.metadata.matte_region,
    ) {
        (None, None) => true,
        (Some(reference), Some(candidate)) => {
            reference.clip == candidate.clip
                && reference.effect == candidate.effect
                && reference.threshold == candidate.threshold
        }
        _ => false,
    };
    if !same_region {
        return Err(ScopeComparisonError::MatteRegionMismatch {
            reference: reference.metadata.matte_region.clone(),
            candidate: candidate.metadata.matte_region.clone(),
        });
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
        matte_covered_pixel_delta: match (
            &reference.metadata.matte_region,
            &candidate.metadata.matte_region,
        ) {
            (Some(reference), Some(candidate)) => Some(signed(
                reference.covered_pixel_count,
                candidate.covered_pixel_count,
                "matte_covered_pixel_count",
            )?),
            _ => None,
        },
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
        // Hand-built code -> (column, row) table for a 4x4 waveform.  Column
        // is `floor(x * 4 / 4)`; row is `floor((255 - luma) * 4 / 256)`:
        //   luma 0   -> row floor(255 * 4 / 256) = 3, column 0
        //   luma 64  -> row floor(191 * 4 / 256) = 2, column 1
        //   luma 128 -> row floor(127 * 4 / 256) = 1, column 2
        //   luma 255 -> row floor(0   * 4 / 256) = 0, column 3
        assert_eq!(
            evidence.waveform.density,
            vec![
                0, 0, 0, 1, //
                0, 0, 1, 0, //
                0, 1, 0, 0, //
                1, 0, 0, 0, //
            ]
        );
        let occupied_rows = evidence
            .waveform
            .density
            .chunks(usize::from(evidence.waveform.columns))
            .enumerate()
            .filter(|(_, row)| row.iter().any(|count| *count != 0))
            .map(|(row, _)| row)
            .collect::<Vec<_>>();
        assert_eq!(occupied_rows, vec![0, 1, 2, 3]);
        // A neutral ramp puts every parade channel on the same rows.
        assert_eq!(evidence.parade.red.density, evidence.waveform.density);
        assert_eq!(evidence.parade.green.density, evidence.waveform.density);
        assert_eq!(evidence.parade.blue.density, evidence.waveform.density);
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

    /// Rows and vectorscope cells map from 8-bit codes, so their maxima are
    /// the code counts.  Columns map from ROI width and keep the larger cap.
    #[test]
    fn code_derived_grid_maxima_are_the_code_counts() {
        assert_eq!(SCOPE_MAX_WAVEFORM_ROWS, 256);
        assert_eq!(SCOPE_MAX_VECTORSCOPE_SIZE, 511);
        assert_eq!(SCOPE_MAX_WAVEFORM_COLUMNS, 2_048);

        // A row per code is the finest useful waveform or parade; anything
        // beyond that would leave permanently dead rows.
        for (rows, accepted) in [(256_u16, true), (257, false), (1_024, false)] {
            let mut resolution = default_resolution();
            resolution.waveform_rows = rows;
            assert_eq!(
                resolution.validate().is_ok(),
                accepted,
                "waveform_rows {rows}"
            );
            let mut resolution = default_resolution();
            resolution.parade_rows = rows;
            assert_eq!(
                resolution.validate().is_ok(),
                accepted,
                "parade_rows {rows}"
            );
        }

        // Both chroma axes carry 511 values, so 511 is the finest useful side.
        for (size, accepted) in [(511_u16, true), (512, false), (1_024, false)] {
            let mut resolution = default_resolution();
            resolution.vectorscope_size = size;
            assert_eq!(
                resolution.validate().is_ok(),
                accepted,
                "vectorscope_size {size}"
            );
        }

        // Columns are unaffected by the code-count rationale.
        for (columns, accepted) in [(2_048_u16, true), (2_049, false)] {
            let mut resolution = default_resolution();
            resolution.waveform_columns = columns;
            assert_eq!(
                resolution.validate().is_ok(),
                accepted,
                "waveform_columns {columns}"
            );
            let mut resolution = default_resolution();
            resolution.parade_columns = columns;
            assert_eq!(
                resolution.validate().is_ok(),
                accepted,
                "parade_columns {columns}"
            );
        }
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
    fn default_resolution() -> ScopeResolution {
        ScopeResolution::default()
    }

    fn request_with(roi: NormalizedRoi, resolution: ScopeResolution) -> ScopeRequest {
        ScopeRequest {
            stage: ScopeStage::MonitoringPostComposite,
            roi,
            resolution,
        }
    }

    #[test]
    fn waveform_rows_bucket_every_code_uniformly() {
        for rows in [2_u16, 4, 8, 64, 100, SCOPE_MAX_WAVEFORM_ROWS] {
            let mut counts = vec![0_usize; usize::from(rows)];
            for code in 0..=u8::MAX {
                let row = inverted_code_index(code, rows);
                assert!(row < usize::from(rows), "row {row} out of range for {rows}");
                counts[row] += 1;
            }
            assert_eq!(counts.iter().sum::<usize>(), 256);
            let lowest = *counts.iter().min().expect("rows >= 1");
            let highest = *counts.iter().max().expect("rows >= 1");
            assert!(
                highest - lowest <= 1,
                "rows={rows} occupancy is not uniform: {counts:?}"
            );
            // Row zero is white and the final row is black.
            assert_eq!(inverted_code_index(255, rows), 0);
            assert_eq!(inverted_code_index(0, rows), usize::from(rows) - 1);
        }
        // A 256-row scope is exactly the identity inversion.
        for code in 0..=u8::MAX {
            assert_eq!(inverted_code_index(code, 256), usize::from(255 - code));
        }
        // A single-row scope collapses every code onto row zero.
        for code in 0..=u8::MAX {
            assert_eq!(inverted_code_index(code, 1), 0);
        }
    }

    #[test]
    fn vectorscope_axis_buckets_every_value_uniformly() {
        // Hand-computed extremes and neutral cell: floor(510 * size / 511) and
        // floor(255 * size / 511).
        for (size, top, neutral) in [
            (2_u16, 1_usize, 0_usize),
            (4, 3, 1),
            (255, 254, 127),
            (256, 255, 127),
            (SCOPE_MAX_VECTORSCOPE_SIZE, 510, 255),
        ] {
            let mut counts = vec![0_usize; usize::from(size)];
            for value in -255..=255_i32 {
                let index = signed_chroma_index(value, size);
                assert!(
                    index < usize::from(size),
                    "index {index} out of range for {size}"
                );
                counts[index] += 1;
            }
            assert_eq!(counts.iter().sum::<usize>(), 511);
            let lowest = *counts.iter().min().expect("size >= 1");
            let highest = *counts.iter().max().expect("size >= 1");
            assert!(
                highest - lowest <= 1,
                "size={size} occupancy is not uniform: {counts:?}"
            );
            assert_eq!(signed_chroma_index(-255, size), 0);
            assert_eq!(signed_chroma_index(-256, size), 0);
            assert_eq!(signed_chroma_index(255, size), top);
            assert_eq!(signed_chroma_index(256, size), top);
            assert_eq!(signed_chroma_index(0, size), neutral);
        }
        // An odd side length puts neutral chroma on the exact centre cell.
        assert_eq!(signed_chroma_index(0, 511), 255);
        assert_eq!(signed_chroma_index(0, 1), 0);
    }

    #[test]
    fn parade_places_a_primary_row_in_exact_cells() {
        // Four opaque red pixels across a 3-column, 2-row parade.
        //   columns: floor(x * 3 / 4) = 0, 0, 1, 2
        //   rows:    red   255 -> floor(0   * 2 / 256) = 0
        //            green   0 -> floor(255 * 2 / 256) = 1
        //            blue    0 -> floor(255 * 2 / 256) = 1
        let frame = image(4, 1, &[[255, 0, 0, 255]; 4]);
        let probe = request_with(
            NormalizedRoi::full_frame(),
            ScopeResolution::new(4, 3, 2, 3, 2, 4).unwrap(),
        );
        let evidence = measure_scope(&frame, 0, &probe).unwrap();
        assert_eq!(evidence.parade.columns, 3);
        assert_eq!(evidence.parade.rows, 2);
        assert_eq!(evidence.parade.red.density, vec![2, 1, 1, 0, 0, 0]);
        assert_eq!(evidence.parade.green.density, vec![0, 0, 0, 2, 1, 1]);
        assert_eq!(evidence.parade.blue.density, vec![0, 0, 0, 2, 1, 1]);
        // luma = floor(54 * 255 / 256) = 53 -> row floor(202 * 2 / 256) = 1.
        assert_eq!(evidence.waveform.density, vec![0, 0, 0, 2, 1, 1]);
        // Histogram with 4 bins: floor(255 * 4 / 256) = 3, floor(0 * 4 / 256) = 0,
        // floor(53 * 4 / 256) = 0.
        assert_eq!(evidence.histograms.red, vec![0, 0, 0, 4]);
        assert_eq!(evidence.histograms.green, vec![4, 0, 0, 0]);
        assert_eq!(evidence.histograms.blue, vec![4, 0, 0, 0]);
        assert_eq!(evidence.histograms.luma, vec![4, 0, 0, 0]);
        // U = 0 - 255 = -255 -> column 0; V = -255, negated -> row
        // floor(510 * 4 / 511) = 3, so index 3 * 4 + 0 = 12.
        let mut expected_vector = vec![0_u64; 16];
        expected_vector[12] = 4;
        assert_eq!(evidence.vectorscope.density, expected_vector);
        assert_eq!(evidence.statistics.red.mean, 1_000_000);
        assert_eq!(evidence.statistics.green.mean, 0);
        assert_eq!(evidence.clipping.red.white, 10_000);
        assert_eq!(evidence.clipping.green.black, 10_000);
    }

    #[test]
    fn vectorscope_places_primaries_and_secondaries_in_exact_cells() {
        // Hand-computed for a 256-cell side: U = B - R, V = 2G - R - B, both
        // clamped to -255..=255, then index = floor((axis + 255) * 256 / 511)
        // with V negated so positive V is nearer row zero.
        let frame = image(
            8,
            1,
            &[
                [255, 0, 0, 255],     // red:     U=-255 V=-255 -> (0, 255)
                [0, 255, 0, 255],     // green:   U=0    V=+255 -> (127, 0)
                [0, 0, 255, 255],     // blue:    U=+255 V=-255 -> (255, 255)
                [255, 255, 255, 255], // white:   U=0    V=0    -> (127, 127)
                [0, 0, 0, 255],       // black:   U=0    V=0    -> (127, 127)
                [0, 255, 255, 255],   // cyan:    U=+255 V=+255 -> (255, 0)
                [255, 0, 255, 255],   // magenta: U=0    V=-255 -> (127, 255)
                [255, 255, 0, 255],   // yellow:  U=-255 V=+255 -> (0, 0)
            ],
        );
        let probe = request_with(NormalizedRoi::full_frame(), default_resolution());
        let evidence = measure_scope(&frame, 0, &probe).unwrap();
        assert_eq!(evidence.vectorscope.size, 256);
        let cell = |x: usize, y: usize| evidence.vectorscope.density[y * 256 + x];
        assert_eq!(cell(0, 255), 1, "red");
        assert_eq!(cell(127, 0), 1, "green");
        assert_eq!(cell(255, 255), 1, "blue");
        assert_eq!(cell(255, 0), 1, "cyan");
        assert_eq!(cell(127, 255), 1, "magenta");
        assert_eq!(cell(0, 0), 1, "yellow");
        // White and black are both neutral chroma and share the centre cell.
        assert_eq!(cell(127, 127), 2, "neutral");
        assert_eq!(evidence.vectorscope.density.iter().sum::<u64>(), 8);
    }

    #[test]
    fn sub_frame_roi_excludes_outside_pixels_from_every_statistic() {
        // Only the two mid-grey pixels are inside the ROI; the black and white
        // pixels outside it must not reach any statistic, histogram, or clip.
        let frame = image(
            4,
            1,
            &[
                [0, 0, 0, 255],
                [128, 128, 128, 255],
                [128, 128, 128, 255],
                [255, 255, 255, 255],
            ],
        );
        let probe = request_with(
            NormalizedRoi::new(2_500, 0, 5_000, 10_000),
            tiny_resolution(),
        );
        let evidence = measure_scope(&frame, 0, &probe).unwrap();
        assert_eq!(
            evidence.metadata.pixel_roi,
            PixelRoi {
                x: 1,
                y: 0,
                width: 2,
                height: 1
            }
        );
        assert_eq!(
            evidence.metadata.source_resolution,
            ScopeRasterResolution {
                width: 4,
                height: 1
            }
        );
        assert_eq!(
            evidence.metadata.measurement_resolution,
            ScopeRasterResolution {
                width: 2,
                height: 1
            }
        );
        assert_eq!(evidence.metadata.roi_pixel_count, 2);
        assert_eq!(evidence.metadata.visible_pixel_count, 2);
        assert_eq!(evidence.metadata.transparent_pixel_count, 0);
        // 128 * 257 = 32_896; round(128 / 255 * 1_000_000) = 501_961.
        assert_eq!(evidence.statistics.red.mean, 501_961);
        assert_eq!(evidence.statistics.luma.mean, 501_961);
        assert_eq!(evidence.statistics.red.first_percentile, 32_896);
        assert_eq!(evidence.statistics.red.median, 32_896);
        assert_eq!(evidence.statistics.red.ninety_ninth_percentile, 32_896);
        // The excluded pixels are code 0 and code 255, so any leakage would
        // show up as clipping.
        assert_eq!(evidence.clipping.red.black, 0);
        assert_eq!(evidence.clipping.red.white, 0);
        assert_eq!(evidence.clipping.luma.black, 0);
        assert_eq!(evidence.clipping.luma.white, 0);
        // floor(128 * 4 / 256) = 2.
        assert_eq!(evidence.histograms.red, vec![0, 0, 2, 0]);
        assert_eq!(evidence.histograms.luma, vec![0, 0, 2, 0]);
        // Columns are scaled over the ROI extent of 2, not the 4-pixel source:
        // floor(0 * 4 / 2) = 0 and floor(1 * 4 / 2) = 2, both on row
        // floor(127 * 4 / 256) = 1.
        assert_eq!(
            evidence.waveform.density,
            vec![
                0, 0, 0, 0, //
                1, 0, 1, 0, //
                0, 0, 0, 0, //
                0, 0, 0, 0, //
            ]
        );
        // Neutral grey sits on the 4-cell vectorscope centre: floor(255 * 4 / 511) = 1.
        let mut expected_vector = vec![0_u64; 16];
        // Row 1, column 1 of a 4-wide grid.
        expected_vector[5] = 2;
        assert_eq!(evidence.vectorscope.density, expected_vector);
    }

    #[test]
    fn mean_uses_half_up_rounding_of_normalized_millionths() {
        // Codes 1, 1, 2 are 257, 257 and 514 on the 16-bit scale.  Their mean
        // is 1_028 / 3 = 342.666..., and 342.666... / 65_535 * 1_000_000 =
        // 5_228.8..., which rounds half up to 5_229.
        let frame = image(3, 1, &[[1, 1, 1, 255], [1, 1, 1, 255], [2, 2, 2, 255]]);
        let evidence = measure_scope(&frame, 0, &request()).unwrap();
        for channel in [
            evidence.statistics.red,
            evidence.statistics.green,
            evidence.statistics.blue,
            evidence.statistics.luma,
        ] {
            assert_eq!(channel.mean, 5_229);
            // Nearest rank over three samples: ceil(3/100) = 1, ceil(150/100) = 2,
            // ceil(297/100) = 3.
            assert_eq!(channel.first_percentile, 257);
            assert_eq!(channel.median, 257);
            assert_eq!(channel.ninety_ninth_percentile, 514);
        }
        // Two of three samples are code 1, which is inside the black clip
        // band: floor(2 * 10_000 / 3) = 6_666.
        assert_eq!(evidence.clipping.red.black, 6_666);
        assert_eq!(evidence.clipping.red.white, 0);
    }

    #[test]
    fn comparison_records_negative_delta_and_typed_incompatibilities() {
        let bright = image(1, 1, &[[64, 64, 64, 255]]);
        let dark = image(1, 1, &[[32, 32, 32, 255]]);
        let reference = measure_scope(&bright, 0, &request()).unwrap();
        let candidate = measure_scope(&dark, 1, &request()).unwrap();
        let comparison = compare_scopes(&reference, &candidate).unwrap();
        // round(64 / 255 * 1e6) = 250_980 and round(32 / 255 * 1e6) = 125_490.
        assert_eq!(comparison.statistics.luma.mean.reference, 250_980);
        assert_eq!(comparison.statistics.luma.mean.candidate, 125_490);
        assert_eq!(comparison.statistics.luma.mean.delta, -125_490);
        // Percentile codes are 64 * 257 = 16_448 and 32 * 257 = 8_224.
        assert_eq!(comparison.statistics.red.median.reference, 16_448);
        assert_eq!(comparison.statistics.red.median.candidate, 8_224);
        assert_eq!(comparison.statistics.red.median.delta, -8_224);
        assert_eq!(comparison.visible_pixel_count.delta, 0);

        // `ScopeComparisonError::StageMismatch` is not constructible here:
        // `ScopeStage` has exactly one variant, so two measured results can
        // never carry different stages.  The branch is retained for the day a
        // second stage is added; see the note on `ScopeRequest::validate`.

        let wide = image(4, 1, &[[10, 10, 10, 255]; 4]);
        let half = measure_scope(
            &wide,
            0,
            &request_with(NormalizedRoi::new(0, 0, 5_000, 10_000), tiny_resolution()),
        )
        .unwrap();
        let full = measure_scope(
            &wide,
            0,
            &request_with(NormalizedRoi::full_frame(), tiny_resolution()),
        )
        .unwrap();
        assert_eq!(
            compare_scopes(&full, &half),
            Err(ScopeComparisonError::RoiMismatch)
        );

        for (scope, resolution) in [
            ("histogram", ScopeResolution::new(8, 4, 4, 4, 4, 4).unwrap()),
            ("waveform", ScopeResolution::new(4, 8, 4, 4, 4, 4).unwrap()),
            ("parade", ScopeResolution::new(4, 4, 4, 8, 4, 4).unwrap()),
            (
                "vectorscope",
                ScopeResolution::new(4, 4, 4, 4, 4, 8).unwrap(),
            ),
        ] {
            let other = measure_scope(
                &bright,
                0,
                &request_with(NormalizedRoi::full_frame(), resolution),
            )
            .unwrap();
            assert_eq!(
                compare_scopes(&reference, &other),
                Err(ScopeComparisonError::ResolutionMismatch { scope })
            );
        }
    }

    #[test]
    fn comparison_rejects_internally_inconsistent_evidence() {
        let frame = image(2, 1, &[[10, 20, 30, 255], [200, 210, 220, 255]]);
        let evidence = measure_scope(&frame, 0, &request()).unwrap();
        assert_eq!(evidence.validate_shape(), Ok(()));

        // A hostile deserialized result that declares a 4x4 waveform but only
        // carries three cells agrees with a real result on every declared
        // dimension.
        let mut truncated = evidence.clone();
        truncated.waveform.density.truncate(3);
        assert_eq!(
            truncated.validate_shape(),
            Err(ScopeComparisonError::MalformedEvidence {
                which: "evidence",
                field: "waveform.density",
                expected: 16,
                actual: 3,
            })
        );
        assert_eq!(
            compare_scopes(&truncated, &evidence),
            Err(ScopeComparisonError::MalformedEvidence {
                which: "reference",
                field: "waveform.density",
                expected: 16,
                actual: 3,
            })
        );
        assert_eq!(
            compare_scopes(&evidence, &truncated),
            Err(ScopeComparisonError::MalformedEvidence {
                which: "candidate",
                field: "waveform.density",
                expected: 16,
                actual: 3,
            })
        );

        let mut short_histogram = evidence.clone();
        short_histogram.histograms.luma.pop();
        assert_eq!(
            compare_scopes(&short_histogram, &short_histogram),
            Err(ScopeComparisonError::MalformedEvidence {
                which: "reference",
                field: "histograms.luma",
                expected: 4,
                actual: 3,
            })
        );

        let mut inflated_vectorscope = evidence.clone();
        inflated_vectorscope.vectorscope.size = 8;
        assert_eq!(
            compare_scopes(&inflated_vectorscope, &inflated_vectorscope),
            Err(ScopeComparisonError::MalformedEvidence {
                which: "reference",
                field: "vectorscope.density",
                expected: 64,
                actual: 16,
            })
        );

        let mut inflated_parade = evidence.clone();
        inflated_parade.parade.rows = 5;
        assert_eq!(
            inflated_parade.validate_shape(),
            Err(ScopeComparisonError::MalformedEvidence {
                which: "evidence",
                field: "parade.red.density",
                expected: 20,
                actual: 16,
            })
        );
    }

    #[test]
    fn histogram_bins_are_bounded_by_the_eight_bit_code_count() {
        // 8-bit input can never fill more than 256 bins, and the CC2 contract
        // requires code 255 to land in the final bin.
        assert_eq!(SCOPE_MAX_HISTOGRAM_BINS, 256);
        let mut resolution = tiny_resolution();
        resolution.histogram_bins = 257;
        assert_eq!(
            resolution.validate(),
            Err(ScopeError::InvalidResolution {
                field: "histogram_bins",
                requested: 257,
                maximum: 256,
            })
        );
        for bins in [1_u16, 128, 256] {
            resolution.histogram_bins = bins;
            assert_eq!(resolution.validate(), Ok(()));
        }
        assert_eq!(default_resolution().histogram_bins, 256);

        let white = image(1, 1, &[[255, 255, 255, 255]]);
        for bins in [1_u16, 2, 3, 100, 128, 255, 256] {
            let probe = request_with(
                NormalizedRoi::full_frame(),
                ScopeResolution::new(bins, 4, 4, 4, 4, 4).unwrap(),
            );
            let evidence = measure_scope(&white, 0, &probe).unwrap();
            let final_bin = usize::from(bins) - 1;
            assert_eq!(evidence.histograms.bins, bins);
            assert_eq!(evidence.histograms.red.len(), usize::from(bins));
            assert_eq!(
                evidence.histograms.red[final_bin], 1,
                "code 255 must land in the final bin for {bins} bins"
            );
            assert_eq!(
                evidence.histograms.luma[final_bin], 1,
                "luma 255 must land in the final bin for {bins} bins"
            );
            assert_eq!(evidence.histograms.red.iter().sum::<u64>(), 1);
        }
    }

    #[test]
    fn every_resolution_field_rejects_zero_and_one_past_its_maximum() {
        let fields: [(&'static str, u16); 6] = [
            ("histogram_bins", SCOPE_MAX_HISTOGRAM_BINS),
            ("waveform_columns", SCOPE_MAX_WAVEFORM_COLUMNS),
            ("waveform_rows", SCOPE_MAX_WAVEFORM_ROWS),
            ("parade_columns", SCOPE_MAX_WAVEFORM_COLUMNS),
            ("parade_rows", SCOPE_MAX_WAVEFORM_ROWS),
            ("vectorscope_size", SCOPE_MAX_VECTORSCOPE_SIZE),
        ];
        for (field, maximum) in fields {
            for value in [0, maximum + 1] {
                let mut resolution = default_resolution();
                match field {
                    "histogram_bins" => resolution.histogram_bins = value,
                    "waveform_columns" => resolution.waveform_columns = value,
                    "waveform_rows" => resolution.waveform_rows = value,
                    "parade_columns" => resolution.parade_columns = value,
                    "parade_rows" => resolution.parade_rows = value,
                    _ => resolution.vectorscope_size = value,
                }
                assert_eq!(
                    resolution.validate(),
                    Err(ScopeError::InvalidResolution {
                        field,
                        requested: u32::from(value),
                        maximum: u32::from(maximum),
                    }),
                    "{field} = {value} must be rejected"
                );
            }
        }
        assert_eq!(default_resolution().validate(), Ok(()));
    }

    #[test]
    fn typed_errors_cover_hostile_frame_and_buffer_inputs() {
        let one = image(1, 1, &[[8, 8, 8, 255]]);
        let two = image(2, 1, &[[8, 8, 8, 255], [9, 9, 9, 255]]);
        assert_eq!(
            measure_scopes(
                &[ScopeFrame::new(7, &one), ScopeFrame::new(7, &one)],
                &request()
            ),
            Err(ScopeError::DuplicateProjectFrame { project_frame: 7 })
        );
        assert_eq!(
            measure_scopes(
                &[ScopeFrame::new(0, &one), ScopeFrame::new(1, &two)],
                &request()
            ),
            Err(ScopeError::FrameResolutionMismatch {
                project_frame: 1,
                expected_width: 1,
                expected_height: 1,
                actual_width: 2,
                actual_height: 1,
            })
        );
        assert_eq!(
            measure_scope(&one, -3, &request()),
            Err(ScopeError::NegativeProjectFrame { project_frame: -3 })
        );
        let zero_width = RgbaImage {
            width: 0,
            height: 4,
            pixels: Vec::new(),
        };
        assert_eq!(
            measure_scope(&zero_width, 0, &request()),
            Err(ScopeError::InvalidImageDimensions {
                width: 0,
                height: 4
            })
        );
        // u32::MAX * u32::MAX fits in u64, but the four bytes per pixel do not.
        let enormous = RgbaImage {
            width: u32::MAX,
            height: u32::MAX,
            pixels: Vec::new(),
        };
        assert_eq!(
            measure_scope(&enormous, 0, &request()),
            Err(ScopeError::PixelBufferLengthOverflow {
                width: u32::MAX,
                height: u32::MAX,
            })
        );
        assert_eq!(SCOPE_MAX_TEMPORAL_FRAMES, 64);
        let frames = (0..65_i64)
            .map(|project_frame| ScopeFrame::new(project_frame, &one))
            .collect::<Vec<_>>();
        assert_eq!(
            measure_scopes(&frames, &request()),
            Err(ScopeError::TooManyFrames {
                requested: 65,
                maximum: 64,
            })
        );
    }

    #[test]
    fn all_transparent_sub_roi_has_no_visible_pixels() {
        // The opaque pixels sit outside the requested ROI, so the measurement
        // must fail rather than reporting the visible pixels of the frame.
        let frame = image(
            4,
            1,
            &[
                [255, 255, 255, 255],
                [10, 20, 30, 0],
                [40, 50, 60, 0],
                [255, 255, 255, 255],
            ],
        );
        let probe = request_with(
            NormalizedRoi::new(2_500, 0, 5_000, 10_000),
            tiny_resolution(),
        );
        assert_eq!(
            measure_scope(&frame, 0, &probe),
            Err(ScopeError::NoVisiblePixels)
        );
        // The same frame measured over the full raster does have visible data.
        let full = measure_scope(&frame, 0, &request()).unwrap();
        assert_eq!(full.metadata.visible_pixel_count, 2);
        assert_eq!(full.metadata.transparent_pixel_count, 2);
    }

    #[test]
    fn multi_frame_metadata_records_counts_and_resolutions() {
        let later = image(
            2,
            2,
            &[
                [10, 10, 10, 255],
                [20, 20, 20, 255],
                [30, 30, 30, 0],
                [40, 40, 40, 255],
            ],
        );
        let earlier = image(
            2,
            2,
            &[
                [50, 50, 50, 255],
                [60, 60, 60, 0],
                [70, 70, 70, 255],
                [80, 80, 80, 255],
            ],
        );
        let evidence = measure_scopes(
            &[ScopeFrame::new(5, &later), ScopeFrame::new(2, &earlier)],
            &request(),
        )
        .unwrap();
        assert_eq!(evidence.metadata.project_frames, vec![2, 5]);
        // Two frames of four ROI positions each, two of which are transparent.
        assert_eq!(evidence.metadata.roi_pixel_count, 8);
        assert_eq!(evidence.metadata.transparent_pixel_count, 2);
        assert_eq!(evidence.metadata.visible_pixel_count, 6);
        assert!(evidence.metadata.full_resolution);
        assert_eq!(
            evidence.metadata.source_resolution,
            ScopeRasterResolution {
                width: 2,
                height: 2
            }
        );
        assert_eq!(
            evidence.metadata.measurement_resolution,
            ScopeRasterResolution {
                width: 2,
                height: 2
            }
        );
        assert_eq!(
            evidence.metadata.pixel_roi,
            PixelRoi {
                x: 0,
                y: 0,
                width: 2,
                height: 2
            }
        );
        assert_eq!(evidence.histograms.luma.iter().sum::<u64>(), 6);
        assert_eq!(evidence.waveform.density.iter().sum::<u64>(), 6);
        assert_eq!(evidence.parade.red.density.iter().sum::<u64>(), 6);
        assert_eq!(evidence.vectorscope.density.iter().sum::<u64>(), 6);
    }
}
