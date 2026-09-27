//! PF1 X-1…X-5: exact per-code input conversion tables (SDR only).
//!
//! A table entry is today's per-pixel arithmetic run once per 16-bit code,
//! so a lookup is bit-identical to `WorkingFrame::from_rgba64_le` for every
//! descriptor `select_conversion` marks `Separable` (X-2 proves it).

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, LazyLock, Mutex, OnceLock, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
};

use half::f16;
use kinewright_core::{
    ColorBitDepth, ColorDescription, ColorMatrix, ColorRange, ColorSourceError,
    ColorSourceProfileAssumption, ColorTransfer, classify_source_with_assumption,
};

use crate::color_pipeline::{
    ColorPipelineError, decode_transfer, expand_native_range, rgba64_normalization_max,
};

const CODES: usize = 1 << 16;
const REGISTRY_KEYS: usize = 8;

static LIVE_TABLE_BYTES: AtomicUsize = AtomicUsize::new(0);
static ALPHA: LazyLock<Box<[f16]>> = LazyLock::new(|| {
    (0..=u16::MAX)
        .map(|code| f16::from_f32(f32::from(code) / 65_535.0))
        .collect()
});
static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(Mutex::default);

/// Every field that determines `rgba64_normalization_max`, the
/// `expand_native_range` branch and `decode_transfer` (R1).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ConversionKey {
    matrix: ColorMatrix,
    range: ColorRange,
    bit_depth: ColorBitDepth,
    transfer: ColorTransfer,
}

impl ConversionKey {
    pub(crate) fn of(description: &ColorDescription) -> Self {
        Self {
            matrix: description.matrix.clone(),
            range: description.range.clone(),
            bit_depth: description.bit_depth.clone(),
            transfer: description.transfer.clone(),
        }
    }
}

/// Which step of the per-pixel path a descriptor-determined failure hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PixelStage {
    RangeExpansion,
    ColourDecode,
}

/// 65,536 code → f16 entries, or the value-independent error the per-pixel
/// path would raise at its first pixel.
#[derive(Debug)]
pub(crate) struct TransferTable {
    entries: Result<Box<[f16]>, (PixelStage, ColorPipelineError)>,
}

impl TransferTable {
    /// `max` is `rgba64_normalization_max` of any description with this key.
    fn build(key: &ConversionKey, max: u32) -> Self {
        let description = ColorDescription {
            matrix: key.matrix.clone(),
            range: key.range.clone(),
            bit_depth: key.bit_depth.clone(),
            transfer: key.transfer.clone(),
            ..ColorDescription::default()
        };
        let entries = Self::fill(&description, max);
        if let Ok(entries) = &entries {
            LIVE_TABLE_BYTES.fetch_add(entries.len() * 2, Ordering::Relaxed);
        }
        Self { entries }
    }

    fn fill(
        description: &ColorDescription,
        max: u32,
    ) -> Result<Box<[f16]>, (PixelStage, ColorPipelineError)> {
        #[allow(clippy::cast_precision_loss)]
        let rgb_max = max as f32;
        let expand = matches!(description.matrix, ColorMatrix::Rgb | ColorMatrix::Identity)
            && matches!(description.range, ColorRange::Limited);
        let mut entries = Vec::with_capacity(CODES);
        for code in 0..=u16::MAX {
            let mut value = f32::from(code) / rgb_max;
            if expand {
                value = expand_native_range([value; 3], &description.bit_depth, &description.range)
                    .map_err(|error| (PixelStage::RangeExpansion, error))?[0];
            }
            let decoded = decode_transfer(&description.transfer, value)
                .map_err(|error| (PixelStage::ColourDecode, error))?;
            entries.push(f16::from_f32(decoded));
        }
        Ok(entries.into_boxed_slice())
    }

    /// The RGB entries, or the error today's path raises at the first pixel.
    pub(crate) fn entries(&self) -> Result<&[f16], &(PixelStage, ColorPipelineError)> {
        self.entries.as_deref()
    }
}

impl Drop for TransferTable {
    fn drop(&mut self) {
        if let Ok(entries) = &self.entries {
            LIVE_TABLE_BYTES.fetch_sub(entries.len() * 2, Ordering::Relaxed);
        }
    }
}

/// The static alpha table: code / 65,535.
pub(crate) fn alpha_table() -> &'static [f16] {
    &ALPHA
}

