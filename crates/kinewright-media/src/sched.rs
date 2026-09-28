//! PF1 S2b: the scheduler's reader side (H-2, H-3), pure and generic over the
//! source key and the frame so the H-8 model drives it event by event, with
//! no thread and no clock of its own.
//!
//! Every method runs under the `Sched` lock (the preview lane's state). What a
//! method removes (frames, errors) comes back to the caller, which drops it
//! after unlock (H-4).

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    hash::Hash,
    time::Duration,
};

use kinewright_core::MediaError;

/// H-2: an inactive reader retires this long after it went inactive.
pub(crate) const QUIESCENCE: Duration = Duration::from_secs(5);
/// H-1: at most this many readers per source.
const READERS_PER_SOURCE: usize = 2;
/// Times on one source further apart than this are separate regions; a
/// reader decodes forward across a smaller gap rather than seek.
const REGION_GAP: i64 = 16;

/// H-5: R = clamp(P, 1, 8).
pub(crate) fn reader_limit(parallelism: usize) -> usize {
    parallelism.clamp(1, 8)
}

/// H-5: a reader wants w = clamp(⌊P / n⌋, 1, 16) frame threads, n being the
/// readers the current plan needs.
fn want(pool: usize, readers: usize) -> usize {
    (pool / readers.max(1)).clamp(1, 16)
}

/// H-2's reader states (S2b-2: Idle, `PermitWait`, Decoding, Retiring).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ReaderState {
    /// Waiting on `work`, timed to `since` + [`QUIESCENCE`].
    Idle { since: Duration },
    /// Queued for permits in `Permits`, holding none and no decoder.
    PermitWait,
    /// Decoding `at` for plan `version`, outside the lock.
    Decoding { at: i64, version: u64 },
    /// Closing its decoder outside all locks, then exiting.
    Retiring,
}

/// One reader's `DecodePlan`: required times, then lookahead.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Plan {
    pub(crate) version: u64,
    pub(crate) required: Vec<i64>,
    pub(crate) lookahead: Vec<i64>,
}

#[derive(Clone, Debug)]
pub(crate) struct Slot<K> {
    pub(crate) id: u64,
    pub(crate) key: K,
    pub(crate) plan: Plan,
    pub(crate) state: ReaderState,
    /// The time its decoder continues from without a seek.
    cursor: Option<i64>,
    /// A plan version whose lookahead failed: no more lookahead in it.
    lookahead_failed: Option<u64>,
    /// H-5: the permits (frame threads) its decoder holds; 0 when closed.
    pub(crate) threads: usize,
    /// Its ticket's cancel was handed out (once per `PermitWait`).
    cancelled: bool,
}

/// One region of demand on a source, served by one reader.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Region {
    pub(crate) required: Vec<i64>,
    pub(crate) lookahead: Vec<i64>,
}

impl Region {
    fn first(&self) -> i64 {
        let first = self.required.iter().chain(&self.lookahead).min();
        first.copied().unwrap_or(0)
    }
}

/// What a reader does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Next {
    Decode {
        at: i64,
        version: u64,
    },
    /// H-5: queue for `want` permits, then open at the grant.
    Open {
        want: usize,
    },
    /// H-5: close the decoder and release its permits (a `shrink`, or a
    /// short reader closing before it grows).
    Close,
    Wait {
        until: Duration,
    },
    Retire,
}

/// What a post or an assignment hands back: readers to start, and what
/// was removed, to drop after unlock.
pub(crate) struct Posted<K, F> {
    pub(crate) spawn: Vec<(u64, K)>,
    /// H-5: readers in `PermitWait` whose demand is obsolete: their
    /// tickets are cancelled in `Permits`, after unlock.
    pub(crate) cancel: Vec<u64>,
    /// Idle readers the assignment retired (they need a `work` wake).
    pub(crate) retired: bool,
    pub(crate) dropped: (Vec<F>, Vec<MediaError>),
}

/// Split one source's demand into at most two regions (required ones
/// first); `None`-free: an empty demand gives no region.
fn regions(required: &[i64], lookahead: &[i64]) -> Vec<Region> {
    let mut times: Vec<(i64, bool)> = required.iter().map(|t| (*t, true)).collect();
    times.extend(lookahead.iter().map(|t| (*t, false)));
    times.sort_by_key(|(t, required)| (*t, !required));
    times.dedup_by_key(|(t, _)| *t);
    let mut runs: Vec<Region> = Vec::new();
    let mut last = None;
    for (t, required) in times {
        if last.is_none_or(|last| t - last > REGION_GAP) {
            runs.push(Region::default());
        }
        let run = runs.last_mut().expect("a run");
        if required {
            run.required.push(t);
        } else {
            run.lookahead.push(t);
        }
        last = Some(t);
    }
    runs.sort_by_key(|run| run.required.is_empty());
    if runs.len() > READERS_PER_SOURCE {
        let rest = runs.split_off(READERS_PER_SOURCE);
        let second = &mut runs[READERS_PER_SOURCE - 1];
        for run in rest {
            second.required.extend(run.required);
            second.lookahead.extend(run.lookahead);
        }
        second.required.sort_unstable();
        second.lookahead.sort_unstable();
    }
    runs
}

/// H-1/H-5: the regions a job's demand needs, within `limit` readers:
/// pre-roll-only regions go first, then each source keeps one reader.
/// `None` means more distinct sources than readers: the frame renders
/// synchronously (K-3).
pub(crate) fn plan_regions<K: Clone>(
    demand: &[(K, Vec<i64>, Vec<i64>)],
    limit: usize,
) -> Option<Vec<(K, Region)>> {
    let needed = demand
        .iter()
        .filter(|(_, required, _)| !required.is_empty());
    if needed.count() > limit {
        return None;
    }
    let mut per_source: Vec<(K, Vec<Region>)> = (demand.iter())
        .map(|(key, required, lookahead)| (key.clone(), regions(required, lookahead)))
        .collect();
    let count = |sources: &[(K, Vec<Region>)]| sources.iter().map(|(_, r)| r.len()).sum::<usize>();
    if count(&per_source) > limit {
        for (_, runs) in &mut per_source {
            runs.retain(|run| !run.required.is_empty());
        }
    }
    if count(&per_source) > limit {
        for (_, runs) in &mut per_source {
            let mut rest = runs.split_off(1.min(runs.len()));
            if let Some(first) = runs.first_mut() {
                for run in rest.drain(..) {
                    first.required.extend(run.required);
                    first.lookahead.extend(run.lookahead);
                }
                first.required.sort_unstable();
                first.lookahead.sort_unstable();
            }
        }
    }
    let mut all: Vec<(K, Region)> = (per_source.into_iter())
        .flat_map(|(key, runs)| runs.into_iter().map(move |run| (key.clone(), run)))
        .collect();
    all.sort_by_key(|(_, run)| run.required.is_empty());
    Some(all)
}

