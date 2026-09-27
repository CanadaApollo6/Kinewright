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
    ColorPipelineError, decode_transfer, encode_monitor_rgba8, expand_native_range,
    rgba64_normalization_max,
};

const CODES: usize = 1 << 16;
const REGISTRY_KEYS: usize = 8;

static LIVE_TABLE_BYTES: AtomicUsize = AtomicUsize::new(0);
static ALPHA: LazyLock<Box<[f16]>> = LazyLock::new(|| {
    (0..=u16::MAX)
        .map(|code| f16::from_f32(f32::from(code) / 65_535.0))
        .collect()
});
static REGISTRY: LazyLock<Registry<TransferTable>> = LazyLock::new(Registry::new);

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
    /// The live-bytes counter this table is charged to (tests isolate it).
    counter: &'static AtomicUsize,
}

impl TransferTable {
    /// `max` is `rgba64_normalization_max` of any description with this key.
    fn build(key: &ConversionKey, max: u32) -> Self {
        Self::build_counted(key, max, &LIVE_TABLE_BYTES)
    }

    fn build_counted(key: &ConversionKey, max: u32, counter: &'static AtomicUsize) -> Self {
        let description = ColorDescription {
            matrix: key.matrix.clone(),
            range: key.range.clone(),
            bit_depth: key.bit_depth.clone(),
            transfer: key.transfer.clone(),
            ..ColorDescription::default()
        };
        let entries = Self::fill(&description, max);
        if let Ok(entries) = &entries {
            counter.fetch_add(entries.len() * 2, Ordering::Relaxed);
        }
        Self { entries, counter }
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
            self.counter.fetch_sub(entries.len() * 2, Ordering::Relaxed);
        }
    }
}

/// PF1 G-1: BT.709 monitor codes per f16 bit pattern, RGB then alpha, each
/// entry computed by `encode_monitor_rgba8`'s own f32 math.
static MONITOR: LazyLock<(Vec<u8>, Vec<u8>)> = LazyLock::new(|| {
    (0..=u16::MAX)
        .map(|bits| {
            let [rgb, _, _, alpha] = encode_monitor_rgba8([f16::from_bits(bits).to_f32(); 4]);
            (rgb, alpha)
        })
        .unzip()
});

/// Exactly `encode_monitor_rgba8` of the f16 working pixel `bits`.
pub(crate) fn monitor_rgba8(bits: [u16; 4]) -> [u8; 4] {
    let (rgb, alpha) = &*MONITOR;
    let [r, g, b, a] = bits.map(usize::from);
    [rgb[r], rgb[g], rgb[b], alpha[a]]
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

type Cell<T> = Arc<OnceLock<Arc<T>>>;
/// The cells by key, and the keys from least to most recently requested.
type Cells<T> = (HashMap<ConversionKey, Cell<T>>, VecDeque<ConversionKey>);

/// X-5: one table per key, built outside the lock. LRU drops registry
/// membership only, so a decoder holding an evicted key keeps its `Arc`.
struct Registry<T> {
    state: Mutex<Cells<T>>,
    keys: usize,
}

impl<T> Registry<T> {
    fn new() -> Self {
        Self::with_keys(REGISTRY_KEYS)
    }

    fn with_keys(keys: usize) -> Self {
        Self {
            state: Mutex::new((HashMap::new(), VecDeque::new())),
            keys,
        }
    }

    fn get(&self, key: &ConversionKey, build: impl FnOnce() -> T) -> Arc<T> {
        let cell = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let (cells, recency) = &mut *state;
            recency.retain(|held| held != key);
            recency.push_back(key.clone());
            let cell = Arc::clone(cells.entry(key.clone()).or_default());
            // Only ready tables leave: a build in progress keeps its cell, so
            // a concurrent request for its key waits on it (one build per key).
            while recency.len() > self.keys {
                let ready =
                    |held: &ConversionKey| cells.get(held).is_none_or(|c| c.get().is_some());
                let Some(oldest) = recency.iter().position(ready) else {
                    break;
                };
                if let Some(oldest) = recency.remove(oldest) {
                    cells.remove(&oldest);
                }
            }
            cell
        };
        Arc::clone(cell.get_or_init(|| Arc::new(build())))
    }
}

