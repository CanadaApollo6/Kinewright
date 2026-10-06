//! PF1 S2c C-4: the acceptance witnesses for §8 S-2 (forward continuation),
//! written from the design before any implementation exists (C-5: bytes).
//!
//! The single integration point is [`TargetPath`]. The reader model: a path
//! is one reader on one source. `produce(None, t)` is a cold seek to `t`.
//! `produce(Some(Cursor(c)), t)` is the scheduler saying "this reader's last
//! produced frame is `c`; give me `t`". The path must check that against its
//! own decoder (a reset reader is not at `c`) and either continue forward or
//! seek; the observation says which (`route`, from the decoder's seek count).
//! [`SeekPath`] is today's `decode_window(t, t)` and defines the oracle;
//! [`ReferenceContinuation`] is a correct continuation built in test code
//! (the positive control, with one switch per rule for the mutations).
//! The implementer wires [`ContinuationPath`] in S2c-2 and un-ignores its
//! tests; the witnesses, the oracles and the wrong paths may not be weakened.
//!
//! What the witnesses fix beyond "same bytes as a fresh Seek":
//! * the run's anchor is observed (`state.first_packet`, the real seek's
//!   first packet) and compared with the shadow's; an unknown or mismatched
//!   anchor requires Seek (injected: `ShadowFails`, `ShadowMismatch`,
//!   `StreamMismatch`);
//! * retained `pending` and `lookahead` frames are compared by content hash,
//!   and every target is followed by a second hop from the produced frame;
//! * the route is decided by the packets actually read (`Truth`: a linear
//!   decode from the anchor at the same thread count against the demux-order
//!   key flags), so it is exact under frame threading;
//! * timestamps: missing or repeated ones (injected `Tamper`) force Seek;
//! * cancellation is asserted by produced-frame progress and stop position:
//!   the continuation checks the flag between produced frames (S-2), so a
//!   run armed for the `n`th frame ends with exactly `n` frames received,
//!   also when frames were already decoded and waiting (frame threads, the
//!   end-of-stream flush); today's packet-boundary check is the Seek path's;
//! * a mismatch between the real seek's first packet and the shadow's
//!   disables continuation for the decoder for good (`MismatchOnce`: one bad
//!   seek, then matching anchors; only replacing the decoder re-enables it);
//! * the pixels a path returns are validated outside the path: the RGBA64
//!   the shared conversion produced must equal the pinned CLI's own
//!   conversion of the same frame, and the working frame must follow from
//!   those bytes by an independent BT.709 transfer table (`OutputOracle`),
//!   so a fault in the output path both the continuation and the Seek share
//!   is seen; the digest of the returned pixels is pinned per OS as well.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use half::f16;
use kinewright_core::{MediaError, TimeCode};

use crate::{
    cache::FrameCache,
    decode::{Anchor, DecoderState, FrameLog, Tamper, VideoDecoder, default_threads},
    frame::WorkingFrame,
    pf1_s2c_fixtures::{Facts, Fixture, KINDS, Kind, RefFrame},
    sha256::sha256_bytes,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Route {
    Seek,
    Continue,
}

/// The frame the scheduler believes the reader last produced.
#[derive(Clone, Copy, Debug)]
#[allow(dead_code, reason = "the seam S2c-2 reads in its ContinuationPath")]
pub(super) struct Cursor(pub(super) i64);

/// What one `produce` yields. `same` compares everything a fresh Seek fixes.
#[derive(Clone, Debug)]
pub(super) struct Observation {
    /// FNV-1a of the produced frame's f16 bits (`None`: no frame cached at `t`).
    pub(super) frame: Option<u64>,
    /// The produced frame's `best_effort_timestamp` (`state.pending.pts`).
    pub(super) pts: Option<i64>,
    pub(super) state: DecoderState,
    /// `Debug` of the `MediaError`, if the run failed (`"Cancelled"`).
    pub(super) err: Option<String>,
    pub(super) route: Route,
    /// The frames received during this call, with the video packets read in
    /// the run when each arrived (the produced-frame progress).
    pub(super) frames: Vec<FrameLog>,
    /// Video packets read in the run when the call ended (the stop position).
    pub(super) packets: u64,
    /// The returned frame's conversion, for the independent output oracle
    /// (not part of `same`: `frame` already covers the pixels).
    pub(super) output: Option<Output>,
    /// How many times the behavioural cancel seam was set or cleared over the
    /// decoder's lifetime (`DecoderProbe::seam_arms`, monotonic); not part of
    /// `same`. Only a path holding a `ControlToken` may have it above zero.
    pub(super) seam_arms: u64,
}

/// The pixels a call returned, and the RGBA64 the shared conversion handed
/// to the working-frame stage for them.
#[derive(Clone)]
pub(super) struct Output {
    /// The converted source frame's own timestamp.
    pts: Option<i64>,
    rgba: Arc<Vec<u8>>,
    pixels: Arc<Vec<f16>>,
}

impl std::fmt::Debug for Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Output(ts {:?}, {} rgba bytes, {} values)",
            self.pts,
            self.rgba.len(),
            self.pixels.len()
        )
    }
}

impl Observation {
    fn same(&self, other: &Self) -> bool {
        (self.frame, self.pts, &self.state, &self.err)
            == (other.frame, other.pts, &other.state, &other.err)
    }

    /// What must not depend on the thread count: the selected frame, the
    /// retained frames, the anchor. (`eof_sent` and the frame log may.)
    fn same_content(&self, other: &Self) -> bool {
        let (a, b) = (&self.state, &other.state);
        (
            self.frame,
            self.pts,
            &a.pending,
            &a.lookahead,
            a.first_packet,
            &self.err,
        ) == (
            other.frame,
            other.pts,
            &b.pending,
            &b.lookahead,
            b.first_packet,
            &other.err,
        )
    }
}

/// A decoder's counters at the start of a call.
#[derive(Clone, Copy)]
pub(super) struct Before {
    seeks: u64,
    received: u64,
    conversions: u64,
}

impl Before {
    pub(super) fn of(decoder: &VideoDecoder) -> Self {
        Self {
            seeks: decoder.seek_count(),
            received: decoder.probe().received,
            conversions: decoder.probe().conversions,
        }
    }
}

/// Observe `decoder` after a run for `t`.
pub(super) fn observe(
    decoder: &VideoDecoder,
    cache: &mut FrameCache<WorkingFrame>,
    t: TimeCode,
    result: Result<(), MediaError>,
    before: Before,
) -> Observation {
    let state = decoder.state();
    let hit = cache.contains(t);
    let returned = hit.then(|| cache.frame_at_or_before(t)).flatten();
    let frame = returned.as_ref().map(|f| fnv_bits(&f.pixels));
    let probe = decoder.probe();
    let converted = probe
        .last_conversion
        .as_ref()
        .filter(|_| probe.conversions > before.conversions);
    let output = returned.zip(converted).map(|(f, c)| Output {
        pts: c.pts,
        rgba: c.rgba.clone(),
        pixels: f.pixels.clone(),
    });
    let took = usize::try_from(probe.received - before.received).unwrap();
    Observation {
        frame,
        pts: state.pending.as_ref().and_then(|p| p.pts),
        state,
        err: result.err().map(|e| format!("{e:?}")),
        route: if decoder.seek_count() > before.seeks {
            Route::Seek
        } else {
            Route::Continue
        },
        frames: probe.frames[probe.frames.len().saturating_sub(took)..].to_vec(),
        packets: probe.packets,
        output,
        seam_arms: probe.seam_arms(),
    }
}

/// FNV-1a of a frame's f16 bits.
fn fnv_bits(pixels: &[f16]) -> u64 {
    pixels.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, p| {
        (h ^ u64::from(p.to_bits())).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The output oracle: what the returned pixels must be, known from outside
/// the decoder. The pinned CLI converts each frame of the file with the
/// managed graph's arguments (`CONVERSION`) and `framehash` gives the
/// SHA-256 of the RGBA64LE; the decoder's conversion of a frame must hash to
/// it, and the working frame the path returns must follow from those bytes
/// by a BT.709 inverse-OETF table written out here (f64; the decoder's is
/// f32, so within one f16 step) with alpha = code / 65535. The fixtures are
/// BT.709 limited range 8-bit, so the code range is 0 to 65280.
pub(super) struct OutputOracle {
    by_ts: BTreeMap<i64, String>,
    all: BTreeSet<String>,
    /// (FNV of RGBA64, FNV of pixels) already verified.
    verified: RefCell<BTreeSet<(u64, u64)>>,
}

/// Independent BT.709 decode of a full-range 16-bit code (limited 8-bit source).
fn transfer_table() -> &'static [f64] {
    static TABLE: std::sync::LazyLock<Vec<f64>> = std::sync::LazyLock::new(|| {
        (0..=u16::MAX)
            .map(|code| {
                let v = f64::from(code) / 65_280.0;
                if v < 0.081 {
                    v / 4.5
                } else {
                    ((v + 0.099) / 1.099).powf(1.0 / 0.45)
                }
            })
            .collect()
    });
    &TABLE
}

fn fnv_bytes(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

impl OutputOracle {
    pub(super) fn new(reference: &[RefFrame]) -> Self {
        Self {
            by_ts: (reference.iter())
                .filter_map(|f| Some((f.ts?, f.out.clone())))
                .collect(),
            all: reference.iter().map(|f| f.out.clone()).collect(),
            verified: RefCell::default(),
        }
    }

    /// Distinct returned frames (by conversion and pixels) verified so far.
    fn verified(&self) -> usize {
        self.verified.borrow().len()
    }

    /// The returned frame's conversion is the CLI's, and the returned pixels
    /// follow from it.
    fn verify(&self, got: &Observation) -> Result<(), String> {
        let (Some(o), Some(_)) = (&got.output, got.frame) else {
            return match got.frame {
                Some(_) => Err("a returned frame without its conversion".to_owned()),
                None => Ok(()),
            };
        };
        let key = (fnv_bytes(&o.rgba), fnv_bits(&o.pixels));
        if self.verified.borrow().contains(&key) {
            return Ok(());
        }
        let sha = sha256_bytes(&o.rgba);
        let cli = o.pts.and_then(|ts| self.by_ts.get(&ts));
        if cli.map_or(!self.all.contains(&sha), |c| *c != sha) {
            return Err(format!(
                "the conversion of frame {:?} hashes to {sha}, the CLI's is {cli:?}",
                o.pts
            ));
        }
        let table = transfer_table();
        if o.rgba.len() != o.pixels.len() * 2 {
            return Err(format!(
                "{} rgba bytes for {} values",
                o.rgba.len(),
                o.pixels.len()
            ));
        }
        for (i, (code, got)) in o.rgba.as_chunks::<2>().0.iter().zip(&*o.pixels).enumerate() {
            let code = u16::from_le_bytes(*code);
            let want = if i % 4 == 3 {
                f64::from(code) / 65_535.0
            } else {
                table[usize::from(code)]
            };
            let got = f64::from(got.to_f32());
            // Every value must be finite first: a NaN fails every comparison,
            // so `NaN > tolerance` alone would wave it through.
            let within = got.is_finite()
                && want.is_finite()
                && (got - want).abs() <= want.abs() * 2f64.powi(-10) + 2f64.powi(-24);
            if !within {
                return Err(format!(
                    "value {i} of frame {:?} is {got}, the independent table gives {want} (code {code})",
                    o.pts
                ));
            }
        }
        self.verified.borrow_mut().insert(key);
        Ok(())
    }
}

#[derive(Clone)]
pub(super) enum Event {
    /// The clip is relinked to another file (always the corpus's `b`): a new
    /// `VideoSourceKey`.
    Relink(Rc<Fixture>),
    /// The key changes but the decoded content does not.
    KeyChange,
    ShrinkReopen,
    /// The reader's decoder hit an error (state must not survive it).
    Error,
    /// Arm cancellation for the next `produce`: the stop flag is raised when
    /// `n` (>= 1) frames have been received in that call.
    Cancel(usize),
    /// From now on the demux-only shadow context fails (S-2 rule 1: unknown).
    ShadowFails,
    /// From now on the shadow's anchor disagrees with the real seek's.
    ShadowMismatch,
    /// From now on the seek's selected stream is not the decoder's video stream.
    StreamMismatch,
    /// At the reader's next seek, once, the shadow's anchor disagrees with the
    /// real seek's first packet; from then on anchors match again. S-2 rule 2
    /// disables continuation for the decoder on that mismatch, for good: the
    /// reader must seek on every later target until its decoder is replaced
    /// (`KeyChange`, `ShrinkReopen`, `Error`, `Relink`), and continues again
    /// after that.
    MismatchOnce,
    /// From now on the shadow's A(t) for a later target differs from the run's
    /// anchor where the file's does not (on the fixtures every anchor change
    /// comes with a key packet, so this injection moves A(t) inside a GOP:
    /// rule 2's A(t) = A(t0) test, Amendment R51's whole rule 2).
    AnchorDrifts,
    /// From now on the decoder meets this timestamp fault (`Tamper`); the
    /// path must read timestamps as `decode.rs` does, through the probe.
    Tamper(Tamper),
}

pub(super) trait TargetPath {
    /// The capability to arm the behavioural cancel seam
    /// (`DecoderProbe::arm_between_frames`), held only by the positive
    /// control. A path over production code returns `None` (the default) and
    /// cannot obtain a token: its constructor is private to the `reference`
    /// submodule, so claiming control elsewhere does not compile. Such a path
    /// keeps its own per-frame check, which is what the exact-stop witnesses
    /// observe, and the harness fails it if its decoder's `seam_arms` is
    /// above zero (`produced`), even if it cleared the seam again.
    fn control(&self) -> Option<&ControlToken> {
        None
    }

    fn produce(&mut self, from: Option<Cursor>, t: TimeCode) -> Observation;
    fn event(&mut self, event: Event);
}

/// Today's fresh `Seek`: a new decoder per call, `decode_window(t, t)`.
pub(super) struct SeekPath {
    fx: Rc<Fixture>,
    threads: usize,
    cancel: Option<usize>,
    tampers: Vec<Tamper>,
}

impl SeekPath {
    pub(super) fn new(fx: &Rc<Fixture>, threads: usize) -> Self {
        Self {
            fx: fx.clone(),
            threads,
            cancel: None,
            tampers: Vec::new(),
        }
    }
}

/// When a call's cancellation fires (the correct one is `Exact`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CancelAt {
    Exact,
    Late(u64),
    Immediate,
    /// Fires on time but the run drains the frames already decoded before it
    /// looks at the flag again (a check between packets, not between frames).
    Drain,
}

/// Arm a call's cancellation on `decoder`: inject the stop signal (the flag
/// the decoder checks, raised when the armed frame is received) and nothing
/// else. It never touches the behavioural seam, so it is what a path over
/// production code uses; only the positive control adds the seam, with its
/// `ControlToken` (`reference::ReferenceContinuation`).
fn arm(decoder: &mut VideoDecoder, cancel: Option<usize>, how: CancelAt) {
    let stop = Arc::new(AtomicBool::new(false));
    let n = cancel.map(|n| u64::try_from(n).unwrap());
    let armed = match (n, how) {
        (Some(_), CancelAt::Immediate) => {
            stop.store(true, Ordering::Release);
            None
        }
        (Some(n), CancelAt::Exact | CancelAt::Drain) => Some(n),
        (Some(n), CancelAt::Late(extra)) => Some(n + extra),
        (None, _) => None,
    };
    decoder.arm_cancel_after(armed);
    decoder.set_stop(stop);
}

impl TargetPath for SeekPath {
    fn produce(&mut self, _from: Option<Cursor>, t: TimeCode) -> Observation {
        let mut decoder = self.fx.open(self.threads);
        decoder.probe_mut().tampers.clone_from(&self.tampers);
        // Today's decoder: the flag is checked between packets only.
        arm(&mut decoder, self.cancel.take(), CancelAt::Exact);
        let before = Before::of(&decoder);
        let mut cache = FrameCache::new(1);
        let result = decoder.decode_window(t, t, &mut cache);
        observe(&decoder, &mut cache, t, result, before)
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Relink(fx) => self.fx = fx,
            Event::Cancel(n) => self.cancel = Some(n),
            Event::Tamper(t) => self.tampers.push(t),
            _ => {} // stateless: every call already opens a fresh decoder
        }
    }
}