/// H-2/H-3: the readers' shared state: plans, states, the rings of decoded
/// frames per source and the failures of the current plan version.
#[derive(Clone)]
pub(crate) struct Readers<K, F> {
    version: u64,
    /// H-5: P, the permit pool.
    pool: usize,
    limit: usize,
    /// H-5: the current plan's w.
    want: usize,
    next_id: u64,
    pub(crate) slots: Vec<Slot<K>>,
    rings: HashMap<K, BTreeMap<i64, F>>,
    failures: HashMap<(K, i64), (u64, MediaError)>,
    required: HashSet<(K, i64)>,
    wanted: HashMap<K, HashSet<i64>>,
    /// Required regions no reader could take yet (no free slot).
    pending: Vec<(K, Region)>,
}

impl<K, F> Default for Readers<K, F> {
    fn default() -> Self {
        Self::new(std::thread::available_parallelism().map_or(1, usize::from))
    }
}

impl<K, F> Readers<K, F> {
    /// Readers for P = `pool`: R = clamp(P, 1, 8).
    pub(crate) fn new(pool: usize) -> Self {
        Self {
            version: 0,
            pool: pool.max(1),
            limit: reader_limit(pool),
            want: 1,
            next_id: 0,
            slots: Vec::new(),
            rings: HashMap::new(),
            failures: HashMap::new(),
            required: HashSet::new(),
            wanted: HashMap::new(),
            pending: Vec::new(),
        }
    }

    pub(crate) const fn limit(&self) -> usize {
        self.limit
    }

    #[cfg(test)]
    pub(crate) const fn version(&self) -> u64 {
        self.version
    }
}

impl<K: Clone + Eq + Hash, F: Clone> Readers<K, F> {
    fn slot(&mut self, id: u64) -> Option<&mut Slot<K>> {
        self.slots.iter_mut().find(|slot| slot.id == id)
    }

    /// H-3: a new plan version for a job's regions. Every older failure and
    /// every ring frame outside the new plan is removed (returned); regions
    /// go to the nearest free reader of their source, then to new readers.
    pub(crate) fn post(&mut self, regions: Vec<(K, Region)>, now: Duration) -> Posted<K, F> {
        self.version += 1;
        let version = self.version;
        (self.required, self.wanted) = (HashSet::new(), HashMap::new());
        for (key, region) in &regions {
            let wanted = self.wanted.entry(key.clone()).or_default();
            wanted.extend(region.required.iter().chain(&region.lookahead));
            let required = region.required.iter().map(|t| (key.clone(), *t));
            self.required.extend(required);
        }
        let errors = self.failures.drain().map(|(_, (_, error))| error).collect();
        let mut frames = Vec::new();
        let wanted = &self.wanted;
        self.rings.retain(|key, ring| {
            let keep = wanted.get(key);
            let gone = ring.extract_if(.., |t, _| keep.is_none_or(|keep| !keep.contains(t)));
            frames.extend(gone.map(|(_, frame)| frame));
            !ring.is_empty()
        });
        for slot in &mut self.slots {
            slot.plan = Plan {
                version,
                ..Plan::default()
            };
        }
        self.pending.clear();
        let regions_len = regions.len();
        for (key, region) in regions {
            let free = (self.slots.iter_mut())
                .filter(|slot| slot.key == key && slot.state != ReaderState::Retiring)
                .filter(|slot| slot.plan.required.is_empty() && slot.plan.lookahead.is_empty())
                .min_by_key(|slot| slot.cursor.map_or(u64::MAX, |c| c.abs_diff(region.first())));
            match free {
                Some(slot) => slot.plan = plan(version, region),
                None => self.pending.push((key, region)),
            }
        }
        self.want = want(self.pool, regions_len);
        Posted {
            dropped: (frames, errors),
            ..self.assign(now)
        }
    }

    /// Give waiting regions new readers while slots are free; while one
    /// still waits, obsolete idle readers retire at once. Returns the
    /// readers to start, and the obsolete `PermitWait` readers whose
    /// tickets to cancel (H-5).
    pub(crate) fn assign(&mut self, now: Duration) -> Posted<K, F> {
        let mut spawn = Vec::new();
        let mut index = 0;
        while self.slots.len() < self.limit && index < self.pending.len() {
            // H-1: ≤ 2 readers per source, a retiring one included.
            let key = &self.pending[index].0;
            if self.slots.iter().filter(|slot| slot.key == *key).count() >= READERS_PER_SOURCE {
                index += 1;
                continue;
            }
            let (key, region) = self.pending.remove(index);
            let id = self.next_id;
            self.next_id += 1;
            self.slots.push(Slot {
                id,
                key: key.clone(),
                plan: plan(self.version, region),
                state: ReaderState::Idle { since: now },
                cursor: None,
                lookahead_failed: None,
                threads: 0,
                cancelled: false,
            });
            spawn.push((id, key));
        }
        // Lookahead-only regions never wait for a reader.
        self.pending
            .retain(|(_, region)| !region.required.is_empty());
        let mut retired = false;
        if !self.pending.is_empty() {
            for slot in &mut self.slots {
                if obsolete(slot) && matches!(slot.state, ReaderState::Idle { .. }) {
                    slot.state = ReaderState::Retiring;
                    retired = true;
                }
            }
        }
        let mut cancel = Vec::new();
        for slot in &mut self.slots {
            if slot.state == ReaderState::PermitWait && obsolete(slot) && !slot.cancelled {
                slot.cancelled = true;
                cancel.push(slot.id);
            }
        }
        Posted {
            spawn,
            cancel,
            retired,
            dropped: (Vec::new(), Vec::new()),
        }
    }

