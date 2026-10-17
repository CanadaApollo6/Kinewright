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
pub(crate) static MONITOR: LazyLock<(Vec<u8>, Vec<u8>)> = LazyLock::new(|| {
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

/// Bytes of RGB input tables currently held by the registry or any decoder,
/// process-wide. The public `CacheStats` reply is wire data, so S2a's
/// `Playback::stats` is where it surfaces in production (review B F4).
pub(crate) fn live_table_bytes() -> usize {
    LIVE_TABLE_BYTES.load(Ordering::Relaxed)
}

type Cell<T> = Arc<OnceLock<Arc<T>>>;

/// A build in flight: its cell and how many requests wait on it.
struct Building<T> {
    cell: Cell<T>,
    requests: usize,
}

/// The ready tables (bounded LRU) and the builds in flight (not bounded:
/// one per key being built, gone when its last request returns).
struct State<T> {
    ready: HashMap<ConversionKey, Arc<T>>,
    /// Ready keys, least to most recently requested.
    recency: VecDeque<ConversionKey>,
    building: HashMap<ConversionKey, Building<T>>,
}

/// X-5: one table per key, built outside the lock. A key being built keeps
/// its identity in `building`, outside the LRU, so every request for it
/// waits on the one build; on completion the table joins the ready LRU,
/// trimmed to `keys` (review A F3, re-review D2). LRU drops registry
/// membership only, so a decoder holding an evicted key keeps its `Arc`.
struct Registry<T> {
    state: Mutex<State<T>>,
    keys: usize,
}

impl<T> Registry<T> {
    fn new() -> Self {
        Self::with_keys(REGISTRY_KEYS)
    }

    fn with_keys(keys: usize) -> Self {
        let state = State {
            ready: HashMap::new(),
            recency: VecDeque::new(),
            building: HashMap::new(),
        };
        Self {
            state: Mutex::new(state),
            keys,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State<T>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn get(&self, key: &ConversionKey, build: impl FnOnce() -> T) -> Arc<T> {
        let cell = {
            let mut state = self.lock();
            if let Some(table) = state.ready.get(key).map(Arc::clone) {
                state.recency.retain(|held| held != key);
                state.recency.push_back(key.clone());
                return table;
            }
            let building = state
                .building
                .entry(key.clone())
                .or_insert_with(|| Building {
                    cell: Arc::default(),
                    requests: 0,
                });
            building.requests += 1;
            Arc::clone(&building.cell)
        };
        // Leaves the build on every exit, an unwinding build included.
        let _leave = Leave {
            registry: self,
            key,
            cell: &cell,
        };
        Arc::clone(cell.get_or_init(|| Arc::new(build())))
    }

    /// One request leaves the build of `key`: a finished table joins the
    /// ready LRU (trimmed to `keys`); a build abandoned by its last request
    /// (its builder unwound) is forgotten, so a retry builds afresh.
    fn leave(&self, key: &ConversionKey, cell: &Cell<T>) {
        let mut state = self.lock();
        let Some(building) = state.building.get_mut(key) else {
            return;
        };
        if !Arc::ptr_eq(&building.cell, cell) {
            return;
        }
        building.requests -= 1;
        let table = cell.get().map(Arc::clone);
        if table.is_none() && building.requests > 0 {
            return;
        }
        state.building.remove(key);
        let Some(table) = table else {
            return;
        };
        state.ready.insert(key.clone(), table);
        state.recency.retain(|held| held != key);
        state.recency.push_back(key.clone());
        while state.ready.len() > self.keys {
            let Some(oldest) = state.recency.pop_front() else {
                break;
            };
            state.ready.remove(&oldest);
        }
    }

    /// Keys the registry holds, ready or in flight.
    #[cfg(test)]
    fn held(&self) -> usize {
        let state = self.lock();
        state.ready.len() + state.building.len()
    }
}

struct Leave<'a, T> {
    registry: &'a Registry<T>,
    key: &'a ConversionKey,
    cell: &'a Cell<T>,
}

impl<T> Drop for Leave<'_, T> {
    fn drop(&mut self) {
        self.registry.leave(self.key, self.cell);
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

    /// A value that counts itself live while any owner holds it.
    struct Tracked(Arc<AtomicUsize>);

    impl Drop for Tracked {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// PF1 X-5 (re-review D2): nine distinct builds all in flight at once
    /// finish into the eight-key registry, which then retains eight.
    #[test]
    fn concurrent_builds_finish_within_the_key_bound() {
        let registry = Arc::new(Registry::<Tracked>::with_keys(8));
        let live = Arc::new(AtomicUsize::new(0));
        let all_building = Arc::new(std::sync::Barrier::new(9));
        let builders = (1..=9_u16).map(|bits| {
            let (registry, live) = (Arc::clone(&registry), Arc::clone(&live));
            let all_building = Arc::clone(&all_building);
            std::thread::spawn(move || {
                let table = registry.get(&key(bits, ColorTransfer::Bt709), || {
                    live.fetch_add(1, Ordering::SeqCst);
                    all_building.wait();
                    Tracked(Arc::clone(&live))
                });
                drop(table);
            })
        });
        for builder in builders.collect::<Vec<_>>() {
            builder.join().expect("a build");
        }
        assert_eq!(registry.held(), 8);
        assert_eq!(live.load(Ordering::SeqCst), 8, "the ninth is released");
        drop(registry);
        assert_eq!(live.load(Ordering::SeqCst), 0);
    }

    /// PF1 X-5 (re-review D2): a build that panics leaves no cell behind,
    /// and a retry of its key builds and is registered.
    #[test]
    fn a_panicking_build_is_forgotten_and_a_retry_builds() {
        let registry = Registry::<usize>::with_keys(8);
        let failing = key(8, ColorTransfer::Bt709);
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            registry.get(&failing, || panic!("the build fails"))
        }));
        assert!(unwound.is_err());
        assert_eq!(registry.held(), 0, "no abandoned cell");
        assert_eq!(*registry.get(&failing, || 7), 7);
        assert_eq!(registry.held(), 1);
        assert_eq!(*registry.get(&failing, || 8), 7, "ready, not rebuilt");
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
