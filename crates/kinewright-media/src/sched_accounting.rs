//! Amendment R62 [S2c] item 9: a seeded state-machine test of the readers'
//! accounting (K-1 … K-3, R47, R53 … R62). Plain Rust, no dependency, no
//! thread and no clock: each sequence is one seed's random walk over the
//! operations the preview and its readers perform, and after every
//! operation an independent model checks the scheduler.
//!
//! The model plays the preview (`schedule`'s plan, K-3's fallbacks, the
//! post, admission, cache clears and source removal, detaching a reader
//! that stalls past a retirement) and every reader thread (`read`: each
//! `Next` acted on outside the lock, the decoder's kept refill frames
//! through the decoder's own bookkeeping, [`KeptFrames`]). Sources: 0 is
//! VFR (one decoded frame shows at two grid times, d > f), 1 is CFR.
//!
//! The invariants, after every operation:
//! - I1 (K-1, no double release): `live` is exactly what the model's
//!   owners hold: ring frames, reservations, discards not yet dispatched,
//!   decodes and discards in flight, and the admitted G; live ≤ C. And
//!   (Amendment R64, exact) each reservation is its charge by the one
//!   charge rule, above it only while a reader keeps the time decoded (at
//!   most max(charge, d)). Amendment R66 (item 4): the charge and "kept"
//!   come from the model's own records (the windows it holds, f, d, and
//!   which reader's decoder or refill in flight keeps which time), never
//!   from the scheduler's helpers; its held windows equal the scheduler's.
//! - I2 (K-1, exact): every time a reader's decoder keeps decoded that the
//!   plan requires is charged: reserved at max(f, d) while that reader's
//!   slot keeps it, or its charge rides that reader's discard or decode.
//!   A kept time the plan no longer requires is D13's accepted gap
//!   (bounded by B − 1 per reader; counted), as is one a retiring reader
//!   keeps (F3). Amendment R66 (F13): for a live reader the gap ends when
//!   a plan requires the time again, once admission reserves it.
//! - I3 (K-3): an admitted job's set H + G fits in C less the detached
//!   readers' charges (decodes and discards in flight, discards held).
//!   Amendment R66 (item 4): H is the model's, its charges over the
//!   required set; the scheduler's cached H must equal it.
//! - I4 (R59): no reader goes idle holding discard charges or a flight.
//! - I5 (progress): every required frame of the newest plan is resolved,
//!   in flight, or in a live reader's plan or a waiting region; and from
//!   any state, with the detached readers still stalled, running the rest
//!   to quiescence resolves the newest job (checked every few operations
//!   and at the end of each sequence), with nothing left charged but the
//!   rings and G once the detached readers return.
//! - I6 (Amendment R64, F8): no time is decoded while another live, idle
//!   or ready reader keeps it decoded, but on B's counted path (that
//!   reader's discard drops it, its charge riding the discard). Amendment
//!   R66 (F14, exact): each decode adds to `kept_handoffs` exactly the
//!   times it decodes that another reader's decoder keeps (or its refill
//!   in flight will keep).

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fmt::Write as _,
    panic::{AssertUnwindSafe, catch_unwind},
    time::Duration,
};

use kinewright_core::MediaError;

use super::{
    Admission, KeptFrames, Next, Posted, ReaderState, Readers, WINDOW_FRAMES, Weighed, plan_regions,
};

/// f, each source's converted frame bytes.
const F: usize = 10;
/// Grid times are 0..TIMES.
const TIMES: i64 = 48;
/// Operations per sequence, and sequences per run.
const STEPS: usize = 64;
const SEQUENCES: u64 = 3_000;
/// A settle's step budget.
const SETTLE: usize = 600;

#[derive(Clone, Debug)]
struct Fr {
    key: u8,
    at: i64,
    bytes: usize,
}

impl Weighed for Fr {
    fn bytes(&self) -> usize {
        self.bytes
    }

    fn pinned(&self) -> bool {
        false
    }
}

type Model = Readers<u8, Fr>;

/// A source's required and lookahead times.
type Times = (u8, Vec<i64>, Vec<i64>);

/// A decoded frame a model decoder keeps: its bytes, d.
#[derive(Clone, Debug)]
struct Raw(usize);

/// One seed's generator (xorshift64).
#[derive(Clone)]
struct Rng(u64);

impl Rng {
    fn below(&mut self, bound: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % bound.max(1)
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[usize::try_from(self.below(items.len() as u64)).expect("index")]
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

/// A run's fixed terms.
#[derive(Clone, Copy, Debug)]
struct Run {
    budget: usize,
    pool: usize,
    /// d per source (the decoded frame's bytes).
    decoded: [usize; 2],
    /// Each source's GOP (a refill's decode keeps from t's key frame).
    gop: [i64; 2],
    /// Each source's in-point (a window's floor).
    floor: [i64; 2],
    /// How often the preview posts against the readers' other steps.
    posts: u64,
    /// How often a cache clear or a source's removal retires readers, and
    /// how soon a stalled one returns.
    retires: u64,
    returns: u64,
    /// How often everything not stalled runs to rest at once, and how
    /// soon a dispatched discard completes.
    runs: u64,
    discards: u64,
    /// The scenario the run leans towards (R63 item 4).
    profile: Profile,
}

/// Amendment R63 [S2c] item 4: weighted scenario profiles. Each run leans
/// towards one: its operations' weights follow the state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Profile {
    /// Every operation at its run's fixed weight.
    Uniform,
    /// A reader retired while it converts still keeps frames when a second
    /// reader of the source refills them; its conversion then fails or
    /// stops (two holders, item 1). C is large: the refill fits beside the
    /// detached reader.
    Holders,
    /// A reader is retired and detached while its discard is out (item 6).
    Discards,
    /// A detached reader's charges beside a continued window under a tight
    /// C (items 5, A2b), or a retiring reader's kept frames under a post
    /// (F3).
    Detained,
    /// A step inside a window while a reader converts above it, under a
    /// tight C: admission stops that reader, and its conversion fails
    /// (F6).
    Stops,
    /// Playback over the frames a reader keeps, its G filling C at f
    /// (F7).
    Kept,
}

/// What a reader thread does next, outside the lock unless `Ask`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Doing {
    /// Its next step is `next` (it may be waiting: any wake asks again).
    Ask {
        waiting: bool,
    },
    Open(usize),
    Close,
    Exit,
    Discard((i64, i64), usize),
    Decode {
        at: i64,
        version: u64,
        bytes: usize,
        from: Option<i64>,
        size: usize,
    },
}

#[derive(Clone)]
struct Agent {
    id: u64,
    key: u8,
    doing: Doing,
    decoder: Option<KeptFrames<Raw>>,
    /// Its stop flag (K-2's, or a retirement's).
    stop: bool,
    /// Its ticket was cancelled.
    cancelled: bool,
    /// Detached and stalled outside the lock: it does nothing until it
    /// returns.
    stalled: bool,
    /// Amendment R63 (F2): the window times its refill in flight keeps.
    retain: BTreeSet<i64>,
    /// Amendment R66 (F14): the kept times the discard-only job it served
    /// dropped, until it is back (its next step): the scheduler learns of
    /// it then.
    served: BTreeSet<i64>,
    /// D13's accepted gap: kept times its decoder holds that no plan
    /// requires (a post left their window, or a superseded refill kept
    /// them), uncharged until the decoder drops them or (Amendment R66,
    /// F13) a plan requires them again.
    unowed: BTreeSet<i64>,
}

/// The newest job posted to the readers.
#[derive(Clone, Debug)]
struct Job {
    required: Vec<(u8, i64)>,
    generated: usize,
    /// K-2: its set is not admitted yet.
    admitting: bool,
    /// The G admitted (held until the next job).
    granted: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Ok,
    Fail,
    /// A decode interrupted by its stop flag.
    Cancel,
    /// Amendment R66 (F15): the reader panics while it decodes (R43's
    /// exit: its hold and its decoder drop, then `fail_start`).
    Panic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Post,
    Admit,
    Assign,
    Step(u64),
    Grant(u64),
    Finish(u64, Outcome),
    Discarded(u64),
    Closed(u64),
    Exited(u64),
    Clear,
    Forget(u8),
    Return(u64),
    Tick,
    /// Every reader not stalled runs to rest (time passes).
    Run,
}

/// What the runs reached (a reachability witness, not an oracle).
#[derive(Clone, Debug, Default)]
struct Reach {
    posts: u64,
    fallbacks: [u64; 3],
    refills: u64,
    stale_refills: u64,
    conversions: u64,
    failed_conversions: u64,
    vfr_conversions: u64,
    continued: u64,
    cut: u64,
    discards: u64,
    detached: u64,
    detached_with_discards: u64,
    unowed: u64,
    two_holders: u64,
    /// Amendment R64 (F8): kept times another reader decoded again, B's
    /// counted path.
    handoffs: u64,
    /// The scheduler's count of them (`kept_handoffs`).
    redecoded: u64,
    waits: u64,
    settles: u64,
    /// Amendment R66 (F15): readers that panicked.
    panics: u64,
}

impl Reach {
    fn add(&mut self, r: &Self) {
        self.posts += r.posts;
        for (total, n) in self.fallbacks.iter_mut().zip(r.fallbacks) {
            *total += n;
        }
        self.refills += r.refills;
        self.stale_refills += r.stale_refills;
        self.conversions += r.conversions;
        self.failed_conversions += r.failed_conversions;
        self.vfr_conversions += r.vfr_conversions;
        self.continued += r.continued;
        self.cut += r.cut;
        self.discards += r.discards;
        self.detached += r.detached;
        self.detached_with_discards += r.detached_with_discards;
        self.unowed += r.unowed;
        self.two_holders += r.two_holders;
        self.handoffs += r.handoffs;
        self.redecoded += r.redecoded;
        self.waits += r.waits;
        self.settles += r.settles;
        self.panics += r.panics;
    }

    /// The runs reached every operation the ruling names.
    fn assert_reached(&self) {
        assert!(self.refills > 0 && self.stale_refills > 0, "{self:?}");
        assert!(
            self.conversions > 0 && self.failed_conversions > 0,
            "{self:?}"
        );
        assert!(self.vfr_conversions > 0, "{self:?}");
        assert!(self.continued > 0 && self.cut > 0, "{self:?}");
        assert!(self.discards > 0, "{self:?}");
        assert!(
            self.detached > 0 && self.detached_with_discards > 0,
            "{self:?}"
        );
        assert!(self.fallbacks.iter().all(|n| *n > 0), "{self:?}");
        assert!(self.two_holders > 0, "{self:?}");
        assert!(self.panics > 0, "{self:?}");
    }
}