/// Bytes of RGB input tables currently held by the registry or any decoder.
/// Read by the PF1 harness; the public `CacheStats` reply is wire data, so
/// S2a's preview `stats` is where it surfaces in production.
#[cfg(test)]
pub(crate) fn live_table_bytes() -> usize {
    LIVE_TABLE_BYTES.load(Ordering::Relaxed)
}

#[derive(Default)]
struct Registry {
    cells: HashMap<ConversionKey, Arc<OnceLock<Arc<TransferTable>>>>,
    recency: VecDeque<ConversionKey>,
}

/// One table per key; LRU drops registry membership only, so a decoder
/// holding an evicted key keeps its `Arc` (X-5).
fn transfer_table(key: &ConversionKey, max: u32) -> Arc<TransferTable> {
    let cell = {
        let mut registry = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
        registry.recency.retain(|held| held != key);
        registry.recency.push_back(key.clone());
        let cell = Arc::clone(registry.cells.entry(key.clone()).or_default());
        while registry.recency.len() > REGISTRY_KEYS {
            if let Some(oldest) = registry.recency.pop_front() {
                registry.cells.remove(&oldest);
            }
        }
        cell
    };
    Arc::clone(cell.get_or_init(|| Arc::new(TransferTable::build(key, max))))
}

/// How a validated source converts to working pixels.
#[derive(Debug, Clone)]
pub(crate) enum Conversion {
    /// Per-channel and SDR: exact tables (X-1).
    Separable(Arc<TransferTable>),
    /// Today's exact f32 per-pixel path.
    PerPixel,
}

/// X-2 production dispatch: classify, normalise, then choose (X-4).
pub(crate) fn select_conversion(
    description: &ColorDescription,
    assumption: Option<ColorSourceProfileAssumption>,
) -> Result<Conversion, ColorSourceError> {
    classify_source_with_assumption(description, assumption)?;
    match rgba64_normalization_max(description) {
        Ok(max) if separable(description) => Ok(Conversion::Separable(transfer_table(
            &ConversionKey::of(description),
            max,
        ))),
        _ => Ok(Conversion::PerPixel),
    }
}

/// SDR, per-channel conversions only (R2); a new transfer or matrix must be
/// placed here explicitly.
fn separable(description: &ColorDescription) -> bool {
    let transfer = match description.transfer {
        ColorTransfer::Srgb | ColorTransfer::Bt709 | ColorTransfer::Bt1886 => true,
        ColorTransfer::Unknown
        | ColorTransfer::Linear
        | ColorTransfer::Gamma22
        | ColorTransfer::Gamma28
        | ColorTransfer::Smpte170M
        | ColorTransfer::Smpte2084
        | ColorTransfer::AribStdB67
        | ColorTransfer::Log
        | ColorTransfer::LogC
        | ColorTransfer::Log3G10
        | ColorTransfer::Other(_) => false,
    };
    let matrix = match description.matrix {
        ColorMatrix::Identity | ColorMatrix::Rgb | ColorMatrix::Bt709 => true,
        ColorMatrix::Unknown
        | ColorMatrix::Bt2020Ncl
        | ColorMatrix::Bt2020Cl
        | ColorMatrix::Smpte170M
        | ColorMatrix::Smpte240M
        | ColorMatrix::Ycgco
        | ColorMatrix::ChromaDerivedNcl
        | ColorMatrix::ChromaDerivedCl
        | ColorMatrix::Ictcp
        | ColorMatrix::Other(_) => false,
    };
    transfer && matrix
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_tables_are_counted_while_held_and_alpha_is_exact() {
        let key = ConversionKey {
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Full,
            bit_depth: ColorBitDepth::Eight,
            transfer: ColorTransfer::Bt709,
        };
        let table = TransferTable::build(&key, 255 << 8);
        let bytes = table.entries().map(<[f16]>::len).expect("builds") * 2;
        assert_eq!(bytes, 128 * 1024);
        assert!(live_table_bytes() >= bytes);
        let failed = TransferTable::build(
            &ConversionKey {
                transfer: ColorTransfer::Unknown,
                ..key
            },
            255 << 8,
        );
        assert_eq!(
            failed.entries().map(<[f16]>::len),
            Err(&(
                PixelStage::ColourDecode,
                ColorPipelineError::UnknownTransfer
            ))
        );
        for (code, value) in (0..=u16::MAX).zip(alpha_table()) {
            let expected = f16::from_f32(f32::from(code) / 65_535.0);
            assert_eq!(value.to_bits(), expected.to_bits());
        }
    }
}