    /// H-5: the reader `id` left `PermitWait` with `granted` permits
    /// (`None`: its ticket was cancelled or the lane stopped).
    pub(crate) fn granted(&mut self, id: u64, granted: Option<usize>, now: Duration) {
        if let Some(slot) = self.slot(id) {
            (slot.threads, slot.cancelled) = (granted.unwrap_or(0), false);
            if slot.state == ReaderState::PermitWait {
                slot.state = ReaderState::Idle { since: now };
            }
        }
    }

    /// H-5: the reader `id` closed its decoder and released its permits.
    pub(crate) fn closed(&mut self, id: u64) {
        if let Some(slot) = self.slot(id) {
            (slot.threads, slot.cursor) = (0, None);
        }
    }

    /// Whether a required region still waits for a reader.
    pub(crate) fn waiting(&self) -> bool {
        !self.pending.is_empty()
    }

    /// H-2: the reader `id`'s next step, evaluated under the lock.
    pub(crate) fn next(&mut self, id: u64, now: Duration, shutdown: bool) -> Next {
        let pending = self.waiting();
        let work = self.work_for(id);
        let owed = self.owed(id);
        let want = self.want;
        let others = self.slots.iter().filter(|slot| slot.id != id);
        let ticket = others
            .clone()
            .any(|slot| slot.state == ReaderState::PermitWait);
        let short = others
            .clone()
            .any(|slot| slot.threads > 0 && slot.threads < want);
        let free = (self.pool).saturating_sub(self.slots.iter().map(|slot| slot.threads).sum());
        let Some(slot) = self.slot(id) else {
            return Next::Retire;
        };
        if shutdown || slot.state == ReaderState::Retiring {
            slot.state = ReaderState::Retiring;
            return Next::Retire;
        }
        if let Some(at) = work {
            if slot.threads == 0 {
                slot.state = ReaderState::PermitWait;
                return Next::Open { want };
            }
            let version = slot.plan.version;
            slot.state = ReaderState::Decoding { at, version };
            return Next::Decode { at, version };
        }
        // Inactive: H-5 rebalancing, then retirement.
        let shrink = slot.threads > want && (ticket || short);
        let grow = slot.threads > 0 && slot.threads < want && free > 0;
        if shrink || grow {
            return Next::Close;
        }
        let since = match slot.state {
            ReaderState::Idle { since } => since,
            ReaderState::Decoding { .. } | ReaderState::PermitWait | ReaderState::Retiring => now,
        };
        // A required time another reader is decoding stays owed: if that
        // result is stale and fails, this reader re-demands it (H-3).
        let quiet = now >= since + QUIESCENCE && !owed;
        if ((pending || ticket) && obsolete(slot)) || quiet {
            slot.state = ReaderState::Retiring;
            return Next::Retire;
        }
        slot.state = ReaderState::Idle { since };
        let until = since + QUIESCENCE;
        Next::Wait {
            until: if until > now { until } else { now + QUIESCENCE },
        }
    }

    /// Whether a required time of `id`'s plan has neither a frame nor a
    /// failure yet.
    fn owed(&self, id: u64) -> bool {
        let Some(slot) = self.slots.iter().find(|slot| slot.id == id) else {
            return false;
        };
        let ring = self.rings.get(&slot.key);
        (slot.plan.required.iter()).any(|t| {
            !ring.is_some_and(|ring| ring.contains_key(t))
                && !self.failures.contains_key(&(slot.key.clone(), *t))
        })
    }

    /// The first time in `id`'s plan nobody has: required, then lookahead.
    fn work_for(&self, id: u64) -> Option<i64> {
        let slot = self.slots.iter().find(|slot| slot.id == id)?;
        let ring = self.rings.get(&slot.key);
        let taken = |t: &&i64| {
            ring.is_some_and(|ring| ring.contains_key(t))
                || self.failures.contains_key(&(slot.key.clone(), **t))
                || self.slots.iter().any(|other| {
                    other.id != id
                        && other.key == slot.key
                        && matches!(other.state, ReaderState::Decoding { at, .. } if at == **t)
                })
        };
        let required = slot.plan.required.iter().find(|t| !taken(t));
        let lookahead = (slot.lookahead_failed != Some(slot.plan.version))
            .then(|| slot.plan.lookahead.iter().find(|t| !taken(t)))
            .flatten();
        required.or(lookahead).copied()
    }

    /// H-3: a reader's result for `at`, decoded under plan `version`. A frame
    /// is kept while the current plan wants it (bytes are determined by
    /// (source, time)); a failure only if its version is the reader's current
    /// one and the time is required now. Anything else is returned.
    pub(crate) fn deliver(
        &mut self,
        id: u64,
        at: i64,
        version: u64,
        result: Result<F, MediaError>,
        now: Duration,
    ) -> (Option<F>, Option<MediaError>) {
        let is_required = |key: &K| self.required.contains(&(key.clone(), at));
        let required = (self.slots.iter().find(|slot| slot.id == id))
            .is_some_and(|slot| is_required(&slot.key));
        let Some(slot) = self.slot(id) else {
            return (result.as_ref().ok().cloned(), result.err());
        };
        slot.state = ReaderState::Idle { since: now };
        let key = slot.key.clone();
        match result {
            Ok(frame) => {
                slot.cursor = Some(at + 1);
                if self
                    .wanted
                    .get(&key)
                    .is_some_and(|wanted| wanted.contains(&at))
                {
                    let ring = self.rings.entry(key).or_default();
                    (ring.insert(at, frame), None)
                } else {
                    (Some(frame), None)
                }
            }
            Err(error) => {
                slot.cursor = None;
                let current = version == slot.plan.version;
                if current && required {
                    let old = self.failures.insert((key, at), (version, error));
                    (None, old.map(|(_, error)| error))
                } else {
                    if slot.plan.lookahead.contains(&at) {
                        slot.lookahead_failed = Some(slot.plan.version);
                    }
                    (None, Some(error))
                }
            }
        }
    }