#[derive(Clone)]
struct World {
    run: Run,
    readers: Model,
    agents: Vec<Agent>,
    /// The preview's detached readers (Amendment R47).
    detached: Vec<u64>,
    job: Option<Job>,
    last: [i64; 2],
    now: Duration,
    reach: Reach,
    trace: Vec<String>,
    /// `Kept`: the last post was playback at (key, t); a paused step to t
    /// or t + 1 may follow (its window over t's reservation at f, F1; t
    /// reserved before its window, F4).
    played: Option<(u8, i64)>,
    /// A fixed case's world: the readers a retirement detaches are the
    /// script's (`Fixed::Stall`), not drawn.
    script: Option<Vec<u64>>,
    /// The last post's job and the readers the last retirement detached
    /// (`PF1_MODEL_EMIT`'s transcript).
    posted_job: Option<(Vec<Times>, bool, usize)>,
    stalled_now: Vec<u64>,
    /// The operation being applied, until it is transcribed.
    applying: Option<Op>,
    /// Amendment R66 (F14): the model's record of the last post's
    /// handoffs ([`Self::handed_off`]), and that post's version.
    handoff: HashSet<(u8, i64)>,
    handoff_version: u64,
    /// Amendment R66 (item 4): the model's record of the windows the
    /// readers hold (S-3): each source's (start, t), set by the post that
    /// holds them, gone with a post that does not plan the source, and
    /// clipped by a current refill to what its decoder kept. The model's
    /// charges are computed from it, never by the scheduler's helpers.
    held: HashMap<u8, (i64, i64)>,
}

fn source(key: u8) -> usize {
    usize::from(key)
}

impl World {
    fn new(run: Run) -> Self {
        Self {
            run,
            readers: Model::new(run.pool).with_budget(run.budget),
            agents: Vec::new(),
            detached: Vec::new(),
            job: None,
            last: [TIMES - 4, TIMES / 2],
            now: Duration::ZERO,
            reach: Reach::default(),
            trace: Vec::new(),
            played: None,
            script: None,
            posted_job: None,
            stalled_now: Vec::new(),
            applying: None,
            handoff: HashSet::new(),
            handoff_version: 0,
            held: HashMap::new(),
        }
    }

    fn kept_size(&self, key: u8) -> usize {
        F.max(self.run.decoded[source(key)])
    }

    /// Amendment R66 (item 4): a frame's charge by the one charge rule,
    /// from the model's own records: max(f, d) below t in the window the
    /// model holds for its source, f otherwise.
    fn charge(&self, key: u8, at: i64) -> usize {
        let window = (self.held.get(&key)).is_some_and(|(start, t)| (*start..*t).contains(&at));
        if window {
            F.max(self.kept_size(key))
        } else {
            F
        }
    }

    /// Amendment R66 (item 4): whether a reader keeps `at` of `key`
    /// decoded, by the model's records: its decoder's frames, or the
    /// window times its refill in flight will keep.
    fn keeps(&self, key: u8, at: i64) -> bool {
        (self.agents.iter()).any(|agent| {
            let refilling = matches!(agent.doing, Doing::Decode { from: Some(_), .. });
            agent.key == key
                && ((agent.decoder.as_ref()).is_some_and(|kept| kept.times().contains(&at))
                    || refilling && agent.retain.contains(&at))
        })
    }

    fn agent(&mut self, id: u64) -> &mut Agent {
        (self.agents.iter_mut())
            .find(|agent| agent.id == id)
            .expect("the agent")
    }

    /// The operations enabled now.
    fn ops(&self) -> Vec<(Op, u64)> {
        let (retires, posts) = self.leaning();
        let mut ops = vec![(Op::Post, posts), (Op::Tick, 1), (Op::Clear, retires)];
        ops.extend([
            (Op::Forget(0), self.forgets(0)),
            (Op::Forget(1), self.forgets(1)),
        ]);
        ops.push((Op::Run, self.run.runs));
        if self.job.as_ref().is_some_and(|job| job.admitting) || self.readers.unreserved() {
            ops.push((Op::Admit, 4));
        }
        if self.readers.waiting() {
            ops.push((Op::Assign, 2));
        }
        for agent in &self.agents {
            if agent.stalled {
                ops.push((Op::Return(agent.id), self.run.returns));
                continue;
            }
            let id = agent.id;
            match agent.doing {
                Doing::Ask { waiting } => ops.push((Op::Step(id), if waiting { 1 } else { 6 })),
                Doing::Open(_) => ops.push((Op::Grant(id), 4)),
                Doing::Close => ops.push((Op::Closed(id), self.closes())),
                Doing::Exit => ops.push((Op::Exited(id), self.closes())),
                Doing::Discard(..) => ops.push((Op::Discarded(id), self.run.discards)),
                Doing::Decode { .. } => {
                    let stopped = agent.stop && self.run.profile == Profile::Stops;
                    let fails = match self.run.profile {
                        Profile::Stops if agent.stop => 6,
                        Profile::Holders => 3,
                        _ => 1,
                    };
                    ops.push((Op::Finish(id, Outcome::Ok), 8));
                    ops.push((Op::Finish(id, Outcome::Fail), fails));
                    if agent.stop {
                        ops.push((Op::Finish(id, Outcome::Cancel), if stopped { 6 } else { 2 }));
                    }
                    ops.push((Op::Finish(id, Outcome::Panic), 1));
                }
            }
        }
        ops
    }

    /// The retirements' and the posts' weights now: a profile leans on
    /// the state it wants to catch.
    fn leaning(&self) -> (u64, u64) {
        let (retires, posts) = (self.run.retires, self.run.posts);
        let keeps = |agent: &Agent| {
            !agent.stalled
                && agent
                    .decoder
                    .as_ref()
                    .is_some_and(|kept| !kept.times().is_empty())
        };
        let retiring = (self.readers.slots.iter()).any(|slot| slot.state == ReaderState::Retiring);
        let converting =
            |agent: &Agent| matches!(agent.doing, Doing::Decode { from: None, .. }) && keeps(agent);
        match self.run.profile {
            Profile::Uniform | Profile::Kept => (retires, posts),
            // Retire a reader that keeps frames, then let a second one
            // refill them.
            Profile::Holders if !retiring && self.agents.iter().any(converting) => (6, posts),
            // A post right after a refill makes discards; a retirement
            // while one is out detaches it.
            Profile::Discards => {
                let out = |agent: &Agent| matches!(agent.doing, Doing::Discard(..));
                if self.agents.iter().any(out) {
                    (12, 1)
                } else if self.agents.iter().any(keeps) {
                    (0, 12)
                } else {
                    (0, posts)
                }
            }
            // Retirements come as a source's removal (`forgets`).
            Profile::Detained if self.detached.is_empty() => (1, posts),
            // Hold the retiring or detached reader; post beside it.
            Profile::Holders | Profile::Detained => (0, 8),
            // Post while a reader converts a kept frame.
            Profile::Stops => (
                0,
                if self.agents.iter().any(converting) {
                    12
                } else {
                    posts
                },
            ),
        }
    }

    /// G close to C more often: beside a detached reader (`Discards`,
    /// `Detained`), or a paused step after playback (`Kept`).
    fn near(&self) -> bool {
        match self.run.profile {
            Profile::Discards | Profile::Detained => !self.detached.is_empty(),
            Profile::Kept => self.played.is_some(),
            _ => false,
        }
    }

    /// A source's removal: under `Detained`, while one of its readers is
    /// outside the lock (it may stall, detached).
    fn forgets(&self, key: u8) -> u64 {
        let retires = self.run.retires;
        if self.run.profile != Profile::Detained || !self.detached.is_empty() {
            return retires;
        }
        let outside = |agent: &Agent| {
            agent.key == key && matches!(agent.doing, Doing::Decode { .. } | Doing::Discard(..))
        };
        if self.agents.iter().any(outside) {
            8
        } else {
            0
        }
    }

    /// A profile's target: a time some reader keeps decoded (a retiring
    /// one's too), and whether the post is paused. `Holders`: playback at
    /// a time two readers keep, else a step just past a retiring reader's
    /// kept frames (a second reader refills over them). `Kept`: playback
    /// at a kept time, or 12 before it beside it (its reader's plan then
    /// starts at an unreserved time, as seed 2008's).
    fn target(&self, rng: &mut Rng) -> Option<(u8, i64, bool)> {
        if let Some((key, t)) = self.played.filter(|_| rng.chance(80)) {
            let step = i64::try_from(rng.below(2)).expect("step");
            return Some((key, t + step, true));
        }
        if self.run.profile == Profile::Uniform || !rng.chance(60) {
            return None;
        }
        let kept: Vec<(u8, i64)> = (self.agents.iter())
            .filter_map(|agent| agent.decoder.as_ref().map(|kept| (agent.key, kept.times())))
            .flat_map(|(key, times)| times.into_iter().map(move |at| (key, at)))
            .collect();
        let mut seen = HashSet::new();
        let twice: Vec<(u8, i64)> = (kept.iter())
            .filter(|frame| !seen.insert(**frame))
            .copied()
            .collect();
        let retiring = |id: u64| {
            (self.readers.slots.iter())
                .any(|slot| slot.id == id && slot.state == ReaderState::Retiring)
        };
        let past: Vec<(u8, i64)> = (self.agents.iter())
            .filter(|agent| retiring(agent.id))
            .filter_map(|agent| {
                let last = agent.decoder.as_ref()?.times().last().copied()?;
                Some((agent.key, last))
            })
            .collect();
        match self.run.profile {
            Profile::Holders if !twice.is_empty() => {
                let (key, at) = rng.pick(&twice);
                Some((key, at, rng.chance(10)))
            }
            Profile::Holders if !past.is_empty() => {
                // A step back to just past them refills over them: from
                // above it (a forward step first).
                let (key, kept) = rng.pick(&past);
                let step = if self.last[source(key)] > kept + 1 {
                    1
                } else {
                    2
                };
                Some((key, kept + step, true))
            }
            Profile::Kept if !kept.is_empty() => {
                let (key, at) = rng.pick(&kept);
                if rng.chance(50) && at >= 12 {
                    Some((key, at - 12, false))
                } else {
                    Some((key, at, rng.chance(30)))
                }
            }
            _ if !kept.is_empty() => {
                let (key, at) = rng.pick(&kept);
                Some((key, at, rng.chance(75)))
            }
            _ => None,
        }
    }

    /// A retiring reader's close and exit: slow while two holders matter.
    fn closes(&self) -> u64 {
        if self.run.profile == Profile::Holders {
            1
        } else {
            4
        }
    }

    fn choose(&self, rng: &mut Rng) -> Op {
        let ops = self.ops();
        let total: u64 = ops.iter().map(|(_, weight)| weight).sum();
        let mut roll = rng.below(total);
        for (op, weight) in ops {
            if roll < weight {
                return op;
            }
            roll -= weight;
        }
        unreachable!("a weighted op")
    }

    fn apply(&mut self, op: Op, rng: &mut Rng) {
        match op {
            Op::Post => self.post(rng),
            Op::Admit => self.admit(),
            Op::Assign => {
                self.assign();
            }
            Op::Step(id) => self.step(id),
            Op::Grant(id) => {
                let agent = self.agent(id);
                let Doing::Open(want) = agent.doing else {
                    unreachable!("opening");
                };
                let granted = (!agent.cancelled).then_some(want);
                (agent.cancelled, agent.doing) = (false, Doing::Ask { waiting: false });
                let now = self.now;
                self.readers.granted(id, granted, now);
            }
            Op::Finish(id, outcome) => self.finish(id, outcome),
            Op::Discarded(id) => {
                let agent = self.agent(id);
                let Doing::Discard(keep, bytes) = agent.doing else {
                    unreachable!("discarding");
                };
                if let Some(decoder) = agent.decoder.as_mut() {
                    let before = decoder.times();
                    decoder.discard_outside(keep);
                    let after = decoder.times();
                    agent.served = (before.into_iter())
                        .filter(|t| !after.contains(t))
                        .collect();
                }
                agent.doing = Doing::Ask { waiting: false };
                self.readers.release(bytes);
            }
            Op::Closed(id) => {
                let agent = self.agent(id);
                (agent.decoder, agent.doing) = (None, Doing::Ask { waiting: false });
                self.readers.closed(id);
            }
            Op::Exited(id) => {
                self.agents.retain(|agent| agent.id != id);
                self.readers.exited(id);
            }
            Op::Clear => {
                // Its empty post holds no window.
                self.held.clear();
                let (retired, posted) = self.readers.clear_cache(false, self.now);
                self.retired(&retired, posted, rng);
            }
            Op::Forget(key) => {
                let (retired, posted) = self.readers.forget(|k| *k != key);
                self.retired(&retired, posted, rng);
            }
            Op::Return(id) => self.agent(id).stalled = false,
            Op::Tick => self.now += Duration::from_secs(1),
            Op::Run => self.quiesce(),
        }
    }

