//! PF1 S2c C-4: the acceptance witnesses for §8 S-2 (forward continuation),
//! written from the design before any implementation exists (C-5: bytes).
//!
//! The single integration point is [`TargetPath`]. The reader model: a path
//! is one reader on one source. `produce(None, t)` is a cold seek to `t`.
//! `produce(Some(Cursor(c)), t)` is the scheduler saying "this reader's last
//! produced frame is `c`; give me `t`". The path must check that against its
//! own decoder (a reset reader is not at `c`) and either continue forward or
//! seek; the observation says which (`route`, from the decoder's seek count).
//! [`SeekPath`] is today's fresh `decode_window(t, t)` and defines the oracle.
//! The implementer wires [`ContinuationPath`] in S2c-2 and un-ignores its
//! tests; the witnesses, the oracle and the wrong paths may not be weakened.

use std::{
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use kinewright_core::{MediaError, TimeCode};

use crate::{
    cache::FrameCache,
    decode::{DecoderState, VideoDecoder},
    frame::WorkingFrame,
    pf1_s2c_fixtures::{Fixture, KINDS, Kind},
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

/// What one `produce` yields. `same` compares everything except `route`.
#[derive(Clone, Debug)]
pub(super) struct Observation {
    /// FNV-1a of the produced frame's f16 bits (`None`: no frame cached at `t`).
    pub(super) frame: Option<u64>,
    /// The produced frame's `best_effort_timestamp` (`state.pending.1`).
    pub(super) pts: Option<i64>,
    pub(super) state: DecoderState,
    /// `Debug` of the `MediaError`, if the run failed (`"Cancelled"`).
    pub(super) err: Option<String>,
    pub(super) route: Route,
}

impl Observation {
    fn same(&self, other: &Self) -> bool {
        (self.frame, self.pts, &self.state, &self.err)
            == (other.frame, other.pts, &other.state, &other.err)
    }
}

/// Observe `decoder` after a run for `t` (`seeks` = its seek count before it).
pub(super) fn observe(
    decoder: &VideoDecoder,
    cache: &mut FrameCache<WorkingFrame>,
    t: TimeCode,
    result: Result<(), MediaError>,
    seeks: u64,
) -> Observation {
    let state = decoder.state();
    let hit = cache.contains(t);
    let frame = hit.then(|| cache.frame_at_or_before(t)).flatten().map(|f| {
        (f.pixels.iter()).fold(0xcbf2_9ce4_8422_2325_u64, |h, p| {
            (h ^ u64::from(p.to_bits())).wrapping_mul(0x0100_0000_01b3)
        })
    });
    Observation {
        frame,
        pts: state.pending.and_then(|p| p.1),
        state,
        err: result.err().map(|e| format!("{e:?}")),
        route: if decoder.seek_count() > seeks {
            Route::Seek
        } else {
            Route::Continue
        },
    }
}

#[derive(Clone)]
pub(super) enum Event {
    /// The clip is relinked to another file: a new `VideoSourceKey`.
    Relink(Rc<Fixture>),
    /// The key changes but the decoded content does not.
    KeyChange,
    ShrinkReopen,
    /// The reader's decoder hit an error (state must not survive it).
    Error,
    /// Arm cancellation for the next `produce`, after this many produced frames.
    Cancel(usize),
}

pub(super) trait TargetPath {
    fn produce(&mut self, from: Option<Cursor>, t: TimeCode) -> Observation;
    fn event(&mut self, event: Event);
}

/// Today's fresh `Seek`: a new decoder per call, `decode_window(t, t)`.
pub(super) struct SeekPath {
    fx: Rc<Fixture>,
    cancel: Option<usize>,
}

impl SeekPath {
    pub(super) fn new(fx: &Rc<Fixture>) -> Self {
        Self {
            fx: fx.clone(),
            cancel: None,
        }
    }
}

impl TargetPath for SeekPath {
    fn produce(&mut self, _from: Option<Cursor>, t: TimeCode) -> Observation {
        let mut decoder = self.fx.open();
        let stop = Arc::new(AtomicBool::new(false));
        match self.cancel.take() {
            Some(0) => stop.store(true, Ordering::Release),
            Some(_) => unimplemented!("a mid-run cancel needs the S2c-2 hook"),
            None => {}
        }
        decoder.set_stop(stop);
        let mut cache = FrameCache::new(1);
        let result = decoder.decode_window(t, t, &mut cache);
        observe(&decoder, &mut cache, t, result, 0)
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Relink(fx) => self.fx = fx,
            Event::Cancel(n) => self.cancel = Some(n),
            _ => {} // stateless: every call already opens a fresh decoder
        }
    }
}

/// S2c-2 wires this: one reader (a `VideoDecoder` kept across calls).
/// `produce(Some(Cursor(c)), t)` continues from `c` only in the S-2 domain
/// and only if its decoder really is at `c`; otherwise it seeks. It must
/// report through `observe` with the seek count taken before the call.
pub(super) struct ContinuationPath;

impl TargetPath for ContinuationPath {
    fn produce(&mut self, _from: Option<Cursor>, _t: TimeCode) -> Observation {
        unimplemented!("S2c-2 wires continuation")
    }

    fn event(&mut self, _event: Event) {
        unimplemented!("S2c-2 wires continuation")
    }
}

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

/// One S-2 test target: the reader is at `c` and is asked for `t`.
#[derive(Debug)]
pub(super) struct Target {
    c: i64,
    t: i64,
    kind: &'static str,
}

/// A fixture, its 50 seeded targets and the fresh-`Seek` oracle for each.
pub(super) struct Corpus {
    fx: Rc<Fixture>,
    targets: Vec<Target>,
    oracle: Vec<Observation>,
}

fn splitmix(state: &mut u64) -> i64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    i64::try_from((z ^ (z >> 31)) >> 1).unwrap()
}