    /// A reader that could not start records its required times as failed
    /// in the current version (E-2) and leaves.
    pub(crate) fn fail_start(&mut self, id: u64, error: &MediaError) {
        if let Some(index) = self.slots.iter().position(|slot| slot.id == id) {
            let slot = self.slots.remove(index);
            for at in slot.plan.required {
                let failure = (self.version, error.clone());
                self.failures.insert((slot.key.clone(), at), failure);
            }
        }
    }

    /// The reader `id` closed its decoder and exits.
    pub(crate) fn exited(&mut self, id: u64) {
        self.slots.retain(|slot| slot.id != id);
    }

    /// The job's required frames in order, once each has a frame or a
    /// failure of the current plan version.
    pub(crate) fn resolve(&self, required: &[(K, i64)]) -> Option<Vec<Result<F, MediaError>>> {
        let resolved = |(key, at): &(K, i64)| {
            let frame = self.rings.get(key).and_then(|ring| ring.get(at));
            if let Some(frame) = frame {
                return Some(Ok(frame.clone()));
            }
            let failure = self.failures.get(&(key.clone(), *at));
            let current = failure.filter(|(version, _)| *version == self.version);
            current.map(|(_, error)| Err(error.clone()))
        };
        required.iter().map(resolved).collect()
    }

    /// Every reader retires (the preview is going).
    pub(crate) fn retire_all(&mut self) {
        for slot in &mut self.slots {
            slot.state = ReaderState::Retiring;
        }
        self.pending.clear();
    }

    /// Every ring frame, removed (a preview cache clear).
    pub(crate) fn clear_rings(&mut self) -> Vec<F> {
        let rings = self.rings.drain().flat_map(|(_, ring)| ring.into_values());
        rings.collect()
    }

    /// Ring frames, each shared allocation counted once: (frames, bytes).
    pub(crate) fn ring_bytes(&self) -> (usize, usize)
    where
        F: crate::frame::CachedFrame,
    {
        let frames = self.rings.values().flat_map(BTreeMap::values);
        let mut seen = HashSet::new();
        let (mut count, mut bytes) = (0, 0usize);
        for frame in frames {
            count += 1;
            if seen.insert(frame.shared_buffer_id()) {
                bytes = bytes.saturating_add(frame.byte_len());
            }
        }
        (count, bytes)
    }
}

/// H-5's `Permits` monitor state: a pool of P frame threads that only
/// readers hold, granted strictly FIFO. Pure; the lane wraps it in its own
/// leaf lock and condvar (H-4: never taken with `Sched`).
#[derive(Clone, Debug)]
pub(crate) struct PermitBook {
    pool: usize,
    held: BTreeMap<u64, usize>,
    tickets: VecDeque<(u64, usize)>,
    /// Cancelled requests, kept until their reader polls (a cancel can
    /// precede the ticket) or exits.
    cancelled: BTreeSet<u64>,
    pub(crate) shutdown: bool,
}

/// A queued request's outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Poll {
    Granted(usize),
    Wait,
    Cancelled,
}

impl PermitBook {
    pub(crate) fn new(pool: usize) -> Self {
        Self {
            pool: pool.max(1),
            held: BTreeMap::new(),
            tickets: VecDeque::new(),
            cancelled: BTreeSet::new(),
            shutdown: false,
        }
    }

    /// Permits held by readers (≤ P, I11).
    pub(crate) fn in_use(&self) -> usize {
        self.held.values().sum()
    }

    /// Queue `id` for `want` permits, behind every waiter.
    pub(crate) fn enqueue(&mut self, id: u64, want: usize) {
        if !self.tickets.iter().any(|(ticket, _)| *ticket == id) {
            self.tickets.push_back((id, want.max(1)));
        }
    }

    /// Only the head is granted, min(want, free) once one is free; a
    /// cancelled or stopped request leaves the queue.
    pub(crate) fn poll(&mut self, id: u64) -> Poll {
        if self.shutdown || self.cancelled.remove(&id) {
            self.tickets.retain(|(ticket, _)| *ticket != id);
            return Poll::Cancelled;
        }
        let free = self.pool.saturating_sub(self.in_use());
        match self.tickets.front() {
            Some(&(head, want)) if head == id && free > 0 => {
                self.tickets.pop_front();
                let granted = want.min(free);
                self.held.insert(id, granted);
                Poll::Granted(granted)
            }
            _ => Poll::Wait,
        }
    }

    pub(crate) fn cancel(&mut self, id: u64) {
        self.cancelled.insert(id);
    }

    pub(crate) fn release(&mut self, id: u64) {
        self.held.remove(&id);
    }

    /// The reader `id` exited: nothing of it stays.
    pub(crate) fn forget(&mut self, id: u64) {
        self.held.remove(&id);
        self.cancelled.remove(&id);
        self.tickets.retain(|(ticket, _)| *ticket != id);
    }
}

fn plan(version: u64, region: Region) -> Plan {
    Plan {
        version,
        required: region.required,
        lookahead: region.lookahead,
    }
}

fn obsolete<K>(slot: &Slot<K>) -> bool {
    slot.plan.required.is_empty() && slot.plan.lookahead.is_empty()
}

/// H-2 `FrameWait`: the preview's next step, evaluated under the lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WaitStep {
    Shutdown,
    Superseded,
    Ready,
    /// A paused wait runs a queued agent job, then re-evaluates (R-4).
    Suspend,
    /// A playback wait ended at its deadline: the previous image is held.
    Held {
        agent: bool,
    },
    Wait,
}

/// What `FrameWait`'s predicate reads (a predicate's inputs, not a state).
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WaitView {
    pub(crate) shutdown: bool,
    pub(crate) superseded: bool,
    pub(crate) resolved: bool,
    pub(crate) playback: bool,
    pub(crate) agent_waiting: bool,
    /// Playback: the clock passed the frame.
    pub(crate) expired: bool,
    /// Playback: the agent deadline (lead + 1 frames) passed.
    pub(crate) agent_due: bool,
}