/// S2c-2 wires this: one reader (a `VideoDecoder` kept across calls).
/// `produce(Some(Cursor(c)), t)` continues from `c` only in the S-2 domain
/// and only if its decoder really is at `c`; otherwise it seeks. It must
/// report through `observe` with the counters taken before the call, keep
/// the decoder's probe fed (see `DecoderProbe`), and implement every
/// `Event` as its doc says. Cancellation is checked between produced frames
/// by the path's own production code (the real check belongs in
/// `receive_frames`): the adapter injects the stop signal with `arm` and must
/// NOT arm the `check_between_frames` seam, which models that check for the
/// positive control only: arming needs a `ControlToken`, which only the
/// `reference` submodule can create, and the harness fails a path whose
/// decoder's `seam_arms` is above zero.
/// A mismatch between the real
/// seek's first packet and the shadow's disables continuation for the decoder
/// until it is replaced (`Event::MismatchOnce`), and the frames it returns
/// are checked against the CLI's own conversion (`Corpus::out`).
///
/// S2c-2: wired in its own file (R46), over `VideoDecoder::decode_paused`.
#[path = "pf1_s2c_continuation.rs"]
mod continuation;
use continuation::ContinuationPath;

/// The frame-thread counts the witnesses run at: one thread, a threaded
/// case, and a synchronous decoder's share (`default_threads`).
fn thread_counts() -> Vec<usize> {
    let mut counts = vec![1, 4, default_threads()];
    counts.sort_unstable();
    counts.dedup();
    counts
}

/// One S-2 test target: the reader is at `c`, is asked for `t`, then (if any)
/// for `t2` from `t` (a chained continuation).
#[derive(Debug)]
pub(super) struct Target {
    c: i64,
    t: i64,
    t2: Option<i64>,
    kind: &'static str,
}

/// The linear decode from each anchor, at one thread count: the frames as
/// they arrive (packets read when each did).
pub(super) struct Truth {
    runs: BTreeMap<isize, Vec<FrameLog>>,
}

impl Truth {
    fn new(fx: &Fixture, threads: usize) -> Self {
        let mut runs = BTreeMap::new();
        for (f, a) in fx.facts.anchors.iter().enumerate() {
            let Some(a) = a else { continue };
            if runs.contains_key(&a.0) {
                continue;
            }
            let f = i64::try_from(f).unwrap();
            let (mut decoder, mut cache) = (fx.open(threads), FrameCache::<WorkingFrame>::new(1));
            decoder
                .decode_window(TimeCode(f), TimeCode(f), &mut cache)
                .unwrap();
            let end = TimeCode(fx.facts.last + 100);
            decoder
                .decode_window_sequential(TimeCode(f + 1), end, &mut cache)
                .unwrap();
            // S2c-5: a run from an open-GOP leading frame seeks once more,
            // before its key (`retries`); otherwise one seek.
            assert_eq!(
                decoder.seek_count(),
                1 + u64::from(retries(&fx.facts, f)),
                "{:?}: the linear run seeked again",
                fx.kind
            );
            runs.insert(a.0, decoder.probe().frames.clone());
        }
        Self { runs }
    }

    fn pts_of(&self, grid: i64) -> Option<i64> {
        (self.runs.values().flatten()).find(|f| f.grid == grid)?.ts
    }
}

/// A fixture, its seeded targets and the fresh-`Seek` oracle for each, per
/// thread count (the oracle is always a live `SeekPath` at that count).
pub(super) struct Corpus {
    fx: Rc<Fixture>,
    targets: Vec<Target>,
    oracle: BTreeMap<usize, Vec<(Observation, Option<Observation>)>>,
    truth: BTreeMap<usize, Truth>,
    /// The independent check of every returned frame (`None` only where a
    /// test shows what the comparison with the Seek path alone cannot see).
    out: Option<OutputOracle>,
    /// The conversion tap, on for the corpus's life (its oracle included).
    _tap: Tap,
}

/// PF1 S2c C-4 (lead ruling after R51/R52): holds the decoder's conversion
/// tap on for this thread (`decode::CONVERSION_TAP`) while it lives.
pub(super) struct Tap;

impl Tap {
    pub(super) fn on() -> Self {
        crate::decode::CONVERSION_TAP.with(|tap| tap.set(tap.get() + 1));
        Self
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        crate::decode::CONVERSION_TAP.with(|tap| tap.set(tap.get() - 1));
    }
}

fn splitmix(state: &mut u64) -> i64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    i64::try_from((z ^ (z >> 31)) >> 1).unwrap()
}

/// S2c-5 (R39 item 4 (b), lead rulings of 2026-10-06): whether a seek to `t`
/// is retried before its key. On the S-2 pair only, when A(t) is a key that
/// is not the stream's first and presents after `t` (an open-GOP leading
/// frame: its references are in the GOP before), production seeks once more
/// to the key before. Other demuxers (AVI) keep one seek.
fn retries(facts: &Facts, t: i64) -> bool {
    let Some(a) = anchor(facts, t).filter(|_| facts.kind != Kind::AviDtsGuess) else {
        return false;
    };
    let i = facts.keys.iter().position(|(pos, _)| *pos == a.0);
    i.is_some_and(|i| i > 0 && facts.keys[i].1 > t)
}

/// S2c-5: where a retried seek to `t` lands, the key packet before A(t), as
/// the shadow gives it (the anchor of the frames that key starts).
fn retried_anchor(facts: &Facts, t: i64) -> Option<Anchor> {
    let a = anchor(facts, t)?;
    let i = facts.keys.iter().position(|(pos, _)| *pos == a.0)?;
    let before = facts.keys.get(i.checked_sub(1)?)?.0;
    facts
        .anchors
        .iter()
        .flatten()
        .find(|x| x.0 == before)
        .copied()
}

/// The anchor of target `t` (shadow, from the file), if known.
fn anchor(facts: &Facts, t: i64) -> Option<Anchor> {
    usize::try_from(t)
        .ok()
        .and_then(|i| facts.anchors.get(i).copied().flatten())
}

fn targets(fx: &Fixture) -> Vec<Target> {
    let facts = &fx.facts;
    let last = facts.last;
    let kind_seed = 0x5EED_0000 + fx.kind as u64;
    let mut out: Vec<Target> = Vec::new();
    let second = |c: i64, t: i64| {
        let mut seed = kind_seed ^ (c * 1000 + t).unsigned_abs();
        let t2 = t + 1 + splitmix(&mut seed) % 12;
        (t2 <= last).then_some(t2)
    };
    let add = |out: &mut Vec<Target>, c: i64, t: i64, t2: Option<i64>, kind| {
        if (1..=last).contains(&t) && c >= 0 && !out.iter().any(|x| (x.c, x.t) == (c, t)) {
            out.push(Target { c, t, t2, kind });
        }
    };
    let mid = last / 2;
    // §8 specials: end of stream, the window's edges, c + 1.
    add(&mut out, last - 1, last, None, "eof");
    add(&mut out, last - 2, last - 1, Some(last), "eof");
    add(&mut out, mid - 12, mid, second(mid - 12, mid), "c+12");
    add(
        &mut out,
        mid - 13,
        mid,
        second(mid - 13, mid),
        "c+13 (outside)",
    );
    for t in [1, 2, 13, mid, mid + 7] {
        add(&mut out, t - 1, t, second(t - 1, t), "c+1");
    }
    // ±1 frame around every corrected anchor boundary (the tick-exact
    // boundaries are the `ticks` test's), and windows straddling it.
    for f in 1..=usize::try_from(last).unwrap() {
        if facts.anchors[f] != facts.anchors[f - 1] {
            let f = i64::try_from(f).unwrap();
            for t in [f - 1, f, f + 1] {
                add(&mut out, t - 1, t, second(t - 1, t), "anchor-boundary");
            }
            add(
                &mut out,
                f - 4,
                f - 1,
                second(f - 4, f - 1),
                "boundary-window",
            );
            add(
                &mut out,
                f - 4,
                f + 1,
                second(f - 4, f + 1),
                "boundary-window",
            );
        }
    }
    // Just before and at each key packet; and chains whose second hop crosses it.
    for &(_, k) in &facts.keys {
        add(&mut out, k - 2, k - 1, second(k - 2, k - 1), "before-key");
        add(&mut out, k - 1, k, second(k - 1, k), "at-key");
        add(&mut out, k - 5, k - 3, Some(k + 1), "second-hop-across-key");
        add(&mut out, k - 3, k - 2, Some(k + 2), "second-hop-across-key");
    }
    let mut seed = kind_seed;
    while out.len() < 50 {
        let t = 1 + splitmix(&mut seed) % last;
        let (pick, gap) = (splitmix(&mut seed) % 20, splitmix(&mut seed));
        let c = match pick {
            0..=11 => t - 1 - gap % 12, // in the window: c + 1 ..= c + 12
            12..=16 => t - 13 - gap % 20,
            _ => t + gap % 10, // not forward at all
        };
        let c = c.max(0);
        add(&mut out, c, t, second(c, t), "random");
    }
    out
}

impl Corpus {
    pub(super) fn new(kind: Kind, threads: &[usize]) -> Self {
        let tap = Tap::on();
        let fx = Rc::new(Fixture::new(kind));
        let targets = targets(&fx);
        let oracle = threads
            .iter()
            .map(|&n| {
                let fresh = |t: i64| SeekPath::new(&fx, n).produce(None, TimeCode(t));
                (
                    n,
                    targets
                        .iter()
                        .map(|g| (fresh(g.t), g.t2.map(fresh)))
                        .collect(),
                )
            })
            .collect();
        let truth = threads.iter().map(|&n| (n, Truth::new(&fx, n))).collect();
        let out = Some(OutputOracle::new(&fx.reference()));
        Self {
            fx,
            targets,
            oracle,
            truth,
            out,
            _tap: tap,
        }
    }

    fn pts_of(&self, grid: i64) -> i64 {
        self.truth
            .values()
            .find_map(|t| t.pts_of(grid))
            .expect("a timestamp at that frame")
    }
}

/// What S-2 says about the reader's run, tracked over a script: the anchor
/// of the run's last seek (`None`: unknown or no run) and whether the shadow
/// is compromised.
pub(super) struct Model<'a> {
    corpus: &'a Corpus,
    run: Option<Anchor>,
    faulty: bool,
    /// A `MismatchOnce` is waiting for the next seek.
    mismatch_next: bool,
    /// Continuation is disabled for the decoder (rule 2's mismatch), until
    /// the decoder is replaced.
    disabled: bool,
}

impl<'a> Model<'a> {
    fn new(corpus: &'a Corpus) -> Self {
        Self {
            corpus,
            run: None,
            faulty: false,
            mismatch_next: false,
            disabled: false,
        }
    }

    /// A cold seek to `t`: the run's anchor is A(t), if the shadow can say.
    /// A seek at which the shadow disagrees with the real context disables
    /// continuation for the decoder for good.
    /// S2c-5: a retried seek (`retries`) leaves a run with no anchor, and
    /// rule 2 is not judged on it (no latch; a waiting `MismatchOnce` waits on).
    fn seeked(&mut self, t: i64) {
        if retries(&self.corpus.fx.facts, t) {
            self.run = None;
            return;
        }
        self.disabled |= std::mem::take(&mut self.mismatch_next);
        self.run = if self.faulty || self.disabled {
            None
        } else {
            anchor(&self.corpus.fx.facts, t)
        };
    }

    /// The decoder is replaced (`KeyChange`, `ShrinkReopen`, `Error`,
    /// `Relink`): no run, and continuation is allowed again.
    fn replaced(&mut self) {
        (self.run, self.disabled) = (None, false);
    }

    /// Where S-2 says the reader at `c` producing `t` seeks or continues.
    /// Continue needs: the `mov` pair; a known run anchor; `c < t <= c + 12`;
    /// A(t) equal to the run's (Amendment R51: rule 2 is this anchor test
    /// alone; a key packet read, read-ahead included, no longer ends a run,
    /// since with the same anchor and an unbroken feed a fresh seek to `t`
    /// feeds the same packets from the same flushed state). Anything else Seek.
    fn expected(&mut self, c: i64, t: i64) -> Route {
        let facts = &self.corpus.fx.facts;
        let continues = !self.faulty
            && !self.disabled
            && facts.kind != Kind::AviDtsGuess
            && c < t
            && t <= c + 12
            && self.run.is_some()
            && anchor(facts, t) == self.run;
        if continues {
            return Route::Continue;
        }
        self.seeked(t);
        Route::Seek
    }
}

/// Compare one observation with its oracle (and, if `check_route`, the route).
fn check(
    got: &Observation,
    want: &Observation,
    expect: Option<Route>,
    out: Option<&OutputOracle>,
    place: &str,
) -> Result<(), String> {
    if !got.same(want) {
        return Err(format!("{place}: {got:?} != oracle {want:?}"));
    }
    if let Some(e) = out.and_then(|o| o.verify(got).err()) {
        return Err(format!("{place}: returned pixels: {e}"));
    }
    match expect {
        Some(r) if r != got.route => Err(format!("{place}: route {:?}, S-2 says {r:?}", got.route)),
        _ => Ok(()),
    }
}