    /// The preview's wait: a region waiting for a reader is assigned one;
    /// only a change wakes the readers. Whether anything changed.
    fn assign(&mut self) -> bool {
        let posted = self.readers.assign(self.now);
        let changed = !posted.spawn.is_empty() || !posted.cancel.is_empty() || posted.retired;
        if changed {
            self.posted(posted);
        }
        changed
    }

    /// What a post or an assignment hands back, acted on after unlock.
    fn posted(&mut self, posted: Posted<u8, Fr>) {
        self.handed_off();
        let dropped: usize = posted.dropped.0.iter().map(Fr::bytes).sum();
        self.readers.release(dropped);
        for (id, key) in posted.spawn {
            self.agents.push(Agent {
                id,
                key,
                doing: Doing::Ask { waiting: false },
                decoder: None,
                stop: false,
                cancelled: false,
                stalled: false,
                retain: BTreeSet::new(),
                served: BTreeSet::new(),
                unowed: BTreeSet::new(),
            });
        }
        for id in posted.cancel {
            if let Some(agent) = self.agents.iter_mut().find(|agent| agent.id == id) {
                agent.cancelled = true;
            }
        }
        for agent in &mut self.agents {
            if let Doing::Ask { waiting } = &mut agent.doing {
                *waiting = false; // a post wakes the readers
            }
        }
    }

    /// A cache clear or a source's removal retired `retired`: their stops
    /// are set; one outside the lock may stall past the retirement's
    /// deadline, detached (Amendment R47). The job is gone.
    fn retired(&mut self, retired: &[u64], posted: Posted<u8, Fr>, rng: &mut Rng) {
        self.posted(posted);
        self.drop_job();
        for agent in (self.agents.iter_mut()).filter(|agent| retired.contains(&agent.id)) {
            agent.stop = true;
            let outside = !matches!(agent.doing, Doing::Ask { .. });
            let stalls = if self.run.profile == Profile::Uniform {
                60
            } else {
                100
            };
            let stalls = match &self.script {
                Some(ids) => ids.contains(&agent.id),
                None => outside && rng.chance(stalls),
            };
            if stalls && !self.detached.contains(&agent.id) {
                assert!(outside, "a reader inside the lock cannot stall");
                self.stalled_now.push(agent.id);
                agent.stalled = true;
                self.detached.push(agent.id);
                self.reach.detached += 1;
                if matches!(agent.doing, Doing::Discard(..)) {
                    self.reach.detached_with_discards += 1;
                }
            }
        }
    }

    fn drop_job(&mut self) {
        if let Some(job) = self.job.take() {
            self.readers.release(job.granted);
        }
    }

    /// The job's G: small, or close to C (K-3's and admission's edge).
    /// `lean` (`Kept`, playback at a kept time): C less the job's set,
    /// at f or at the charges held.
    fn generated(
        &self,
        rng: &mut Rng,
        near: bool,
        lean: Option<&[Times]>,
        distinct: usize,
    ) -> usize {
        // G: small, or close to C (K-3's and admission's edge); under
        // `Kept`, playback's set at f fills C exactly.
        if let Some(per_source) = lean {
            if rng.chance(50) {
                self.run.budget.saturating_sub(distinct * F)
            } else {
                // The set at the charges held: G fills C beside it.
                let sizes: HashMap<u8, usize> =
                    per_source.iter().map(|(key, ..)| (*key, F)).collect();
                let kept: HashMap<u8, usize> = (sizes.keys())
                    .map(|key| (*key, self.kept_size(*key)))
                    .collect();
                let set = self
                    .readers
                    .job_set(per_source, (&HashMap::new(), &kept), &sizes, 0);
                self.run.budget.saturating_sub(set)
            }
        } else if rng.chance(if near { 70 } else { 30 }) {
            let below = usize::try_from(rng.below(5)).expect("G") * F;
            self.run.budget.saturating_sub(F + below)
        } else {
            rng.pick(&[0, F, 3 * F, 6 * F])
        }
    }

    /// The job's demand per source (`Times`): a target's time, or a step
    /// from the source's last time; playback reads ahead, source 0 maybe
    /// at a second playhead 12 later.
    fn demand(
        &mut self,
        rng: &mut Rng,
        target: Option<(u8, i64, bool)>,
        paused: bool,
    ) -> Vec<Times> {
        let keys: Vec<u8> = match (target, rng.below(4)) {
            (Some((key, ..)), _) => vec![key],
            (None, 0) => vec![1],
            (None, 1) => vec![0, 1],
            (None, _) => vec![0],
        };
        let mut per_source: Vec<Times> = Vec::new();
        for key in keys {
            let k = source(key);
            let last = self.last[k];
            let window = self.readers.windows.get(&key).copied();
            let t = match rng.below(100) {
                _ if target.is_some() => target.map_or(last, |(_, at, _)| at),
                0..45 => last - 1,
                45..55 => last - i64::try_from(rng.below(5)).expect("step") - 2,
                55..67 => window.map_or(last - 1, |(start, end)| {
                    start
                        + i64::try_from(rng.below(u64::try_from(end - start + 1).expect("w")))
                            .expect("t")
                }),
                67..77 => last + i64::try_from(rng.below(3)).expect("step") + 1,
                _ => i64::try_from(rng.below(TIMES as u64)).expect("t"),
            };
            let t = t.clamp(self.run.floor[k], TIMES - 1);
            self.last[k] = t;
            if !paused && self.run.profile == Profile::Kept {
                self.played = Some((key, t));
            }
            let (mut required, mut lookahead) = (vec![t], Vec::new());
            if !paused {
                lookahead.extend((t + 1..t + 4).filter(|at| *at < TIMES));
                let beside = self.run.profile == Profile::Kept && target.is_some();
                if key == 0 && (beside || rng.chance(40)) && t + 12 < TIMES {
                    required.push(t + 12);
                    lookahead.extend((t + 13..t + 15).filter(|at| *at < TIMES));
                }
            }
            per_source.push((key, required, lookahead));
        }
        per_source
    }

    /// The preview's `schedule` up to its first admission: the demand, its
    /// backward windows (paused), K-3's plan, then the post.
    fn post(&mut self, rng: &mut Rng) {
        let near = self.near();
        let target = self.target(rng);
        let paused = target.map_or_else(|| rng.chance(75), |(.., paused)| paused);
        self.played = None;
        let per_source = self.demand(rng, target, paused);
        let distinct: HashSet<(u8, i64)> = (per_source.iter())
            .flat_map(|(key, times, _)| times.iter().map(|at| (*key, *at)))
            .collect();
        let lean = (self.run.profile == Profile::Kept && !paused && target.is_some())
            .then_some(&*per_source);
        let generated = self.generated(rng, near, lean, distinct.len());
        self.post_job(per_source, paused, generated);
    }

    /// The post of a job: its demand `per_source`, whether it is paused,
    /// its G.
    fn post_job(&mut self, mut per_source: Vec<Times>, paused: bool, generated: usize) {
        self.posted_job = Some((per_source.clone(), paused, generated));
        self.drop_job();
        self.reach.posts += 1;
        let demand: Vec<(u8, i64)> = (per_source.iter())
            .flat_map(|(key, times, _)| times.iter().map(|at| (*key, *at)))
            .collect();
        let distinct: HashSet<&(u8, i64)> = demand.iter().collect();
        // `demand_plan`.
        let sizes: HashMap<u8, usize> = per_source.iter().map(|(key, ..)| (*key, F)).collect();
        let set = distinct.len() * F + generated;
        let kept: HashMap<u8, usize> = (sizes.keys())
            .map(|key| (*key, self.kept_size(*key)))
            .collect();
        // `backward_windows`.
        let (windows, set) = if paused {
            let steps: Vec<(u8, i64, i64)> = (per_source.iter())
                .map(|(key, times, _)| (*key, times[0], self.run.floor[source(*key)]))
                .collect();
            let job = (set, generated);
            (self.readers).backward_job(&steps, (&sizes, &kept), &mut per_source, job)
        } else {
            let held = (&HashMap::new(), &kept);
            let set = (self.readers).job_set(&per_source, held, &sizes, generated);
            (HashMap::new(), set)
        };
        // `plan`.
        let slots = &self.readers.slots;
        self.detached
            .retain(|id| slots.iter().any(|slot| slot.id == *id));
        let planned = match plan_regions(&per_source, self.readers.limit()) {
            None => Err(0),
            Some(_) if !self.readers.fits(set) => Err(1),
            Some(planned) if self.readers.detained(&self.detached, &planned, set) => Err(2),
            Some(planned) => Ok(planned),
        };
        let planned = match planned {
            Ok(planned) => planned,
            Err(reason) => {
                self.reach.fallbacks[reason] += 1;
                self.held.clear(); // an empty plan holds no window
                let posted = self.readers.post(Vec::new(), (HashMap::new(), 0), self.now);
                self.posted(posted);
                return;
            }
        };
        // The windows this post holds, of the sources it plans.
        let planned_keys: HashSet<u8> = (per_source.iter())
            .filter(|(_, required, lookahead)| !required.is_empty() || !lookahead.is_empty())
            .map(|(key, ..)| *key)
            .collect();
        self.held = (windows.iter())
            .filter(|(key, _)| planned_keys.contains(key))
            .map(|(key, window)| (*key, *window))
            .collect();
        self.readers.hold(windows, kept);
        let (detached, now) = (self.detached.clone(), self.now);
        let keeping: Vec<(u64, u8, BTreeSet<i64>)> = (self.readers.slots.iter())
            .filter(|slot| slot.state != ReaderState::Retiring && !detached.contains(&slot.id))
            .map(|slot| (slot.id, slot.key, slot.retained.clone()))
            .collect();
        let posted = (self.readers).post_planned(planned, (sizes, generated), &detached, now);
        self.check_affinity(&keeping);
        let own: HashSet<(u8, i64)> = (per_source.iter())
            .flat_map(|(key, times, _)| times.iter().map(|at| (*key, *at)))
            .collect();
        if self.readers.required.len() > own.len() {
            self.reach.continued += 1;
        }
        if self
            .readers
            .slots
            .iter()
            .any(|slot| slot.discard.is_some_and(|(low, _)| low != i64::MIN))
        {
            self.reach.cut += 1;
        }
        self.posted(posted);
        self.job = Some(Job {
            required: demand,
            generated,
            admitting: true,
            granted: 0,
        });
        self.admit();
    }