pub(crate) const fn wait_step(view: WaitView) -> WaitStep {
    if view.shutdown {
        WaitStep::Shutdown
    } else if view.superseded {
        WaitStep::Superseded
    } else if view.resolved {
        WaitStep::Ready
    } else if !view.playback && view.agent_waiting {
        WaitStep::Suspend
    } else if view.playback && view.expired {
        WaitStep::Held { agent: false }
    } else if view.playback && view.agent_waiting && view.agent_due {
        WaitStep::Held { agent: true }
    } else {
        WaitStep::Wait
    }
}

#[cfg(test)]
mod tests {
    //! H-8 / I15 (S2b-1), I11 (S2b-2): an exhaustive (event × state) model
    //! of the readers' shared state and the permit book, then seeded
    //! sequences; every loop has a step budget, the clock is injected and
    //! nothing sleeps.
    use std::collections::BTreeSet;

    use super::*;

    type Model = Readers<u8, u32>;
    /// A job: per source, its required and lookahead times.
    type Job = &'static [(u8, &'static [i64], &'static [i64])];

    const JOBS: [Job; 6] = [
        &[(0, &[0], &[1, 2])],
        &[(0, &[0], &[1]), (1, &[5], &[6])],
        // A same-source jump: two regions of source 0.
        &[(0, &[0, 40], &[41])],
        &[(1, &[5], &[]), (2, &[9], &[]), (0, &[1], &[])],
        // Title only.
        &[],
        // Pre-roll: a lookahead-only region.
        &[(0, &[3], &[30, 31])],
    ];
    const BUDGET: usize = 512;

    /// The bytes of (source, time): any reader decodes the same value, so a
    /// relabelled frame is visible.
    fn frame(key: u8, at: i64) -> u32 {
        u32::from(key) << 16 | u32::try_from(at).expect("time")
    }

    #[derive(Clone)]
    struct World {
        readers: Model,
        book: PermitBook,
        /// Readers queued in `Permits` (`PermitWait`).
        waiting: Vec<u64>,
        /// Readers closing their decoder to release (a `Close`).
        releasing: Vec<u64>,
        now: Duration,
        shutdown: bool,
        /// Reader threads running (spawned, not yet exited).
        live: Vec<u64>,
        /// Decodes in flight: (reader, key, time, version).
        flight: Vec<(u64, u8, i64, u64)>,
        /// Readers that returned `Retire` and are closing.
        closing: Vec<u64>,
        /// The newest job's required set, if it went to the readers.
        job: Option<Vec<(u8, i64)>>,
        /// (source, frame threads) of every decode started.
        decodes: Vec<(u8, usize)>,
    }

    #[derive(Clone, Copy, Debug)]
    enum Event {
        Post(usize),
        Step(usize),
        Finish(usize, bool),
        Exit(usize),
        Poll(usize),
        Release(usize),
        Assign { fail: bool },
        Tick,
        Shutdown,
    }

    impl World {
        fn new(parallelism: usize) -> Self {
            Self {
                readers: Model::new(parallelism),
                book: PermitBook::new(parallelism),
                waiting: Vec::new(),
                releasing: Vec::new(),
                now: Duration::ZERO,
                shutdown: false,
                live: Vec::new(),
                flight: Vec::new(),
                closing: Vec::new(),
                job: None,
                decodes: Vec::new(),
            }
        }

        fn events(&self) -> Vec<Event> {
            let mut events: Vec<Event> = (0..JOBS.len()).map(Event::Post).collect();
            events.extend(self.stepping().into_iter().map(Event::Step));
            for index in 0..self.flight.len() {
                events.extend([Event::Finish(index, true), Event::Finish(index, false)]);
            }
            events.extend((0..self.closing.len()).map(Event::Exit));
            events.extend((0..self.waiting.len()).map(Event::Poll));
            events.extend((0..self.releasing.len()).map(Event::Release));
            events.extend([Event::Assign { fail: false }, Event::Assign { fail: true }]);
            events.extend([Event::Tick, Event::Shutdown]);
            events
        }

        /// Live readers that step: not decoding, queued or releasing
        /// (those are outside the lock).
        fn stepping(&self) -> Vec<usize> {
            let busy = |id: &u64| {
                self.flight.iter().any(|(reader, ..)| reader == id)
                    || self.waiting.contains(id)
                    || self.releasing.contains(id)
            };
            (0..self.live.len())
                .filter(|index| !busy(&self.live[*index]))
                .collect()
        }

        fn state_of(&self, id: u64) -> &'static str {
            match self.readers.slots.iter().find(|slot| slot.id == id) {
                Some(slot) => match slot.state {
                    ReaderState::Idle { .. } => "idle",
                    ReaderState::PermitWait => "permit-wait",
                    ReaderState::Decoding { .. } => "decoding",
                    ReaderState::Retiring => "retiring",
                },
                None => "gone",
            }
        }