/// `path.produce`, and the harness check that only the positive control (the
/// path that holds a `ControlToken`) ever armed the behavioural cancel seam:
/// `seam_arms` counts every set and clear, so arming during the call and
/// clearing before the observation still fails.
fn produced<P: TargetPath>(
    path: &mut P,
    from: Option<Cursor>,
    t: i64,
) -> Result<Observation, String> {
    let got = path.produce(from, TimeCode(t));
    if got.seam_arms > 0 && path.control().is_none() {
        return Err(format!(
            "produce({from:?}, {t}): the adapter armed the test-only check_between_frames seam \
             (seam_arms {}) without the control token; a path over production code injects the \
             stop signal only",
            got.seam_arms
        ));
    }
    Ok(got)
}

/// Every target, at `threads` frame threads: the path's frame bytes,
/// timestamps, retained frames and S-2 state equal the same-thread oracle's
/// for the first hop and for the chained second hop; with `check_route`, it
/// also seeks or continues as S-2 says.
pub(super) fn witness<P: TargetPath>(
    path: &mut P,
    corpus: &Corpus,
    threads: usize,
    check_route: bool,
) -> Result<(), String> {
    witness_hops(path, corpus, threads, check_route, true)
}

/// [`witness`], with the chained second hops optional (their absence is what
/// the second-hop mutation shows to matter).
pub(super) fn witness_hops<P: TargetPath>(
    path: &mut P,
    corpus: &Corpus,
    threads: usize,
    check_route: bool,
    chain: bool,
) -> Result<(), String> {
    for (g, (want, want2)) in corpus.targets.iter().zip(&corpus.oracle[&threads]) {
        let place = format!("{:?} x{threads} {g:?}", corpus.fx.kind);
        produced(path, None, g.c)?;
        let mut model = Model::new(corpus);
        model.seeked(g.c);
        let expect = model.expected(g.c, g.t);
        let got = produced(path, Some(Cursor(g.c)), g.t)?;
        check(
            &got,
            want,
            check_route.then_some(expect),
            corpus.out.as_ref(),
            &place,
        )?;
        if let (Some(t2), Some(want2), true) = (g.t2, want2, chain) {
            let expect = model.expected(g.t, t2);
            let got = produced(path, Some(Cursor(g.t)), t2)?;
            check(
                &got,
                want2,
                check_route.then_some(expect),
                corpus.out.as_ref(),
                &format!("{place} then {t2}"),
            )?;
        }
    }
    Ok(())
}

pub(super) enum Expect {
    Auto,
    /// As `Auto`, and the script asserts S-2 does continue here (the recovery
    /// after a decoder is replaced must not be a silent series of seeks).
    Continues,
    Is(Route),
}

pub(super) enum Step {
    /// A cold seek: the reader is now at this frame.
    Prime(i64),
    /// The scheduler believes the reader is at `.0` and asks for `.1`.
    Go(i64, i64, Expect),
    Do(Event),
    /// The armed `Cancel(n)` fires mid-run: `Cancelled`, no continuation
    /// state left, and (`exact`: the S-2 continuation) exactly `n` frames
    /// received, none after the check that saw the flag, else (the Seek
    /// path) at least `n` and no packet read after the `n`th's.
    Cancelled(i64, i64, usize),
}

/// The scripts' fixtures and thread count: `a` is the file the reader runs
/// on, `b` the relink target.
pub(super) struct Ctx<'a> {
    pub(super) a: &'a Corpus,
    pub(super) b: &'a Corpus,
    pub(super) threads: usize,
}

/// Run `steps` on `path` and on a fresh-`Seek` reference (which sees the
/// same relinks and timestamp faults); every produced frame equals the
/// reference's, and (with `check_route`) routes are as expected: `Auto` from
/// S-2, `Is(..)` as stated.
pub(super) fn run_script<P: TargetPath>(
    path: &mut P,
    cx: &Ctx,
    steps: &[Step],
    check_route: bool,
) -> Result<(), String> {
    let mut reference = SeekPath::new(&cx.a.fx, cx.threads);
    let mut model = Model::new(cx.a);
    for (n, step) in steps.iter().enumerate() {
        match step {
            Step::Prime(c) => {
                produced(path, None, *c).map_err(|e| format!("step {n}: {e}"))?;
                model.seeked(*c);
            }
            Step::Go(c, t, expect) => {
                let got =
                    produced(path, Some(Cursor(*c)), *t).map_err(|e| format!("step {n}: {e}"))?;
                let want = reference.produce(None, TimeCode(*t));
                let prior = model.run;
                let auto = model.expected(*c, *t);
                if matches!(expect, Expect::Continues) && auto != Route::Continue {
                    return Err(format!("step {n}: the script expects S-2 to continue"));
                }
                let expect = match expect {
                    Expect::Auto | Expect::Continues => auto,
                    Expect::Is(r) => {
                        model.run = prior;
                        if *r == Route::Seek {
                            model.seeked(*t);
                        }
                        *r
                    }
                };
                if !got.same(&want) {
                    return Err(format!("step {n}: {got:?} != reference {want:?}"));
                }
                if let Some(e) = model.corpus.out.as_ref().and_then(|o| o.verify(&got).err()) {
                    return Err(format!("step {n}: returned pixels: {e}"));
                }
                if check_route && expect != got.route {
                    return Err(format!(
                        "step {n}: route {:?}, expected {expect:?}",
                        got.route
                    ));
                }
            }
            Step::Do(event) => {
                match event {
                    Event::Relink(_) => {
                        model.corpus = cx.b;
                        model.replaced();
                        reference.event(event.clone());
                    }
                    Event::KeyChange | Event::ShrinkReopen | Event::Error => model.replaced(),
                    Event::MismatchOnce => model.mismatch_next = true,
                    Event::ShadowFails
                    | Event::ShadowMismatch
                    | Event::StreamMismatch
                    | Event::AnchorDrifts => {
                        model.faulty = true;
                        model.run = None;
                    }
                    Event::Tamper(_) => reference.event(event.clone()),
                    Event::Cancel(_) => {}
                }
                path.event(event.clone());
            }
            Step::Cancelled(c, t, frames) => {
                let got =
                    produced(path, Some(Cursor(*c)), *t).map_err(|e| format!("step {n}: {e}"))?;
                cancelled(&got, *frames, check_route).map_err(|e| format!("step {n}: {e}"))?;
                model.run = None; // the interrupted run is abandoned
            }
        }
    }
    Ok(())
}

/// The cancel witness: `Cancelled`, no continuation state left, and
/// progress up to the stop. `exact` (the S-2 continuation, which checks the
/// flag between produced frames): exactly `n` frames were received, so none
/// after the check that saw the flag, whether or not more were already
/// decoded and waiting. Otherwise (today's Seek, which checks between
/// packets): at least `n`, and no packet read after the `n`th's.
fn cancelled(got: &Observation, n: usize, exact: bool) -> Result<(), String> {
    let stopped = got.err.as_deref() == Some("Cancelled") && got.state.continuation_at.is_none();
    if !stopped {
        return Err(format!("cancel left {got:?}"));
    }
    if exact && got.frames.len() != n {
        return Err(format!(
            "cancelled after {} frames, the flag was seen at frame {n}: {:?}",
            got.frames.len(),
            got.frames
        ));
    }
    let Some(nth) = got.frames.get(n - 1) else {
        return Err(format!(
            "cancelled after {} frames, armed for {n}: {got:?}",
            got.frames.len()
        ));
    };
    if nth.packets != got.packets || got.frames.last().is_some_and(|l| l.packets != got.packets) {
        return Err(format!(
            "cancelled {} packets in, frame {n} arrived at packet {}: {:?}",
            got.packets, nth.packets, got.frames
        ));
    }
    Ok(())
}

/// The non-fixture cases of S-2, over `a` and `b`: an edit, a relink, a
/// same-source jump cut, cancellation, the resets, and the injected
/// failures (an unknown or mismatched anchor, timestamp faults).
#[allow(clippy::too_many_lines, reason = "a table of scripts")]
pub(super) fn scripts(cx: &Ctx, cancel_after: usize) -> Vec<(&'static str, Vec<Step>)> {
    use {Expect::*, Route::Seek, Step::*};
    let reset = |event: Event| {
        vec![
            Prime(10),
            Go(10, 11, Auto),
            Do(event),
            Go(11, 12, Is(Seek)), // a reset reader is not at 11: seek
            Go(12, 13, Auto),
        ]
    };
    // After a shadow fault the reader must seek, run after run (early in a
    // GOP, so that without the fault the same hops continue).
    let faulted = |event: Event| {
        vec![
            Prime(6),
            Go(6, 7, Auto),
            Do(event),
            Prime(6),
            Go(6, 7, Auto),
            Go(7, 8, Auto),
            Go(8, 9, Auto),
        ]
    };
    // One bad seek (the shadow disagrees with the real context, once) disables
    // continuation for the decoder for good: later seeks match again, and
    // still every target seeks, until the decoder is replaced; then the
    // reader continues again. Frames are early in a GOP (`base` + 1 ..), so
    // the recovery hops share their anchor and are asserted to continue, not
    // just to agree.
    let mismatch_once = |replace: Event, base: i64| {
        vec![
            Prime(1),
            Go(1, 2, Auto), // before the bad seek this continues
            Do(Event::MismatchOnce),
            Prime(1), // the bad seek
            Go(1, 2, Is(Seek)),
            Prime(1), // a later seek whose anchors match
            Go(1, 2, Is(Seek)),
            Go(2, 3, Is(Seek)),
            Go(3, 4, Is(Seek)),
            Do(replace),
            Go(base, base + 1, Is(Seek)), // the new decoder is not at `base`
            Go(base + 1, base + 2, Continues),
            Go(base + 2, base + 3, Auto),
        ]
    };
    let (first_a, first_b) = (cx.a.fx.facts.keys[0].1, cx.b.fx.facts.keys[0].1);
    // Frame 33's timestamp is missing or repeated; a run that has not met it
    // continues, one that has must seek (S-2 rule 3), and so must every later
    // run anchored before it; a run anchored past it continues again.
    let stamp = |fault: fn(i64) -> Tamper| {
        vec![
            Do(Event::Tamper(fault(cx.a.pts_of(33)))),
            Prime(28),
            Go(28, 30, Auto),
            Go(30, 34, Is(Seek)),
            Go(34, 35, Is(Seek)),
            Go(35, 50, Is(Seek)),
            Go(50, 51, Auto),
            Go(51, 52, Auto),
        ]
    };
    vec![
        // An edit remaps source time: forward within reach, then back.
        (
            "edit",
            vec![
                Prime(20),
                Go(20, 21, Auto),
                Go(21, 5, Is(Seek)),
                Go(5, 6, Auto),
            ],
        ),
        (
            "relink",
            vec![
                Prime(30),
                Go(30, 31, Auto),
                Do(Event::Relink(cx.b.fx.clone())),
                Go(30, 31, Is(Seek)),
                Go(31, 32, Auto),
            ],
        ),
        (
            "jump cut",
            vec![
                Prime(10),
                Go(10, 11, Auto),
                Go(11, 60, Is(Seek)), // c + 49 is outside the window
                Go(60, 61, Auto),
                Go(61, 12, Is(Seek)), // backward
                Go(12, 13, Auto),
            ],
        ),
        (
            "cancel",
            vec![
                Prime(10),
                Do(Event::Cancel(cancel_after)),
                Cancelled(10, 14, cancel_after),
                Go(10, 14, Is(Seek)),
                Go(14, 15, Auto),
            ],
        ),
        // Near the end of the stream the flush (and frame threads, at every
        // count the witnesses run) leaves several frames decoded and waiting
        // when the flag is raised: 89 is the last frame, so from 86 the
        // window's frames come out of the decoder together after one packet
        // (or none). S-2 checks between produced frames, so the run ends with
        // exactly the frames received up to the check that saw the flag.
        (
            "cancel with frames waiting",
            vec![
                Prime(cx.a.fx.facts.last - 3),
                Do(Event::Cancel(1)),
                Cancelled(cx.a.fx.facts.last - 3, cx.a.fx.facts.last, 1),
                Go(cx.a.fx.facts.last - 3, cx.a.fx.facts.last, Is(Seek)),
                Prime(cx.a.fx.facts.last - 4),
                Do(Event::Cancel(2)),
                Cancelled(cx.a.fx.facts.last - 4, cx.a.fx.facts.last, 2),
                Go(cx.a.fx.facts.last - 4, cx.a.fx.facts.last - 1, Is(Seek)),
            ],
        ),
        ("key change", reset(Event::KeyChange)),
        ("shrink reopen", reset(Event::ShrinkReopen)),
        ("error", reset(Event::Error)),
        (
            "mismatch once, key change",
            mismatch_once(Event::KeyChange, first_a),
        ),
        (
            "mismatch once, shrink reopen",
            mismatch_once(Event::ShrinkReopen, first_a),
        ),
        ("mismatch once, error", mismatch_once(Event::Error, first_a)),
        (
            "mismatch once, relink",
            mismatch_once(Event::Relink(cx.b.fx.clone()), first_b),
        ),
        ("shadow fails", faulted(Event::ShadowFails)),
        ("shadow mismatch", faulted(Event::ShadowMismatch)),
        ("stream mismatch", faulted(Event::StreamMismatch)),
        ("anchor drifts", faulted(Event::AnchorDrifts)),
        ("missing timestamp", stamp(Tamper::Missing)),
        ("repeated timestamp", stamp(Tamper::Repeat)),
    ]
}

/// The positive control: a correct continuation built in test code, and the
/// only place that can create a `ControlToken`.
mod reference {
    use super::*;

    /// The capability to arm the behavioural cancel seam. The field is private
    /// to this module and so is `new`: no other adapter can make one without
    /// editing this submodule, and one that tries to arm the seam without it
    /// does not compile (`DecoderProbe::arm_between_frames` takes `&ControlToken`).
    pub(crate) struct ControlToken(());