fn targets(fx: &Fixture) -> Vec<Target> {
    let last = fx.last;
    let mut out: Vec<Target> = Vec::new();
    let mut add = |c: i64, t: i64, kind| {
        if (1..=last).contains(&t) && c >= 0 && !out.iter().any(|x| (x.c, x.t) == (c, t)) {
            out.push(Target { c, t, kind });
        }
    };
    let mid = last / 2;
    // §8 specials: end of stream, the window's edges, c + 1.
    add(last - 1, last, "eof");
    add(last - 2, last - 1, "eof");
    add(mid - 12, mid, "c+12");
    add(mid - 13, mid, "c+13 (outside)");
    for t in [1, 2, 13, mid, mid + 7] {
        add(t - 1, t, "c+1");
    }
    // ±1 frame around every corrected anchor boundary: where today's seek's
    // anchor changes (key index timestamp + min_corrected_pts + dts_shift),
    // and windows that end just before and just past it.
    for f in 1..=usize::try_from(last).unwrap() {
        if fx.anchors[f] != fx.anchors[f - 1] {
            let f = i64::try_from(f).unwrap();
            for t in [f - 1, f, f + 1] {
                add(t - 1, t, "anchor-boundary");
            }
            add(f - 4, f - 1, "boundary-window");
            add(f - 4, f + 1, "boundary-window");
        }
    }
    // Just before and at each key packet, with c + 1.
    for &(_, k) in &fx.keys {
        add(k - 2, k - 1, "before-key");
        add(k - 1, k, "at-key");
    }
    out.truncate(40);
    let mut seed = 0x5EED_0000 + fx.kind as u64;
    while out.len() < 50 {
        let t = 1 + splitmix(&mut seed) % last;
        let (pick, gap) = (splitmix(&mut seed) % 20, splitmix(&mut seed));
        let c = match pick {
            0..=11 => t - 1 - gap % 12, // in the window: c + 1 ..= c + 12
            12..=16 => t - 13 - gap % 20,
            _ => t + gap % 10, // not forward at all
        };
        let c = c.max(0);
        if !out.iter().any(|x| (x.c, x.t) == (c, t)) {
            out.push(Target {
                c,
                t,
                kind: "random",
            });
        }
    }
    out
}

impl Corpus {
    pub(super) fn new(kind: Kind) -> Self {
        let fx = Rc::new(Fixture::new(kind));
        let targets = targets(&fx);
        let oracle = (targets.iter())
            .map(|g| SeekPath::new(&fx).produce(None, TimeCode(g.t)))
            .collect();
        Self {
            fx,
            targets,
            oracle,
        }
    }
}