        /// Apply `event`; returns the (event × state) cell it covered.
        fn apply(&mut self, event: Event) -> String {
            match event {
                Event::Post(job) => {
                    let demand: Vec<(u8, Vec<i64>, Vec<i64>)> = (JOBS[job].iter())
                        .map(|(k, r, l)| (*k, r.to_vec(), l.to_vec()))
                        .collect();
                    self.post(&demand)
                }
                Event::Step(index) => self.step(index),
                Event::Finish(index, ok) => {
                    let (id, key, at, version) = self.flight.remove(index);
                    let current = version == self.readers.version();
                    let required = self
                        .job
                        .as_ref()
                        .is_some_and(|job| job.contains(&(key, at)));
                    let result = if ok {
                        Ok(frame(key, at))
                    } else {
                        Err(MediaError::Backend(format!("decode {key}@{at} v{version}")))
                    };
                    let (frame_back, _) = self.readers.deliver(id, at, version, result, self.now);
                    if let Some(value) = frame_back {
                        assert_eq!(value, frame(key, at), "a returned frame was relabelled");
                    }
                    let fresh = if current { "current" } else { "stale" };
                    let need = if required { "required" } else { "lookahead" };
                    format!("finish-{}×{fresh}-{need}", if ok { "ok" } else { "err" })
                }
                Event::Exit(index) => {
                    let id = self.closing.remove(index);
                    self.book.forget(id);
                    self.readers.exited(id);
                    "exit×retiring".into()
                }
                Event::Poll(index) => {
                    let id = self.waiting[index];
                    let head = self.book.tickets.front().map(|(ticket, _)| *ticket);
                    match self.book.poll(id) {
                        Poll::Granted(granted) => {
                            assert_eq!(head, Some(id), "a grant bypassed the FIFO head");
                            self.waiting.remove(index);
                            self.readers.granted(id, Some(granted), self.now);
                            let short = granted < self.readers.want;
                            format!("poll→granted{}", if short { "-short" } else { "" })
                        }
                        Poll::Cancelled => {
                            self.waiting.remove(index);
                            self.readers.granted(id, None, self.now);
                            "poll→cancelled".into()
                        }
                        Poll::Wait if head == Some(id) => "poll→wait-none-free".into(),
                        Poll::Wait => "poll→wait-behind".into(),
                    }
                }
                Event::Release(index) => {
                    let id = self.releasing.remove(index);
                    self.book.release(id);
                    self.readers.closed(id);
                    "release".into()
                }
                Event::Assign { fail } => {
                    let assigned = self.readers.assign(self.now);
                    assigned.cancel.iter().for_each(|id| self.book.cancel(*id));
                    let any = !assigned.spawn.is_empty();
                    for (id, _) in assigned.spawn {
                        if fail {
                            let error = MediaError::Backend("decode-reader: injected".into());
                            self.readers.fail_start(id, &error);
                        } else {
                            self.live.push(id);
                        }
                    }
                    format!("assign-{}×{}", if fail { "fail" } else { "ok" }, any)
                }
                Event::Tick => {
                    self.now += QUIESCENCE;
                    let idle = (self.readers.slots.iter())
                        .any(|slot| matches!(slot.state, ReaderState::Idle { .. }));
                    format!("tick×{}", if idle { "idle" } else { "none" })
                }
                Event::Shutdown => {
                    let states: BTreeSet<_> =
                        self.live.iter().map(|id| self.state_of(*id)).collect();
                    (self.shutdown, self.book.shutdown) = (true, true);
                    format!("shutdown×{states:?}")
                }
            }
        }

        /// A reader's `next` under the lock, and what the thread does.
        fn step(&mut self, index: usize) -> String {
            let id = self.live[index];
            let state = self.state_of(id);
            match self.readers.next(id, self.now, self.shutdown) {
                Next::Decode { at, version } => {
                    let key = self.key(id);
                    self.flight.push((id, key, at, version));
                    let threads = self.slot(id).threads;
                    assert!(threads >= 1, "a decoder without permits");
                    self.decodes.push((key, threads));
                    let deferred = threads > self.readers.want
                        && (self.readers.slots.iter()).any(|slot| {
                            slot.state == ReaderState::PermitWait
                                || slot.threads > 0 && slot.threads < self.readers.want
                        });
                    if deferred {
                        return format!("step×{state}→decode-shrink-deferred");
                    }
                    format!("step×{state}→decode")
                }
                Next::Open { want } => {
                    let behind = !self.book.tickets.is_empty();
                    self.book.enqueue(id, want);
                    self.waiting.push(id);
                    format!("step×{state}→open{}", if behind { "-behind" } else { "" })
                }
                Next::Close => {
                    let threads = self.slot(id).threads;
                    self.releasing.push(id);
                    let why = if threads > self.readers.want {
                        "shrink"
                    } else {
                        "grow"
                    };
                    format!("step×{state}→close-{why}")
                }
                Next::Wait { until } => {
                    assert!(until > self.now, "a wait that never sleeps");
                    format!("step×{state}→wait")
                }
                Next::Retire => {
                    self.live.remove(index);
                    self.closing.push(id);
                    let prompt = self
                        .readers
                        .slots
                        .iter()
                        .any(|slot| slot.state == ReaderState::PermitWait)
                        && !self.shutdown;
                    format!("step×{state}→retire{}", if prompt { "-prompt" } else { "" })
                }
            }
        }

        /// Post `demand` as the monitor's `schedule` does.
        fn post(&mut self, demand: &[(u8, Vec<i64>, Vec<i64>)]) -> String {
            if self.shutdown {
                return "post×shutdown".into();
            }
            let quiet = self.flight.is_empty();
            let regions = plan_regions(demand, self.readers.limit());
            let fallback = regions.is_none();
            let posted = self.readers.post(regions.unwrap_or_default(), self.now);
            self.live.extend(posted.spawn.iter().map(|(id, _)| *id));
            let cancelled = !posted.cancel.is_empty();
            posted.cancel.iter().for_each(|id| self.book.cancel(*id));
            let required = demand
                .iter()
                .flat_map(|(k, r, _)| r.iter().map(|t| (*k, *t)));
            self.job = (!fallback).then(|| required.collect());
            if fallback {
                // More sources than readers: synchronous (K-3).
                return "post→fallback".into();
            }
            if cancelled {
                return "post→cancel-ticket".into();
            }
            format!("post×{}", if quiet { "quiet" } else { "decoding" })
        }

        fn slot(&self, id: u64) -> &Slot<u8> {
            let slot = self.readers.slots.iter().find(|slot| slot.id == id);
            slot.expect("a live reader has a slot")
        }

        fn key(&self, id: u64) -> u8 {
            (self.readers.slots.iter())
                .find(|slot| slot.id == id)
                .map(|slot| slot.key)
                .expect("a live reader has a slot")
        }