    impl ControlToken {
        fn new() -> Self {
            Self(())
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(super) enum Corrupt {
        Never,
        /// The held lookahead frame is damaged before the call is observed.
        AtOnce,
        /// It is damaged only at the end of the second continuation in a row, so
        /// only a chained hop sees it.
        SecondHop,
    }

    /// One switch per S-2 rule; `Rules::ALL` is the design.
    #[derive(Clone, Copy)]
    #[allow(clippy::struct_excessive_bools, reason = "one switch per rule")]
    pub(super) struct Rules {
        pub(super) pair: bool,
        pub(super) shadow_known: bool,
        pub(super) real_matches: bool,
        /// A mismatch at a seek disables continuation for the decoder for good.
        pub(super) latch: bool,
        pub(super) same_anchor: bool,
        pub(super) timestamps: bool,
        pub(super) window: bool,
        pub(super) stream: bool,
        pub(super) events: bool,
        pub(super) cancel: CancelAt,
        pub(super) corrupt: Corrupt,
    }

    impl Rules {
        pub(in super::super) const ALL: Self = Self {
            pair: true,
            shadow_known: true,
            real_matches: true,
            latch: true,
            same_anchor: true,
            timestamps: true,
            window: true,
            stream: true,
            events: true,
            cancel: CancelAt::Exact,
            corrupt: Corrupt::Never,
        };
    }

    #[derive(Clone, Copy)]
    pub(super) enum Fault {
        ShadowFails,
        ShadowMismatch,
        StreamMismatch,
        AnchorDrifts,
    }

    /// A reader that keeps its decoder, continues under S-2's rules and
    /// otherwise seeks. The continuation is today's `decode_window_sequential`,
    /// judged after the fact against the timestamps it met (rule 3); rule 2
    /// (Amendment R51: the anchor test alone) is judged before it. The shadow
    /// is the fixture's `anchors`. Test-only: S2c-2's is the real one.
    pub(super) struct ReferenceContinuation {
        token: ControlToken,
        fx: Rc<Fixture>,
        threads: usize,
        pub(super) rules: Rules,
        decoder: Option<VideoDecoder>,
        run: Option<Anchor>,
        fault: Option<Fault>,
        cancel: Option<usize>,
        tampers: Vec<Tamper>,
        /// Continuations since the run's seek.
        continued: u32,
        /// A `MismatchOnce` waits for the next seek.
        mismatch_once: bool,
        /// Continuation is disabled for this decoder (a seek's real first
        /// packet disagreed with the shadow's) until it is replaced.
        disabled: bool,
    }

    impl ReferenceContinuation {
        pub(super) fn new(fx: &Rc<Fixture>, threads: usize, rules: Rules) -> Self {
            Self {
                token: ControlToken::new(),
                fx: fx.clone(),
                threads,
                rules,
                decoder: None,
                run: None,
                fault: None,
                cancel: None,
                tampers: Vec::new(),
                continued: 0,
                mismatch_once: false,
                disabled: false,
            }
        }

        /// The shadow's A(t) (`None`: unknown), at a run's seek or later.
        fn shadow(&self, t: i64, at_seek: bool) -> Option<Anchor> {
            let a = anchor(&self.fx.facts, t)?;
            match self.fault {
                Some(Fault::ShadowFails) => None,
                Some(Fault::ShadowMismatch) => Some((a.0 + 1, a.1, a.2)),
                Some(Fault::AnchorDrifts) if !at_seek => Some((a.0 + 1, a.1, a.2)),
                Some(Fault::StreamMismatch) if self.rules.stream => None,
                _ => Some(a),
            }
        }

        fn may_continue(&self, c: i64, t: i64) -> bool {
            let (Some(d), r) = (&self.decoder, self.rules) else {
                return false;
            };
            let known = self.run.is_some() || !r.shadow_known;
            !self.disabled
                && d.state().continuation_at == Some(c + 1)
                && t > c
                && (!r.window || t <= c + 12)
                && (!r.pair || self.fx.facts.kind != Kind::AviDtsGuess)
                && known
                && (!r.real_matches || self.run.is_none() || d.state().first_packet == self.run)
                && (!r.same_anchor || self.shadow(t, false) == self.run)
        }

        /// After a continuation: were S-2's conditions kept while it ran?
        fn run_valid(&self, d: &VideoDecoder) -> bool {
            if self.rules.timestamps {
                let mut prev: Option<i64> = None;
                for f in &d.probe().frames {
                    let Some(ts) = f.ts else { return false };
                    if prev.is_some_and(|p| ts <= p) {
                        return false;
                    }
                    prev = Some(ts);
                }
            }
            // Amendment R51: no key-packet clause; rule 2 is `may_continue`'s
            // anchor test (A(t) = A(t0), the real first packet A(t0)).
            true
        }

        fn seek(&mut self, t: TimeCode, before: Option<Before>) -> Observation {
            let mut d = self.decoder.take().unwrap_or_else(|| {
                let mut d = self.fx.open(self.threads);
                d.probe_mut().tampers.clone_from(&self.tampers);
                d
            });
            let before = before.unwrap_or_else(|| Before::of(&d));
            arm(&mut d, self.cancel.take(), self.rules.cancel);
            d.probe_mut()
                .arm_between_frames(self.rules.cancel != CancelAt::Drain, &self.token);
            let mut cache = FrameCache::new(1);
            let result = d.decode_window(t, t, &mut cache);
            // S2c-5 (lead rulings of 2026-10-06): a retried seek's first
            // packet is the earlier key's, not A(t), so rule 2's latch is
            // skipped there; the run has no anchor and the next frame seeks.
            if retries(&self.fx.facts, t.0) {
                (self.run, self.continued) = (None, 0);
                let observation = observe(&d, &mut cache, t, result, before);
                self.decoder = Some(d);
                return observation;
            }
            let shadow = match anchor(&self.fx.facts, t.0) {
                Some(a) if std::mem::take(&mut self.mismatch_once) => Some((a.0 + 1, a.1, a.2)),
                _ => self.shadow(t.0, true),
            };
            // Rule 2: the real seek's first packet must be the shadow's, else
            // continuation is disabled for the decoder.
            if self.rules.real_matches
                && self.rules.latch
                && shadow.is_some()
                && d.state().first_packet.is_some()
                && d.state().first_packet != shadow
            {
                self.disabled = true;
            }
            (self.run, self.continued) = (shadow, 0);
            let observation = observe(&d, &mut cache, t, result, before);
            self.decoder = Some(d);
            observation
        }
    }

    impl TargetPath for ReferenceContinuation {
        fn control(&self) -> Option<&ControlToken> {
            Some(&self.token)
        }

        fn produce(&mut self, from: Option<Cursor>, t: TimeCode) -> Observation {
            let Some(Cursor(c)) = from.filter(|c| self.may_continue(c.0, t.0)) else {
                return self.seek(t, None);
            };
            let mut d = self.decoder.take().expect("checked by may_continue");
            arm(&mut d, self.cancel.take(), self.rules.cancel);
            d.probe_mut()
                .arm_between_frames(self.rules.cancel != CancelAt::Drain, &self.token);
            let before = Before::of(&d);
            let mut cache = FrameCache::new(13);
            let result = d.decode_window_sequential(TimeCode(c + 1), t, &mut cache);
            if result.is_ok() && !self.run_valid(&d) {
                self.decoder = Some(d);
                return self.seek(t, Some(before)); // abandon: today's seek
            }
            self.continued += 1;
            let damage = match self.rules.corrupt {
                Corrupt::Never => false,
                Corrupt::AtOnce => true,
                Corrupt::SecondHop => self.continued == 2,
            };
            if result.is_ok() && damage {
                d.corrupt_lookahead();
            }
            let observation = observe(&d, &mut cache, t, result, before);
            self.decoder = Some(d);
            observation
        }

        fn event(&mut self, event: Event) {
            let reset = |this: &mut Self| {
                if this.rules.events {
                    (this.decoder, this.run, this.disabled) = (None, None, false);
                }
            };
            match event {
                Event::Cancel(n) => self.cancel = Some(n),
                Event::Tamper(t) => {
                    self.tampers.push(t);
                    self.decoder
                        .iter_mut()
                        .for_each(|d| d.probe_mut().tampers.push(t));
                }
                Event::ShadowFails => self.fault = Some(Fault::ShadowFails),
                Event::ShadowMismatch => self.fault = Some(Fault::ShadowMismatch),
                Event::StreamMismatch => self.fault = Some(Fault::StreamMismatch),
                Event::AnchorDrifts => self.fault = Some(Fault::AnchorDrifts),
                Event::MismatchOnce => self.mismatch_once = true,
                Event::Relink(fx) => {
                    self.fx = fx;
                    reset(self);
                }
                Event::KeyChange | Event::ShrinkReopen | Event::Error => reset(self),
            }
        }
    }

    /// Test doubles that keep the reference's decoder behaviour but are
    /// presented as production adapters (no token claimed): they show the
    /// harness catching a seam that a production path must not have.
    ///
    /// `KeepsArmed` leaves the seam armed after the call.
    pub(super) struct KeepsArmed(pub(super) ReferenceContinuation);

    impl TargetPath for KeepsArmed {
        fn produce(&mut self, from: Option<Cursor>, t: TimeCode) -> Observation {
            self.0.produce(from, t)
        }

        fn event(&mut self, event: Event) {
            self.0.event(event);
        }
    }

    /// `ArmsThenClears` arms the seam during decoding and clears it again
    /// before `observe`, so a snapshot of the flag at observation time is off.
    pub(super) struct ArmsThenClears {
        token: ControlToken,
        fx: Rc<Fixture>,
        threads: usize,
        cancel: Option<usize>,
    }

    impl ArmsThenClears {
        pub(super) fn new(fx: &Rc<Fixture>, threads: usize) -> Self {
            Self {
                token: ControlToken::new(),
                fx: fx.clone(),
                threads,
                cancel: None,
            }
        }
    }

    impl TargetPath for ArmsThenClears {
        fn produce(&mut self, _from: Option<Cursor>, t: TimeCode) -> Observation {
            let mut decoder = self.fx.open(self.threads);
            arm(&mut decoder, self.cancel.take(), CancelAt::Exact);
            decoder.probe_mut().arm_between_frames(true, &self.token);
            let before = Before::of(&decoder);
            let mut cache = FrameCache::new(1);
            let result = decoder.decode_window(t, t, &mut cache);
            decoder.probe_mut().arm_between_frames(false, &self.token);
            observe(&decoder, &mut cache, t, result, before)
        }

        fn event(&mut self, event: Event) {
            if let Event::Cancel(n) = event {
                self.cancel = Some(n);
            }
        }
    }
}

pub(crate) use reference::ControlToken;

#[cfg(test)]
mod tests {
    use super::reference::{ArmsThenClears, Corrupt, KeepsArmed, ReferenceContinuation, Rules};
    use super::*;
    use crate::decode::{OUTPUT_FAULT, OutputFault};

    type ReferenceMutation = fn(&mut Vec<RefFrame>);
    type RuleMutation = fn(&mut Rules);
    type ObservationEdit = fn(&mut Observation);

    use crate::{
        ffmpeg,
        pf1_s2c_fixtures::{CANONICAL, CONVERSION, generated_sha256},
    };

    /// One fixture's pins on one OS: SHA-256 of the generated file, an FNV-1a
    /// digest of what a one-thread fresh Seek observes at every target
    /// (decoder-level state only: timestamps, retained-plane hashes, anchors),
    /// an FNV-1a digest of the returned pixels at those targets (the f16 bits
    /// after the shared conversion), and the `FFmpeg` build (`build_id`) the
    /// state digest was taken with. An empty `file` is a pin still to be
    /// taken on that OS (an error on Linux). Policy, see `pin_check`: the file
    /// hash and the output digest are advisory, the state digest is enforced
    /// when the file and the build match.
    #[derive(Clone, Copy)]
    struct Pin {
        kind: Kind,
        os: &'static str,
        file: &'static str,
        digest: &'static str,
        output: &'static str,
        build: &'static str,
    }

    /// The `FFmpeg` build the Linux pins were taken with.
    const LINUX_BUILD: &str = "avcodec 0x3e0b64 avformat 0x3e0364 avfilter 0xb0464 swscale 0x90164";

    const fn pin(
        kind: Kind,
        file: &'static str,
        digest: &'static str,
        output: &'static str,
    ) -> Pin {
        Pin {
            kind,
            os: "linux",
            file,
            digest,
            output,
            build: LINUX_BUILD,
        }
    }

    /// The Windows pin is taken on the windows-latest run (its first
    /// `PIN-MISS` line prints the observed values). TODO(windows-pins).
    const fn windows_todo(kind: Kind) -> Pin {
        Pin {
            kind,
            os: "windows",
            file: "",
            digest: "",
            output: "",
            build: "",
        }
    }

    /// The linked `FFmpeg` libraries' versions: what the state digest depends
    /// on besides the file.
    fn build_id() -> String {
        format!(
            "avcodec {:#x} avformat {:#x} avfilter {:#x} swscale {:#x}",
            ffmpeg::codec::version(),
            ffmpeg::format::version(),
            ffmpeg::filter::version(),
            ffmpeg::software::scaling::version(),
        )
    }

    const PINS: [Pin; 12] = [
        pin(
            Kind::Default,
            "a7b4e147a02e9bd9beb6cf953356f09eebf0e8c19b8c44d72a79664289e40946",
            "cf7bdfbdfae6dc80",
            "78df7ac42d41d2ac",
        ),
        pin(
            Kind::Pyramid,
            "8731e441728d8d862f29ffcb9b457407d4b34da51c6d9137d836b804caa96ac4",
            "5d808e839b1f37ab",
            "e6917eb44ee9a7b0",
        ),
        pin(
            Kind::EditList,
            "61f8c4028e9a8d28d29cff97cf13219c367c2bd5308037f1f893781eadb84f7d",
            "3b36d3396bb99343",
            "66275c01e8f692fb",
        ),
        pin(
            Kind::OpenGop,
            "2ba78079067b3fcceab67b6a0d860bd48a92d8d19c2bdf3f1d51e9aa02d8a9a9",
            "ce0029f0ab06abf5",
            "854e56e5b3109248",
        ),
        pin(
            Kind::Vfr,
            "68e21cfe209d739d56091fdb2baa1b47db4e11a4fa039de26a73d138956f2a7f",
            "9f17e10c9c5ee744",
            "6d54cc1039c3d0d7",
        ),
        pin(
            Kind::AviDtsGuess,
            "73eb51ed1e3a59d3c50c18cbe503fb9891e119edfda0bd739471bc3b1afba967",
            "97ddeeddc1a749c3",
            "6d9188ca1ed4507f",
        ),
        windows_todo(Kind::Default),
        windows_todo(Kind::Pyramid),
        windows_todo(Kind::EditList),
        windows_todo(Kind::OpenGop),
        windows_todo(Kind::Vfr),
        windows_todo(Kind::AviDtsGuess),
    ];

    fn corpora(threads: &[usize]) -> Vec<Corpus> {
        KINDS.iter().map(|k| Corpus::new(*k, threads)).collect()
    }

    fn fnv(text: &str) -> u64 {
        text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        })
    }