fn transfer_table(key: &ConversionKey, max: u32) -> Arc<TransferTable> {
    REGISTRY.get(key, || TransferTable::build(key, max))
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

    fn key(bits: u16, transfer: ColorTransfer) -> ConversionKey {
        ConversionKey {
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Full,
            bit_depth: ColorBitDepth::Integer(bits),
            transfer,
        }
    }

    /// The live-bytes lifecycle on an isolated counter and registry: a
    /// table stays charged after eviction while any owner holds it, only the
    /// last drop releases it, and a failed build charges nothing.
    #[test]
    fn built_tables_are_counted_while_held_and_alpha_is_exact() {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let registry = Registry::with_keys(2);
        let build = |key: &ConversionKey| TransferTable::build_counted(key, 255 << 8, &COUNTER);
        let held = key(8, ColorTransfer::Bt709);
        let table = registry.get(&held, || build(&held));
        let bytes = table.entries().map(<[f16]>::len).expect("builds") * 2;
        assert_eq!(
            (bytes, COUNTER.load(Ordering::Relaxed)),
            (128 * 1024, bytes)
        );
        let owner = Arc::clone(&table);
        for bits in 9..12 {
            let failed = key(bits, ColorTransfer::Unknown);
            let failed = registry.get(&failed, || build(&failed));
            assert_eq!(
                failed.entries().map(<[f16]>::len),
                Err(&(
                    PixelStage::ColourDecode,
                    ColorPipelineError::UnknownTransfer
                ))
            );
        }
        assert_eq!(
            COUNTER.load(Ordering::Relaxed),
            bytes,
            "failed builds add 0"
        );
        let rebuilt = registry.get(&held, || build(&held));
        assert!(!Arc::ptr_eq(&rebuilt, &table), "evicted from the registry");
        assert_eq!(COUNTER.load(Ordering::Relaxed), 2 * bytes);
        drop((rebuilt, table));
        assert_eq!(
            COUNTER.load(Ordering::Relaxed),
            2 * bytes,
            "the registry and an owner"
        );
        drop(owner);
        assert_eq!(
            COUNTER.load(Ordering::Relaxed),
            bytes,
            "the last owner releases"
        );
        drop(registry);
        assert_eq!(COUNTER.load(Ordering::Relaxed), 0);
        for (code, value) in (0..=u16::MAX).zip(alpha_table()) {
            let expected = f16::from_f32(f32::from(code) / 65_535.0);
            assert_eq!(value.to_bits(), expected.to_bits());
        }
    }

    /// X-5 (review A F3): a key whose build is in progress survives LRU
    /// pressure, so a concurrent request waits for that build instead of
    /// starting a second one.
    #[test]
    fn a_key_being_built_is_built_once_under_eviction() {
        use std::sync::mpsc;
        let registry = Arc::new(Registry::<usize>::with_keys(1));
        let builds = Arc::new(AtomicUsize::new(0));
        let slow = key(8, ColorTransfer::Bt709);
        let (started_tx, started) = mpsc::channel();
        let (release, release_rx) = mpsc::channel::<()>();
        let spawn = |started: Option<mpsc::Sender<()>>, wait: Option<mpsc::Receiver<()>>| {
            let (registry, builds, slow) =
                (Arc::clone(&registry), Arc::clone(&builds), slow.clone());
            std::thread::spawn(move || {
                registry.get(&slow, || {
                    if let (Some(started), Some(wait)) = (started, wait) {
                        started.send(()).expect("the test waits");
                        wait.recv().expect("the test releases");
                    }
                    builds.fetch_add(1, Ordering::SeqCst)
                })
            })
        };
        let first = spawn(Some(started_tx), Some(release_rx));
        started.recv().expect("the first build started");
        for bits in 9..12 {
            registry.get(&key(bits, ColorTransfer::Bt709), || 99);
        }
        let second = spawn(None, None);
        release.send(()).expect("release the first build");
        let (first, second) = (first.join().unwrap(), second.join().unwrap());
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(builds.load(Ordering::SeqCst), 1);
    }

    /// PF1 G-1 (I1): every f16 pattern, NaN, ±Inf, denormals and ±0 included.
    #[test]
    fn monitor_table_equals_the_f32_encode_for_every_f16_pattern() {
        for bits in 0..=u16::MAX {
            let linear = f16::from_bits(bits).to_f32();
            let rotated = [bits, bits.rotate_left(5), !bits, bits ^ 0x3C00];
            let expected = encode_monitor_rgba8(rotated.map(|b| f16::from_bits(b).to_f32()));
            assert_eq!(monitor_rgba8(rotated), expected, "{bits:#06x}");
            // NaN, and a zero exponent field: ±0 and every denormal.
            if linear.is_nan() || bits & 0x7C00 == 0 {
                assert_eq!(monitor_rgba8([bits; 4]), [0; 4], "{bits:#06x}");
            }
        }
        assert_eq!(monitor_rgba8([f16::INFINITY.to_bits(); 4]), [255; 4]);
        assert_eq!(monitor_rgba8([f16::NEG_INFINITY.to_bits(); 4]), [0; 4]);
    }
}