        /// The safety invariants, after every event.
        fn check(&self) {
            let readers = &self.readers;
            assert!(
                readers.slots.len() <= readers.limit(),
                "more readers than R"
            );
            // I11: reader frame threads ≤ P; the book and the slots agree;
            // a ticket exactly for each reader in `PermitWait`.
            assert!(self.book.in_use() <= self.book.pool, "permits over P");
            for slot in &readers.slots {
                let held = self.book.held.get(&slot.id).copied().unwrap_or(0);
                assert_eq!(slot.threads, held, "reader {} threads", slot.id);
            }
            let tickets: BTreeSet<u64> = self.book.tickets.iter().map(|(id, _)| *id).collect();
            assert_eq!(
                tickets.len(),
                self.book.tickets.len(),
                "a reader queued twice"
            );
            assert_eq!(tickets, self.waiting.iter().copied().collect(), "tickets");
            for key in 0..4u8 {
                let per_key = readers.slots.iter().filter(|slot| slot.key == key).count();
                assert!(
                    per_key <= READERS_PER_SOURCE,
                    "more than two readers of {key}"
                );
            }
            for ((key, at), (version, _)) in &readers.failures {
                assert_eq!(*version, readers.version(), "a stale failure was kept");
                assert!(
                    readers.required.contains(&(*key, *at)),
                    "a lookahead failure was kept"
                );
            }
            for (key, ring) in &readers.rings {
                let wanted = readers.wanted.get(key);
                for (at, value) in ring {
                    assert!(
                        wanted.is_some_and(|w| w.contains(at)),
                        "an unwanted ring frame"
                    );
                    assert_eq!(*value, frame(*key, *at), "a relabelled ring frame");
                }
            }
            let mut decoding = HashSet::new();
            for slot in &readers.slots {
                if let ReaderState::Decoding { at, .. } = slot.state {
                    assert!(
                        decoding.insert((slot.key, at)),
                        "two readers decode one frame"
                    );
                }
            }
            // No lost required request.
            if let (Some(job), false) = (&self.job, self.shutdown) {
                for (key, at) in job {
                    let resolved = readers
                        .rings
                        .get(key)
                        .is_some_and(|ring| ring.contains_key(at))
                        || readers.failures.contains_key(&(*key, *at));
                    let owned = readers.slots.iter().any(|slot| {
                        slot.key == *key
                            && slot.state != ReaderState::Retiring
                            && slot.plan.required.contains(at)
                    });
                    let waiting = (readers.pending.iter())
                        .any(|(k, region)| k == key && region.required.contains(at));
                    assert!(resolved || owned || waiting, "required {key}@{at} was lost");
                }
            }
        }

        /// A fair run with no new post and no clock: every reader steps,
        /// every decode succeeds, every closing reader exits.
        fn settle(&mut self) {
            for _ in 0..BUDGET {
                let done = self.flight.is_empty()
                    && self.closing.is_empty()
                    && self.waiting.is_empty()
                    && self.releasing.is_empty();
                let resolved =
                    (self.job.as_ref()).is_none_or(|job| self.readers.resolve(job).is_some());
                let quiet = self.live.iter().all(|id| {
                    let mut probe = self.readers.clone();
                    matches!(probe.next(*id, self.now, self.shutdown), Next::Wait { .. })
                });
                if done && quiet && (resolved && !self.readers.waiting() || self.shutdown) {
                    return;
                }
                if !self.shutdown {
                    self.apply(Event::Assign { fail: false });
                }
                for index in self.stepping().into_iter().rev() {
                    self.apply(Event::Step(index));
                }
                while !self.flight.is_empty() {
                    self.apply(Event::Finish(0, true));
                }
                while !self.closing.is_empty() {
                    self.apply(Event::Exit(0));
                }
                while !self.releasing.is_empty() {
                    self.apply(Event::Release(0));
                }
                for index in (0..self.waiting.len()).rev() {
                    self.apply(Event::Poll(index));
                }
                self.check();
            }
            panic!("the fair run did not settle within {BUDGET} steps");
        }