    /// The returned pixels at every target, from the one-thread Seek.
    fn output_digest(c: &Corpus) -> u64 {
        let rows: Vec<_> = (c.targets.iter().zip(&c.oracle[&1]))
            .flat_map(|(g, (o, o2))| {
                [Some((g.t, o)), g.t2.zip(o2.as_ref())]
                    .into_iter()
                    .flatten()
                    .map(|(t, o)| format!("{t} {:?}", o.frame))
            })
            .collect();
        fnv(&rows.join("\n"))
    }

    /// The digest pinned per file: what the one-thread Seek observed, minus
    /// the returned-pixel hash (pinned separately: `output_digest`).
    fn digest(c: &Corpus) -> u64 {
        let rows: Vec<_> = (c.targets.iter().zip(&c.oracle[&1]))
            .flat_map(|(g, (o, o2))| {
                [Some((g.t, o)), g.t2.zip(o2.as_ref())]
                    .into_iter()
                    .flatten()
                    .map(|(t, o)| format!("{t} {:?} {:?} {:?}", o.pts, o.state, o.err))
            })
            .collect();
        fnv(&rows.join("\n"))
    }

    #[test]
    fn fixtures_have_the_shape_their_names_promise() {
        for kind in KINDS {
            Fixture::new(kind).assert_shape();
        }
    }