    /// K-2: the preview's admission (its G once).
    fn admit(&mut self) {
        let generated = match &self.job {
            Some(job) if job.admitting => job.generated,
            _ => 0,
        };
        match self.readers.admit(generated) {
            Admission::Ready { generated } => {
                if let Some(job) = self.job.as_mut().filter(|job| job.admitting) {
                    (job.admitting, job.granted) = (false, generated);
                } else {
                    self.readers.release(generated);
                }
            }
            Admission::Wait { evicted, stop } => {
                self.readers.release(evicted.iter().map(Fr::bytes).sum());
                for agent in &mut self.agents {
                    agent.stop |= stop.contains(&agent.id);
                }
            }
        }
    }

    /// A reader asks for its next step (`read`'s loop head).
    fn step(&mut self, id: u64) {
        let now = self.now;
        let counted = self.readers.kept_handoffs;
        let next = self.readers.next(id, now, false);
        let slot = (self.readers.slots.iter()).find(|slot| slot.id == id);
        let retain = self.readers.retaining(id);
        let agent = (self.agents.iter_mut()).find(|agent| agent.id == id);
        let agent = agent.expect("the agent");
        agent.served.clear(); // it is back
        let mut converts = false;
        agent.doing = match next {
            Next::Retire => Doing::Exit,
            Next::Wait { .. } => {
                let slot = slot.expect("a waiting reader's slot");
                assert!(
                    slot.discard.is_none() && slot.discarding == 0 && slot.flight == 0,
                    "I4: reader {id} idles holding charges: discard {:?}, discarding {}, flight {}",
                    slot.discard,
                    slot.discarding,
                    slot.flight
                );
                Doing::Ask { waiting: true }
            }
            Next::Open { want } => Doing::Open(want),
            Next::Close => Doing::Close,
            Next::Discard { keep, bytes } => Doing::Discard(keep, bytes),
            Next::Decode {
                at,
                version,
                bytes,
                from,
                size,
                discard,
            } => {
                agent.stop = false;
                agent.retain = if from.is_some() {
                    retain
                } else {
                    BTreeSet::new()
                };
                // `SourceSpec::decode`'s steps before any IO: the discard,
                // then a refill or any decode but a kept time's conversion
                // drops the kept frames (no allocation comes first).
                if let Some(decoder) = agent.decoder.as_mut() {
                    if let Some(keep) = discard {
                        decoder.discard_outside(keep);
                    }
                    converts = from.is_none() && decoder.frames().any(|k| k.times.contains(&at));
                    if !converts {
                        decoder.clear();
                    }
                }
                Doing::Decode {
                    at,
                    version,
                    bytes,
                    from,
                    size,
                }
            }
        };
        if let Next::Decode { at, from, .. } = next {
            self.check_redecode(id, (at, from, converts), counted);
        }
        if matches!(next, Next::Wait { .. }) {
            self.reach.waits += 1;
        }
        if matches!(next, Next::Discard { .. }) {
            self.reach.discards += 1;
        }
    }

    /// I6 (Amendment R64, F8, keeper affinity): after a post, a live
    /// reader that kept (`keeping`, before it) times the plan wants has a
    /// region holding some of them, unless each of them went to another
    /// reader that kept times of its own region.
    fn check_affinity(&self, keeping: &[(u64, u8, BTreeSet<i64>)]) {
        let readers = &self.readers;
        let planned = |slot: &super::Slot<u8>, at: &i64| {
            slot.plan.required.contains(at) || slot.plan.lookahead.contains(at)
        };
        let kept_before = |id: u64| {
            (keeping.iter())
                .find(|(other, ..)| *other == id)
                .map(|(.., kept)| kept)
        };
        for (id, key, kept) in keeping {
            let wanted = readers.wanted.get(key);
            let wanted: Vec<i64> = (kept.iter())
                .copied()
                .filter(|at| wanted.is_some_and(|wanted| wanted.contains(at)))
                .collect();
            if wanted.is_empty() {
                continue;
            }
            let slot = (readers.slots.iter()).find(|slot| slot.id == *id);
            let slot = slot.expect("a keeper's slot");
            if wanted.iter().any(|at| planned(slot, at)) {
                continue;
            }
            let to_keeper = |at: &i64| {
                (readers.slots.iter()).any(|other| {
                    other.id != *id
                        && other.key == *key
                        && planned(other, at)
                        && kept_before(other.id).is_some_and(|kept| {
                            (other.plan.required.iter())
                                .chain(&other.plan.lookahead)
                                .any(|t| kept.contains(t))
                        })
                })
            };
            assert!(
                wanted.iter().all(to_keeper),
                "I6: reader {id} keeps {wanted:?} of source {key}, which the plan wants, \
                 and has none of them (plan {:?})",
                slot.plan
            );
        }
    }

    /// I6 (Amendment R64, F8): reader `id`'s decode of `at` (a refill's:
    /// and of the window times it keeps) decodes no time another live
    /// reader (neither retiring nor detached) keeps decoded, but on B's
    /// path: that reader's slot no longer keeps it, its discard drops it.
    /// Amendment R66 (F14, exact): the scheduler counts (`kept_handoffs`
    /// past `counted`) exactly the times this decode decodes (a refill's
    /// window from its start, t alone otherwise, none for a conversion)
    /// that the model's own record of the last post's handoffs holds
    /// ([`Self::handed_off`]), each once.
    fn check_redecode(
        &mut self,
        id: u64,
        (at, from, converts): (i64, Option<i64>, bool),
        counted: u64,
    ) {
        let agent = self.agents.iter().find(|agent| agent.id == id);
        let agent = agent.expect("the agent");
        let key = agent.key;
        let mut times = if from.is_some() {
            agent.retain.clone()
        } else {
            BTreeSet::new()
        };
        times.insert(at);
        let others = (self.agents.iter()).filter(|other| other.id != id && other.key == key);
        for other in others {
            let Some(decoder) = &other.decoder else {
                continue;
            };
            let slot = (self.readers.slots.iter()).find(|slot| slot.id == other.id);
            let Some(slot) = slot.filter(|slot| slot.state != ReaderState::Retiring) else {
                continue;
            };
            if self.detached.contains(&other.id) {
                continue;
            }
            for t in decoder.times().iter().filter(|t| times.contains(t)) {
                let outside = |(low, bound): (i64, i64)| !(low..=bound).contains(t);
                let handed = !slot.retained.contains(t)
                    && (slot.discard.is_some_and(outside)
                        || matches!(other.doing, Doing::Discard(keep, _) if outside(keep)));
                assert!(
                    handed,
                    "I6: reader {id} decodes {t} of source {key} while reader {} keeps it decoded \
                     (not handed off)",
                    other.id
                );
                self.reach.handoffs += 1;
            }
        }
        let decoded: Vec<i64> = match from {
            _ if converts => Vec::new(),
            Some(start) => (start..=at).collect(),
            None => vec![at],
        };
        let again: Vec<i64> = (decoded.into_iter())
            .filter(|t| self.handoff.remove(&(key, *t)))
            .collect();
        let delta = self.readers.kept_handoffs - counted;
        assert_eq!(
            delta,
            again.len() as u64,
            "I6 (exact): reader {id}'s decode of {at} (source {key}) decodes {again:?} again, \
             handed off at the last post; counted {delta}"
        );
    }

    /// Amendment R66 (F14): the model's record of a post's handoffs (B):
    /// each time a reader keeps decoded (its decoder's frames but the one
    /// its conversion in flight takes, the times its refill in flight will
    /// keep, and those a discard it served dropped before it is back) that
    /// its new plan does not want and the plan wants elsewhere. Remade
    /// after each post (`version`), from the decoders, not from the
    /// scheduler's slots.
    fn handed_off(&mut self) {
        if self.readers.version == self.handoff_version {
            return;
        }
        self.handoff_version = self.readers.version;
        self.handoff.clear();
        for agent in &self.agents {
            let slot = (self.readers.slots.iter()).find(|slot| slot.id == agent.id);
            let Some(slot) = slot else {
                continue;
            };
            let mut kept: BTreeSet<i64> = (agent.decoder.as_ref())
                .map(KeptFrames::times)
                .unwrap_or_default()
                .into_iter()
                .collect();
            match agent.doing {
                Doing::Decode { at, from: None, .. } => {
                    kept.remove(&at);
                }
                Doing::Decode { from: Some(_), .. } => kept.extend(agent.retain.iter().copied()),
                _ => {}
            }
            kept.extend(agent.served.iter().copied());
            let wanted = self.readers.wanted.get(&agent.key);
            let planned =
                |t: &i64| slot.plan.required.contains(t) || slot.plan.lookahead.contains(t);
            let elsewhere =
                |t: &i64| !planned(t) && wanted.is_some_and(|wanted| wanted.contains(t));
            let handed = kept.into_iter().filter(elsewhere).map(|t| (agent.key, t));
            self.handoff.extend(handed);
        }
    }

    /// `SourceSpec::decode` on the model's decoder: a refill, a kept
    /// time's conversion, or a decode that drops the kept frames.
    fn decoded(
        &mut self,
        decoder: &mut KeptFrames<Raw>,
        (key, at, from): (u8, i64, Option<i64>),
        retain: &BTreeSet<i64>,
        outcome: Outcome,
    ) -> Result<(), MediaError> {
        let d = self.run.decoded[source(key)];
        if let Some(start) = from {
            // `decode_refill`: t's own decode from its key frame keeps the
            // window's frames it produces, grouped per decoded frame (VFR:
            // one frame shows at 2k and 2k + 1). Amendment R63 (F2): each
            // keeps only the times `retain` holds (the window less the ring's).
            decoder.clear();
            if !matches!(outcome, Outcome::Ok) {
                return Err(MediaError::Backend("model: the refill fails".to_owned()));
            }
            let gop = self.run.gop[source(key)];
            let first = start.max(at - at.rem_euclid(gop));
            let mut frames: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
            for time in first..at {
                let frame = if key == 0 { time / 2 } else { time };
                frames.entry(frame).or_default().insert(time);
            }
            if first < at {
                decoder.cover(first);
            }
            for (_, times) in frames {
                let times = times.into_iter().filter(|t| retain.contains(t)).collect();
                decoder.push(times, Raw(d));
            }
            Ok(())
        } else if let Some(kept) = decoder.take(at) {
            let shared = !kept.times.is_empty();
            decoder.put_back(kept);
            self.reach.conversions += 1;
            self.reach.vfr_conversions += u64::from(shared);
            match outcome {
                Outcome::Ok => Ok(()),
                Outcome::Fail | Outcome::Cancel | Outcome::Panic => {
                    self.reach.failed_conversions += 1;
                    Err(MediaError::Backend(
                        "model: the conversion fails".to_owned(),
                    ))
                }
            }
        } else {
            decoder.clear();
            match outcome {
                Outcome::Ok => Ok(()),
                Outcome::Fail => Err(MediaError::Backend("model: the decode fails".to_owned())),
                Outcome::Cancel => Err(MediaError::Cancelled),
                Outcome::Panic => unreachable!("a panic unwinds (`panicked`)"),
            }
        }
    }