        /// Liveness: a fair run resolves the job, or after shutdown every
        /// reader exits; a quiescence tick then retires every reader.
        fn check_live(&self) {
            let mut world = self.clone();
            if world.shutdown {
                // H-6: shutdown ends every permit wait at once, before any
                // holder frees a permit.
                for id in world.waiting.clone() {
                    let poll = world.book.clone().poll(id);
                    assert!(matches!(poll, Poll::Cancelled), "shutdown left {id} queued");
                }
            }
            world.settle();
            if world.shutdown {
                assert!(world.readers.slots.is_empty(), "a reader outlived shutdown");
                assert_eq!(world.book.in_use(), 0, "permits outlived shutdown");
                return;
            }
            world.apply(Event::Tick);
            world.settle();
            assert!(world.readers.slots.is_empty(), "a quiescent reader stayed");
            assert_eq!(world.book.in_use(), 0, "a retired reader kept permits");
            assert!(world.book.tickets.is_empty() && world.book.cancelled.is_empty());
        }
    }

    fn explore(world: &World, depth: usize, cells: &mut BTreeSet<String>, visited: &mut usize) {
        *visited += 1;
        if depth == 0 {
            world.check_live();
            return;
        }
        for event in world.events() {
            let mut next = world.clone();
            cells.insert(next.apply(event));
            next.check();
            explore(&next, depth - 1, cells, visited);
        }
    }

    /// The (event × state) cells the model must reach, or it is vacuous.
    const CELLS: [&str; 27] = [
        "post→fallback",
        "post×decoding",
        "step×idle→decode",
        "step×idle→wait",
        "step×idle→retire",
        "step×retiring→retire",
        "finish-ok×current-required",
        "finish-ok×stale-required",
        "finish-err×current-required",
        "finish-err×stale-required",
        "finish-err×current-lookahead",
        "assign-fail×true",
        "tick×idle",
        "shutdown×{\"decoding\"}",
        "shutdown×{\"idle\"}",
        // S2b-2 (H-5, I11).
        "step×idle→open",
        "step×idle→open-behind",
        "step×idle→close-shrink",
        "step×idle→close-grow",
        "step×idle→decode-shrink-deferred",
        "step×idle→retire-prompt",
        "poll→granted",
        "poll→granted-short",
        "poll→wait-behind",
        "poll→wait-none-free",
        "poll→cancelled",
        "post→cancel-ticket",
    ];

    /// I15 (S2b-1): every sequence of four events from each P's start, then
    /// the liveness run from every leaf.
    #[test]
    fn the_reader_model_holds_for_every_short_sequence() {
        let mut cells = BTreeSet::new();
        let mut visited = 0;
        for parallelism in [1, 2, 3, 20] {
            let mut world = World::new(parallelism);
            // Start from a posted job with its readers running.
            world.apply(Event::Post(1));
            explore(&world, 5, &mut cells, &mut visited);
        }
        // A reader holding 16 of 20 permits when the plan widens: it
        // shrinks, and the head ticket waits with none free.
        let mut world = World::new(20);
        world.apply(Event::Post(0));
        world.settle();
        world.apply(Event::Post(3));
        explore(&world, 4, &mut cells, &mut visited);
        let missing: Vec<_> = CELLS
            .iter()
            .filter(|cell| !cells.contains(**cell))
            .collect();
        assert!(
            missing.is_empty(),
            "uncovered cells {missing:?}; covered {cells:?}"
        );
        assert!(visited > 10_000, "{visited} states");
        eprintln!("{visited} states, {} cells", cells.len());
    }

    /// I15 (S2b-1): seeded sequences, longer than the exhaustive depth.
    #[test]
    fn the_reader_model_holds_for_seeded_sequences() {
        let mut cells = BTreeSet::new();
        for seed in 1..=400u64 {
            let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let mut below = |bound: usize| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                usize::try_from(state % bound as u64).expect("index")
            };
            let mut world = World::new([1, 2, 3, 20][below(4)]);
            for step in 0..48 {
                let events = world.events();
                let mut event = events[below(events.len())];
                // Shutdown is terminal: keep it rare.
                if matches!(event, Event::Shutdown) && step < 40 {
                    event = Event::Tick;
                }
                cells.insert(world.apply(event));
                world.check();
                if step % 8 == 7 {
                    world.check_live();
                }
            }
            world.check_live();
        }
        assert!(cells.len() >= CELLS.len(), "{cells:?}");
    }

    /// H-3: a reader owed a frame another reader is decoding for an older
    /// plan does not quiesce; when that decode fails stale, it re-demands it.
    #[test]
    fn an_owed_reader_outlives_the_quiescence_deadline() {
        let mut world = World::new(20);
        world.apply(Event::Post(2));
        let (near, far) = (world.live[0], world.live[1]);
        for index in [0, 1] {
            assert_eq!(world.apply(Event::Step(index)), "step×idle→open");
            world.apply(Event::Poll(0));
        }
        world.apply(Event::Step(0));
        world.apply(Event::Step(1));
        // `far` decodes 40 and 41, so its cursor is nearest a new region.
        world.apply(Event::Finish(1, true));
        world.apply(Event::Step(1));
        world.apply(Event::Finish(1, true));
        let regions = vec![(
            0u8,
            Region {
                required: vec![0],
                lookahead: vec![],
            },
        )];
        world.readers.post(regions, world.now);
        world.job = Some(vec![(0, 0)]);
        let plan = |world: &World, id| {
            let slot = world.readers.slots.iter().find(|slot| slot.id == id);
            slot.expect("slot").plan.required.clone()
        };
        assert_eq!((plan(&world, far), plan(&world, near)), (vec![0], vec![]));
        assert_eq!(
            world.readers.next(far, world.now, false),
            Next::Wait { until: QUIESCENCE }
        );
        world.apply(Event::Tick);
        let label = world.apply(Event::Step(1));
        world.check();
        assert_eq!(label, "step×idle→wait", "the owed reader stays");
        // The older plan's decode of 0 fails: dropped, and re-demanded.
        let label = world.apply(Event::Finish(0, false));
        assert_eq!(label, "finish-err×stale-required");
        assert!(world.readers.failures.is_empty());
        assert_eq!(world.apply(Event::Step(1)), "step×idle→decode");
        world.check_live();
    }

    /// H-5 at P = 20: one source decodes on 16 threads, two on 10 + 10
    /// after one reopen each, three on 6 + 6 + 6, four on 5 × 4, and a
    /// same-source jump on 10 + 10; each shape reached from the last.
    #[test]
    fn permits_split_the_pool_by_the_plan() {
        let mut world = World::new(20);
        let shapes: [(&[u8], &[usize]); 6] = [
            (&[0], &[16]),
            (&[0, 1], &[10, 10]),
            (&[0, 1, 2], &[6, 6, 6]),
            (&[0, 1, 2, 3], &[5, 5, 5, 5]),
            (&[0, 0], &[10, 10]),
            (&[1], &[16]),
        ];
        let mut base = 0;
        for (sources, expected) in shapes {
            // The first post rebalances; the second is fresh work.
            for _ in 0..2 {
                base += 1000;
                let demand: Vec<(u8, Vec<i64>, Vec<i64>)> = if sources == [0, 0] {
                    vec![(0, vec![base, base + 100], vec![])]
                } else {
                    (sources.iter())
                        .map(|key| (*key, vec![base + i64::from(*key)], vec![]))
                        .collect()
                };
                world.decodes.clear();
                world.post(&demand);
                world.settle();
                world.check();
            }
            let mut threads: Vec<usize> = world.decodes.iter().map(|(_, t)| *t).collect();
            threads.sort_unstable();
            assert_eq!(threads, expected, "{sources:?}");
            assert!(world.book.in_use() <= 20);
        }
        world.check_live();
    }

    /// H-2 `FrameWait`: every predicate view, against the table's rules.
    #[test]
    fn frame_wait_steps_follow_the_table() {
        for bits in 0u8..128 {
            let bit = |n: u8| bits & (1 << n) != 0;
            let view = WaitView {
                shutdown: bit(0),
                superseded: bit(1),
                resolved: bit(2),
                playback: bit(3),
                agent_waiting: bit(4),
                expired: bit(5),
                agent_due: bit(6),
            };
            let step = wait_step(view);
            let expected = if view.shutdown {
                WaitStep::Shutdown
            } else if view.superseded {
                WaitStep::Superseded
            } else if view.resolved {
                WaitStep::Ready
            } else if !view.playback {
                // Paused: untimed; only an agent push suspends it (R-4).
                if view.agent_waiting {
                    WaitStep::Suspend
                } else {
                    WaitStep::Wait
                }
            } else if view.expired {
                WaitStep::Held { agent: false }
            } else if view.agent_waiting && view.agent_due {
                WaitStep::Held { agent: true }
            } else {
                WaitStep::Wait
            };
            assert_eq!(step, expected, "{view:?}");
        }
    }
}