    /// The shape checks reject a substitute that lacks the property: each
    /// file kind generated as another kind would be fails `assert_shape`.
    #[test]
    fn a_substitute_fixture_fails_the_shape_check() {
        let substitutes = [
            (Kind::Pyramid, Kind::Default),
            (Kind::EditList, Kind::Default),
            (Kind::OpenGop, Kind::Default),
            (Kind::Vfr, Kind::Default),
            (Kind::AviDtsGuess, Kind::Default),
            (Kind::Default, Kind::OpenGop),
            (Kind::Default, Kind::Pyramid),
            (Kind::Default, Kind::Vfr),
            (Kind::Default, Kind::EditList),
        ];
        for (kind, made_like) in substitutes {
            let fx = Fixture::new_as(kind, made_like);
            let rejected =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fx.assert_shape()));
            let e = rejected.expect_err(&format!(
                "{kind:?} made like {made_like:?} passed the shape check"
            ));
            let e = e.downcast_ref::<String>().cloned().unwrap_or_default();
            eprintln!("pf1-c4 mutation shape {kind:?} made like {made_like:?}: {e}");
        }
    }

    /// The witnesses are only as strong as the oracle's coverage: every §8
    /// special present, real anchor boundaries, EOF hit, chained hops, no
    /// errors, varied frames (a constant oracle would pass anything), and
    /// the real seek's first packet equal to the shadow's at every target (the
    /// retried anchor at an S2c-5 retried target).
    #[test]
    fn the_oracle_covers_the_special_targets() {
        for c in corpora(&[1]) {
            let (kind, facts) = (c.fx.kind, &c.fx.facts);
            let pairs: Vec<(i64, i64)> = c.targets.iter().map(|g| (g.c, g.t)).collect();
            let pair = |c: i64, t: i64| pairs.contains(&(c, t));
            assert!(c.targets.len() >= 50, "{kind:?}");
            for k in [
                "eof",
                "c+1",
                "c+12",
                "c+13 (outside)",
                "second-hop-across-key",
            ] {
                assert!(c.targets.iter().any(|g| g.kind == k), "{kind:?} lacks {k}");
            }
            assert!(
                c.targets.iter().filter(|g| g.t2.is_some()).count() >= 40,
                "{kind:?}"
            );
            let lacking: Vec<_> = (c.targets.iter().filter(|g| g.t2.is_none()))
                .map(|g| (g.kind, g.c, g.t))
                .collect();
            eprintln!(
                "pf1-c4 second hops {kind:?}: {} of {} targets have none: {lacking:?}",
                lacking.len(),
                c.targets.len()
            );
            for &(_, k) in facts.keys.iter().filter(|k| k.1 > 1) {
                assert!(pair(k - 2, k - 1) && pair(k - 1, k), "{kind:?} key {k}");
            }
            let last = usize::try_from(facts.last).unwrap();
            let moves: Vec<i64> = (1..=last)
                .filter(|&f| facts.anchors[f] != facts.anchors[f - 1])
                .map(|f| i64::try_from(f).unwrap())
                .collect();
            assert!(moves.len() >= 3, "{kind:?}: anchor moves at {moves:?}");
            for f in moves {
                let ok = [f - 1, f, f + 1].iter().all(|&t| pair(t - 1, t));
                assert!(ok, "{kind:?} boundary {f}");
            }
            let oracle = &c.oracle[&1];
            let all = || {
                oracle
                    .iter()
                    .flat_map(|(o, o2)| [Some(o), o2.as_ref()])
                    .flatten()
            };
            assert!(all().all(|o| o.err.is_none() && o.route == Route::Seek));
            assert!(
                all().any(|o| o.state.eof_sent),
                "{kind:?} never reaches EOF"
            );
            assert!(all().any(|o| o.state.lookahead.is_some()), "{kind:?}");
            let distinct: std::collections::BTreeSet<_> = all().map(|o| o.frame).collect();
            assert!(
                distinct.len() >= 25,
                "{kind:?}: {} distinct frames",
                distinct.len()
            );
            // Rule 1's premise, observed: the real seek's first packet is A(t);
            // for a retried target (S2c-5, lead rulings of 2026-10-06) it is
            // the retried anchor, the key's before A(t), instead.
            let first = |t: i64| {
                if retries(facts, t) {
                    retried_anchor(facts, t)
                } else {
                    anchor(facts, t)
                }
            };
            for (g, (o, o2)) in c.targets.iter().zip(oracle) {
                assert_eq!(o.state.first_packet, first(g.t), "{kind:?} {g:?}");
                if let (Some(t2), Some(o2)) = (g.t2, o2) {
                    assert_eq!(o2.state.first_packet, first(t2), "{kind:?} {g:?}");
                }
            }
            eprintln!(
                "pf1-c4 oracle-digest {kind:?} targets={} {:016x}",
                c.targets.len(),
                digest(&c)
            );
        }
    }

    /// Compare the fresh Seek at every frame of `fx` with `reference` (the
    /// CLI's linear decode): returns (matched, unreachable, timestamp guesses
    /// that differ). A seek that cannot reach a frame (open-GOP leading
    /// frames: their references are in the previous GOP) is counted, not
    /// failed; AVI timestamps are guesses, so only the frame's content is
    /// compared there.
    fn cli_check(fx: &Fixture, reference: &[RefFrame]) -> Result<(usize, usize, usize), String> {
        let kind = fx.kind;
        let sha = |plane: &Option<Vec<u8>>| plane.as_deref().map(sha256_bytes);
        let (mut checked, mut unreachable, mut guessed) = (0, 0, 0);
        for t in 1..=fx.facts.last {
            let mut decoder = fx.open(1);
            let mut cache = FrameCache::<WorkingFrame>::new(1);
            decoder
                .decode_window(TimeCode(t), TimeCode(t), &mut cache)
                .unwrap();
            let (state, planes) = (decoder.state(), decoder.retained_planes());
            let held = state.pending.as_ref().expect("a held frame");
            let want = reference
                .iter()
                .rposition(|f| f.grid.is_some_and(|g| g <= t));
            let place = format!("{kind:?} t={t}");
            if kind == Kind::AviDtsGuess {
                let found = reference
                    .iter()
                    .any(|f| Some(&f.sha) == sha(&planes[0]).as_ref());
                if !found {
                    return Err(format!("{place}: a frame the CLI never decoded"));
                }
                guessed += usize::from(want.map(|k| reference[k].ts) != Some(held.pts));
                checked += 1;
                continue;
            }
            let Some(k) = want.filter(|_| held.grid <= t) else {
                unreachable += 1;
                continue;
            };
            if held.pts != reference[k].ts {
                return Err(format!(
                    "{place}: selected timestamp {:?}, CLI {:?}",
                    held.pts, reference[k].ts
                ));
            }
            if sha(&planes[0]).as_deref() != Some(reference[k].sha.as_str()) {
                return Err(format!("{place}: selected frame differs from the CLI's"));
            }
            match (&state.lookahead, reference.get(k + 1)) {
                (Some(l), Some(next)) => {
                    if l.pts != next.ts || sha(&planes[1]).as_deref() != Some(next.sha.as_str()) {
                        return Err(format!(
                            "{place}: lookahead differs from the CLI's next frame"
                        ));
                    }
                }
                (None, None) => {}
                (l, n) => {
                    return Err(format!(
                        "{place}: lookahead {l:?} but CLI next {:?}",
                        n.map(|n| n.ts)
                    ));
                }
            }
            checked += 1;
        }
        Ok((checked, unreachable, guessed))
    }

    /// The oracle is not the decoder agreeing with itself: for every frame,
    /// the fresh Seek's selected frame and its retained frames equal what the
    /// pinned `FFmpeg` CLI's own linear decode produces (`ffprobe -show_frames`
    /// timestamps, `ffmpeg -f framehash` SHA-256 of the raw planes). On the
    /// S-2 pair no frame is unreachable (S2c-5's retry).
    #[test]
    fn the_seek_path_matches_the_cli_reference() {
        for kind in KINDS {
            let fx = Fixture::new(kind);
            let reference = fx.reference();
            let (checked, unreachable, guessed) = cli_check(&fx, &reference).unwrap();
            eprintln!(
                "pf1-c4 cli-reference {kind:?}: {} frames, {checked} seeks matched (AVI: found among \
                 the CLI's frames), {unreachable} unreachable (open-GOP leading), {guessed} \
                 timestamp guesses differ (AVI)",
                reference.len()
            );
            assert!(checked >= 60, "{kind:?} checked {checked}");
            // S2c-5 (lead rulings of 2026-10-06): on the retried pair (every
            // kind but AVI) the retry reaches every frame, open-GOP leading
            // frames included; AVI keeps the assertion it had.
            if kind == Kind::AviDtsGuess {
                assert_eq!(unreachable > 0, kind == Kind::OpenGop, "{kind:?}");
            } else {
                assert_eq!(unreachable, 0, "{kind:?}");
            }
        }
    }

    /// The CLI comparison fails for a wrong reference: another file's frames,
    /// one flipped frame hash, one shifted timestamp.
    #[test]
    fn a_wrong_cli_reference_fails_the_comparison() {
        let (a, b) = (Fixture::new(Kind::Default), Fixture::new(Kind::Vfr));
        cli_check(&a, &a.reference()).unwrap();
        let e = cli_check(&a, &b.reference()).unwrap_err();
        eprintln!("pf1-c4 mutation cli reference of another file: {e}");
        let mutations: [(&str, ReferenceMutation); 3] = [
            ("flipped hash", |r| r[30].sha = "0".repeat(64)),
            ("shifted timestamp", |r| r[30].ts = r[30].ts.map(|t| t + 1)),
            ("dropped frame", |r| drop(r.remove(30))),
        ];
        for (name, mutate) in mutations {
            let mut reference = a.reference();
            mutate(&mut reference);
            let e = cli_check(&a, &reference).unwrap_err();
            eprintln!("pf1-c4 mutation cli reference {name}: {e}");
        }
    }

    /// What a pin comparison did, over the three pins of one file (file hash,
    /// state digest, output digest): compared and equal, compared and
    /// different but only advisory, and not compared.
    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    struct Pinned {
        verified: usize,
        mismatched: usize,
        skipped: usize,
    }

    /// The observed values, as printed for pinning.
    struct Seen<'a> {
        file: &'a str,
        digest: &'a str,
        output: &'a str,
        build: &'a str,
    }

    /// The pin policy for one file on one OS.
    ///
    /// - On Linux a complete pin entry (file hash, state digest, output
    ///   digest and build, all non-empty) is REQUIRED for every fixture kind,
    ///   with or without `CI`: a missing or empty entry is a failure, never a
    ///   skip. Elsewhere (Windows, pins still to be taken) a missing entry is
    ///   a loud skip.
    /// - The file hash is advisory everywhere, Linux CI included: a
    ///   difference (another x264/FFmpeg build made the file) is a
    ///   `PIN-ADVISORY` line and a count, never a failure.
    /// - The state digest is compared only when the file hash and the
    ///   `FFmpeg` build match the pinned ones; then a difference fails (a
    ///   decoder regression cannot move both the path and the oracle).
    ///   Otherwise it is skipped, loudly, and counted.
    /// - The output digest is advisory: the decoder's pixels are already
    ///   checked live against the CLI's conversion and the f64 table.
    ///
    /// What is enforced whatever the pins say, per fixture (`live_oracle`):
    /// the live CLI conversion check and the finite f64 table comparison of
    /// every returned frame, and, in other tests, same-machine thread equality.
    fn pin_check(kind: Kind, os: &str, pin: Option<&Pin>, seen: &Seen) -> Result<Pinned, String> {
        let observed = format!(
            "{kind:?} on {os}: file {} digest {} output {} build [{}]",
            seen.file, seen.digest, seen.output, seen.build
        );
        let complete = pin.filter(|p| {
            [p.file, p.digest, p.output, p.build]
                .iter()
                .all(|v| !v.is_empty())
        });
        let Some(pin) = complete else {
            if os == "linux" {
                return Err(format!(
                    "{kind:?}: no complete Linux pin entry (file hash, state digest, output \
                     digest and build are all required for every fixture); observed {observed}"
                ));
            }
            println!("pf1-c4 PIN-MISS (no pin taken on this OS yet): observed {observed}");
            return Ok(Pinned {
                skipped: 3,
                ..Pinned::default()
            });
        };
        let mut done = Pinned::default();
        let same_file = seen.file == pin.file;
        if same_file {
            done.verified += 1;
        } else {
            done.mismatched += 1;
            println!(
                "pf1-c4 PIN-ADVISORY file hash differs (another FFmpeg/x264 build made it): \
                 observed {observed}, pinned {}",
                pin.file
            );
        }
        if same_file && seen.build == pin.build {
            if seen.digest != pin.digest {
                return Err(format!(
                    "{kind:?}: the fresh-Seek oracle moved on the pinned file and build: digest \
                     {} (pinned {}); observed {observed}",
                    seen.digest, pin.digest
                ));
            }
            done.verified += 1;
        } else {
            println!(
                "pf1-c4 PIN-SKIP state digest not compared (file or FFmpeg build is not the \
                 pinned one): observed {observed}"
            );
            done.skipped += 1;
        }
        if seen.output == pin.output {
            done.verified += 1;
        } else {
            done.mismatched += 1;
            println!(
                "pf1-c4 PIN-ADVISORY output digest differs (historical digest; the live CLI \
                 and table checks are what is enforced): observed {observed}, pinned {}",
                pin.output
            );
        }
        Ok(done)
    }

    /// The fewest distinct returned frames a fixture's live oracle must have
    /// verified (CLI conversion hash and finite f64 table) for its pin
    /// result to count.
    const LIVE_FLOOR: usize = 20;

    /// The live-oracle floor for one fixture: every returned frame of the
    /// one-thread Seek passes the CLI conversion check and the finite table
    /// comparison, and at least `LIVE_FLOOR` distinct frames were verified.
    /// Per fixture, not an aggregate: five good fixtures cannot cover a
    /// sixth that verified nothing.
    fn live_oracle(c: &Corpus) -> Result<usize, String> {
        let kind = c.fx.kind;
        let out = (c.out.as_ref()).ok_or_else(|| format!("{kind:?}: no output oracle"))?;
        let observations = c.oracle[&1]
            .iter()
            .flat_map(|(o, o2)| std::iter::once(o).chain(o2));
        for o in observations {
            out.verify(o).map_err(|e| format!("{kind:?}: {e}"))?;
        }
        let checked = out.verified();
        if checked < LIVE_FLOOR {
            return Err(format!(
                "{kind:?}: the live oracle verified {checked} distinct frames, fewer than \
                 {LIVE_FLOOR}"
            ));
        }
        Ok(checked)
    }

    /// Appends `line` to the CI job summary when there is one, so skips and
    /// advisory mismatches show on a passing run (test output is captured).
    fn summarize(line: &str) {
        println!("{line}");
        if let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY")
            && let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(path)
        {
            use std::io::Write;
            let _ = writeln!(file, "{line}");
        }
    }

    /// Fixture, oracle and returned-pixel pins, per OS, under the policy of
    /// `pin_check`, and the per-fixture live-oracle floor. Prints the
    /// verified, mismatched and skipped counts.
    #[test]
    fn the_pinned_oracle_has_not_drifted() {
        let os = std::env::consts::OS;
        let build = build_id();
        println!("pf1-c4 build [{build}]");
        let (mut total, mut failures) = (Pinned::default(), Vec::new());
        for c in corpora(&[1]) {
            let kind = c.fx.kind;
            let pin = PINS.iter().find(|p| p.kind == kind && p.os == os);
            let (file, digest, output) = (
                c.fx.sha256(),
                format!("{:016x}", digest(&c)),
                format!("{:016x}", output_digest(&c)),
            );
            println!("pf1-c4 pin {kind:?} on {os}: file {file} digest {digest} output {output}");
            let seen = Seen {
                file: &file,
                digest: &digest,
                output: &output,
                build: &build,
            };
            // Every fixture reports before the test fails, so one run prints
            // all the observed values to pin.
            let done = pin_check(kind, os, pin, &seen).unwrap_or_else(|e| {
                println!("pf1-c4 PIN-FAIL {e}");
                failures.push(e);
                Pinned::default()
            });
            match live_oracle(&c) {
                Ok(n) => println!("pf1-c4 live oracle {kind:?}: {n} distinct frames verified"),
                Err(e) => {
                    println!("pf1-c4 LIVE-FAIL {e}");
                    failures.push(e);
                }
            }
            summarize(&format!("pf1-c4 pin {kind:?}: {done:?}"));
            total.verified += done.verified;
            total.mismatched += done.mismatched;
            total.skipped += done.skipped;
            // The production-flags CLI conversion against the bit-exact one:
            // advisory, printed so a portable pixel pin can be judged from
            // CI logs (they are different scalers, so they may differ).
            let (production, canonical) = (
                c.fx.converted(CONVERSION, false),
                c.fx.converted(CANONICAL, false),
            );
            let same = production.iter().zip(&canonical).filter(|(a, b)| a == b);
            println!(
                "pf1-c4 advisory {kind:?}: production-flags CLI conversion equals the bit-exact \
                 one for {} of {} frames",
                same.count(),
                canonical.len()
            );
        }
        summarize(&format!(
            "pf1-c4 pins on {os}: {} verified, {} advisory mismatches, {} skipped",
            total.verified, total.mismatched, total.skipped
        ));
        assert!(failures.is_empty(), "{failures:#?}");
        assert_eq!(
            total.verified + total.mismatched + total.skipped,
            3 * KINDS.len()
        );
    }

    /// The fixture recipe is CPU-independent: with every assembly path off
    /// (`FFmpeg` `-cpuflags 0`, x264 `asm=0`) each file is byte-identical to
    /// the one generated with the CPU's own SIMD, so the file hash (advisory)
    /// and the state digest that depends on it are comparable across machines
    /// of the same architecture. Evidence is from x86-64 with and without asm
    /// only; it is not a claim about other architectures.
    #[test]
    fn the_fixture_files_do_not_depend_on_the_cpu() {
        for kind in KINDS {
            let (full, limited) = (generated_sha256(kind, false), generated_sha256(kind, true));
            println!("pf1-c4 cpu {kind:?}: asm on {full}, asm off {limited}");
            assert_eq!(full, limited, "{kind:?}: generation depends on the CPU");
        }
    }

    /// The bit-exact CLI conversion gives the same bytes with and without
    /// `FFmpeg`'s assembly, on every fixture. The production-flags conversion
    /// (`CONVERSION`) is compared the same way and reported, not asserted
    /// (swscale has no accelerated yuv420p to rgba64le path here, so it is
    /// expected to match on x86-64; other architectures are untested).
    #[test]
    fn the_canonical_conversion_does_not_depend_on_the_cpu() {
        for kind in KINDS {
            let fx = Fixture::new(kind);
            let (on, off) = (
                fx.converted(CANONICAL, false),
                fx.converted(CANONICAL, true),
            );
            assert_eq!(
                on, off,
                "{kind:?}: the bit-exact conversion depends on the CPU"
            );
            let (p_on, p_off) = (
                fx.converted(CONVERSION, false),
                fx.converted(CONVERSION, true),
            );
            let same = p_on.iter().zip(&p_off).filter(|(a, b)| a == b).count();
            println!(
                "pf1-c4 cpu {kind:?}: canonical conversion identical for {} of {} frames with asm \
                 on and off; production-flags conversion identical for {same} of {}",
                on.len(),
                on.len(),
                p_on.len()
            );
        }
    }

    /// The policy of `pin_check`, one line per branch. Linux requires a
    /// complete pin for every kind (absent, or with an empty file, digest,
    /// output or build: failure, with or without `CI`). A moved state digest
    /// on the pinned file and build fails. A different file hash never fails
    /// (advisory), even where CI would have enforced it, and skips the state
    /// digest; so does a different build. The output digest never fails. A
    /// missing Windows entry is a skip.
    #[test]
    fn the_pin_check_follows_the_policy() {
        let pin = &PINS[0];
        let kind = pin.kind;
        let good = Seen {
            file: pin.file,
            digest: pin.digest,
            output: pin.output,
            build: pin.build,
        };
        let counts = |verified, mismatched, skipped| {
            Ok(Pinned {
                verified,
                mismatched,
                skipped,
            })
        };
        let line = |branch: &str, r: &Result<Pinned, String>| {
            let r = r
                .as_ref()
                .map_or_else(|e| format!("FAILS: {e}"), |p| format!("{p:?}"));
            eprintln!(
                "pf1-c4 mutation pin policy, {branch}: {}",
                r.chars().take(230).collect::<String>()
            );
        };
        let r = pin_check(kind, "linux", Some(pin), &good);
        assert_eq!(r, counts(3, 0, 0));
        line("all match", &r);
        let moved = Seen {
            digest: "0123456789abcdef",
            ..good
        };
        let r = pin_check(kind, "linux", Some(pin), &moved);
        assert!(r.as_ref().unwrap_err().contains("oracle moved"));
        line("state digest moved on the pinned file and build", &r);
        let moved = Seen {
            output: "0123456789abcdef",
            ..good
        };
        let r = pin_check(kind, "linux", Some(pin), &moved);
        assert_eq!(r, counts(2, 1, 0));
        line("output digest moved (advisory)", &r);
        let other = Seen {
            file: "another file",
            digest: "0123456789abcdef",
            output: "0123456789abcdef",
            ..good
        };
        let r = pin_check(kind, "linux", Some(pin), &other);
        assert_eq!(r, counts(0, 2, 1));
        line(
            "file hash differs (advisory, never a failure, Linux CI included)",
            &r,
        );
        let elsewhere = Seen {
            build: "another build",
            digest: "0123456789abcdef",
            ..good
        };
        let r = pin_check(kind, "linux", Some(pin), &elsewhere);
        assert_eq!(r, counts(2, 0, 1));
        line("build differs (state digest skipped)", &r);
        let r = pin_check(kind, "linux", None, &good);
        assert!(r.as_ref().unwrap_err().contains("no complete Linux pin"));
        line("Linux entry absent", &r);
        for (field, empty) in [
            ("file", Pin { file: "", ..*pin }),
            ("digest", Pin { digest: "", ..*pin }),
            ("output", Pin { output: "", ..*pin }),
            ("build", Pin { build: "", ..*pin }),
        ] {
            let r = pin_check(kind, "linux", Some(&empty), &good);
            assert!(
                r.as_ref().unwrap_err().contains("no complete Linux pin"),
                "{field}"
            );
            line(&format!("Linux entry with an empty {field}"), &r);
        }
        let windows = PINS.iter().find(|p| p.os == "windows").unwrap();
        for entry in [Some(windows), None] {
            let r = pin_check(kind, "windows", entry, &other);
            assert_eq!(r, counts(0, 0, 3));
            line("Windows pin not taken (skip)", &r);
        }
        // The table itself has a complete Linux entry for every kind.
        for kind in KINDS {
            let entry = PINS.iter().find(|p| p.kind == kind && p.os == "linux");
            assert!(
                entry.is_some_and(|p| !p.file.is_empty()
                    && !p.digest.is_empty()
                    && !p.output.is_empty()
                    && !p.build.is_empty()),
                "{kind:?}"
            );
        }
    }

    /// The live-oracle floor is per fixture: a fixture whose output oracle is
    /// absent, whose reference is another file's, or that verified too few
    /// frames fails, however many other fixtures pass.
    #[test]
    fn the_live_oracle_floor_is_enforced_per_fixture() {
        let mut c = Corpus::new(Kind::Default, &[1]);
        let n = live_oracle(&c).unwrap();
        assert!(n >= LIVE_FLOOR);
        eprintln!("pf1-c4 mutation live oracle floor, intact Default: {n} distinct frames");
        let oracle = c.out.take();
        let e = live_oracle(&c).unwrap_err();
        eprintln!("pf1-c4 mutation live oracle floor, no oracle: {e}");
        let other = Fixture::new(Kind::Vfr);
        c.out = Some(OutputOracle::new(&other.reference()));
        let e = live_oracle(&c).unwrap_err();
        eprintln!("pf1-c4 mutation live oracle floor, another file's reference: {e}");
        drop(oracle);
        // Too few frames verified: a corpus whose one-thread observations carry
        // no returned frames does not meet the floor.
        let mut empty = Corpus::new(Kind::Default, &[1]);
        for (o, o2) in empty.oracle.get_mut(&1).unwrap() {
            for obs in std::iter::once(o).chain(o2.as_mut()) {
                obs.frame = None;
            }
        }
        let e = live_oracle(&empty).unwrap_err();
        assert!(e.contains("fewer than"), "{e}");
        eprintln!("pf1-c4 mutation live oracle floor, nothing verified: {e}");
    }

    fn tick_check(facts: &Facts, scratch: &mut VideoDecoder) -> Result<(), String> {
        let mut real = |tick: i64| scratch.probe_anchor_at(facts.us_of_tick(tick));
        for b in &facts.ticks {
            for (tick, want) in [
                (b.tick - 1, b.before),
                (b.tick, b.after),
                (b.tick + 1, b.after),
            ] {
                let got = real(tick);
                if got != want {
                    return Err(format!(
                        "{:?} tick {tick}: real {got:?}, shadow {want:?}",
                        facts.kind
                    ));
                }
            }
        }
        Ok(())
    }

    /// The corrected anchor boundaries, to the stream tick: at every place
    /// today's anchor changes, the real context's seek (a raw timestamp,
    /// probed on a scratch decoder) picks the shadow's anchor one tick either
    /// side of the boundary. This covers the raw `avformat_seek_file` probe on
    /// the real context against the fixture facts, not the future
    /// continuation's own shadow context (S2c-2 owns a tick test for that). The boundary is where the design says it is
    /// (key index timestamp less `min_corrected_pts + dts_shift`), measured
    /// rather than assumed: it lies between the seek times of two adjacent
    /// frames, and its distance from each is logged.
    #[test]
    fn the_anchor_boundaries_hold_at_the_tick_level() {
        for kind in KINDS {
            let fx = Fixture::new(kind);
            let facts = &fx.facts;
            assert!(
                facts.ticks.len() >= 3,
                "{kind:?}: {} boundaries",
                facts.ticks.len()
            );
            tick_check(facts, &mut fx.open(1)).unwrap();
            for b in &facts.ticks {
                let f = (1..facts.seek_ticks.len())
                    .find(|&f| facts.seek_ticks[f - 1] < b.tick && b.tick <= facts.seek_ticks[f])
                    .unwrap_or_else(|| {
                        panic!("{kind:?}: boundary {} between no two frames", b.tick)
                    });
                eprintln!(
                    "pf1-c4 tick {kind:?} boundary tick {} between frame {} (tick {}) and {f} \
                     (tick {}): {} ticks above the earlier, {} below the later",
                    b.tick,
                    f - 1,
                    facts.seek_ticks[f - 1],
                    facts.seek_ticks[f],
                    b.tick - facts.seek_ticks[f - 1],
                    facts.seek_ticks[f] - b.tick,
                );
            }
        }
    }

    /// A shadow that puts a boundary one tick early or late is caught.
    #[test]
    fn a_shadow_boundary_one_tick_off_fails_the_tick_witness() {
        for kind in [Kind::Default, Kind::Pyramid, Kind::EditList] {
            let fx = Fixture::new(kind);
            for shift in [-1, 1] {
                let mut facts = fx.facts.clone();
                for b in &mut facts.ticks {
                    b.tick += shift;
                }
                let e = tick_check(&facts, &mut fx.open(1)).unwrap_err();
                eprintln!("pf1-c4 mutation tick shift {shift} {kind:?}: {e}");
            }
        }
    }

    /// Frame threads delay the decoder's output but must not change what is
    /// selected or held: the threaded oracles equal the one-thread oracle in
    /// frame, timestamp, retained frames and anchor. (`eof_sent` near the end
    /// of the stream may differ; the count is logged.)
    #[test]
    fn the_seek_selection_does_not_depend_on_the_thread_count() {
        let threads = thread_counts();
        for c in corpora(&threads) {
            for &n in &threads {
                let mut eof = 0;
                for (a, b) in c.oracle[&1].iter().zip(&c.oracle[&n]) {
                    let pairs = [(Some(&a.0), Some(&b.0)), (a.1.as_ref(), b.1.as_ref())];
                    for (one, many) in pairs.into_iter().filter_map(|(x, y)| x.zip(y)) {
                        assert!(
                            one.same_content(many),
                            "{:?} x{n}: {one:?} vs {many:?}",
                            c.fx.kind
                        );
                        eof += usize::from(one.state.eof_sent != many.state.eof_sent);
                    }
                }
                eprintln!(
                    "pf1-c4 threads {:?} x{n}: eof_sent differs in {eof} observations",
                    c.fx.kind
                );
            }
        }
    }

    #[test]
    fn the_seek_path_reproduces_the_oracle_on_every_fixture() {
        let threads = thread_counts();
        eprintln!("pf1-c4 frame-thread counts run: {threads:?}");
        for c in corpora(&threads) {
            for &n in &threads {
                witness(&mut SeekPath::new(&c.fx, n), &c, n, false).unwrap();
            }
            let verified = c.out.as_ref().unwrap().verified();
            eprintln!(
                "pf1-c4 output oracle {:?}: {verified} distinct returned frames verified against \
                 the CLI conversion and the independent transfer table",
                c.fx.kind
            );
            assert!(verified >= 25, "{:?}: {verified}", c.fx.kind);
        }
    }

    /// The output oracle is not vacuous: another file's CLI conversions, or
    /// every hash altered, fail the comparison on the very frames the Seek
    /// path returns.
    #[test]
    fn a_wrong_output_reference_fails_the_output_oracle() {
        let (a, b) = (Fixture::new(Kind::Default), Fixture::new(Kind::Vfr));
        let mut corpus = Corpus::new(Kind::Default, &[1]);
        let seek = |c: &Corpus| witness(&mut SeekPath::new(&c.fx, 1), c, 1, false);
        seek(&corpus).unwrap();
        corpus.out = Some(OutputOracle::new(&b.reference()));
        let e = seek(&corpus).unwrap_err();
        eprintln!(
            "pf1-c4 mutation output reference of another file: {}",
            e.chars().take(150).collect::<String>()
        );
        let mut altered = a.reference();
        for f in &mut altered {
            f.out = f.out.replace('0', "1");
        }
        corpus.out = Some(OutputOracle::new(&altered));
        let e = seek(&corpus).unwrap_err();
        eprintln!(
            "pf1-c4 mutation output reference hashes altered: {}",
            e.chars().take(150).collect::<String>()
        );
    }

    /// Lead ruling (tap gate): the conversion copy runs only under a C-4
    /// witness's `Tap`; with the tap off the output oracle fails on the
    /// first returned frame, it does not skip it.
    #[test]
    fn the_output_oracle_fails_without_the_conversion_tap() {
        let corpus = Corpus::new(Kind::Default, &[1]);
        let seek = |c: &Corpus| witness(&mut SeekPath::new(&c.fx, 1), c, 1, false);
        seek(&corpus).unwrap();
        let held = crate::decode::CONVERSION_TAP.with(|tap| tap.replace(0));
        let e = seek(&corpus);
        crate::decode::CONVERSION_TAP.with(|tap| tap.set(held));
        let e = e.unwrap_err();
        assert!(e.contains("a returned frame without its conversion"), "{e}");
    }

    /// The tap gate on one decoder: a conversion made with the tap off
    /// records nothing and clears the record before it, so no observation
    /// can pair a returned frame with an earlier frame's conversion.
    #[test]
    fn a_conversion_without_the_tap_clears_the_last_record() {
        let fx = Fixture::new(Kind::Default);
        let (mut decoder, mut cache) = (fx.open(1), FrameCache::<WorkingFrame>::new(1));
        let tap = Tap::on();
        decoder
            .decode_window(TimeCode(5), TimeCode(5), &mut cache)
            .unwrap();
        assert!(decoder.probe().last_conversion.is_some(), "the tap copies");
        drop(tap);
        let conversions = decoder.probe().conversions;
        decoder
            .decode_window(TimeCode(30), TimeCode(30), &mut cache)
            .unwrap();
        assert!(decoder.probe().conversions > conversions, "it converted");
        assert!(decoder.probe().last_conversion.is_none(), "a stale record");
    }

    /// The non-fixture cases already hold for the fresh Seek (cancel: the
    /// stop flag is raised after n received frames), so they are live for
    /// the implementation too; all but the exact stop with frames waiting.
    #[test]
    fn the_scripted_cases_hold_for_the_seek_path() {
        let threads = thread_counts();
        let (a, b) = (
            Corpus::new(Kind::Default, &threads),
            Corpus::new(Kind::EditList, &threads),
        );
        for &n in &threads {
            let cx = Ctx {
                a: &a,
                b: &b,
                threads: n,
            };
            for (name, steps) in scripts(&cx, 2) {
                // Today's decoder checks the flag between packets, so it
                // drains frames already waiting (or completes the window at
                // the end of the stream): the exact stop is S-2's, for the
                // continuation, and the scripts below assert it there.
                if name == "cancel with frames waiting" {
                    continue;
                }
                run_script(&mut SeekPath::new(&a.fx, n), &cx, &steps, false)
                    .unwrap_or_else(|e| panic!("{name} x{n}: {e}"));
            }
        }
    }

    /// The six corpora, built once for a set of thread counts.
    struct Suite {
        corpora: Vec<Corpus>,
    }

    impl Suite {
        fn new(threads: &[usize]) -> Self {
            Self {
                corpora: corpora(threads),
            }
        }

        fn of(&self, kind: Kind) -> &Corpus {
            self.corpora.iter().find(|c| c.fx.kind == kind).unwrap()
        }
    }

    /// Run every witness (the scripts over the default and edit-list files,
    /// then every corpus, at each thread count) on a reference continuation
    /// with `rules`; the first failure.
    fn run_all(
        suite: &Suite,
        rules: Rules,
        cancel_after: usize,
        threads: &[usize],
    ) -> Result<(), String> {
        let (a, b) = (suite.of(Kind::Default), suite.of(Kind::EditList));
        for &n in threads {
            let cx = Ctx { a, b, threads: n };
            for (name, steps) in scripts(&cx, cancel_after) {
                let mut path = ReferenceContinuation::new(&a.fx, n, rules);
                run_script(&mut path, &cx, &steps, true)
                    .map_err(|e| format!("script {name} x{n}: {e}"))?;
            }
            for c in &suite.corpora {
                witness(&mut ReferenceContinuation::new(&c.fx, n, rules), c, n, true)?;
            }
        }
        Ok(())
    }

    /// The witnesses are satisfiable: a correct continuation passes every one
    /// (bytes, state, retained frames, routes, cancel) on every file at every
    /// thread count, so the route truth and the design agree with a real decode.
    #[test]
    fn a_correct_continuation_satisfies_every_witness() {
        let threads = thread_counts();
        run_all(&Suite::new(&threads), Rules::ALL, 2, &threads).unwrap();
    }

    /// The same reference with exactly one rule removed fails the witnesses,
    /// each rule at the file or script aimed at it.
    #[test]
    fn a_continuation_that_drops_one_rule_fails() {
        let mutants: [(&str, RuleMutation); 13] = [
            ("pair", |r| r.pair = false),
            ("shadow unknown", |r| r.shadow_known = false),
            ("real anchor unchecked", |r| r.real_matches = false),
            ("mismatch not latched", |r| r.latch = false),
            ("A(t) unchecked", |r| r.same_anchor = false),
            ("timestamps", |r| r.timestamps = false),
            ("window", |r| r.window = false),
            ("stream", |r| r.stream = false),
            ("events", |r| r.events = false),
            ("cancel late", |r| r.cancel = CancelAt::Late(1)),
            ("cancel immediate", |r| r.cancel = CancelAt::Immediate),
            ("cancel drains buffered frames", |r| {
                r.cancel = CancelAt::Drain;
            }),
            ("retained pixels", |r| r.corrupt = Corrupt::AtOnce),
        ];
        // The mutants aimed at one script fail at that script.
        let aimed = |name: &str| match name {
            "mismatch not latched" => "mismatch once",
            "cancel drains buffered frames" => "cancel with frames waiting",
            _ => "",
        };
        let suite = Suite::new(&[1, 4]);
        for (name, mutate) in mutants {
            let mut rules = Rules::ALL;
            mutate(&mut rules);
            let e = run_all(&suite, rules, 1, &[1, 4]).expect_err(name);
            assert!(e.contains(aimed(name)), "{name} failed elsewhere: {e}");
            eprintln!(
                "pf1-c4 mutation {name}: {}",
                e.chars().take(140).collect::<String>()
            );
        }
    }

    /// Every replacement script aims at the latch on its own: a mismatch that
    /// is not latched (a later matching seek re-enables continuation), or a
    /// decoder that is never marked replaced (no recovery), fails each of the
    /// four scripts, not just the first.
    #[test]
    fn each_replacement_script_needs_the_latch_and_the_recovery() {
        let suite = Suite::new(&[1]);
        let (a, b) = (suite.of(Kind::Default), suite.of(Kind::EditList));
        let cx = Ctx { a, b, threads: 1 };
        let all = scripts(&cx, 1);
        let mismatch: Vec<_> = all
            .iter()
            .filter(|(name, _)| name.starts_with("mismatch once"))
            .collect();
        assert_eq!(mismatch.len(), 4);
        let mutants: [(&str, RuleMutation); 2] = [
            ("latch", |r| r.latch = false),
            ("events", |r| r.events = false),
        ];
        for (rule, mutate) in mutants {
            let mut rules = Rules::ALL;
            mutate(&mut rules);
            for (name, steps) in &mismatch {
                let good = &mut ReferenceContinuation::new(&a.fx, 1, Rules::ALL);
                run_script(good, &cx, steps, true).unwrap_or_else(|e| panic!("{name}: {e}"));
                let e = run_script(
                    &mut ReferenceContinuation::new(&a.fx, 1, rules),
                    &cx,
                    steps,
                    true,
                )
                .expect_err(name);
                eprintln!(
                    "pf1-c4 mutation {rule} off, script {name}: {}",
                    e.chars().take(120).collect::<String>()
                );
            }
        }
    }

    /// Damage to the retained lookahead frame that only a later hop consumes
    /// (the second continuation in a row) is missed without the chained hops.
    #[test]
    fn damage_that_only_a_chained_hop_sees_needs_the_chained_hops() {
        let suite = Suite::new(&[1]);
        let mut rules = Rules::ALL;
        rules.corrupt = Corrupt::SecondHop;
        for c in &suite.corpora {
            let path = || ReferenceContinuation::new(&c.fx, 1, rules);
            witness_hops(&mut path(), c, 1, true, false).unwrap();
        }
        let e = suite.corpora.iter().find_map(|c| {
            witness_hops(
                &mut ReferenceContinuation::new(&c.fx, 1, rules),
                c,
                1,
                true,
                true,
            )
            .err()
        });
        eprintln!(
            "pf1-c4 mutation second-hop damage (first hops alone pass): {}",
            e.expect("chained hops catch it")
                .chars()
                .take(140)
                .collect::<String>()
        );
    }

    /// Sets the thread's output fault for its lifetime.
    struct FaultGuard;

    impl FaultGuard {
        fn set(fault: OutputFault) -> Self {
            OUTPUT_FAULT.with(|f| f.set(Some(fault)));
            Self
        }
    }

    impl Drop for FaultGuard {
        fn drop(&mut self) {
            OUTPUT_FAULT.with(|f| f.set(None));
        }
    }

    /// A fault in the output path the continuation and the Seek share (the
    /// conversion returns wrong RGBA64, or the working-frame stage receives
    /// it) makes both return the same wrong pixels: every comparison with the
    /// Seek path passes, on the Seek path itself and on a correct
    /// continuation. Only the independent output oracle (the CLI's own
    /// conversion of the frame, and the table-derived working frame) sees it.
    #[test]
    fn a_fault_in_the_shared_output_path_is_seen_only_by_the_output_oracle() {
        for (name, fault) in [
            ("conversion", OutputFault::Conversion),
            ("working frame", OutputFault::WorkingFrame),
            ("NaN sample", OutputFault::Nan),
        ] {
            let _fault = FaultGuard::set(fault);
            for kind in [Kind::Default, Kind::AviDtsGuess] {
                let mut corpus = Corpus::new(kind, &[1]);
                let oracle = corpus.out.take();
                let seek = |c: &Corpus| witness(&mut SeekPath::new(&c.fx, 1), c, 1, false);
                let path = ReferenceContinuation::new;
                let continued = |c: &Corpus| witness(&mut path(&c.fx, 1, Rules::ALL), c, 1, true);
                seek(&corpus).unwrap_or_else(|e| panic!("{name} {kind:?}: Seek: {e}"));
                continued(&corpus).unwrap_or_else(|e| panic!("{name} {kind:?}: continued: {e}"));
                corpus.out = oracle;
                for (who, e) in [
                    (
                        "Seek path",
                        seek(&corpus).expect_err("Seek path with the oracle"),
                    ),
                    (
                        "continuation",
                        continued(&corpus).expect_err("continuation with the oracle"),
                    ),
                ] {
                    eprintln!(
                        "pf1-c4 mutation shared output fault {name} {kind:?} ({who}; comparison with \
                         the Seek path alone passes): {}",
                        e.chars().take(150).collect::<String>()
                    );
                }
                let digest = format!("{:016x}", output_digest(&corpus));
                let clean = PINS.iter().find(|p| p.kind == kind && p.os == "linux");
                eprintln!(
                    "pf1-c4 mutation shared output fault {name} {kind:?}: output digest {digest}, \
                     pinned {:?}",
                    clean.map(|p| p.output)
                );
            }
        }
    }

    /// A path that is not the positive control (it holds no `ControlToken`)
    /// but whose decoder had the test-only cancel seam armed fails the
    /// harness, whether it left the seam armed (`KeepsArmed`) or armed it
    /// during decoding and cleared it before the observation
    /// (`ArmsThenClears`, invisible to a snapshot of the flag). The control
    /// itself passes. Claiming control without a token, or arming without
    /// one, does not compile; there is no runtime test for that.
    #[test]
    fn a_production_adapter_may_not_arm_the_cancel_seam() {
        let c = Corpus::new(Kind::Default, &[1]);
        let control = ReferenceContinuation::new(&c.fx, 1, Rules::ALL);
        assert!(control.control().is_some());
        witness(
            &mut ReferenceContinuation::new(&c.fx, 1, Rules::ALL),
            &c,
            1,
            true,
        )
        .unwrap();
        let e = witness(
            &mut KeepsArmed(ReferenceContinuation::new(&c.fx, 1, Rules::ALL)),
            &c,
            1,
            true,
        )
        .unwrap_err();
        assert!(e.contains("control token"), "{e}");
        eprintln!("pf1-c4 mutation adapter keeps the seam armed (witness): {e}");
        let mut clears = ArmsThenClears::new(&c.fx, 1);
        assert!(clears.control().is_none());
        let e = witness(&mut clears, &c, 1, false).unwrap_err();
        assert!(e.contains("control token"), "{e}");
        eprintln!("pf1-c4 mutation adapter arms then clears the seam (witness): {e}");
        let (a, b) = (c, Corpus::new(Kind::EditList, &[1]));
        let cx = Ctx {
            a: &a,
            b: &b,
            threads: 1,
        };
        let (name, steps) = scripts(&cx, 2)
            .into_iter()
            .find(|(name, _)| *name == "cancel with frames waiting")
            .expect("the exact-stop script");
        let mut path = KeepsArmed(ReferenceContinuation::new(&a.fx, 1, Rules::ALL));
        let e = run_script(&mut path, &cx, &steps, true).unwrap_err();
        assert!(e.contains("control token"), "{name}: {e}");
        eprintln!("pf1-c4 mutation adapter keeps the seam armed ({name}): {e}");
        let e = run_script(&mut ArmsThenClears::new(&a.fx, 1), &cx, &steps, false).unwrap_err();
        assert!(e.contains("control token"), "{name}: {e}");
        eprintln!("pf1-c4 mutation adapter arms then clears the seam ({name}): {e}");
    }

    // ---- wrong paths (kept from the first round) ----

    /// A path that is wrong on purpose: `retarget` changes what is decoded,
    /// `rewrite` changes what is reported.
    struct Wrong {
        inner: SeekPath,
        retarget: fn(&Fixture, i64) -> i64,
        rewrite: fn(&mut Observation),
    }

    impl TargetPath for Wrong {
        fn produce(&mut self, from: Option<Cursor>, t: TimeCode) -> Observation {
            let t = TimeCode((self.retarget)(&self.inner.fx, t.0));
            let mut observation = self.inner.produce(from, t);
            (self.rewrite)(&mut observation);
            observation
        }

        fn event(&mut self, event: Event) {
            self.inner.event(event);
        }
    }

    fn wrong(
        kind: Kind,
        retarget: fn(&Fixture, i64) -> i64,
        rewrite: fn(&mut Observation),
    ) -> Result<(), String> {
        let c = Corpus::new(kind, &[1]);
        let inner = SeekPath::new(&c.fx, 1);
        witness(
            &mut Wrong {
                inner,
                retarget,
                rewrite,
            },
            &c,
            1,
            false,
        )
    }

    #[test]
    fn a_path_that_skips_a_frame_fails() {
        let error = wrong(Kind::Default, |_, t| t + 1, |_| {}).unwrap_err();
        assert!(error.contains("!= oracle"), "{error}");
    }

    #[test]
    fn a_path_that_ignores_the_edit_list_offset_fails_on_the_edit_list_file_only() {
        let ignore = |fx: &Fixture, t: i64| (t - fx.facts.offset_frames).max(0);
        wrong(Kind::EditList, ignore, |_| {}).unwrap_err();
        assert!(Fixture::new(Kind::EditList).facts.offset_frames > 0);
        // Control: with no offset the same wrong path is the Seek path.
        wrong(Kind::Default, ignore, |_| {}).unwrap();
    }

    #[test]
    fn a_path_that_returns_the_previous_gop_across_a_key_packet_fails() {
        let previous_gop = |fx: &Fixture, t: i64| {
            let key = fx
                .facts
                .keys
                .iter()
                .rev()
                .find(|k| k.1 <= t)
                .map_or(0, |k| k.1);
            if key > 0 && t - key <= 2 { key - 1 } else { t }
        };
        wrong(Kind::Default, previous_gop, |_| {}).unwrap_err();
        wrong(Kind::OpenGop, previous_gop, |_| {}).unwrap_err();
    }

    /// The state fields are compared, not just the pixels: each scalar, the
    /// retained frames' timestamps and content hashes, and the anchor.
    #[test]
    fn a_path_with_the_right_frame_and_wrong_state_fails() {
        let same = |_: &Fixture, t: i64| t;
        wrong(Kind::Vfr, same, |o| o.state.lookahead = None).unwrap_err();
        wrong(Kind::Vfr, same, |o| o.state.eof_sent = false).unwrap_err();
        wrong(Kind::Vfr, same, |o| o.state.continuation_at = None).unwrap_err();
        wrong(Kind::Vfr, same, |o| o.state.fallback_index += 1).unwrap_err();
        wrong(Kind::Vfr, same, |o| o.pts = o.pts.map(|p| p + 1)).unwrap_err();
        wrong(Kind::Vfr, same, |o| o.state.first_packet = None).unwrap_err();
        wrong(Kind::Vfr, same, |o| {
            o.state.pending.as_mut().unwrap().hash ^= 1;
        })
        .unwrap_err();
        wrong(Kind::Vfr, same, |o| {
            if let Some(l) = o.state.lookahead.as_mut() {
                l.hash ^= 1;
            }
        })
        .unwrap_err();
        wrong(Kind::Vfr, same, |o| {
            if let Some(l) = o.state.lookahead.as_mut() {
                l.pts = l.pts.map(|p| p + 1);
            }
        })
        .unwrap_err();
    }

    /// Cancellation is asserted by progress and stop position, not by the
    /// error alone: an immediate cancel, a late one and a doctored count
    /// fail. For the S-2 continuation the stop is exact: a frame received
    /// after the one that raised the flag fails, though the packet-boundary
    /// rule (today's Seek) lets it through.
    #[test]
    fn a_cancel_that_is_early_late_or_unproductive_fails() {
        let fx = Rc::new(Fixture::new(Kind::Default));
        let mut path = SeekPath::new(&fx, 1);
        path.event(Event::Cancel(2));
        let seek = path.produce(None, TimeCode(14));
        super::cancelled(&seek, 2, false).unwrap();
        assert!(seek.frames.len() >= 2 && seek.frames[1].packets == seek.packets);
        let mut reference = ReferenceContinuation::new(&fx, 1, Rules::ALL);
        reference.produce(None, TimeCode(10));
        reference.event(Event::Cancel(2));
        let exact = reference.produce(Some(Cursor(10)), TimeCode(14));
        super::cancelled(&exact, 2, true).unwrap();
        assert_eq!(exact.frames.len(), 2);
        let mutations: [(&str, ObservationEdit); 5] = [
            ("no progress", |o| o.frames.clear()),
            ("one frame", |o| o.frames.truncate(1)),
            ("a packet after the stop", |o| o.packets += 1),
            ("not cancelled", |o| o.err = None),
            ("continuation state kept", |o| {
                o.state.continuation_at = Some(15);
            }),
        ];
        for (name, f) in mutations {
            for (real, is_exact) in [(&seek, false), (&exact, true)] {
                let mut o = real.clone();
                f(&mut o);
                let e = super::cancelled(&o, 2, is_exact).unwrap_err();
                eprintln!(
                    "pf1-c4 mutation cancel {name} (exact {is_exact}): {}",
                    e.chars().take(110).collect::<String>()
                );
            }
        }
        // Frames already decoded and waiting: from the last frame's neighbours
        // the stream's end flushes several at once, and only a check between
        // frames stops after the first (a check between packets drains them).
        let last = fx.facts.last;
        let buffered = |how: CancelAt| {
            let mut rules = Rules::ALL;
            rules.cancel = how;
            let mut p = ReferenceContinuation::new(&fx, 1, rules);
            p.produce(None, TimeCode(last - 3));
            p.event(Event::Cancel(1));
            p.produce(Some(Cursor(last - 3)), TimeCode(last))
        };
        let waiting = buffered(CancelAt::Exact);
        super::cancelled(&waiting, 1, true).unwrap();
        let drained = buffered(CancelAt::Drain);
        assert!(drained.frames.len() >= 2, "{:?}", drained.frames);
        assert!(drained.frames.iter().all(|f| f.packets == drained.packets));
        let e = super::cancelled(&drained, 1, true).unwrap_err();
        eprintln!(
            "pf1-c4 mutation cancel drains waiting frames ({} frames, all at packet {}): {}",
            drained.frames.len(),
            drained.packets,
            e.chars().take(110).collect::<String>()
        );
        // A third frame received in the same packet after the stop.
        let mut over = exact.clone();
        over.frames.push(*over.frames.last().unwrap());
        super::cancelled(&over, 2, false).unwrap();
        let e = super::cancelled(&over, 2, true).unwrap_err();
        eprintln!(
            "pf1-c4 mutation cancel a frame after the stop (packet rule passes): {}",
            e.chars().take(110).collect::<String>()
        );
    }

    /// Today's `decode_window_sequential` kept across calls: it continues
    /// whenever its cursor matches and `c < t <= c + 12`, with no anchor,
    /// key-packet or timestamp check. A working implementation of the wiring,
    /// and exactly what S-2 rules 1 to 4 forbid.
    struct NaiveContinuation {
        fx: Rc<Fixture>,
        decoder: Option<VideoDecoder>,
        /// Report the fresh seek's anchor instead of the run's (the state
        /// comparison then sees the frames and timestamps alone).
        mask_anchor: bool,
    }

    impl TargetPath for NaiveContinuation {
        fn produce(&mut self, from: Option<Cursor>, t: TimeCode) -> Observation {
            if let (Some(Cursor(c)), Some(decoder)) = (from, self.decoder.as_mut())
                && decoder.state().continuation_at == Some(c + 1)
                && (c + 1..=c + 12).contains(&t.0)
            {
                let (before, mut cache) = (Before::of(decoder), FrameCache::new(13));
                let result = decoder.decode_window_sequential(TimeCode(c + 1), t, &mut cache);
                let mut o = observe(decoder, &mut cache, t, result, before);
                if self.mask_anchor {
                    o.state.first_packet = anchor(&self.fx.facts, t.0);
                }
                return o;
            }
            let mut decoder = self.fx.open(1);
            let (before, mut cache) = (Before::of(&decoder), FrameCache::new(1));
            let result = decoder.decode_window(t, t, &mut cache);
            let observation = observe(&decoder, &mut cache, t, result, before);
            self.decoder = Some(decoder);
            observation
        }

        fn event(&mut self, event: Event) {
            if let Event::Relink(fx) = event {
                self.fx = fx;
            }
            self.decoder = None;
        }
    }

    /// Continuing across an anchor change (Amendment R51: a key packet read
    /// with the anchor unchanged is no boundary) is caught by the
    /// route check on every file, and by the bytes-and-state comparison
    /// alone on every file too: the run's anchor is part of the state, so a
    /// continuation that carries the old run's first packet across a
    /// boundary differs from the fresh seek's (on the open-GOP file the
    /// frame itself differs as well: the fresh seek yields none at t = 47).
    #[test]
    fn a_continuation_that_ignores_the_domain_fails() {
        for c in corpora(&[1]) {
            let path = |mask_anchor| NaiveContinuation {
                fx: c.fx.clone(),
                decoder: None,
                mask_anchor,
            };
            witness(&mut path(false), &c, 1, true).unwrap_err();
            let bytes = witness(&mut path(false), &c, 1, false).unwrap_err();
            assert!(bytes.contains("!= oracle"), "{:?}: {bytes}", c.fx.kind);
            // With the anchor masked only the frames and timestamps remain:
            // they still differ on the open-GOP and AVI files.
            let masked = witness(&mut path(true), &c, 1, false);
            let differs = matches!(c.fx.kind, Kind::OpenGop | Kind::AviDtsGuess);
            assert_eq!(masked.is_err(), differs, "{:?}: {masked:?}", c.fx.kind);
            // And with the anchor masked the route check alone catches it.
            let routed = witness(&mut path(true), &c, 1, true).unwrap_err();
            assert!(
                routed.contains("S-2 says") || differs,
                "{:?}: {routed}",
                c.fx.kind
            );
        }
    }

    /// See the note on `continuation_holds_the_scripted_cases`: the real path
    /// runs with the cancel seam off and its own per-frame check.
    /// R46 (rereview-c4d.md): the production adapter's file names none of
    /// the positive control's capabilities and builds no `Observation` but
    /// through `observe()` (no literal, no field edited afterwards).
    #[test]
    fn the_continuation_adapter_holds_no_control_capability() {
        let source = include_str!("pf1_s2c_continuation.rs");
        let banned = [
            "reference",
            "ReferenceContinuation",
            "ControlToken",
            "control(",
            "arm_between_frames",
            "probe_mut",
            "seam_arms",
        ];
        for word in banned {
            assert!(
                !source.contains(word),
                "pf1_s2c_continuation.rs mentions `{word}`"
            );
        }
        // `Observation` appears once, as `produce`'s return type.
        assert_eq!(
            source.matches("Observation").count(),
            source.matches("-> Observation {").count() + 1,
            "pf1_s2c_continuation.rs names `Observation` other than in its import and \
             `produce`'s signature"
        );
        assert_eq!(source.matches("-> Observation {").count(), 1);
        let fields = [
            ".frame", ".pts", ".state", ".err", ".route", ".frames", ".packets", ".output",
        ];
        for field in fields {
            assert!(
                !source.contains(field),
                "pf1_s2c_continuation.rs touches an observation's `{field}`"
            );
        }
        assert_eq!(
            source.matches("observe(").count(),
            1,
            "the adapter reports through `observe()`, once"
        );
    }

    #[test]
    fn continuation_reproduces_seek_on_every_fixture() {
        let threads = thread_counts();
        for c in corpora(&threads) {
            for &n in &threads {
                witness(&mut ContinuationPath::new(&c.fx, n), &c, n, true).unwrap();
            }
        }
    }

    /// S2c-2 MUST also show, against the real `ContinuationPath` (which runs
    /// with the `check_between_frames` seam untouched; the harness enforces that),
    /// that deleting its own production per-frame stop check in
    /// `receive_frames` fails the `cancel with frames waiting` script of
    /// `continuation_holds_the_scripted_cases`. The seam models that check
    /// for the positive control only; it cannot stand in for it.
    #[test]
    fn continuation_holds_the_scripted_cases() {
        let threads = thread_counts();
        let (a, b) = (
            Corpus::new(Kind::Default, &threads),
            Corpus::new(Kind::EditList, &threads),
        );
        for &n in &threads {
            let cx = Ctx {
                a: &a,
                b: &b,
                threads: n,
            };
            for (name, steps) in scripts(&cx, 2) {
                run_script(&mut ContinuationPath::new(&a.fx, n), &cx, &steps, true)
                    .unwrap_or_else(|e| panic!("{name} x{n}: {e}"));
            }
        }
    }
}