    /// A reader's decode came back (`read`'s `Decode` arm and
    /// `SourceSpec::decode`).
    fn finish(&mut self, id: u64, outcome: Outcome) {
        if outcome == Outcome::Panic {
            self.panicked(id);
            return;
        }
        let now = self.now;
        let agent = self.agent(id);
        let Doing::Decode {
            at,
            version,
            bytes,
            from,
            size,
        } = agent.doing
        else {
            unreachable!("decoding");
        };
        agent.doing = Doing::Ask { waiting: false };
        let key = agent.key;
        let retain = std::mem::take(&mut agent.retain);
        let mut decoder = agent.decoder.take().unwrap_or_default();
        let result = self.decoded(&mut decoder, (key, at, from), &retain, outcome);
        let kept_from = decoder.kept_from();
        let agent = (self.agents.iter_mut()).find(|agent| agent.id == id);
        let agent = agent.expect("the agent");
        let stopped = result.is_err() && agent.stop || matches!(result, Err(MediaError::Cancelled));
        if stopped {
            decoder.settle(false); // the read loop's stopped path (F6)
        }
        agent.decoder = Some(decoder);
        if stopped {
            self.readers.stopped(id, now);
            self.readers.release(bytes);
            return;
        }
        if from.is_some() && result.is_ok() {
            self.reach.refills += 1;
            // D13: a refill a newer post (or a retirement) superseded keeps
            // its window's frames uncharged until the decoder drops them,
            // while no plan requires them (Amendment R66, F13).
            let retiring = (self.readers.slots.iter())
                .any(|slot| slot.id == id && slot.state == ReaderState::Retiring);
            if version != self.readers.version || retiring {
                self.reach.stale_refills += 1;
                let times = agent.decoder.as_ref().map(KeptFrames::times);
                agent.unowed.extend(times.unwrap_or_default());
            } else if let Some((start, t)) = self.held.get_mut(&key)
                && *t == at
            {
                // A current refill's window starts where its decoder kept.
                *start = (*start).max(kept_from.unwrap_or(at).min(at));
            }
            self.readers.refilled(id, (at, version), kept_from);
        }
        // `pinned`: f is exact and reserved; the hold keeps f.
        let result = result.and_then(|()| {
            if bytes >= size {
                Ok(())
            } else {
                Err(MediaError::Backend("model: K-1 underreserved".to_owned()))
            }
        });
        let result = match result {
            Ok(()) => {
                self.readers.release(bytes - size);
                Ok(Fr {
                    key,
                    at,
                    bytes: size,
                })
            }
            Err(error) => {
                self.readers.release(bytes);
                Err(error)
            }
        };
        let agent = self.agent(id);
        if let Some(decoder) = agent.decoder.as_mut() {
            decoder.settle(result.is_ok());
        }
        let (back, _) = self.readers.deliver(id, at, version, result, now);
        self.readers.release(back.map_or(0, |back| back.bytes));
    }

    /// Amendment R66 (F15): reader `id` panicked while it decoded. The
    /// unwind drops its hold (the decode's bytes return) and its decoder
    /// (its kept frames), then its exit guard calls `fail_start` (R43).
    fn panicked(&mut self, id: u64) {
        let Doing::Decode { bytes, .. } = self.agent(id).doing else {
            unreachable!("decoding");
        };
        self.agents.retain(|agent| agent.id != id);
        self.readers.release(bytes);
        let error = MediaError::Backend("model: the reader panicked".to_owned());
        self.readers.fail_start(id, &error);
        self.reach.panics += 1;
    }

    /// `PF1_MODEL_EMIT`: the operation just applied as a fixed case's
    /// step.
    fn transcribe(&self, op: Op) {
        let step = match op {
            Op::Post => {
                let (per_source, paused, generated) = self.posted_job.clone().expect("a post");
                let per_source: Vec<String> = (per_source.iter())
                    .map(|(key, required, lookahead)| {
                        format!("({key}, vec!{required:?}, vec!{lookahead:?})")
                    })
                    .collect();
                let per_source = per_source.join(", ");
                format!("Post(vec![{per_source}], {paused}, {generated}),")
            }
            Op::Clear | Op::Forget(_) => format!("Stall(Op::{op:?}, vec!{:?}),", self.stalled_now),
            op => format!("Do(Op::{op:?}),"),
        };
        eprintln!("EMIT {step}");
    }

    /// The charges the detached readers hold (I3).
    fn detached_charges(&self) -> usize {
        let held = |id: &u64| {
            let slot = self.readers.slots.iter().find(|slot| slot.id == *id);
            let agent = self.agents.iter().find(|agent| agent.id == *id);
            let flying = agent.map_or(0, |agent| match agent.doing {
                Doing::Decode { bytes, .. } | Doing::Discard(_, bytes) => bytes,
                _ => 0,
            });
            flying + slot.map_or(0, |slot| slot.discarding)
        };
        self.detached.iter().map(held).sum()
    }

    /// The state a failing seed's trace shows (`PF1_MODEL_TRACE`).
    fn snapshot(&self) -> String {
        let r = &self.readers;
        let mut out = String::new();
        let mut required: Vec<_> = r.required.iter().copied().collect();
        required.sort_unstable();
        let mut reserved: Vec<_> = r.reserved.iter().map(|(k, v)| (*k, *v)).collect();
        reserved.sort_unstable();
        let _ = writeln!(
            out,
            "  v{} live {} windows {:?} kept {:?} required {required:?} reserved {reserved:?} job {:?}",
            r.version, r.live, r.windows, r.kept_sizes, self.job
        );
        let rings: BTreeMap<u8, Vec<i64>> = (r.rings.iter())
            .map(|(key, ring)| (*key, ring.keys().copied().collect()))
            .collect();
        let _ = writeln!(out, "  rings {rings:?} handoff {}", r.kept_handoffs);
        for slot in &r.slots {
            let agent = self.agents.iter().find(|agent| agent.id == slot.id);
            let _ = writeln!(
                out,
                "  reader {} src {} {:?} plan {:?} retained {:?} discard {:?}/{} flight {} | {:?} kept {:?}",
                slot.id,
                slot.key,
                slot.state,
                slot.plan,
                slot.retained,
                slot.discard,
                slot.discarding,
                slot.flight,
                agent.map(|agent| agent.doing),
                agent
                    .and_then(|agent| agent.decoder.as_ref())
                    .map(KeptFrames::times),
            );
        }
        out
    }

    /// I2 (K-1, exact): each kept time the plan requires is charged.
    fn check_kept(&mut self) {
        let readers = &self.readers;
        // I2.
        let mut holders: HashMap<(u8, i64), usize> = HashMap::new();
        for agent in &mut self.agents {
            let Some(decoder) = &agent.decoder else {
                agent.unowed.clear();
                continue;
            };
            let slot = (readers.slots.iter()).find(|slot| slot.id == agent.id);
            let slot = slot.expect("a live reader's slot");
            let times = decoder.times();
            agent.unowed.retain(|at| times.contains(at));
            let kept = decoder
                .frames()
                .flat_map(|kept| kept.times.iter().map(|at| (*at, kept.value.0)));
            for (at, raw) in kept {
                let frame = (agent.key, at);
                *holders.entry(frame).or_default() += 1;
                // D13's gap is a kept time no plan requires, or one a
                // retiring reader keeps (F3: its charges do not grow after
                // detention decided; another reader decodes the time, B).
                // For a live reader it ends when a plan requires the time
                // again (Amendment R66, F13), once that plan's admission
                // reserves it (K-2: until then the time is missing).
                if !readers.required.contains(&frame) || slot.state == ReaderState::Retiring {
                    agent.unowed.insert(at);
                    continue;
                }
                if readers.missing(&agent.key, at) && agent.unowed.contains(&at) {
                    continue;
                }
                agent.unowed.remove(&at);
                let charged = slot.retained.contains(&at)
                    && readers
                        .reserved
                        .get(&frame)
                        .is_some_and(|bytes| *bytes >= raw.max(F));
                let outside = |(low, bound): (i64, i64)| !(low..=bound).contains(&at);
                let discarded = !slot.retained.contains(&at)
                    && (slot.discard.is_some_and(outside)
                        || matches!(agent.doing, Doing::Discard(keep, _) if outside(keep)));
                let converting = matches!(agent.doing, Doing::Decode { at: t, .. } if t == at);
                assert!(
                    charged || discarded || converting,
                    "I2: reader {} keeps {at} of source {} decoded, required, uncharged: retained {}, reserved {:?}, discard {:?}, doing {:?}",
                    agent.id,
                    agent.key,
                    slot.retained.contains(&at),
                    readers.reserved.get(&frame),
                    slot.discard,
                    agent.doing
                );
            }
            assert!(
                agent.unowed.len() < WINDOW_FRAMES,
                "D13's bound: reader {} keeps {:?} uncharged",
                agent.id,
                agent.unowed
            );
            self.reach.unowed += agent.unowed.len() as u64;
        }
        // Two readers of a source retaining one time (the `unkept` case).
        for slot in &readers.slots {
            for at in &slot.retained {
                *holders.entry((slot.key, *at)).or_default() += 0x100;
            }
        }
        let two = |n: &usize| *n & 0xff > 1 || *n >> 8 > 1;
        self.reach.two_holders += holders.values().filter(|n| two(n)).count() as u64;
    }

    fn check(&mut self) {
        let readers = &self.readers;
        // I1.
        let ring: usize = (readers.rings.values())
            .flat_map(BTreeMap::values)
            .map(Fr::bytes)
            .sum();
        let reserved: usize = readers.reserved.values().sum();
        let discarding: usize = readers.slots.iter().map(|slot| slot.discarding).sum();
        let flying: usize = (self.agents.iter())
            .map(|agent| match agent.doing {
                Doing::Decode { bytes, .. } | Doing::Discard(_, bytes) => bytes,
                _ => 0,
            })
            .sum();
        let granted = self.job.as_ref().map_or(0, |job| job.granted);
        let owned = ring + reserved + discarding + flying + granted;
        assert_eq!(
            readers.live, owned,
            "I1: live {} against ring {ring} + reserved {reserved} + discarding {discarding} + in flight {flying} + G {granted}",
            readers.live
        );
        assert!(readers.live <= readers.budget, "I1: live over C");
        // I1 (K-1, the one charge rule, exact): a reservation is its
        // charge, and above it only while a reader keeps the time decoded
        // (at most max(charge, d)).
        // Amendment R66 (item 4): both from the model's records (its held
        // windows, f, d, and which reader keeps which time).
        let windows: BTreeMap<u8, (i64, i64)> = (readers.windows.iter())
            .map(|(key, window)| (*key, *window))
            .collect();
        let held: BTreeMap<u8, (i64, i64)> = self.held.iter().map(|(k, w)| (*k, *w)).collect();
        assert_eq!(
            windows, held,
            "the windows held, the scheduler's and the model's"
        );
        for ((key, at), bytes) in &readers.reserved {
            let charge = self.charge(*key, *at);
            if *bytes == charge {
                continue;
            }
            let kept = self.keeps(*key, *at);
            let most = if kept {
                charge.max(self.kept_size(*key))
            } else {
                charge
            };
            assert!(
                (charge..=most).contains(bytes),
                "I1: {at} of source {key} reserved {bytes}, its charge {charge} (kept {kept})"
            );
        }
        self.check_kept();
        let readers = &self.readers;
        // I3.
        if let Some(job) = &self.job {
            let room = readers.budget.saturating_sub(self.detached_charges());
            // Amendment R66 (item 4): H by the model's charges.
            let h: usize = (readers.required.iter())
                .map(|(key, at)| self.charge(*key, *at))
                .sum();
            assert_eq!(
                readers.required_bytes, h,
                "I3: the scheduler's H against the model's"
            );
            let set = h + job.generated;
            assert!(
                set <= room,
                "I3: H + G = {set} over C less the detached readers' charges ({room})"
            );
        }
        for (key, ring) in &readers.rings {
            for (at, frame) in ring {
                assert_eq!(
                    (frame.key, frame.at),
                    (*key, *at),
                    "a ring frame relabelled"
                );
            }
        }
        // I5: every required frame is resolved, in flight or planned.
        for (key, at) in &readers.required {
            let resolved = (readers.rings.get(key)).is_some_and(|ring| ring.contains_key(at))
                || readers.failures.contains_key(&(*key, *at));
            let planned = (readers.slots.iter()).any(|slot| {
                slot.key == *key
                    && (slot.plan.required.contains(at)
                        || matches!(slot.state, ReaderState::Decoding { at: t, .. } if t == *at))
                    && slot.state != ReaderState::Retiring
            });
            let waiting = (readers.pending.iter())
                .any(|(k, region)| k == key && region.required.contains(at));
            assert!(
                resolved || planned || waiting,
                "I5: required {at} of source {key} has no reader"
            );
        }
    }

