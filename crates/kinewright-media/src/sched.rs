//! PF1 S2b: the scheduler's reader side (H-2, H-3), pure and generic over the
//! source key and the frame so the H-8 model drives it event by event, with
//! no thread and no clock of its own.
//!
//! Every method runs under the `Sched` lock (the preview lane's state). What a
//! method removes (frames, errors) comes back to the caller, which drops it
//! after unlock (H-4).
//!
//! S2b-3 (K-1…K-5): `live` counts every byte the scheduler path owns: the
//! required set reserved at admission, lookahead admitted per decode, the
//! frames those became (their drop guards release them) and generated
//! rasters. Every increase is a check against C here, under the lock, so
//! live ≤ C at all times (I12).

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    hash::Hash,
    time::Duration,
};

use kinewright_core::MediaError;

use crate::cache::{Travel, follow};

/// H-2: an inactive reader retires this long after it went inactive.
pub(crate) const QUIESCENCE: Duration = Duration::from_secs(5);
/// H-1: at most this many readers per source.
const READERS_PER_SOURCE: usize = 2;
/// Amendment R37 (G1/G14 (iv)): a region continues only across real decoder
/// continuation. A reader decodes its required times, then its lookahead,
/// each ascending, and a decoder continues without a seek only at exactly
/// the next frame; so a time more than this after the last one, or a
/// required time after lookahead (the reader would decode it first, then
/// seek back), starts a new region.
const REGION_GAP: i64 = 1;

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
    /// Lookahead only, not admissible (K-2); timed like `Idle`.
    BudgetWait { since: Duration },
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
    /// K-1: the bytes reserved for its decode in flight (a handoff).
    flight: usize,
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
    /// Decode `at` under `bytes` reserved (K-1): the reader's drop guard
    /// owns them from here.
    Decode {
        at: i64,
        version: u64,
        bytes: usize,
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

/// K-1: what the scheduler reads of a frame: its reservation's bytes, and
/// whether anything besides the ring holds it (pinned, K-5).
pub(crate) trait Weighed: Clone {
    fn bytes(&self) -> usize;
    fn pinned(&self) -> bool;
}

/// K-2's outcome for the current job's required set.
pub(crate) enum Admission<F> {
    /// Reserved; `generated` bytes (K-1's rasters) now belong to the caller.
    Ready { generated: usize },
    /// Draining: evicted lookahead (drop after unlock) and the readers
    /// whose lookahead decodes to stop; wait on `ready`, then admit again.
    Wait { evicted: Vec<F>, stop: Vec<u64> },
}

/// Split one source's demand into at most two regions (required ones
/// first); `None`-free: an empty demand gives no region. Runs beyond the
/// second merge into it (R37's third-playhead residual: that reader seeks
/// between its playheads).
fn regions(required: &[i64], lookahead: &[i64]) -> Vec<Region> {
    let mut times: Vec<(i64, bool)> = required.iter().map(|t| (*t, true)).collect();
    times.extend(lookahead.iter().map(|t| (*t, false)));
    times.sort_by_key(|(t, required)| (*t, !required));
    times.dedup_by_key(|(t, _)| *t);
    let mut runs: Vec<Region> = Vec::new();
    let mut last: Option<(i64, bool)> = None;
    for (t, required) in times {
        let continues = last.is_some_and(|(last, last_required)| {
            t - last <= REGION_GAP && (last_required || !required)
        });
        if !continues {
            runs.push(Region::default());
        }
        let run = runs.last_mut().expect("a run");
        if required {
            run.required.push(t);
        } else {
            run.lookahead.push(t);
        }
        last = Some((t, required));
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

/// H-1/H-5: the regions a job's demand needs, within `limit` readers, and
/// how many merges fitting them took. Over the limit (R38, review B F3),
/// lookahead-only regions go first; only then, as a last resort, are two of
/// one source's required regions merged, the nearest pair first, keeping
/// only the lookahead past the merged region's last required time, so its
/// reader still decodes forward only. `None` means more distinct required
/// sources than readers: the frame renders synchronously (K-3).
pub(crate) fn plan_regions<K: Clone>(
    demand: &[(K, Vec<i64>, Vec<i64>)],
    limit: usize,
) -> Option<(Vec<(K, Region)>, u64)> {
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
    let mut merged = 0u64;
    while count(&per_source) > limit {
        let Some((source, index)) = nearest_pair(&per_source) else {
            break; // unreachable: one region per required source fits
        };
        let runs = &mut per_source[source].1;
        let later = runs.remove(index);
        let earlier = &mut runs[index - 1];
        earlier.required.extend(later.required);
        earlier.required.sort_unstable();
        let last = earlier.required.last().copied().unwrap_or(i64::MIN);
        earlier.lookahead.extend(later.lookahead);
        earlier.lookahead.retain(|t| *t > last);
        earlier.lookahead.sort_unstable();
        merged += 1;
    }
    let mut all: Vec<(K, Region)> = (per_source.into_iter())
        .flat_map(|(key, runs)| runs.into_iter().map(move |run| (key.clone(), run)))
        .collect();
    all.sort_by_key(|(_, run)| run.required.is_empty());
    Some((all, merged))
}

/// R38: the two adjacent required regions of one source nearest in time
/// (the later's first required time less the earlier's last), as the
/// source's index and the later region's; ties go to the earliest.
fn nearest_pair<K>(per_source: &[(K, Vec<Region>)]) -> Option<(usize, usize)> {
    let pairs = per_source
        .iter()
        .enumerate()
        .flat_map(|(source, (_, runs))| {
            (1..runs.len()).map(move |index| {
                let earlier = runs[index - 1].required.last().copied().unwrap_or(0);
                let later = runs[index].required.first().copied().unwrap_or(0);
                (later.saturating_sub(earlier).max(0), source, index)
            })
        });
    pairs.min().map(|(_, source, index)| (source, index))
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
    /// K-1: C, and the bytes reserved now (every owner's); `peak` is I12's
    /// witness.
    budget: usize,
    live: usize,
    peak: usize,
    /// The current plan's frame bytes per source (f at the proxy raster).
    sizes: HashMap<K, usize>,
    /// K-2: required frames reserved and not yet decoding.
    reserved: HashMap<(K, i64), usize>,
    /// K-2: no lookahead is admitted while the required set waits.
    draining: bool,
    /// The job's generated rasters (G) and required bytes (H).
    generated: usize,
    required_bytes: usize,
    /// K-2: lookahead refusals, once per source per plan.
    pub(crate) starved: u64,
    /// R38 (review B F3): required regions merged to fit the reader limit.
    pub(crate) regions_merged: u64,
    starved_keys: HashSet<K>,
    /// K-5 (review B F4): per source, its last required times and the
    /// direction its demand travels, for eviction.
    travel: HashMap<K, (Vec<i64>, Travel)>,
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
            budget: usize::MAX,
            live: 0,
            peak: 0,
            sizes: HashMap::new(),
            reserved: HashMap::new(),
            draining: false,
            generated: 0,
            required_bytes: 0,
            starved: 0,
            regions_merged: 0,
            starved_keys: HashSet::new(),
            travel: HashMap::new(),
        }
    }

    /// K-1: C for this path.
    pub(crate) const fn with_budget(mut self, budget: usize) -> Self {
        self.budget = budget;
        self
    }

    pub(crate) const fn limit(&self) -> usize {
        self.limit
    }

    /// K-1: bytes reserved now, and the most ever (I12: both ≤ C).
    #[cfg(test)]
    pub(crate) const fn live(&self) -> (usize, usize) {
        (self.live, self.peak)
    }

    #[cfg(test)]
    pub(crate) const fn draining(&self) -> bool {
        self.draining
    }

    /// K-3: whether a required set of `bytes` fits in C at all.
    pub(crate) const fn fits(&self, bytes: usize) -> bool {
        bytes <= self.budget
    }

    /// K-1: a drop guard's bytes come back (its last owner went).
    pub(crate) const fn release(&mut self, bytes: usize) {
        self.live = self.live.saturating_sub(bytes);
    }

    /// R38 D3: charge `bytes` of rasters another path already cached, if
    /// they fit in C beside everything live and no required set is
    /// draining; `false`: the caller drops them instead.
    pub(crate) fn adopt(&mut self, bytes: usize) -> bool {
        !self.draining && self.reserve(bytes, self.budget)
    }

    /// K-1: reserve `bytes` if live stays ≤ `limit`.
    fn reserve(&mut self, bytes: usize, limit: usize) -> bool {
        let fits = self
            .live
            .checked_add(bytes)
            .is_some_and(|next| next <= limit);
        if fits {
            self.live += bytes;
            self.peak = self.peak.max(self.live);
        }
        fits
    }

    #[cfg(test)]
    pub(crate) const fn version(&self) -> u64 {
        self.version
    }
}

impl<K: Clone + Eq + Hash, F: Weighed> Readers<K, F> {
    fn slot(&mut self, id: u64) -> Option<&mut Slot<K>> {
        self.slots.iter_mut().find(|slot| slot.id == id)
    }

    /// H-3: a new plan version for a job's regions, with its sources' f
    /// and G (K-2). Every older failure and every ring frame outside the new
    /// plan is removed (returned); regions go to the nearest free reader of
    /// their source, then to new readers.
    pub(crate) fn post(
        &mut self,
        regions: Vec<(K, Region)>,
        (sizes, generated): (HashMap<K, usize>, usize),
        now: Duration,
    ) -> Posted<K, F> {
        self.version += 1;
        let version = self.version;
        (self.required, self.wanted) = (HashSet::new(), HashMap::new());
        (self.sizes, self.generated) = (sizes, generated);
        self.starved_keys.clear();
        let mut points: HashMap<K, Vec<i64>> = HashMap::new();
        for (key, region) in &regions {
            let wanted = self.wanted.entry(key.clone()).or_default();
            wanted.extend(region.required.iter().chain(&region.lookahead));
            let required = region.required.iter().map(|t| (key.clone(), *t));
            self.required.extend(required);
            let key_points = points.entry(key.clone()).or_default();
            key_points.extend(&region.required);
        }
        // K-5: a source's travel follows its required times; a source
        // without them keeps its direction.
        for (key, points) in points.into_iter().filter(|(_, p)| !p.is_empty()) {
            let (last, travel) = self
                .travel
                .entry(key)
                .or_insert((Vec::new(), Travel::Forward));
            *travel = follow(*travel, last, &points);
            *last = points;
        }
        let errors = self.failures.drain().map(|(_, (_, error))| error).collect();
        // Reservations of frames no longer required return at once.
        let required = &self.required;
        let gone = self
            .reserved
            .extract_if(|frame, _| !required.contains(frame));
        let released: usize = gone.map(|(_, bytes)| bytes).sum();
        self.release(released);
        self.required_bytes = (self.required.iter())
            .map(|(key, _)| self.sizes.get(key).copied().unwrap_or(0))
            .sum();
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
                flight: 0,
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
        let blocked = matches!(work, Err(true));
        let shrink = slot.threads > want && (ticket || short);
        if let Ok((at, required)) = work {
            if slot.threads == 0 {
                slot.state = ReaderState::PermitWait;
                return Next::Open { want };
            }
            // Review A F2: a needed shrink is served at the decode boundary
            // before more lookahead, so a waiting ticket (required work) is
            // not queued behind this reader's horizon (H-3, H-5).
            if shrink && !required {
                return Next::Close;
            }
            return self.decode(id, at, required);
        }
        // Inactive: H-5 rebalancing, then retirement.
        let grow = slot.threads > 0 && slot.threads < want && free > 0;
        if shrink || grow {
            return Next::Close;
        }
        let since = match slot.state {
            ReaderState::Idle { since } | ReaderState::BudgetWait { since } => since,
            ReaderState::Decoding { .. } | ReaderState::PermitWait | ReaderState::Retiring => now,
        };
        // A required time another reader is decoding stays owed: if that
        // result is stale and fails, this reader re-demands it (H-3).
        let quiet = now >= since + QUIESCENCE && !owed;
        if ((pending || ticket) && obsolete(slot)) || quiet {
            slot.state = ReaderState::Retiring;
            return Next::Retire;
        }
        slot.state = if blocked {
            ReaderState::BudgetWait { since }
        } else {
            ReaderState::Idle { since }
        };
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

    /// The first time in `id`'s plan nobody has: a reserved required one,
    /// then admissible lookahead (K-2). `Err(true)`: lookahead waits on the
    /// budget.
    fn work_for(&mut self, id: u64) -> Result<(i64, bool), bool> {
        let Some(slot) = self.slots.iter().find(|slot| slot.id == id) else {
            return Err(false);
        };
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
        let key = slot.key.clone();
        let reserved = |t: &&i64| self.reserved.contains_key(&(key.clone(), **t));
        let required = slot
            .plan
            .required
            .iter()
            .filter(reserved)
            .find(|t| !taken(t));
        if let Some(at) = required {
            return Ok((*at, true));
        }
        let lookahead = (slot.lookahead_failed != Some(slot.plan.version))
            .then(|| slot.plan.lookahead.iter().find(|t| !taken(t)))
            .flatten()
            .copied();
        let Some(at) = lookahead else {
            return Err(false);
        };
        if self.admissible(&key) {
            return Ok((at, false));
        }
        if self.starved_keys.insert(key) {
            self.starved += 1;
        }
        Err(true)
    }

    /// K-2: lookahead for `key` while not draining, live + f ≤ C − H and
    /// its lookahead below share = (C − G − H) / n.
    fn admissible(&self, key: &K) -> bool {
        let f = self.sizes.get(key).copied().unwrap_or(0);
        let headroom = self.budget.saturating_sub(self.required_bytes);
        let share = headroom.saturating_sub(self.generated) / self.sizes.len().max(1);
        let fits = self
            .live
            .checked_add(f)
            .is_some_and(|next| next <= headroom);
        !self.draining && fits && self.lookahead_bytes(key).saturating_add(f) <= share
    }

    /// K-5: `key`'s lookahead bytes: ring frames and decodes in flight that
    /// the job does not require.
    fn lookahead_bytes(&self, key: &K) -> usize {
        let lookahead = |at: &i64| !self.required.contains(&(key.clone(), *at));
        let ring = self.rings.get(key).into_iter().flatten();
        let resident: usize = ring
            .filter(|(at, _)| lookahead(at))
            .map(|(_, f)| f.bytes())
            .sum();
        let flying = self
            .slots
            .iter()
            .filter(|slot| &slot.key == key)
            .filter_map(|slot| {
                let ReaderState::Decoding { at, .. } = slot.state else {
                    return None;
                };
                lookahead(&at).then_some(slot.flight)
            });
        resident + flying.sum::<usize>()
    }

    /// Start decoding `at`: a required frame takes its reservation; a
    /// lookahead one reserves f now (checked by `admissible`).
    fn decode(&mut self, id: u64, at: i64, required: bool) -> Next {
        let Some(key) = self
            .slots
            .iter()
            .find(|slot| slot.id == id)
            .map(|slot| slot.key.clone())
        else {
            return Next::Retire;
        };
        let bytes = if required {
            self.reserved.remove(&(key, at)).unwrap_or(0)
        } else {
            let f = self.sizes.get(&key).copied().unwrap_or(0);
            let reserved = self.reserve(f, self.budget);
            debug_assert!(reserved, "admissible lookahead fits");
            f
        };
        let slot = self.slot(id).expect("the reader's slot");
        let version = slot.plan.version;
        (slot.state, slot.flight) = (ReaderState::Decoding { at, version }, bytes);
        Next::Decode { at, version, bytes }
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
        // A reader retired while it decoded stays retiring (H-6), and its
        // result comes back: the preview is going and has cleared the rings
        // (review A S1), so nothing may repopulate them.
        slot.flight = 0;
        if slot.state == ReaderState::Retiring {
            return (result.as_ref().ok().cloned(), result.err());
        }
        slot.state = ReaderState::Idle { since: now };
        let key = slot.key.clone();
        match result {
            Ok(frame) => {
                slot.cursor = Some(at + 1);
                let wanted = (self.wanted.get(&key)).is_some_and(|wanted| wanted.contains(&at));
                // K-2: while draining, only required frames are kept.
                if wanted && (required || !self.draining) {
                    if let Some(bytes) = self.reserved.remove(&(key.clone(), at)) {
                        self.release(bytes);
                    }
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

    /// K-2: the reader `id`'s lookahead decode stopped (its guard releases
    /// the bytes).
    pub(crate) fn stopped(&mut self, id: u64, now: Duration) {
        if let Some(slot) = self.slot(id) {
            (slot.flight, slot.cursor) = (0, None);
            if slot.state != ReaderState::Retiring {
                slot.state = ReaderState::Idle { since: now };
            }
        }
    }

    /// K-2: whether a required frame has no frame, failure, reservation or
    /// decode in flight (admission, again).
    pub(crate) fn unreserved(&self) -> bool {
        self.required.iter().any(|(key, at)| self.missing(key, *at))
    }

    fn missing(&self, key: &K, at: i64) -> bool {
        let frame = (key.clone(), at);
        let flying = (self.slots.iter()).any(|slot| {
            &slot.key == key && matches!(slot.state, ReaderState::Decoding { at: t, .. } if t == at)
        });
        !(self.rings.get(key)).is_some_and(|ring| ring.contains_key(&at))
            && !self.failures.contains_key(&frame)
            && !self.reserved.contains_key(&frame)
            && !flying
    }

    /// K-2: reserve the job's missing required frames and `generated`
    /// bytes atomically. If they do not fit, drain: no lookahead is
    /// admitted, unpinned lookahead is evicted (K-5) and lookahead decodes
    /// stop; the caller waits for live ownership and admits again.
    /// A set over C never gets here (K-3 is decided before the post).
    pub(crate) fn admit(&mut self, generated: usize) -> Admission<F> {
        let set = self.required_bytes.saturating_add(generated);
        debug_assert!(set <= self.budget, "K-3: a set over C was posted");
        let size = |key: &K| self.sizes.get(key).copied().unwrap_or(0);
        let missing: Vec<(K, i64)> = (self.required.iter())
            .filter(|(key, at)| self.missing(key, *at))
            .cloned()
            .collect();
        let missing: Vec<((K, i64), usize)> = (missing.into_iter())
            .map(|frame| {
                let bytes = size(&frame.0);
                (frame, bytes)
            })
            .collect();
        let q = missing.iter().map(|(_, bytes)| bytes).sum::<usize>() + generated;
        if self.reserve(q, self.budget) {
            self.reserved.extend(missing);
            self.draining = false;
            return Admission::Ready { generated };
        }
        self.draining = true;
        let evicted = self.evict(self.live.saturating_add(q) - self.budget);
        let required = &self.required;
        let stop = (self.slots.iter())
            .filter(|slot| {
                matches!(slot.state, ReaderState::Decoding { at, .. }
                    if !required.contains(&(slot.key.clone(), at)))
            })
            .map(|slot| slot.id)
            .collect();
        Admission::Wait { evicted, stop }
    }

    /// K-5: unpinned lookahead, sources over their share first, then frames
    /// behind their source's travel (review B F4), then the frame farthest
    /// from its source's required times, until `need` bytes will be
    /// released.
    fn evict(&mut self, need: usize) -> Vec<F> {
        type Rank = (bool, bool, std::cmp::Reverse<u64>);
        let headroom = self.budget.saturating_sub(self.required_bytes);
        let share = headroom.saturating_sub(self.generated) / self.sizes.len().max(1);
        let mut victims: Vec<(Rank, K, i64)> = Vec::new();
        for (key, ring) in &self.rings {
            let over = self.lookahead_bytes(key) > share;
            let travel = self.travel.get(key).map_or(Travel::Forward, |(_, t)| *t);
            let rank = |at: i64| -> Rank {
                let required = self.required.iter().filter(|(k, _)| k == key);
                let nearest = (required.map(|(_, t)| *t)).min_by_key(|t| (t.abs_diff(at), *t));
                let Some(point) = nearest else {
                    return (!over, false, std::cmp::Reverse(u64::MAX));
                };
                let behind = match travel {
                    Travel::Forward => at < point,
                    Travel::Backward => at > point,
                };
                (!over, !behind, std::cmp::Reverse(point.abs_diff(at)))
            };
            for (at, frame) in ring {
                if !self.required.contains(&(key.clone(), *at)) && !frame.pinned() {
                    victims.push((rank(*at), key.clone(), *at));
                }
            }
        }
        victims.sort_by_key(|(rank, _, at)| (*rank, *at));
        let mut freed = 0;
        let mut evicted = Vec::new();
        for (_, key, at) in victims {
            if freed >= need {
                break;
            }
            if let Some(frame) = self.rings.get_mut(&key).and_then(|ring| ring.remove(&at)) {
                freed += frame.bytes();
                evicted.push(frame);
            }
        }
        self.rings.retain(|_, ring| !ring.is_empty());
        evicted
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

    /// Every reader retires (the preview is going); the reservations of
    /// frames not yet decoding return.
    pub(crate) fn retire_all(&mut self) {
        for slot in &mut self.slots {
            slot.state = ReaderState::Retiring;
        }
        self.pending.clear();
        let reserved: usize = self.reserved.drain().map(|(_, bytes)| bytes).sum();
        self.release(reserved);
    }

    /// The readers decoding now (Amendment R37: a withdrawn plan stops
    /// them).
    pub(crate) fn decoding(&self) -> Vec<u64> {
        let decoding = |slot: &&Slot<K>| matches!(slot.state, ReaderState::Decoding { .. });
        self.slots
            .iter()
            .filter(decoding)
            .map(|slot| slot.id)
            .collect()
    }

    /// Review A F1: a preview cache clear. While a job waits (`active`),
    /// its required frames stay (its readers, reservations and wait are
    /// unchanged) and every other ring frame goes. Otherwise the demand is
    /// complete and is invalidated with the frames: an empty plan (K-3's),
    /// so no reader stays owed a cleared frame, and idle readers quiesce.
    pub(crate) fn clear_cache(&mut self, active: bool, now: Duration) -> Posted<K, F> {
        if !active {
            return self.post(Vec::new(), (HashMap::new(), 0), now);
        }
        let required = &self.required;
        let mut frames = Vec::new();
        self.rings.retain(|key, ring| {
            let gone = ring.extract_if(.., |t, _| !required.contains(&(key.clone(), *t)));
            frames.extend(gone.map(|(_, frame)| frame));
            !ring.is_empty()
        });
        Posted {
            spawn: Vec::new(),
            cancel: Vec::new(),
            retired: false,
            dropped: (frames, Vec::new()),
        }
    }

    /// Every ring frame, removed (the preview is going).
    pub(crate) fn clear_rings(&mut self) -> Vec<F> {
        let rings = self.rings.drain().flat_map(|(_, ring)| ring.into_values());
        rings.collect()
    }

    /// The times `key`'s ring holds.
    #[cfg(test)]
    pub(crate) fn ring_times(&self, key: &K) -> Vec<i64> {
        self.rings
            .get(key)
            .map(|ring| ring.keys().copied().collect())
            .unwrap_or_default()
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
    /// Polls per reader (review A F3's witness).
    #[cfg(test)]
    pub(crate) polls: BTreeMap<u64, usize>,
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
            #[cfg(test)]
            polls: BTreeMap::new(),
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
        #[cfg(test)]
        {
            *self.polls.entry(id).or_default() += 1;
        }
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
    //! H-8 / I15 (S2b-1), I11 (S2b-2), I12 (S2b-3): an exhaustive
    //! (event × state) model of the readers' shared state, the permit book
    //! and the preview's admission, then seeded sequences; every loop has a
    //! step budget, the clock is injected and nothing sleeps.
    use std::collections::BTreeSet;

    use super::*;

    /// A model frame: its value, its reservation, and whether a render
    /// pins it (the preview's `Arc` count).
    #[derive(Clone, Debug)]
    struct Fr {
        value: u32,
        bytes: usize,
        pinned: bool,
    }

    impl Weighed for Fr {
        fn bytes(&self) -> usize {
            self.bytes
        }

        fn pinned(&self) -> bool {
            self.pinned
        }
    }

    type Model = Readers<u8, Fr>;
    /// A job: per source, its required and lookahead times; then G.
    type Job = (&'static [(u8, &'static [i64], &'static [i64])], usize);

    const JOBS: [Job; 8] = [
        (&[(0, &[0], &[1, 2])], 0),
        (&[(0, &[0], &[1]), (1, &[5], &[6])], F),
        // A same-source jump: two regions of source 0.
        (&[(0, &[0, 40], &[41])], 0),
        (&[(1, &[5], &[]), (2, &[9], &[]), (0, &[1], &[])], 0),
        // Title only.
        (&[], F),
        // Pre-roll: a lookahead-only region.
        (&[(0, &[3], &[30, 31])], 0),
        // K-3: a required set over C.
        (&[(0, &[0, 1, 2, 3, 4, 5], &[])], 0),
        // K-2: job 0's lookahead still wanted beside a wider set: it drains.
        (&[(0, &[0], &[1, 2]), (1, &[5], &[]), (2, &[9], &[])], 2 * F),
    ];
    const BUDGET: usize = 512;
    /// f, each source's frame bytes, and C (K-1) for the model.
    const F: usize = 10;
    const C: usize = 50;

    /// The value of (source, time): any reader decodes the same value, so a
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
        /// Decodes in flight: (reader, key, time, version, reserved bytes).
        flight: Vec<(u64, u8, i64, u64, usize)>,
        /// Readers that returned `Retire` and are closing.
        closing: Vec<u64>,
        /// The newest job's required set, if it went to the readers.
        job: Option<Vec<(u8, i64)>>,
        /// (source, frame threads) of every decode started.
        decodes: Vec<(u8, usize)>,
        /// K-2: the job's G and sources (n), and whether it still waits for
        /// admission.
        generated: usize,
        sources: usize,
        admitting: bool,
        /// K-1's other owners: the admitted G, the kept title rasters, and
        /// a render's pins (and its G).
        granted: usize,
        titles: usize,
        render: Option<(Vec<(u8, i64)>, usize)>,
        /// Readers whose stop flag is set (K-2).
        stops: BTreeSet<u64>,
    }

    #[derive(Clone, Copy, Debug)]
    enum Event {
        Post(usize),
        Step(usize),
        Finish(usize, bool),
        /// A stopped lookahead decode returns `Cancelled` (K-2).
        Cancel(usize),
        /// The preview admits the job's set (again).
        Admit,
        /// The job renders (pins its frames), then is done (unpins).
        Render,
        Done,
        Exit(usize),
        Poll(usize),
        Release(usize),
        Assign {
            fail: bool,
        },
        Tick,
        Shutdown,
    }

    impl World {
        fn new(parallelism: usize) -> Self {
            Self {
                readers: Model::new(parallelism).with_budget(C),
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
                generated: 0,
                sources: 0,
                admitting: false,
                granted: 0,
                titles: 0,
                render: None,
                stops: BTreeSet::new(),
            }
        }

        fn events(&self) -> Vec<Event> {
            // The preview renders synchronously: no post while it renders.
            let mut events: Vec<Event> = if self.render.is_none() {
                (0..JOBS.len()).map(Event::Post).collect()
            } else {
                vec![Event::Done]
            };
            events.extend(self.stepping().into_iter().map(Event::Step));
            for (index, (id, ..)) in self.flight.iter().enumerate() {
                events.extend([Event::Finish(index, true), Event::Finish(index, false)]);
                if self.stops.contains(id) {
                    events.push(Event::Cancel(index));
                }
            }
            if self.admits() {
                events.push(Event::Admit);
            }
            if self.renders() {
                events.push(Event::Render);
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

        /// The preview admits while its job has G or a frame unreserved.
        fn admits(&self) -> bool {
            let pending = self.admitting || self.readers.unreserved();
            !self.shutdown && self.render.is_none() && self.job.is_some() && pending
        }

        /// The job renders once admitted and resolved.
        fn renders(&self) -> bool {
            let resolved =
                (self.job.as_ref()).is_some_and(|job| self.readers.resolve(job).is_some());
            self.render.is_none() && !self.admitting && resolved
        }

        /// Frames handed back release their bytes (the last owner went):
        /// never one a render pins.
        fn drop_frames(&mut self, frames: Vec<Fr>) {
            for frame in frames {
                assert!(!frame.pinned, "K-5: a pinned frame left the rings");
                self.readers.release(frame.bytes);
            }
        }

        fn state_of(&self, id: u64) -> &'static str {
            match self.readers.slots.iter().find(|slot| slot.id == id) {
                Some(slot) => match slot.state {
                    ReaderState::Idle { .. } => "idle",
                    ReaderState::BudgetWait { .. } => "budget-wait",
                    ReaderState::PermitWait => "permit-wait",
                    ReaderState::Decoding { .. } => "decoding",
                    ReaderState::Retiring => "retiring",
                },
                None => "gone",
            }
        }

        /// Apply `event`; returns the (event × state) cell it covered.
        #[allow(clippy::too_many_lines)] // one arm per event
        fn apply(&mut self, event: Event) -> String {
            match event {
                Event::Post(job) => {
                    let demand: Vec<(u8, Vec<i64>, Vec<i64>)> = (JOBS[job].0.iter())
                        .map(|(k, r, l)| (*k, r.to_vec(), l.to_vec()))
                        .collect();
                    self.post(&demand, JOBS[job].1)
                }
                Event::Step(index) => self.step(index),
                Event::Finish(index, ok) => {
                    let (id, key, at, version, bytes) = self.flight.remove(index);
                    let current = version == self.readers.version();
                    let required = self
                        .job
                        .as_ref()
                        .is_some_and(|job| job.contains(&(key, at)));
                    let draining = self.readers.draining;
                    let result = if ok {
                        let value = frame(key, at);
                        Ok(Fr {
                            value,
                            bytes,
                            pinned: false,
                        })
                    } else {
                        // The reader's guard releases (after unlock).
                        Err(MediaError::Backend(format!("decode {key}@{at} v{version}")))
                    };
                    let (frame_back, _) = self.readers.deliver(id, at, version, result, self.now);
                    let kept = ok && frame_back.is_none();
                    assert!(
                        !(kept && draining && !required),
                        "K-2: lookahead kept draining"
                    );
                    if let Some(back) = &frame_back {
                        assert_eq!(
                            back.value,
                            frame(key, at),
                            "a returned frame was relabelled"
                        );
                    }
                    self.drop_frames(frame_back.into_iter().collect());
                    if !ok {
                        self.readers.release(bytes);
                    }
                    let fresh = if current { "current" } else { "stale" };
                    let need = if required { "required" } else { "lookahead" };
                    let drain = if draining { "-draining" } else { "" };
                    format!(
                        "finish-{}×{fresh}-{need}{drain}",
                        if ok { "ok" } else { "err" }
                    )
                }
                Event::Cancel(index) => {
                    let (id, .., bytes) = self.flight.remove(index);
                    self.readers.stopped(id, self.now);
                    self.readers.release(bytes);
                    "cancel".into()
                }
                Event::Admit => self.admit(),
                Event::Render => {
                    let job = self.job.clone().expect("a job");
                    for (key, at) in &job {
                        let ring = self.readers.rings.get_mut(key);
                        if let Some(frame) = ring.and_then(|ring| ring.get_mut(at)) {
                            frame.pinned = true;
                        }
                    }
                    self.render = Some((job, std::mem::take(&mut self.granted)));
                    "render".into()
                }
                Event::Done => {
                    let (job, generated) = self.render.take().expect("a render");
                    for (key, at) in &job {
                        let ring = self.readers.rings.get_mut(key);
                        if let Some(frame) = ring.and_then(|ring| ring.get_mut(at)) {
                            frame.pinned = false;
                        }
                    }
                    // The renderer keeps only this job's rasters (G).
                    self.titles += generated;
                    let excess = self.titles.saturating_sub(self.generated);
                    self.titles -= excess;
                    self.readers.release(excess);
                    "done".into()
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
                    // The preview goes: its drop retires every reader.
                    self.readers.retire_all();
                    format!("shutdown×{states:?}")
                }
            }
        }

        /// A reader's `next` under the lock, and what the thread does.
        fn step(&mut self, index: usize) -> String {
            let id = self.live[index];
            let state = self.state_of(id);
            let slot = self.readers.slots.iter().find(|slot| slot.id == id);
            let ahead = slot.map_or(0, |slot| self.lookahead(slot.key));
            let before = (self.readers.draining, self.readers.live);
            match self.readers.next(id, self.now, self.shutdown) {
                Next::Decode { at, version, bytes } => {
                    let key = self.key(id);
                    let required = self
                        .job
                        .as_ref()
                        .is_some_and(|job| job.contains(&(key, at)));
                    if !required {
                        // K-2, independently: ¬draining, live + f ≤ C − H
                        // and the source's lookahead + f ≤ its share.
                        let h = self.job.as_ref().map_or(0, Vec::len) * F;
                        let share = (C - h).saturating_sub(self.generated) / self.sources.max(1);
                        let (draining, live) = before;
                        assert!(!draining, "K-2: lookahead started draining");
                        assert!(live + F <= C - h, "K-2: lookahead over C − H");
                        assert!(ahead + F <= share, "K-2: lookahead over its share");
                    }
                    // The reader clears its stop flag under `Sched`.
                    self.stops.remove(&id);
                    self.flight.push((id, key, at, version, bytes));
                    let threads = self.slot(id).threads;
                    assert!(threads >= 1, "a decoder without permits");
                    self.decodes.push((key, threads));
                    let deferred = threads > self.readers.want
                        && (self.readers.slots.iter()).any(|slot| {
                            slot.state == ReaderState::PermitWait
                                || slot.threads > 0 && slot.threads < self.readers.want
                        });
                    if deferred {
                        // Review A F2: only a required decode defers a shrink.
                        assert!(required, "lookahead decoded before a needed shrink");
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
                    let blocked = self.state_of(id) == "budget-wait";
                    format!(
                        "step×{state}→{}",
                        if blocked { "budget-wait" } else { "wait" }
                    )
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
        fn post(&mut self, demand: &[(u8, Vec<i64>, Vec<i64>)], generated: usize) -> String {
            if self.shutdown {
                return "post×shutdown".into();
            }
            // The last job's unrendered G returns (it was superseded).
            self.readers.release(std::mem::take(&mut self.granted));
            let quiet = self.flight.is_empty();
            let required: BTreeSet<(u8, i64)> = demand
                .iter()
                .flat_map(|(k, r, _)| r.iter().map(|t| (*k, *t)))
                .collect();
            let regions = plan_regions(demand, self.readers.limit()).map(|(regions, _)| regions);
            let over = !self.readers.fits(required.len() * F + generated);
            let fallback = regions.is_none() || over;
            let sizes = if fallback {
                HashMap::new()
            } else {
                demand.iter().map(|(k, ..)| (*k, F)).collect()
            };
            let regions = regions.filter(|_| !over).unwrap_or_default();
            let posted = self.readers.post(regions, (sizes, generated), self.now);
            self.live.extend(posted.spawn.iter().map(|(id, _)| *id));
            let cancelled = !posted.cancel.is_empty();
            posted.cancel.iter().for_each(|id| self.book.cancel(*id));
            self.drop_frames(posted.dropped.0);
            self.job = (!fallback).then(|| required.into_iter().collect());
            (self.generated, self.admitting) = (generated, !fallback);
            self.sources = demand.len();
            if fallback {
                // More sources than readers, or a set over C: K-3.
                return format!("post→fallback-{}", if over { "budget" } else { "readers" });
            }
            if cancelled {
                return "post→cancel-ticket".into();
            }
            format!("post×{}", if quiet { "quiet" } else { "decoding" })
        }

        /// K-2 as the preview's `schedule` loop does it.
        fn admit(&mut self) -> String {
            let generated = if self.admitting { self.generated } else { 0 };
            match self.readers.admit(generated) {
                Admission::Ready { generated } => {
                    self.granted += generated;
                    self.admitting = false;
                    "admit→ready".into()
                }
                Admission::Wait { evicted, stop } => {
                    let label = format!(
                        "admit→wait{}{}",
                        if evicted.is_empty() { "" } else { "-evict" },
                        if stop.is_empty() { "" } else { "-stop" }
                    );
                    self.drop_frames(evicted);
                    self.stops.extend(stop);
                    // The title rasters go too.
                    self.readers.release(std::mem::take(&mut self.titles));
                    label
                }
            }
        }

        /// K-5, independently: `key`'s lookahead bytes, resident and in
        /// flight (frames the job does not require).
        fn lookahead(&self, key: u8) -> usize {
            let job = self.job.as_deref().unwrap_or_default();
            let ring = self.readers.rings.get(&key).into_iter().flatten();
            let resident = ring.filter(|(at, _)| !job.contains(&(key, **at)));
            let flying = (self.flight.iter())
                .filter(|(_, k, at, ..)| *k == key && !job.contains(&(key, *at)));
            resident.map(|(_, f)| f.bytes).sum::<usize>()
                + flying.map(|(.., bytes)| bytes).sum::<usize>()
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
            // K-1/I12: live counts every owner exactly, and never passes C.
            let rings: usize = readers
                .rings
                .values()
                .flat_map(BTreeMap::values)
                .map(|f| f.bytes)
                .sum();
            let reserved: usize = readers.reserved.values().sum();
            let flying: usize = self.flight.iter().map(|(.., bytes)| bytes).sum();
            let rendering = self.render.as_ref().map_or(0, |(_, generated)| *generated);
            let owners = rings + reserved + flying + self.granted + self.titles;
            assert_eq!(
                readers.live,
                owners + rendering,
                "K-1: live bytes and their owners"
            );
            assert!(readers.live().1 <= C, "I12: live bytes over C");
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
                    assert_eq!(value.value, frame(*key, *at), "a relabelled ring frame");
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
                let resolved = !self.admitting
                    && (self.job.as_ref()).is_none_or(|job| self.readers.resolve(job).is_some());
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
                if self.render.is_some() {
                    self.apply(Event::Done);
                }
                if self.admits() {
                    self.apply(Event::Admit);
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
    const CELLS: [&str; 38] = [
        "post→fallback-readers",
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
        // S2b-3 (K-1…K-5, I12).
        "post→fallback-budget",
        "admit→ready",
        "admit→wait-evict",
        "admit→wait-stop",
        "cancel",
        "finish-ok×stale-lookahead-draining",
        "step×idle→budget-wait",
        "step×budget-wait→decode",
        "step×budget-wait→wait",
        "render",
        "done",
    ];

    /// I15 (S2b-1): every sequence of five events from each P's start (four
    /// from the two targeted starts below), then the liveness run from every
    /// leaf. Bounded exploration of the pure transitions: condvar wake-ups
    /// and cache clears are not modelled (their witnesses are in preview).
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
        world.apply(Event::Admit);
        explore(&world, 4, &mut cells, &mut visited);
        // K-2: job 0's reader decodes its lookahead when a wider set that
        // still wants it arrives: admission drains, stops and evicts.
        let mut world = World::new(20);
        for event in [Event::Post(0), Event::Admit, Event::Step(0), Event::Poll(0)] {
            world.apply(event);
        }
        for event in [Event::Step(0), Event::Finish(0, true), Event::Step(0)] {
            world.apply(event);
        }
        assert_eq!(world.flight.len(), 1, "a lookahead decode in flight");
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
        world.apply(Event::Admit);
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
        let posted = world
            .readers
            .post(regions, (HashMap::from([(0, F)]), 0), world.now);
        world.drop_frames(posted.dropped.0);
        world.job = Some(vec![(0, 0)]);
        world.apply(Event::Admit);
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
        // Unreserved again: the preview re-admits it (K-2).
        assert_eq!(world.apply(Event::Admit), "admit→ready");
        assert_eq!(world.apply(Event::Step(1)), "step×idle→decode");
        world.check_live();
    }

    /// K-2 (S2b-3): a source's lookahead stops at its share
    /// (C − G − H) / n, here below C − H: G counts from the post.
    #[test]
    fn lookahead_stops_at_its_share_of_c_less_g() {
        let mut readers = Model::new(20).with_budget(10 * F);
        let demand = [(0, vec![0], vec![1, 2, 3, 4])];
        let regions = plan_regions(&demand, readers.limit()).expect("a reader").0;
        readers.post(regions, (HashMap::from([(0, F)]), 6 * F), Duration::ZERO);
        // share = (10 − 6 − 1) f = 3f; C − H = 9f.
        for at in 1..=3 {
            assert!(readers.admissible(&0), "lookahead {at} is within the share");
            let fr = Fr {
                value: frame(0, at),
                bytes: F,
                pinned: false,
            };
            readers.rings.entry(0).or_default().insert(at, fr);
            assert!(readers.reserve(F, usize::MAX));
        }
        assert!(!readers.admissible(&0), "a fourth frame passes the share");
    }

    /// K-5 (S2b-3): a drain evicts unpinned lookahead of sources over
    /// their share first, farthest from the required times first, and only
    /// what the set needs; required and pinned frames stay.
    #[test]
    fn eviction_takes_over_share_sources_then_the_farthest() {
        let mut readers = Model::new(20).with_budget(10 * F);
        let demand = [(0, vec![0], vec![1, 2, 3, 4, 5]), (1, vec![0], vec![1, 2])];
        let regions = plan_regions(&demand, readers.limit())
            .expect("two readers")
            .0;
        let sizes = HashMap::from([(0, F), (1, F)]);
        readers.post(regions, (sizes, 4 * F), Duration::ZERO);
        let fr = |key: u8, at: i64| Fr {
            value: frame(key, at),
            bytes: F,
            pinned: key == 0 && at == 5,
        };
        for (key, ats) in [(0u8, 1..=6), (1, 1..=2)] {
            let ring = readers.rings.entry(key).or_default();
            ring.extend(ats.map(|at| (at, fr(key, at))));
        }
        assert!(readers.reserve(8 * F, usize::MAX));
        // H = 2f, G = 4f: share = (10 − 2 − 4) f / 2 = 2f. Source 0 holds
        // 6f (over), source 1 2f (not over). Live 8f + the missing 2f + G
        // = 14f: 4f must go.
        let Admission::Wait { evicted, stop } = readers.admit(4 * F) else {
            panic!("the set does not fit beside the lookahead");
        };
        let order: Vec<u32> = evicted.iter().map(|f| f.value).collect();
        assert_eq!(order, [frame(0, 6), frame(0, 4), frame(0, 3), frame(0, 2)]);
        assert!(stop.is_empty());
        assert!(!readers.admissible(&1), "no lookahead while draining");
    }

    /// K-5 (review B F4): a drain evicts the lookahead behind its source's
    /// travel first: forward (99 → 100) evicts 99 before 101, a reversal
    /// (101 → 100) evicts 101 before 99, and a tie (99 and 101 → 100: steps
    /// +1 and −1) is forward.
    #[test]
    fn eviction_follows_the_travel_direction() {
        for (previous, first) in [(&[99][..], 99), (&[101][..], 101), (&[99, 101][..], 99)] {
            let mut readers = Model::new(20).with_budget(4 * F);
            let sizes = || (HashMap::from([(0, F)]), 0);
            let region = |required: &[i64], lookahead: &[i64]| {
                let region = Region {
                    required: required.to_vec(),
                    lookahead: lookahead.to_vec(),
                };
                vec![(0u8, region)]
            };
            readers.post(region(previous, &[]), sizes(), Duration::ZERO);
            readers.post(region(&[100], &[99, 101]), sizes(), Duration::ZERO);
            for at in [99, 101] {
                let fr = Fr {
                    value: frame(0, at),
                    bytes: F,
                    pinned: false,
                };
                readers.rings.entry(0).or_default().insert(at, fr);
            }
            assert!(readers.reserve(2 * F, usize::MAX));
            // Live 2f + the missing 100 (f) + G 2f = 5f > C = 4f: one goes.
            let Admission::Wait { evicted, .. } = readers.admit(2 * F) else {
                panic!("{previous:?}: the set does not fit beside the lookahead");
            };
            let order: Vec<u32> = evicted.iter().map(|f| f.value).collect();
            assert_eq!(order, [frame(0, first)], "{previous:?} → 100");
        }
    }

    /// H-6: a reader retired while its decode ran (the preview went) stays
    /// retiring when the result arrives, though its required frame is gone.
    /// Review A S1: that late frame comes back (it is not kept in the
    /// cleared rings), and its bytes are released with it.
    #[test]
    fn a_reader_retired_mid_decode_stays_retiring() {
        let mut world = World::new(20);
        world.apply(Event::Post(1));
        world.apply(Event::Admit);
        let id = world.live[0];
        assert_eq!(world.apply(Event::Step(0)), "step×idle→open");
        world.apply(Event::Poll(0));
        world.apply(Event::Step(0));
        let slot = world.readers.slots.iter().find(|slot| slot.id == id);
        let Some(ReaderState::Decoding { at, version }) = slot.map(|slot| slot.state) else {
            panic!("the reader decodes");
        };
        world.readers.retire_all();
        let cleared = world.readers.clear_rings();
        world.drop_frames(cleared);
        let (_, _, _, _, bytes) = world.flight.remove(0);
        let fr = Fr {
            value: frame(0, at),
            bytes,
            pinned: false,
        };
        let (back, _) = world.readers.deliver(id, at, version, Ok(fr), world.now);
        let back = back.expect("the late frame comes back");
        assert_eq!(back.value, frame(0, at));
        world.drop_frames(vec![back]);
        assert!(world.readers.rings.is_empty(), "the rings stay empty");
        let owners = world.granted + world.titles;
        assert_eq!(world.readers.live().0, owners, "its bytes were released");
        assert_eq!(world.readers.next(id, world.now, false), Next::Retire);
    }

    /// Review A F2 (H-3/H-5): at P = 2, reader A holds both permits; the
    /// plan widens to A (its required frame cached, lookahead admissible)
    /// and B (required, waiting for a permit). A closes at its decode
    /// boundary instead of decoding lookahead, and B is granted.
    #[test]
    fn a_needed_shrink_comes_before_more_lookahead() {
        let mut readers = Model::new(2).with_budget(1_000 * F);
        let mut book = PermitBook::new(2);
        let sizes = || HashMap::from([(0, F), (1, F)]);
        let post = |readers: &mut Model, demand: &[(u8, Vec<i64>, Vec<i64>)]| {
            let regions = plan_regions(demand, readers.limit()).expect("readers").0;
            let posted = readers.post(regions, (sizes(), 0), Duration::ZERO);
            assert!(matches!(readers.admit(0), Admission::Ready { .. }));
            posted.spawn
        };
        post(&mut readers, &[(0, vec![0], vec![])]);
        let a = readers.slots[0].id;
        assert_eq!(
            readers.next(a, Duration::ZERO, false),
            Next::Open { want: 2 }
        );
        book.enqueue(a, 2);
        assert_eq!(book.poll(a), Poll::Granted(2));
        readers.granted(a, Some(2), Duration::ZERO);
        let Next::Decode { at, version, bytes } = readers.next(a, Duration::ZERO, false) else {
            panic!("A decodes its required frame");
        };
        let fr = Fr {
            value: frame(0, at),
            bytes,
            pinned: false,
        };
        assert!(
            readers
                .deliver(a, at, version, Ok(fr), Duration::ZERO)
                .0
                .is_none()
        );
        let spawned = post(
            &mut readers,
            &[(0, vec![0], vec![1, 2, 3, 4, 5]), (1, vec![5], vec![])],
        );
        let [(b, 1)] = spawned[..] else {
            panic!("B starts: {spawned:?}");
        };
        assert_eq!(
            readers.next(b, Duration::ZERO, false),
            Next::Open { want: 1 }
        );
        book.enqueue(b, 1);
        assert_eq!(book.poll(b), Poll::Wait, "A holds the pool");
        assert!(readers.admissible(&0), "A's lookahead is admissible");
        assert_eq!(readers.next(a, Duration::ZERO, false), Next::Close);
        book.release(a);
        readers.closed(a);
        assert_eq!(
            book.poll(b),
            Poll::Granted(1),
            "B's required frame goes next"
        );
        readers.granted(b, Some(1), Duration::ZERO);
        assert!(matches!(
            readers.next(b, Duration::ZERO, false),
            Next::Decode { at: 5, .. }
        ));
        assert_eq!(
            readers.next(a, Duration::ZERO, false),
            Next::Open { want: 1 }
        );
    }

    /// Run every reader of `readers` until each waits: grants in full,
    /// decodes that succeed (recorded per reader), closes and exits.
    fn drive(readers: &mut Model, decoded: &mut BTreeMap<u64, Vec<i64>>) {
        for _ in 0..BUDGET {
            let mut moved = false;
            let ids: Vec<(u64, u8)> = (readers.slots.iter())
                .map(|slot| (slot.id, slot.key))
                .collect();
            for (id, key) in ids {
                match readers.next(id, Duration::ZERO, false) {
                    Next::Open { want } => readers.granted(id, Some(want), Duration::ZERO),
                    Next::Decode { at, version, bytes } => {
                        decoded.entry(id).or_default().push(at);
                        let value = frame(key, at);
                        let fr = Fr {
                            value,
                            bytes,
                            pinned: false,
                        };
                        let (back, _) = readers.deliver(id, at, version, Ok(fr), Duration::ZERO);
                        readers.release(back.map_or(0, |back| back.bytes));
                    }
                    Next::Close => readers.closed(id),
                    Next::Retire => readers.exited(id),
                    Next::Wait { .. } => continue,
                }
                moved = true;
            }
            if !moved {
                return;
            }
        }
        panic!("the readers did not settle within {BUDGET} steps");
    }

    /// Amendment R37 (review B F3): `typical_1080p`'s two same-source
    /// playheads 14 frames apart, each with a 10-frame window, played for
    /// 40 frames: two readers, each decoding forward only, one seek each
    /// (its open); a merged region would alternate between the playheads.
    #[test]
    fn two_playheads_fourteen_apart_read_forward_on_two_readers() {
        let shapes = |required: &[i64], lookahead: &[i64]| -> Vec<(Vec<i64>, Vec<i64>)> {
            let runs = regions(required, lookahead).into_iter();
            runs.map(|run| (run.required, run.lookahead)).collect()
        };
        // Continuation: required then lookahead, each the exact next frame.
        assert_eq!(shapes(&[0, 1], &[2, 3]), [(vec![0, 1], vec![2, 3])]);
        // A gap in the lookahead (a same-source cut) is a new region.
        let cut = shapes(&[0], &[1, 2, 3, 6, 7]);
        assert_eq!(cut, [(vec![0], vec![1, 2, 3]), (vec![], vec![6, 7])]);
        // A required time after lookahead is decoded first: a new region.
        let behind = shapes(&[5], &[3, 4]);
        assert_eq!(behind, [(vec![5], vec![]), (vec![], vec![3, 4])]);
        let mut readers = Model::new(20).with_budget(1_000 * F);
        let mut decoded = BTreeMap::new();
        for t in 0..40 {
            let lookahead = (t + 1..=t + 9).chain(t + 15..=t + 23).collect();
            let demand = [(0u8, vec![t, t + 14], lookahead)];
            let regions = plan_regions(&demand, readers.limit()).expect("readers").0;
            let posted = readers.post(regions, (HashMap::from([(0, F)]), 0), Duration::ZERO);
            let dropped: usize = posted.dropped.0.iter().map(|fr| fr.bytes).sum();
            readers.release(dropped);
            assert!(matches!(readers.admit(0), Admission::Ready { .. }));
            drive(&mut readers, &mut decoded);
            let resolved = readers.resolve(&[(0, t), (0, t + 14)]);
            assert!(resolved.is_some(), "frame {t} resolved");
            assert_eq!(readers.slots.len(), 2, "frame {t}: one reader per playhead");
        }
        assert_eq!(decoded.len(), 2, "two readers decoded: {decoded:?}");
        for (id, times) in &decoded {
            let seeks = 1 + times.windows(2).filter(|w| w[1] != w[0] + 1).count();
            assert_eq!(seeks, 1, "reader {id} seeks once (its open): {times:?}");
        }
    }

    /// R38 (review B F3, the reader limit): at P = 2, A requires 0 and 14
    /// (lookahead 1, 2, 15, 16) and B requires 7: three required regions,
    /// two readers. The last resort merges A's two regions, keeps only the
    /// lookahead past 14 and counts one merge; every reader decodes forward
    /// only (the old fallback kept 1 and 2 and read A 0 → 14 → 1).
    #[test]
    fn over_the_reader_limit_a_merged_region_still_reads_forward() {
        let mut readers = Model::new(2).with_budget(1_000 * F);
        assert_eq!(readers.limit(), 2);
        let demand = [(0u8, vec![0, 14], vec![1, 2, 15, 16]), (1, vec![7], vec![])];
        let (regions, merged) = plan_regions(&demand, readers.limit()).expect("two readers");
        let shapes: Vec<(u8, Vec<i64>, Vec<i64>)> = (regions.iter())
            .map(|(key, run)| (*key, run.required.clone(), run.lookahead.clone()))
            .collect();
        assert_eq!(
            shapes,
            [(0, vec![0, 14], vec![15, 16]), (1, vec![7], vec![])],
            "A's regions merged forward only"
        );
        assert_eq!(merged, 1, "the last resort is counted");
        let sizes = HashMap::from([(0, F), (1, F)]);
        readers.post(regions, (sizes, 0), Duration::ZERO);
        assert!(matches!(readers.admit(0), Admission::Ready { .. }));
        let mut decoded = BTreeMap::new();
        drive(&mut readers, &mut decoded);
        assert!(readers.resolve(&[(0, 0), (0, 14), (1, 7)]).is_some());
        assert_eq!(decoded.len(), 2, "two readers: {decoded:?}");
        for (id, times) in &decoded {
            assert!(
                times.windows(2).all(|w| w[1] > w[0]),
                "reader {id} decodes forward only: {times:?}"
            );
        }
        // Without the pressure nothing merges and nothing is counted.
        let (_, merged) = plan_regions(&demand, 3).expect("three readers");
        assert_eq!(merged, 0, "no merge within the limit");
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
                world.post(&demand, 0);
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