/// Where S-2 says the reader at `c` producing `t` seeks (`Some(Seek)`) or
/// continues (`Some(Continue)`), from the file alone. `None`: the design
/// leaves it open (the next key packet is within the decoder's reorder depth
/// of `t`), so only byte and state identity is required.
pub(super) fn expected_route(fx: &Fixture, c: i64, t: i64) -> Option<Route> {
    let anchor = |x: i64| {
        usize::try_from(x)
            .ok()
            .and_then(|i| fx.anchors.get(i).copied().flatten())
    };
    if fx.kind == Kind::NoPts
        || t <= c
        || t > c + 12
        || anchor(c).is_none()
        || anchor(c) != anchor(t)
    {
        return Some(Route::Seek); // not the demonstrated domain: today's seek
    }
    let pos = anchor(c)?.0;
    let at = fx.keys.iter().position(|k| k.0 == pos)?;
    let next = fx.keys.get(at + 1).map_or(i64::MAX, |k| k.1);
    match next {
        n if n <= t => Some(Route::Seek), // a key packet before t: abandon
        n if n > t + 6 => Some(Route::Continue),
        _ => None,
    }
}

/// Every target: the path's frame bytes, timestamps and S-2 state equal the
/// oracle's; with `check_route`, it also seeks or continues as S-2 says.
pub(super) fn witness<P: TargetPath>(
    path: &mut P,
    corpus: &Corpus,
    check_route: bool,
) -> Result<(), String> {
    for (g, want) in corpus.targets.iter().zip(&corpus.oracle) {
        path.produce(None, TimeCode(g.c));
        let got = path.produce(Some(Cursor(g.c)), TimeCode(g.t));
        let place = format!("{:?} {g:?}", corpus.fx.kind);
        if !got.same(want) {
            return Err(format!("{place}: {got:?} != oracle {want:?}"));
        }
        let expect = expected_route(&corpus.fx, g.c, g.t).filter(|_| check_route);
        if expect.is_some_and(|r| r != got.route) {
            return Err(format!(
                "{place}: route {:?}, S-2 says {expect:?}",
                got.route
            ));
        }
    }
    Ok(())
}

pub(super) enum Expect {
    Auto,
    Is(Route),
}

pub(super) enum Step {
    /// A cold seek: the reader is now at this frame.
    Prime(i64),
    /// The scheduler believes the reader is at `.0` and asks for `.1`.
    Go(i64, i64, Expect),
    Do(Event),
    /// The armed cancel fires: `Cancelled`, and no continuation state left.
    Cancelled(i64, i64),
}

/// Run `steps` on `path` and on a fresh-`Seek` reference; every produced
/// frame equals the reference's, and (with `check_route`) routes are as
/// expected: `Auto` from S-2, `Is(..)` as stated (after any reset, Seek).
pub(super) fn run_script<P: TargetPath>(
    path: &mut P,
    first: &Rc<Fixture>,
    steps: &[Step],
    check_route: bool,
) -> Result<(), String> {
    let (mut reference, mut fx) = (SeekPath::new(first), first.clone());
    for (n, step) in steps.iter().enumerate() {
        match step {
            Step::Prime(c) => drop(path.produce(None, TimeCode(*c))),
            Step::Go(c, t, expect) => {
                let got = path.produce(Some(Cursor(*c)), TimeCode(*t));
                let want = reference.produce(None, TimeCode(*t));
                if !got.same(&want) {
                    return Err(format!("step {n}: {got:?} != reference {want:?}"));
                }
                let expect = match expect {
                    Expect::Auto => expected_route(&fx, *c, *t),
                    Expect::Is(r) => Some(*r),
                };
                if check_route && expect.is_some_and(|r| r != got.route) {
                    return Err(format!(
                        "step {n}: route {:?}, expected {expect:?}",
                        got.route
                    ));
                }
            }
            Step::Do(event) => {
                if let Event::Relink(next) = event {
                    fx = next.clone();
                    reference.event(event.clone());
                }
                path.event(event.clone());
            }
            Step::Cancelled(c, t) => {
                let got = path.produce(Some(Cursor(*c)), TimeCode(*t));
                if got.err.as_deref() != Some("Cancelled") || got.state.continuation_at.is_some() {
                    return Err(format!("step {n}: cancel left {got:?}"));
                }
            }
        }
    }
    Ok(())
}