    /// I5 (progress): run every reader but the stalled ones to quiescence
    /// (decodes succeed); the newest job resolves. Then the stalled
    /// readers return: nothing stays charged but the rings and G, and no
    /// reader keeps a discard.
    fn settle(&self) {
        let mut world = self.clone();
        world.quiesce();
        if let Some(job) = &world.job {
            assert!(!job.admitting, "I5: the newest job's set is never admitted");
            let resolved = world.readers.resolve(&job.required);
            assert!(
                resolved.is_some(),
                "I5: the newest job does not resolve: {:?}",
                job.required
            );
        }
        for agent in &mut world.agents {
            agent.stalled = false;
        }
        world.quiesce();
        let readers = &world.readers;
        for slot in &readers.slots {
            assert!(
                slot.discard.is_none() && slot.discarding == 0 && slot.flight == 0,
                "I4 at rest: reader {} holds discard {:?} / {} / flight {}",
                slot.id,
                slot.discard,
                slot.discarding,
                slot.flight
            );
        }
        let ring: usize = (readers.rings.values())
            .flat_map(BTreeMap::values)
            .map(Fr::bytes)
            .sum();
        let granted = world.job.as_ref().map_or(0, |job| job.granted);
        let reserved: usize = readers.reserved.values().sum();
        assert_eq!(
            readers.live,
            ring + granted + reserved,
            "I1 at rest: live against ring {ring} + G {granted} + reserved {reserved}"
        );
    }

    fn quiesce(&mut self) {
        let mut rng = Rng(1);
        let verbose = std::env::var_os("PF1_MODEL_TRACE").is_some();
        for round in 0..SETTLE {
            if verbose && round + 3 >= SETTLE {
                eprintln!("settle round {round}:\n{}", self.snapshot());
            }
            let mut moved = false;
            if self.job.as_ref().is_some_and(|job| job.admitting) || self.readers.unreserved() {
                let admitting = |world: &Self| world.job.as_ref().is_some_and(|job| job.admitting);
                let before = (self.readers.live, admitting(self));
                self.admit();
                moved |= (self.readers.live, admitting(self)) != before;
                self.check();
            }
            if self.readers.waiting() {
                moved |= self.assign();
            }
            let ids: Vec<u64> = self
                .agents
                .iter()
                .filter(|a| !a.stalled)
                .map(|a| a.id)
                .collect();
            for id in ids {
                let Some(agent) = self.agents.iter().find(|agent| agent.id == id) else {
                    continue;
                };
                let op = match agent.doing {
                    Doing::Ask { waiting: true } => {
                        self.step(id);
                        let agent = self.agents.iter().find(|agent| agent.id == id);
                        moved |= agent.is_none_or(|a| a.doing != Doing::Ask { waiting: true });
                        self.check();
                        continue;
                    }
                    Doing::Ask { waiting: false } => Op::Step(id),
                    Doing::Open(_) => Op::Grant(id),
                    Doing::Close => Op::Closed(id),
                    Doing::Exit => Op::Exited(id),
                    Doing::Discard(..) => Op::Discarded(id),
                    Doing::Decode { .. } => Op::Finish(id, Outcome::Ok),
                };
                self.apply(op, &mut rng);
                self.check();
                moved = true;
            }
            if !moved {
                return;
            }
        }
        panic!(
            "I5: the readers did not settle within {SETTLE} rounds:\n{}",
            self.snapshot()
        );
    }
}

/// One seed's run: its terms, then `STEPS` operations, each checked; a
/// settle every few operations and at the end.
fn sequence(seed: u64, reach: &mut Reach) {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let profile = rng.pick(&[
        Profile::Uniform,
        Profile::Uniform,
        Profile::Holders,
        Profile::Discards,
        Profile::Detained,
        Profile::Stops,
        Profile::Kept,
    ]);
    let (budgets, pools): (&[usize], &[usize]) = match profile {
        Profile::Uniform => (&[6, 10, 12, 16, 20, 30], &[1, 2, 3, 4, 8]),
        // Holders: room for a second reader beside a retiring one.
        Profile::Holders => (&[40, 60], &[4, 8]),
        Profile::Discards | Profile::Kept => (&[16, 20, 30], &[2, 3, 4]),
        Profile::Detained => (&[10, 12, 16, 20], &[2, 3, 4]),
        Profile::Stops => (&[8, 10, 12], &[1, 2, 3]),
    };
    let run = Run {
        budget: rng.pick(budgets) * F,
        pool: rng.pick(pools),
        decoded: [rng.pick(&[2 * F, 3 * F]), rng.pick(&[F / 2, F, 2 * F])],
        gop: [rng.pick(&[4, 8, 12]), rng.pick(&[3, 6, 48])],
        floor: [rng.pick(&[0, 0, 6]), rng.pick(&[0, 4])],
        posts: rng.pick(&[2, 4, 8]),
        retires: rng.pick(&[0, 1, 3]),
        returns: rng.pick(&[1, 4]),
        runs: rng.pick(&[0, 1, 2]),
        discards: rng.pick(&[1, 4]),
        profile,
    };
    let mut world = World::new(run);
    let verbose = std::env::var_os("PF1_MODEL_TRACE").is_some();
    // A seed written out as a fixed case's steps (`fixed`).
    let emit = std::env::var_os("PF1_MODEL_EMIT").is_some();
    if emit {
        eprintln!("EMIT {run:?}");
    }
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        for step in 0..STEPS {
            let op = world.choose(&mut rng);
            world.trace.push(format!("{op:?}"));
            (world.stalled_now, world.applying) = (Vec::new(), Some(op));
            world.apply(op, &mut rng);
            if emit {
                world.transcribe(op);
            }
            world.applying = None;
            if verbose {
                eprintln!("{step}: {op:?}\n{}", world.snapshot());
            }
            world.check();
            if step % 8 == 7 {
                world.settle();
                world.reach.settles += 1;
            }
        }
        world.settle();
    }));
    if let Err(panic) = outcome {
        // The operation that failed is the fixed case's last step.
        if let Some(op) = world.applying.filter(|_| emit) {
            world.transcribe(op);
        }
        let message = (panic.downcast_ref::<String>().cloned())
            .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_default();
        let mut trace = String::new();
        for (index, op) in world.trace.iter().enumerate() {
            let _ = write!(trace, "{index}:{op} ");
        }
        panic!("seed {seed} {run:?}: {message}\n  ops: {trace}");
    }
    world.reach.redecoded = world.readers.kept_handoffs;
    reach.add(&world.reach);
}

/// Amendment R62 [S2c] item 9: the readers' accounting holds I1–I5 after
/// every operation of `SEQUENCES` seeded sequences, and the runs reach
/// every operation the ruling names.
#[test]
fn the_readers_accounting_holds_for_seeded_sequences() {
    let mut reach = Reach::default();
    let first = std::env::var("PF1_MODEL_SEED")
        .ok()
        .and_then(|seed| seed.parse::<u64>().ok());
    let count = std::env::var("PF1_MODEL_SEQUENCES").ok();
    let count = count
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(SEQUENCES);
    let seeds: Vec<u64> = first.map_or_else(|| (1..=count).collect(), |seed| vec![seed]);
    // Exploration only: count the failing seeds per invariant.
    if std::env::var_os("PF1_MODEL_SURVEY").is_some() {
        std::panic::set_hook(Box::new(|_| {}));
        let mut failed: BTreeMap<String, (u64, u64)> = BTreeMap::new();
        for seed in seeds {
            if let Err(panic) = catch_unwind(AssertUnwindSafe(|| sequence(seed, &mut reach))) {
                let message = panic.downcast_ref::<String>().cloned().unwrap_or_default();
                let message = message
                    .split_once("}: ")
                    .map_or(&*message, |(_, rest)| rest);
                let class: String = message
                    .chars()
                    .take(40)
                    .filter(|c| !c.is_ascii_digit())
                    .collect();
                failed.entry(class).or_insert((0, seed)).0 += 1;
            }
        }
        let _ = std::panic::take_hook();
        eprintln!("R62 item 9 survey: {failed:?}\n{reach:?}");
        return;
    }
    for seed in seeds {
        sequence(seed, &mut reach);
    }
    eprintln!("R62 item 9: {reach:?}");
    if first.is_none() {
        reach.assert_reached();
    }
}

/// Amendment R63 [S2c] (the slow tier): the same model over `SLOW` seeds
/// (from 1, the fast tier's among them), four threads each taking every
/// fourth seed.
const SLOW: u64 = 300_000;

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn the_readers_accounting_holds_for_300k_seeded_sequences() {
    let reaches: Vec<Reach> = std::thread::scope(|scope| {
        let threads: Vec<_> = (0..4u64)
            .map(|lane| {
                scope.spawn(move || {
                    let mut reach = Reach::default();
                    for seed in (1 + lane..=SLOW).step_by(4) {
                        sequence(seed, &mut reach);
                    }
                    reach
                })
            })
            .collect();
        (threads.into_iter())
            .map(|thread| {
                thread
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    });
    let mut reach = Reach::default();
    for lane in &reaches {
        reach.add(lane);
    }
    eprintln!("R63 slow tier: {reach:?}");
    reach.assert_reached();
}

/// A fixed case's step: a post of a given job (its demand per source,
/// whether it is paused, its G), or an operation (enabled now).
#[derive(Debug)]
enum Fixed {
    Post(Vec<Times>, bool, usize),
    Op(Op),
    /// A retirement (`Clear`, `Forget`) and the readers it detaches.
    Stall(Op, Vec<u64>),
}

/// A fixed operation sequence (a seed's scenario written out), each step
/// checked as a seed's are; settled at the end. Returns the world.
fn fixed(run: Run, steps: Vec<Fixed>) -> World {
    let mut world = World::new(run);
    world.script = Some(Vec::new());
    let mut rng = Rng(1);
    let verbose = std::env::var_os("PF1_MODEL_TRACE").is_some();
    for (index, step) in steps.into_iter().enumerate() {
        if verbose {
            eprintln!("{index}: {step:?}");
        }
        match step {
            Fixed::Post(per_source, paused, generated) => {
                world.post_job(per_source, paused, generated);
            }
            Fixed::Op(op) => {
                let enabled = op == Op::Run || world.ops().iter().any(|(other, _)| *other == op);
                assert!(
                    enabled,
                    "step {index}: {op:?} is not enabled:\n{}",
                    world.snapshot()
                );
                world.apply(op, &mut rng);
            }
            Fixed::Stall(op, ids) => {
                assert!(
                    matches!(op, Op::Clear | Op::Forget(_)),
                    "step {index}: {op:?}"
                );
                world.script = Some(ids);
                world.apply(op, &mut rng);
                world.script = Some(Vec::new());
            }
        }
        world.check();
        if verbose {
            eprintln!("{}", world.snapshot());
        }
    }
    world.settle();
    world
}

/// The run seed 2008 drew (under R62's generator): C = 20f, P = 2, d = 2f.
const RUN_2008: Run = Run {
    budget: 20 * F,
    pool: 2,
    decoded: [2 * F, 2 * F],
    gop: [12, 6],
    floor: [6, 0],
    posts: 2,
    retires: 0,
    returns: 4,
    runs: 0,
    discards: 4,
    profile: Profile::Uniform,
};

/// Amendment R63 [S2c] (F7, K-3): seed 2008's scenario. A paused step back
/// from 24 to 21 refills source 0's window, and reader 0 converts 21–19,
/// keeping 18 (and below) decoded at max(f, d) = 2f. Playback then
/// requires 6 and 18 of source 0 and 18 of source 1 beside G = 17f. At f
/// the set is 3f + 17f = C, but admission charges 18 the 2f its kept frame
/// holds: 21f > C. Before the fix K-3 posted it, and the job was never
/// admitted (I5); K-3 now counts 18 at 2f and takes its fallback.
#[test]
fn a_kept_required_frame_counts_at_its_held_charge_in_k3() {
    use Fixed::{Op as Do, Post};
    let world = fixed(
        RUN_2008,
        vec![
            Post(vec![(0, vec![24], vec![])], true, 0),
            Do(Op::Run),
            Post(vec![(0, vec![21], vec![])], true, 0),
            Do(Op::Step(0)),
            Do(Op::Finish(0, Outcome::Ok)),
            Do(Op::Step(0)),
            Do(Op::Finish(0, Outcome::Ok)),
            Do(Op::Step(0)),
            Do(Op::Finish(0, Outcome::Ok)),
            Post(
                vec![
                    (0, vec![6, 18], vec![7, 8, 9, 19, 20]),
                    (1, vec![18], vec![19, 20, 21]),
                ],
                false,
                17 * F,
            ),
            Do(Op::Run),
        ],
    );
    assert_eq!(
        world.reach.fallbacks[1], 1,
        "K-3's fallback: {:?}",
        world.reach
    );
}

/// The run seed 16102 drew (`Kept`): C = 20f, P = 4, d = 3f on source 0.
const RUN_16102: Run = Run {
    budget: 20 * F,
    pool: 4,
    decoded: [3 * F, F / 2],
    gop: [4, 3],
    floor: [6, 0],
    posts: 8,
    retires: 0,
    returns: 1,
    runs: 2,
    discards: 4,
    profile: Profile::Kept,
};

/// Amendment R64 [S2c] (F8): seed 16102's scenario. A paused step to 38
/// refills its window, and reader 0 keeps 36 and 37 decoded. Playback
/// then requires 25 and 37 (two regions): 37's goes to reader 0, which
/// converts it; 25's to a new reader. Before the fix, 25's region took
/// reader 0 (the only free reader of the source) and a new reader decoded
/// 37 again while reader 0 kept it, its reservation consumed (I2, I6).
#[test]
fn a_region_goes_to_the_reader_that_keeps_its_times() {
    use Fixed::{Op as Do, Post};
    let world = fixed(
        RUN_16102,
        vec![
            Post(vec![(0, vec![39], vec![])], true, 6 * F),
            Do(Op::Run),
            Post(vec![(0, vec![38], vec![])], true, 6 * F),
            Do(Op::Step(0)),
            Do(Op::Finish(0, Outcome::Ok)),
            Post(
                vec![(0, vec![25, 37], vec![26, 27, 28, 38, 39])],
                false,
                16 * F,
            ),
            Do(Op::Admit),
            Do(Op::Step(1)),
            Do(Op::Grant(1)),
            Do(Op::Step(1)),
            Do(Op::Run),
        ],
    );
    let slot = |id: u64| (world.readers.slots.iter()).find(|slot| slot.id == id);
    assert!(
        world
            .readers
            .rings
            .get(&0)
            .is_some_and(|ring| ring.contains_key(&37)),
        "37 is converted:\n{}",
        world.snapshot()
    );
    assert_eq!(world.reach.conversions, 1, "reader 0 converted 37 (once)");
    assert_eq!(world.readers.kept_handoffs, 0, "nothing was decoded again");
    assert!(slot(1).is_some(), "25's region took a new reader");
}

/// The run seed 11 drew (`Discards`): C = 30f, P = 4, d = 2f on source 0.
const RUN_11: Run = Run {
    budget: 30 * F,
    pool: 4,
    decoded: [2 * F, F],
    gop: [4, 48],
    floor: [0, 0],
    posts: 4,
    retires: 0,
    returns: 1,
    runs: 2,
    discards: 1,
    profile: Profile::Discards,
};

/// Amendment R63 [S2c] (F1, K-1): seed 11's scenario, reduced. Playback
/// requires 0 and 12 of source 0, each reserved at f. A paused step to 4
/// then holds [0, 4]: 0 is in the held window below t, so its charge is
/// max(f, d) = 2f. Before the fix the reservation carried from playback
/// stayed at f (I1, exact); the post now returns it, and admission
/// reserves 0 again at its charge.
#[test]
fn a_carried_reservation_below_its_charge_is_made_again_at_it() {
    use Fixed::Post;
    let world = fixed(
        RUN_11,
        vec![
            Post(vec![(0, vec![0, 12], vec![1, 2, 3, 13, 14])], false, 0),
            Post(vec![(0, vec![4], vec![])], true, 6 * F),
        ],
    );
    assert_eq!(
        world.readers.reserved.get(&(0, 0)),
        Some(&(2 * F)),
        "0 at max(f, d):\n{}",
        world.snapshot()
    );
}

/// The run seed 674 drew (`Holders`): C = 40f, P = 8, d = 3f on source 0.
const RUN_674: Run = Run {
    budget: 40 * F,
    pool: 8,
    decoded: [3 * F, F / 2],
    gop: [4, 3],
    floor: [0, 0],
    posts: 4,
    retires: 0,
    returns: 1,
    runs: 1,
    discards: 1,
    profile: Profile::Holders,
};

/// Amendment R63 [S2c] (F3, K-3): seed 674's scenario, reduced. Reader 0
/// refills source 0's window and keeps 37 decoded while it decodes 36 for
/// playback; a cache clear then retires it, stalled (detached), still
/// keeping 37. Paused steps to 39, 38 and 36 hold windows over 37. Before
/// the fix `discard_outside` gave the retiring reader a discard of 37
/// after K-3's detention had decided, so H + G passed C less the detached
/// readers' charges (I3); a retiring reader's kept frames stay its own.
#[test]
fn a_retiring_reader_is_given_no_discard_after_detention() {
    use Fixed::{Op as Do, Post, Stall};
    let world = fixed(
        RUN_674,
        vec![
            Post(
                vec![(0, vec![43], vec![]), (1, vec![11], vec![])],
                true,
                3 * F,
            ),
            Do(Op::Step(0)),
            Do(Op::Grant(0)),
            Post(vec![(0, vec![39], vec![]), (1, vec![10], vec![])], true, F),
            Do(Op::Step(0)),
            Do(Op::Step(1)),
            Do(Op::Finish(0, Outcome::Ok)),
            Do(Op::Step(0)),
            Do(Op::Finish(0, Outcome::Ok)),
            Post(vec![(0, vec![36], vec![37, 38, 39])], false, 6 * F),
            Do(Op::Step(0)),
            Stall(Op::Clear, vec![0, 1]),
            Post(vec![(0, vec![39], vec![])], true, F),
            Post(vec![(0, vec![38], vec![])], true, 3 * F),
            Post(vec![(0, vec![36], vec![])], true, F),
        ],
    );
    let slot = (world.readers.slots.iter()).find(|slot| slot.id == 0);
    assert!(
        slot.is_some_and(|slot| slot.state == ReaderState::Retiring && slot.discard.is_none()),
        "reader 0 retires with no discard:\n{}",
        world.snapshot()
    );
}

/// The run seed 3123 drew (`Holders`): C = 40f, P = 4, d = 3f on source 0.
const RUN_3123: Run = Run {
    budget: 40 * F,
    pool: 4,
    decoded: [3 * F, F / 2],
    gop: [8, 3],
    floor: [0, 0],
    posts: 2,
    retires: 0,
    returns: 4,
    runs: 2,
    discards: 1,
    profile: Profile::Holders,
};

/// Amendment R63 [S2c] (F4, K-1; R64 item 4): seed 3123's scenario (F4's
/// counterexample at 30k seeds), reduced. It needs F11 too (38's
/// reservation returns to f at the last post), so it lands with F11. A paused step to 40 refills [28, 40] and reader 0
/// decodes 40; a step to 39 supersedes the refill, and reader 0 is left
/// holding the discard of 34–39. A step to 38 (G = 3f) holds [27, 38]:
/// 38 (t) and 34–37 are reserved (carried), 27–33 are not, and admission
/// waits on the discard. Before the fix reader 0 dispatched 38's refill
/// since its t was reserved, keeping 27–33 decoded uncharged (I2); a
/// refill's t now waits until every window time is reserved or resolved.
#[test]
fn a_refill_never_keeps_its_window_uncharged() {
    use Fixed::{Op as Do, Post};
    let world = fixed(
        RUN_3123,
        vec![
            Post(
                vec![(0, vec![1], vec![]), (1, vec![36], vec![])],
                true,
                36 * F,
            ),
            Do(Op::Run),
            Post(
                vec![
                    (0, vec![41], vec![42, 43, 44]),
                    (1, vec![35], vec![36, 37, 38]),
                ],
                false,
                6 * F,
            ),
            Post(vec![(0, vec![40], vec![])], true, 0),
            Do(Op::Step(0)),
            Post(
                vec![(0, vec![39], vec![]), (1, vec![34], vec![])],
                true,
                6 * F,
            ),
            Do(Op::Finish(0, Outcome::Ok)),
            Post(vec![(0, vec![38], vec![])], true, 3 * F),
        ],
    );
    let slot = (world.readers.slots.iter()).find(|slot| slot.id == 0);
    assert!(
        slot.is_some_and(|slot| slot.retained.is_empty()),
        "reader 0 keeps nothing while admission waits:\n{}",
        world.snapshot()
    );
}

/// The run seed 106741 drew (`Discards`): C = 30f, P = 4, d = 2f on source 0.
const RUN_106741: Run = Run {
    budget: 30 * F,
    pool: 4,
    decoded: [2 * F, F / 2],
    gop: [12, 48],
    floor: [0, 4],
    posts: 4,
    retires: 0,
    returns: 1,
    runs: 1,
    discards: 4,
    profile: Profile::Discards,
};

/// Amendment R64 [S2c] (F8, B): seed 106741's scenario (the slow tier's).
/// Reader 0 refills 22's window from 9; while it decodes, a paused step
/// to 16 drops 17 to 21 through a discard it serves at its next step.
/// Playback then requires 9 and 21: 9's region goes to reader 0 and 21's
/// to reader 1. Reader 0's refill finishes, its decoder keeping 21 until
/// the discard, and reader 1 decodes 21. Before the fix only a time the
/// keeper still kept at the post was handed off, so that decode was not
/// counted (I6).
#[test]
fn a_time_a_busy_readers_discard_drops_is_counted_when_decoded_again() {
    use Fixed::{Op as Do, Post};
    let world = fixed(
        RUN_106741,
        vec![
            Post(
                vec![(0, vec![19, 31], vec![20, 21, 22, 32, 33])],
                false,
                6 * F,
            ),
            Do(Op::Run),
            Post(vec![(1, vec![26], vec![])], true, F),
            Post(vec![(0, vec![22], vec![])], true, 0),
            Do(Op::Step(0)),
            Post(vec![(0, vec![16], vec![])], true, 0),
            Post(vec![(0, vec![9, 21], vec![10, 11, 12, 22, 23])], false, F),
            Do(Op::Finish(0, Outcome::Ok)),
            Do(Op::Step(1)),
        ],
    );
    assert_eq!(
        world.reach.handoffs,
        1,
        "reader 1 decoded 21 while reader 0 kept it:\n{}",
        world.snapshot()
    );
    assert_eq!(world.readers.kept_handoffs, 1, "21 decoded again, counted");
}

/// Amendment R66 [S2c] (F13, K-1): Astra's scenario on seed 16102's run.
/// A paused step to 38 refills its window, and reader 0 keeps 36 and 37
/// decoded at d = 3f. Playback at 25 releases 37's reservation (no plan
/// requires it); playback then requires 37 again before reader 0 moves
/// on. K-3 counts 37 at 3f and its region goes to reader 0, which keeps
/// it; before the fix admission reserved it at f (I2, exact).
#[test]
fn a_kept_time_required_again_is_admitted_at_its_kept_charge() {
    use Fixed::{Op as Do, Post};
    let world = fixed(
        RUN_16102,
        vec![
            Post(vec![(0, vec![39], vec![])], true, 6 * F),
            Do(Op::Run),
            Post(vec![(0, vec![38], vec![])], true, 6 * F),
            Do(Op::Step(0)),
            Do(Op::Finish(0, Outcome::Ok)),
            Post(vec![(0, vec![25], vec![26, 27, 28])], false, 6 * F),
            Post(vec![(0, vec![37], vec![38, 39])], false, 6 * F),
            Do(Op::Run),
        ],
    );
    assert_eq!(world.reach.conversions, 1, "reader 0 converted 37 (once)");
    assert_eq!(world.readers.kept_handoffs, 0, "nothing was decoded again");
}

/// Amendment R66 [S2c] (F14, B): Astra's scenario on seed 16102's run.
/// Reader 0 refills 38's window, keeping 36 and 37 decoded. A paused step
/// to 38 again under G = 17f cuts its continuation (K-3): 36 and 37 go
/// out through a discard-only job, which reader 0 is given and has not
/// served. Playback then requires 25 and 37: 25's region goes to reader
/// 0, 37's to reader 1, which decodes it while reader 0's decoder still
/// keeps it. Before the fix the dispatch forgot the times its discard
/// drops, so the post did not hand 37 off and that decode was not
/// counted (I6, exact).
#[test]
fn a_time_a_dispatched_discard_drops_is_counted_when_decoded_again() {
    use Fixed::{Op as Do, Post};
    let world = fixed(
        RUN_16102,
        vec![
            Post(vec![(0, vec![39], vec![])], true, 6 * F),
            Do(Op::Run),
            Post(vec![(0, vec![38], vec![])], true, 6 * F),
            Do(Op::Step(0)),
            Do(Op::Finish(0, Outcome::Ok)),
            Post(vec![(0, vec![38], vec![])], true, 17 * F),
            Do(Op::Step(0)),
            Post(
                vec![(0, vec![25, 37], vec![26, 27, 28, 38, 39])],
                false,
                6 * F,
            ),
            Do(Op::Step(1)),
            Do(Op::Grant(1)),
            Do(Op::Step(1)),
        ],
    );
    assert_eq!(
        world.reach.handoffs,
        1,
        "reader 1 decoded 37 while reader 0's decoder kept it:\n{}",
        world.snapshot()
    );
    assert_eq!(world.readers.kept_handoffs, 1, "37 decoded again, counted");
}

/// Amendment R66 [S2c] (K-1, F8): found by F14's exact count. Reader 0
/// refills 38's window [35, 38]; a cache clear retires it while the
/// refill is in flight, and its decoder keeps only 36 and 37 (from 38's
/// key frame). Before the fix its slot still kept 35 too (a retiring
/// reader's refill was not clipped), so playback's post handed 35 off
/// from it, and reader 1's decode of 35 was counted as decoded again
/// (I6, exact); admission also reserved 35 at max(f, d) (F13's rule).
#[test]
fn a_retired_readers_refill_keeps_only_what_its_decoder_kept() {
    use Fixed::{Op as Do, Post, Stall};
    let world = fixed(
        RUN_16102,
        vec![
            Post(vec![(0, vec![39], vec![])], true, 6 * F),
            Do(Op::Run),
            Post(vec![(0, vec![38], vec![])], true, 6 * F),
            Do(Op::Step(0)),
            Stall(Op::Clear, vec![]),
            Do(Op::Finish(0, Outcome::Ok)),
            Post(vec![(0, vec![35], vec![36, 37])], false, 6 * F),
            Do(Op::Step(1)),
            Do(Op::Grant(1)),
            Do(Op::Step(1)),
        ],
    );
    let slot = (world.readers.slots.iter()).find(|slot| slot.id == 0);
    assert!(
        slot.is_none_or(|slot| slot.retained.iter().all(|at| *at >= 36)),
        "reader 0 keeps 36 and 37 at most:\n{}",
        world.snapshot()
    );
    assert_eq!(world.readers.kept_handoffs, 0, "35 was never kept");
}

/// Seed 16102's run with C = 60f: a backward window wide enough to keep
/// times below its refill's key frame.
const RUN_16102_WIDE: Run = Run {
    budget: 60 * F,
    ..RUN_16102
};

/// Amendment R66 [S2c] (F8, B): found by F14's exact count (seed 3517).
/// Reader 0 refills 38's window [23, 38] from below 36, 38's key frame:
/// its decoder will keep only 36 and 37. While it decodes, a paused step
/// to 37 beside G = 55f cuts the continuation to 37, so a pending discard
/// drops 23–36. Before the fix the refill's result clipped only the times
/// its slot kept, not those its discard drops, so playback's next post
/// handed 26 off from it, and reader 1's decode of 26 was counted as
/// decoded again (I6, exact).
#[test]
fn a_refill_clips_the_times_its_pending_discard_drops() {
    use Fixed::{Op as Do, Post};
    let world = fixed(
        RUN_16102_WIDE,
        vec![
            Post(vec![(0, vec![39], vec![])], true, 6 * F),
            Do(Op::Run),
            Post(vec![(0, vec![38], vec![])], true, 6 * F),
            Do(Op::Step(0)),
            Post(vec![(0, vec![37], vec![])], true, 55 * F),
            Do(Op::Finish(0, Outcome::Ok)),
            Post(vec![(0, vec![26, 37], vec![])], false, 6 * F),
            Do(Op::Step(1)),
            Do(Op::Grant(1)),
            Do(Op::Step(1)),
        ],
    );
    assert_eq!(
        world.readers.kept_handoffs,
        0,
        "26 was never kept:\n{}",
        world.snapshot()
    );
}

/// The run seed 29956 drew (`Discards`): C = 20f, P = 4, d = 2f on source
/// 0, f on source 1.
const RUN_29956: Run = Run {
    budget: 20 * F,
    pool: 4,
    decoded: [2 * F, F],
    gop: [12, 48],
    floor: [0, 0],
    posts: 2,
    retires: 0,
    returns: 1,
    runs: 0,
    discards: 1,
    profile: Profile::Discards,
};

/// Amendment R66 [S2c] (F13, K-3): seed 29956's scenario. Reader 0's
/// refill of source 0's 26 keeps 24 and 25 (from 26's key frame, 24).
/// Source 1's window [6, 21] is refilled and kept. A paused step to 24 of
/// source 0 and to 18 of source 1 continues source 1's window, fitted
/// beside the set. At f the set is 2f + 2f + f (source 0's window
/// [22, 24]) + 12f (7–18) + G = 3f, C exactly; but admission reserves 24,
/// which reader 0 still keeps, at max(f, d) = 2f. Before the fix the fit
/// counted 24 at f, so the job was never admitted (I5); the fit now
/// counts it at 2f and cuts the continuation by one.
#[test]
fn a_continued_window_fits_beside_a_kept_time_at_its_kept_charge() {
    use Fixed::{Op as Do, Post};
    let world = fixed(
        RUN_29956,
        vec![
            Post(vec![(0, vec![27], vec![])], true, 3 * F),
            Do(Op::Run),
            Post(vec![(0, vec![26], vec![])], true, 3 * F),
            Do(Op::Step(0)),
            Post(vec![(1, vec![22], vec![])], true, 3 * F),
            Do(Op::Finish(0, Outcome::Ok)),
            Do(Op::Step(1)),
            Do(Op::Grant(1)),
            Do(Op::Step(1)),
            Do(Op::Finish(1, Outcome::Ok)),
            Post(vec![(1, vec![21], vec![])], true, 3 * F),
            Do(Op::Step(1)),
            Do(Op::Finish(1, Outcome::Ok)),
            Post(
                vec![(0, vec![24], vec![]), (1, vec![18], vec![])],
                true,
                3 * F,
            ),
        ],
    );
    let required = &world.readers.required;
    assert!(
        !required.contains(&(1, 7)) && required.contains(&(1, 8)),
        "the continuation is cut to 8–18:\n{}",
        world.snapshot()
    );
}

/// Amendment R66 [S2c] (F15, K-1): Astra's scenario on seed 16102's run.
/// Reader 0 refills 38's window, keeping 36 and 37 decoded at d = 3f.
/// Playback then requires 36 and 37; their region goes to reader 0, and
/// both are reserved at 3f while it keeps them. Reader 0 panics while it
/// converts 36: its decoder drops 37 too. Before the fix the panic's exit
/// (`fail_start`) left 37 reserved at 3f with no reader keeping it, above
/// its charge f (I1, exact).
#[test]
fn a_panicking_readers_kept_frames_return_to_their_charge() {
    use Fixed::{Op as Do, Post};
    let world = fixed(
        RUN_16102,
        vec![
            Post(vec![(0, vec![39], vec![])], true, 6 * F),
            Do(Op::Run),
            Post(vec![(0, vec![38], vec![])], true, 6 * F),
            Do(Op::Step(0)),
            Do(Op::Finish(0, Outcome::Ok)),
            Post(vec![(0, vec![36, 37], vec![38, 39])], false, 6 * F),
            Do(Op::Step(0)),
            Do(Op::Finish(0, Outcome::Panic)),
        ],
    );
    assert_eq!(world.reach.panics, 1, "reader 0 panicked");
}