/// The non-fixture cases of S-2, over `a` (default) and `b` (edit list):
/// an edit, a relink, a same-source jump cut, cancellation, and the resets.
pub(super) fn scripts(b: &Rc<Fixture>, cancel_after: usize) -> Vec<(&'static str, Vec<Step>)> {
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
                Do(Event::Relink(b.clone())),
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
                Cancelled(10, 14),
                Go(10, 14, Is(Seek)),
                Go(14, 15, Auto),
            ],
        ),
        ("key change", reset(Event::KeyChange)),
        ("shrink reopen", reset(Event::ShrinkReopen)),
        ("error", reset(Event::Error)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c_has(pairs: &[(i64, i64)], c: i64, t: i64) -> bool {
        pairs.contains(&(c, t))
    }

    fn corpora() -> Vec<Corpus> {
        KINDS.map(Corpus::new).into()
    }

    #[test]
    fn fixtures_have_the_shape_their_names_promise() {
        for kind in KINDS {
            Fixture::new(kind).assert_shape();
        }
    }

    /// The witnesses are only as strong as the oracle's coverage: 50 targets
    /// per file, every §8 special present, real anchor boundaries, EOF hit,
    /// no errors, varied frames (a constant oracle would pass anything).
    #[test]
    fn the_oracle_covers_the_special_targets() {
        for c in corpora() {
            let kind = c.fx.kind;
            let c_targets: Vec<(i64, i64)> = c.targets.iter().map(|g| (g.c, g.t)).collect();
            assert_eq!(c.targets.len(), 50, "{kind:?}");
            let pair = |c: i64, t: i64| c_has(&c_targets, c, t);
            let has = |k: &str| c.targets.iter().any(|g| g.kind == k);
            for k in ["eof", "c+1", "c+12", "c+13 (outside)"] {
                assert!(has(k), "{kind:?} lacks {k}");
            }
            // Read from the file: every key's before/at pair, and ±1 frame
            // around every place today's anchor changes, is a target.
            for &(_, k) in c.fx.keys.iter().filter(|k| k.1 > 1) {
                assert!(pair(k - 2, k - 1) && pair(k - 1, k), "{kind:?} key {k}");
            }
            let last = usize::try_from(c.fx.last).unwrap();
            let moves: Vec<i64> = (1..=last)
                .filter(|&f| c.fx.anchors[f] != c.fx.anchors[f - 1])
                .map(|f| i64::try_from(f).unwrap())
                .collect();
            assert!(moves.len() >= 3, "{kind:?}: anchor moves at {moves:?}");
            for f in moves {
                assert!(
                    [f - 1, f, f + 1].iter().all(|&t| pair(t - 1, t)),
                    "{kind:?} boundary {f}"
                );
            }
            assert!(
                c.oracle
                    .iter()
                    .all(|o| o.err.is_none() && o.route == Route::Seek)
            );
            assert!(
                c.oracle.iter().any(|o| o.state.eof_sent),
                "{kind:?} never reaches EOF"
            );
            assert!(
                c.oracle.iter().any(|o| o.state.lookahead.is_some()),
                "{kind:?}"
            );
            let distinct: std::collections::BTreeSet<_> =
                c.oracle.iter().map(|o| o.frame).collect();
            assert!(
                distinct.len() >= 25,
                "{kind:?}: {} distinct frames",
                distinct.len()
            );
            let digest = format!(
                "{:?}",
                c.oracle
                    .iter()
                    .map(|o| (o.frame, &o.state))
                    .collect::<Vec<_>>()
            );
            let digest = (digest.bytes()).fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
                (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
            });
            eprintln!("pf1-c4 oracle-digest {kind:?} targets=50 {digest:016x}");
        }
    }

    #[test]
    fn the_seek_path_reproduces_the_oracle_on_every_fixture() {
        for c in corpora() {
            witness(&mut SeekPath::new(&c.fx), &c, false).unwrap();
        }
    }

    /// The non-fixture cases already hold for the fresh Seek (cancel: before
    /// the first packet), so they are live for the implementation too.
    #[test]
    fn the_scripted_cases_hold_for_the_seek_path() {
        let (a, b) = (Corpus::new(Kind::Default), Corpus::new(Kind::EditList));
        for (name, steps) in scripts(&b.fx, 0) {
            run_script(&mut SeekPath::new(&a.fx), &a.fx, &steps, false)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }

    fn wrong(
        kind: Kind,
        retarget: fn(&Fixture, i64) -> i64,
        rewrite: fn(&mut Observation),
    ) -> Result<(), String> {
        let c = Corpus::new(kind);
        let inner = SeekPath::new(&c.fx);
        witness(
            &mut Wrong {
                inner,
                retarget,
                rewrite,
            },
            &c,
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
        let ignore = |fx: &Fixture, t: i64| (t - fx.offset_frames).max(0);
        wrong(Kind::EditList, ignore, |_| {}).unwrap_err();
        assert!(Fixture::new(Kind::EditList).offset_frames > 0);
        // Control: with no offset the same wrong path is the Seek path.
        wrong(Kind::Default, ignore, |_| {}).unwrap();
    }

    #[test]
    fn a_path_that_returns_the_previous_gop_across_a_key_packet_fails() {
        let previous_gop = |fx: &Fixture, t: i64| {
            let key = fx.keys.iter().rev().find(|k| k.1 <= t).map_or(0, |k| k.1);
            if key > 0 && t - key <= 2 { key - 1 } else { t }
        };
        wrong(Kind::Default, previous_gop, |_| {}).unwrap_err();
        wrong(Kind::OpenGop, previous_gop, |_| {}).unwrap_err();
    }

    /// The state fields are compared, not just the pixels.
    #[test]
    fn a_path_with_the_right_frame_and_wrong_state_fails() {
        let same = |_: &Fixture, t: i64| t;
        wrong(Kind::Vfr, same, |o| o.state.lookahead = None).unwrap_err();
        wrong(Kind::Vfr, same, |o| o.state.eof_sent = false).unwrap_err();
        wrong(Kind::Vfr, same, |o| o.state.continuation_at = None).unwrap_err();
        wrong(Kind::Vfr, same, |o| o.state.fallback_index += 1).unwrap_err();
        wrong(Kind::Vfr, same, |o| o.pts = o.pts.map(|p| p + 1)).unwrap_err();
    }

    /// Today's `decode_window_sequential` kept across calls: it continues
    /// whenever its cursor matches and `c < t <= c + 12`, with no anchor,
    /// key-packet or timestamp check. A working implementation of the wiring,
    /// and exactly what S-2 rules 1 to 4 forbid.
    struct NaiveContinuation {
        fx: Rc<Fixture>,
        decoder: Option<VideoDecoder>,
    }

    impl TargetPath for NaiveContinuation {
        fn produce(&mut self, from: Option<Cursor>, t: TimeCode) -> Observation {
            if let (Some(Cursor(c)), Some(decoder)) = (from, self.decoder.as_mut())
                && decoder.state().continuation_at == Some(c + 1)
                && (c + 1..=c + 12).contains(&t.0)
            {
                let (seeks, mut cache) = (decoder.seek_count(), FrameCache::new(13));
                let result = decoder.decode_window_sequential(TimeCode(c + 1), t, &mut cache);
                return observe(decoder, &mut cache, t, result, seeks);
            }
            let mut decoder = self.fx.open();
            let mut cache = FrameCache::new(1);
            let result = decoder.decode_window(t, t, &mut cache);
            let observation = observe(&decoder, &mut cache, t, result, 0);
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

    /// Continuing across an anchor change is caught by the route check on
    /// every file; on the open-GOP file the bytes and state alone catch it
    /// (the fresh seek yields no frame at t = 47, a continuation would); on the
    /// missing-PTS file its timestamps do. So the byte and state witnesses
    /// discriminate where the design says they must, not only the route.
    #[test]
    fn a_continuation_that_ignores_the_domain_fails() {
        for c in corpora() {
            let path = || NaiveContinuation {
                fx: c.fx.clone(),
                decoder: None,
            };
            let error = witness(&mut path(), &c, true).unwrap_err();
            assert!(error.contains("S-2 says Some(Seek)"), "{error}");
            let bytes = witness(&mut path(), &c, false);
            let differs = matches!(c.fx.kind, Kind::OpenGop | Kind::NoPts);
            assert_eq!(bytes.is_err(), differs, "{:?}: {bytes:?}", c.fx.kind);
        }
    }

    #[test]
    #[ignore = "S2c-2 wires continuation"]
    fn continuation_reproduces_seek_on_every_fixture() {
        for c in corpora() {
            witness(&mut ContinuationPath, &c, true).unwrap();
        }
    }

    #[test]
    #[ignore = "S2c-2 wires continuation"]
    fn continuation_holds_the_scripted_cases() {
        let (a, b) = (Corpus::new(Kind::Default), Corpus::new(Kind::EditList));
        for (name, steps) in scripts(&b.fx, 2) {
            run_script(&mut ContinuationPath, &a.fx, &steps, true)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }
}
