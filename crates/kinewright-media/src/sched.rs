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

/// Amendment R41 (U-1): at most this many sources in a [`SourceMemory`].
pub(crate) const SOURCE_MEMORY: usize = 256;

/// Amendment R41 (U-1): what the preview remembers per source between jobs
/// (a frame size, a travel direction). Its owner prunes a source that
/// leaves the document or the plan; past `cap` sources the least recently
/// used is forgotten, and is measured or followed again if it returns.
#[derive(Clone, Debug)]
pub(crate) struct SourceMemory<K, V> {
    entries: HashMap<K, (V, u64)>,
    tick: u64,
    cap: usize,
}

impl<K, V> Default for SourceMemory<K, V> {
    fn default() -> Self {
        Self::with_cap(SOURCE_MEMORY)
    }
}

impl<K, V> SourceMemory<K, V> {
    pub(crate) fn with_cap(cap: usize) -> Self {
        Self {
            entries: HashMap::new(),
            tick: 0,
            cap: cap.max(1),
        }
    }

    #[cfg(test)]
    pub(crate) fn keys(&self) -> impl Iterator<Item = &K> {
        self.entries.keys()
    }

    #[cfg(test)]
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter().map(|(key, (value, _))| (key, value))
    }

    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        self.entries.retain(|key, _| keep(key));
    }
}

impl<K: Clone + Eq + Hash, V> SourceMemory<K, V> {
    /// The value, without touching its recency.
    pub(crate) fn peek(&self, key: &K) -> Option<&V> {
        self.entries.get(key).map(|(value, _)| value)
    }

    /// The value, now the most recently used.
    pub(crate) fn get(&mut self, key: &K) -> Option<&V> {
        self.tick += 1;
        let tick = self.tick;
        let (value, used) = self.entries.get_mut(key)?;
        *used = tick;
        Some(value)
    }

    /// `key`'s value (inserted by `make` if absent), now the most recently
    /// used; a new source past `cap` forgets the least recently used one.
    pub(crate) fn get_or_insert_with(&mut self, key: K, make: impl FnOnce() -> V) -> &mut V {
        self.tick += 1;
        if !self.entries.contains_key(&key) {
            self.make_room();
        }
        let entry = self.entries.entry(key).or_insert_with(|| (make(), 0));
        entry.1 = self.tick;
        &mut entry.0
    }

    /// `key` holds `value`, now the most recently used.
    pub(crate) fn insert(&mut self, key: K, value: V) {
        self.tick += 1;
        if !self.entries.contains_key(&key) {
            self.make_room();
        }
        self.entries.insert(key, (value, self.tick));
    }

    /// Past `cap`, the least recently used source is forgotten.
    fn make_room(&mut self) {
        if self.entries.len() < self.cap {
            return;
        }
        let oldest = (self.entries.iter())
            .min_by_key(|(_, (_, used))| *used)
            .map(|(key, _)| key.clone());
        if let Some(oldest) = oldest {
            self.entries.remove(&oldest);
        }
    }
}

impl<K: Clone + Eq + Hash, V> FromIterator<(K, V)> for SourceMemory<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut memory = Self::default();
        for (key, value) in iter {
            memory.insert(key, value);
        }
        memory
    }
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
    /// Amendment R41: a fallback region (see [`Region::merged`]).
    pub(crate) merged: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Slot<K> {
    pub(crate) id: u64,
    pub(crate) key: K,
    pub(crate) plan: Plan,
    pub(crate) state: ReaderState,
    /// The time its decoder continues from without a seek.
    cursor: Option<i64>,
    /// Amendment R41: its position, the time it last started decoding
    /// (just before it, if that decode stopped), kept across closes and
    /// plans; a decode at or before it is a rewind.
    last: Option<i64>,
    /// Amendment R43: its position within the current plan version (`None`
    /// before its first decode in it); its lookahead stays past it.
    floor: Option<i64>,
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
    /// Amendment R41: a fallback region, two or more of one source's
    /// playheads on one reader. Within a job it decodes forward only;
    /// across jobs it may rewind once (`merged_rewinds`).
    pub(crate) merged: bool,
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
/// first); an empty demand gives no region. H-1's cap (Amendment R41):
/// runs beyond the second fold into it, a fallback region that keeps only
/// the lookahead past its last required time, so its reader decodes
/// forward within a job and may rewind once per job across jobs. Returns
/// the regions and how many runs folded.
fn regions(required: &[i64], lookahead: &[i64]) -> (Vec<Region>, u64) {
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
    let mut folds = 0;
    if runs.len() > READERS_PER_SOURCE {
        let rest = runs.split_off(READERS_PER_SOURCE);
        folds = u64::try_from(rest.len()).unwrap_or(u64::MAX);
        let second = &mut runs[READERS_PER_SOURCE - 1];
        for run in rest {
            second.required.extend(run.required);
            second.lookahead.extend(run.lookahead);
        }
        second.required.sort_unstable();
        let last = second.required.last().copied().unwrap_or(i64::MIN);
        second.lookahead.retain(|t| *t > last);
        second.lookahead.sort_unstable();
        second.merged = true;
    }
    (runs, folds)
}

/// What [`plan_regions`] planned: the regions, and the fallbacks taken to
/// fit them (each a merged region, Amendment R41).
pub(crate) struct Planned<K> {
    pub(crate) regions: Vec<(K, Region)>,
    /// R38: required regions merged to fit the reader limit R.
    pub(crate) merged: u64,
    /// Amendment R41: runs folded past H-1's two readers per source.
    pub(crate) folded: u64,
}

/// H-1/H-5: the regions a job's demand needs, within `limit` readers, and
/// the merges and H-1 folds fitting them took. Over the limit (R38, review B F3),
/// lookahead-only regions go first; only then, as a last resort, are two of
/// one source's required regions merged, the nearest pair first, keeping
/// only the lookahead past the merged region's last required time, so its
/// reader decodes forward only within the job. Across jobs its playheads
/// advance, so it may rewind once per job (Amendment R41: counted in
/// `merged_rewinds`). `None` means more distinct required sources than
/// readers: the frame renders synchronously (K-3).
pub(crate) fn plan_regions<K: Clone>(
    demand: &[(K, Vec<i64>, Vec<i64>)],
    limit: usize,
) -> Option<Planned<K>> {
    let needed = demand
        .iter()
        .filter(|(_, required, _)| !required.is_empty());
    if needed.count() > limit {
        return None;
    }
    let mut folded = 0u64;
    let mut per_source: Vec<(K, Vec<Region>)> = Vec::with_capacity(demand.len());
    for (key, required, lookahead) in demand {
        let (runs, folds) = regions(required, lookahead);
        folded += folds;
        per_source.push((key.clone(), runs));
    }
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
        earlier.merged = true;
        merged += 1;
    }
    let mut all: Vec<(K, Region)> = (per_source.into_iter())
        .flat_map(|(key, runs)| runs.into_iter().map(move |run| (key.clone(), run)))
        .collect();
    all.sort_by_key(|(_, run)| run.required.is_empty());
    Some(Planned {
        regions: all,
        merged,
        folded,
    })
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
    /// Amendment R41: runs folded past H-1's two readers per source.
    pub(crate) regions_folded: u64,
    /// Amendment R41: decodes by a merged region's reader at or before the
    /// time it decoded last (a backward seek).
    pub(crate) merged_rewinds: u64,
    starved_keys: HashSet<K>,
    /// K-5 (review B F4): per source, its last required times and the
    /// direction its demand travels, for eviction (bounded, U-1).
    travel: SourceMemory<K, (Vec<i64>, Travel)>,
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
            regions_folded: 0,
            merged_rewinds: 0,
            starved_keys: HashSet::new(),
            travel: SourceMemory::default(),
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
                .get_or_insert_with(key, || (Vec::new(), Travel::Forward));
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
            slot.floor = None;
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

    /// R38 / R41: [`Self::post`] a planned job, counting its merges and
    /// H-1 folds (the preview drains them at Ready).
    pub(crate) fn post_planned(
        &mut self,
        planned: Planned<K>,
        plan: (HashMap<K, usize>, usize),
        now: Duration,
    ) -> Posted<K, F> {
        self.regions_merged += planned.merged;
        self.regions_folded += planned.folded;
        self.post(planned.regions, plan, now)
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
                last: None,
                floor: None,
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

    /// Amendment R43: whether a reader other than `id` requires `at` of
    /// `id`'s source (it waits for `id`'s result: wake it).
    pub(crate) fn awaited(&self, id: u64, at: i64) -> bool {
        let Some(key) = self
            .slots
            .iter()
            .find(|slot| slot.id == id)
            .map(|slot| &slot.key)
        else {
            return false;
        };
        (self.slots.iter())
            .any(|slot| slot.id != id && &slot.key == key && slot.plan.required.contains(&at))
    }

    /// The next time in `id`'s plan (Amendment R43: in order, R37's
    /// forward walk). Its first required time without a frame or a failure
    /// is decoded once reserved; until then (another reader decodes it, or
    /// it waits for admission again) the reader waits rather than passing
    /// it, so no required time it passed can come back. Once every required
    /// time has a result, the first admissible lookahead past its position
    /// in this plan that nobody has (K-2). `Err(true)`: lookahead waits on
    /// the budget.
    fn work_for(&mut self, id: u64) -> Result<(i64, bool), bool> {
        let Some(slot) = self.slots.iter().find(|slot| slot.id == id) else {
            return Err(false);
        };
        let ring = self.rings.get(&slot.key);
        let resolved = |t: &i64| {
            ring.is_some_and(|ring| ring.contains_key(t))
                || self.failures.contains_key(&(slot.key.clone(), *t))
        };
        let taken = |t: &i64| {
            resolved(t)
                || self.slots.iter().any(|other| {
                    other.id != id
                        && other.key == slot.key
                        && matches!(other.state, ReaderState::Decoding { at, .. } if at == *t)
                })
        };
        let key = slot.key.clone();
        if let Some(at) = slot.plan.required.iter().find(|t| !resolved(t)) {
            let reserved = self.reserved.contains_key(&(key, *at));
            return if reserved {
                Ok((*at, true))
            } else {
                Err(false)
            };
        }
        let past = |t: &&i64| slot.floor.is_none_or(|floor| **t > floor);
        let lookahead = (slot.lookahead_failed != Some(slot.plan.version))
            .then(|| (slot.plan.lookahead.iter()).find(|t| past(t) && !taken(t)))
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
        let rewind = slot.plan.merged && slot.last.is_some_and(|last| at <= last);
        (slot.state, slot.flight) = (ReaderState::Decoding { at, version }, bytes);
        (slot.last, slot.floor) = (Some(at), Some(at));
        self.merged_rewinds += u64::from(rewind);
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
            // Amendment R43: a stopped decode leaves its reader just before
            // it: decoding that time again is no rewind.
            if let ReaderState::Decoding { at, version } = slot.state {
                slot.last = Some(at - 1);
                if version == slot.plan.version {
                    slot.floor = Some(at - 1);
                }
            }
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
            let travel = self.travel.peek(key).map_or(Travel::Forward, |(_, t)| *t);
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

    /// Amendment R41 (a source leaves the document; U-1): every reader of a
    /// source `keep` rejects retires now, a queued ticket of theirs is
    /// cancelled, and the source's frames, failures, reservations, waiting
    /// regions and travel go. Returns the retired readers (to stop and wait
    /// for: they close their decoders as they exit) and what to cancel and
    /// drop after unlock.
    pub(crate) fn forget(&mut self, keep: impl Fn(&K) -> bool) -> (Vec<u64>, Posted<K, F>) {
        let (mut retired, mut cancel) = (Vec::new(), Vec::new());
        for slot in self.slots.iter_mut().filter(|slot| !keep(&slot.key)) {
            if slot.state == ReaderState::PermitWait && !slot.cancelled {
                slot.cancelled = true;
                cancel.push(slot.id);
            }
            slot.state = ReaderState::Retiring;
            retired.push(slot.id);
        }
        let gone = self.reserved.extract_if(|(key, _), _| !keep(key));
        let released: usize = gone.map(|(_, bytes)| bytes).sum();
        self.release(released);
        let mut frames = Vec::new();
        self.rings.retain(|key, ring| {
            let kept = keep(key);
            if !kept {
                frames.extend(std::mem::take(ring).into_values());
            }
            kept
        });
        let errors = (self.failures.extract_if(|(key, _), _| !keep(key)))
            .map(|(_, (_, error))| error)
            .collect();
        self.required.retain(|(key, _)| keep(key));
        self.wanted.retain(|key, _| keep(key));
        self.sizes.retain(|key, _| keep(key));
        self.pending.retain(|(key, _)| keep(key));
        self.starved_keys.retain(|key| keep(key));
        self.travel.retain(|key| keep(key));
        let posted = Posted {
            spawn: Vec::new(),
            cancel,
            retired: !retired.is_empty(),
            dropped: (frames, errors),
        };
        (retired, posted)
    }

    /// Whether the current plan wants any of `key`'s frames.
    pub(crate) fn planned(&self, key: &K) -> bool {
        self.wanted.contains_key(key)
    }

    /// U-1's witness: the sources whose travel is remembered.
    #[cfg(test)]
    pub(crate) fn travel_keys(&self) -> Vec<K> {
        self.travel.keys().cloned().collect()
    }

    /// U-1's witness: remember at most `cap` sources' travel.
    #[cfg(test)]
    pub(crate) fn remember(&mut self, cap: usize) {
        self.travel = SourceMemory::with_cap(cap);
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
    /// so no reader stays owed a cleared frame. Amendment R41: every reader
    /// of it then retires (closing its decoder; returned, to stop and wait
    /// for) and the travel of sources no plan wants is forgotten (U-1).
    pub(crate) fn clear_cache(&mut self, active: bool, now: Duration) -> (Vec<u64>, Posted<K, F>) {
        if !active {
            let mut posted = self.post(Vec::new(), (HashMap::new(), 0), now);
            let (retired, forgot) = self.forget(|_| false);
            posted.cancel.extend(forgot.cancel);
            posted.retired |= forgot.retired;
            posted.dropped.0.extend(forgot.dropped.0);
            posted.dropped.1.extend(forgot.dropped.1);
            return (retired, posted);
        }
        let required = &self.required;
        let mut frames = Vec::new();
        self.rings.retain(|key, ring| {
            let gone = ring.extract_if(.., |t, _| !required.contains(&(key.clone(), *t)));
            frames.extend(gone.map(|(_, frame)| frame));
            !ring.is_empty()
        });
        let wanted = &self.wanted;
        self.travel.retain(|key| wanted.contains_key(key));
        let posted = Posted {
            spawn: Vec::new(),
            cancel: Vec::new(),
            retired: false,
            dropped: (frames, Vec::new()),
        };
        (Vec::new(), posted)
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
    /// Amendment R43 (re-review R41 blocker 4): the readers started and not
    /// yet exited. A cancel for any other reader is dropped, so a cancel
    /// decided before its reader exited leaves nothing behind.
    live: BTreeSet<u64>,
    /// Cancelled requests of live readers, kept until their reader polls (a
    /// cancel can precede the ticket) or exits; at most
    /// [`CANCELLED_CAP`].
    cancelled: BTreeSet<u64>,
    pub(crate) shutdown: bool,
    /// Polls per reader (review A F3's witness).
    #[cfg(test)]
    pub(crate) polls: BTreeMap<u64, usize>,
    /// Amendment R43's witness: exits (`forget`) per reader.
    #[cfg(test)]
    pub(crate) forgets: BTreeMap<u64, usize>,
}

/// Amendment R43: at most this many cancelled requests are kept. Only live
/// readers' are (≤ R at a time), so the cap is a backstop: past it the
/// lowest id's is dropped, and that reader, if it is granted, retires at its
/// next step (its plan is obsolete) instead.
pub(crate) const CANCELLED_CAP: usize = 64;

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
            live: BTreeSet::new(),
            cancelled: BTreeSet::new(),
            shutdown: false,
            #[cfg(test)]
            polls: BTreeMap::new(),
            #[cfg(test)]
            forgets: BTreeMap::new(),
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

    /// Amendment R43: the reader `id` starts (before its thread runs, so
    /// before any cancel of its ticket can be decided).
    pub(crate) fn join(&mut self, id: u64) {
        self.live.insert(id);
    }

    /// Cancel `id`'s request: a no-op once it exited (Amendment R43).
    pub(crate) fn cancel(&mut self, id: u64) {
        if !self.live.contains(&id) {
            return;
        }
        if self.cancelled.len() >= CANCELLED_CAP && !self.cancelled.contains(&id) {
            self.cancelled.pop_first();
        }
        self.cancelled.insert(id);
    }

    pub(crate) fn release(&mut self, id: u64) {
        self.held.remove(&id);
    }

    /// The reader `id` exited: nothing of it stays.
    pub(crate) fn forget(&mut self, id: u64) {
        #[cfg(test)]
        {
            *self.forgets.entry(id).or_default() += 1;
        }
        self.live.remove(&id);
        self.held.remove(&id);
        self.cancelled.remove(&id);
        self.tickets.retain(|(ticket, _)| *ticket != id);
    }
}

fn plan(version: u64, mut region: Region) -> Plan {
    // Amendment R43: a reader walks its plan in order.
    region.required.sort_unstable();
    region.lookahead.sort_unstable();
    Plan {
        version,
        required: region.required,
        lookahead: region.lookahead,
        merged: region.merged,
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
        /// Amendment R43: (reader, plan version) of every decode started.
        started: BTreeSet<(u64, u64)>,
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
                started: BTreeSet::new(),
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
                            self.book.join(id);
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
            let position = slot.and_then(|slot| slot.last);
            let before = (self.readers.draining, self.readers.live);
            match self.readers.next(id, self.now, self.shutdown) {
                Next::Decode { at, version, bytes } => {
                    // Amendment R43: after its first decode in a plan
                    // version a reader only moves past its position: at
                    // most one rewind per job, that first decode.
                    if !self.started.insert((id, version)) {
                        assert!(
                            position.is_none_or(|last| at > last),
                            "R43: reader {id} rewound to {at} from {position:?} within v{version}"
                        );
                    }
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
            let regions = plan_regions(demand, self.readers.limit()).map(|planned| planned.regions);
            let over = !self.readers.fits(required.len() * F + generated);
            let fallback = regions.is_none() || over;
            let sizes = if fallback {
                HashMap::new()
            } else {
                demand.iter().map(|(k, ..)| (*k, F)).collect()
            };
            let regions = regions.filter(|_| !over).unwrap_or_default();
            let posted = self.readers.post(regions, (sizes, generated), self.now);
            for (id, _) in &posted.spawn {
                self.book.join(*id);
            }
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
            // Amendment R43: only live readers' cancels are kept.
            assert!(
                self.book.cancelled.is_subset(&self.book.live),
                "a cancel outlived its reader"
            );
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
        // K-2: readers whose required frames are decoded wait on the
        // budget for their lookahead (Amendment R43: a reader walks its
        // required times before any lookahead).
        let mut world = World::new(20);
        world.apply(Event::Post(1));
        world.apply(Event::Admit);
        world.settle();
        let states: Vec<_> = world.live.iter().map(|id| world.state_of(*id)).collect();
        assert!(states.contains(&"budget-wait"), "{states:?}");
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
                ..Region::default()
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
        let regions = plan_regions(&demand, readers.limit())
            .expect("a reader")
            .regions;
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
            .regions;
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
                    ..Region::default()
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
            let regions = plan_regions(demand, readers.limit())
                .expect("readers")
                .regions;
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
            let runs = regions(required, lookahead).0.into_iter();
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
            let regions = plan_regions(&demand, readers.limit())
                .expect("readers")
                .regions;
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
        let planned = plan_regions(&demand, readers.limit()).expect("two readers");
        let (regions, merged) = (planned.regions, planned.merged);
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
        let merged = plan_regions(&demand, 3).expect("three readers").merged;
        assert_eq!(merged, 0, "no merge within the limit");
    }

    /// One job's demand: per source, its required and lookahead times.
    type Demand = Vec<(u8, Vec<i64>, Vec<i64>)>;

    /// What [`drive_jobs`] saw, per job: rewinds within the job (any
    /// reader), rewinds across jobs (a job's first decode at or before the
    /// reader's previous one) by merged readers and by the rest, and the
    /// merged regions; with the merges and H-1 folds counted.
    #[derive(Debug, Default)]
    struct Rewinds {
        within: Vec<usize>,
        merged: Vec<usize>,
        unmerged: Vec<usize>,
        groups: Vec<usize>,
        merges: u64,
        folds: u64,
    }

    /// Post `jobs` demands in turn on `readers` (f = F each, admitted in
    /// full) and run each to completion. A rewind is a decode at or before
    /// the reader's previous decode.
    fn drive_jobs(readers: &mut Model, jobs: &[Demand]) -> Rewinds {
        let mut seen = Rewinds::default();
        let mut decoded: BTreeMap<u64, Vec<i64>> = BTreeMap::new();
        for demand in jobs {
            let planned = plan_regions(demand, readers.limit()).expect("readers");
            let regions = planned.regions;
            (seen.merges, seen.folds) = (seen.merges + planned.merged, seen.folds + planned.folded);
            let groups = regions.iter().filter(|(_, run)| run.merged).count();
            let sizes = demand.iter().map(|(key, ..)| (*key, F)).collect();
            let posted = readers.post(regions, (sizes, 0), Duration::ZERO);
            readers.release(posted.dropped.0.iter().map(|fr| fr.bytes).sum());
            assert!(matches!(readers.admit(0), Admission::Ready { .. }));
            let before: BTreeMap<u64, usize> =
                decoded.iter().map(|(id, t)| (*id, t.len())).collect();
            drive(readers, &mut decoded);
            let required: Vec<(u8, i64)> = (demand.iter())
                .flat_map(|(key, required, _)| required.iter().map(|t| (*key, *t)))
                .collect();
            assert!(readers.resolve(&required).is_some(), "every job resolves");
            let (mut within, mut by_merged, mut by_rest) = (0, 0, 0);
            for (id, times) in &decoded {
                let start = before.get(id).copied().unwrap_or(0);
                let job = &times[start..];
                within += job.windows(2).filter(|w| w[1] <= w[0]).count();
                let previous = start.checked_sub(1).map(|index| times[index]);
                let across = job
                    .first()
                    .zip(previous)
                    .is_some_and(|(at, last)| at <= &last);
                let slot = readers.slots.iter().find(|slot| slot.id == *id);
                if slot.is_some_and(|slot| slot.plan.merged) {
                    by_merged += usize::from(across);
                } else {
                    by_rest += usize::from(across);
                }
            }
            seen.within.push(within);
            seen.merged.push(by_merged);
            seen.unmerged.push(by_rest);
            seen.groups.push(groups);
        }
        seen
    }

    /// Amendment R41's bound on what [`drive_jobs`] saw: within a job every
    /// reader decodes forward only; across jobs a merged reader rewinds at
    /// most once per job (and does: its playheads advance), no other reader
    /// rewinds, and `counted` is every rewind.
    fn bounded(seen: &Rewinds, counted: u64) {
        let within = seen.within.iter().sum::<usize>();
        assert_eq!(
            within, 0,
            "within a job every reader decodes forward: {seen:?}"
        );
        for (job, (rewinds, groups)) in seen.merged.iter().zip(&seen.groups).enumerate() {
            assert!(
                rewinds <= groups,
                "job {job}: ≤ 1 rewind per merged group: {seen:?}"
            );
        }
        let others = seen.unmerged.iter().sum::<usize>();
        assert_eq!(others, 0, "only merged readers rewind: {seen:?}");
        let rewinds: usize = seen.merged.iter().sum();
        assert!(
            rewinds > 0,
            "advancing playheads on one reader rewind: {seen:?}"
        );
        let counted = usize::try_from(counted).expect("count");
        assert_eq!(counted, rewinds, "every rewind is counted");
    }

    /// Amendment R41 (re-review BF3-2): the reviewer's two advancing
    /// playheads over 24 jobs. A requires t and t + 14 (lookahead t + 1,
    /// t + 2, t + 15, t + 16) and B requires t + 7. At P = 2 A's regions
    /// merge every job: within a job every reader decodes forward only;
    /// across jobs A's reader rewinds at most once per job, every rewind is
    /// counted, and no other reader rewinds. At P = 3 the readers suffice:
    /// nothing merges and no reader ever rewinds.
    #[test]
    fn a_merged_reader_rewinds_at_most_once_per_job() {
        let jobs: Vec<_> = (0..24)
            .map(|t| {
                let lookahead = vec![t + 1, t + 2, t + 15, t + 16];
                vec![(0u8, vec![t, t + 14], lookahead), (1, vec![t + 7], vec![])]
            })
            .collect();
        let mut readers = Model::new(2).with_budget(1_000 * F);
        let seen = drive_jobs(&mut readers, &jobs);
        assert_eq!(
            (seen.merges, seen.folds),
            (24, 0),
            "A merges every job: {seen:?}"
        );
        bounded(&seen, readers.merged_rewinds);
        let mut readers = Model::new(3).with_budget(1_000 * F);
        let seen = drive_jobs(&mut readers, &jobs);
        assert_eq!(seen.merges, 0, "within the limit nothing merges");
        let rewinds = (seen.within.iter())
            .chain(&seen.merged)
            .chain(&seen.unmerged)
            .sum::<usize>();
        assert_eq!(rewinds, 0, "readers suffice: no rewind: {seen:?}");
        assert_eq!(readers.merged_rewinds, 0);
    }

    /// Amendment R41 (re-review note 1): H-1's cap. Three playheads of one
    /// source 14 apart, each with 2 frames of lookahead, fold the third
    /// region into the second, which keeps only the lookahead past 28 (the
    /// old fold read 14 → 28 → 15) and counts one fold. Over 24 advancing
    /// jobs at P = 8 (no reader-limit merge) the fold keeps the same bound.
    #[test]
    fn the_h1_fold_reads_forward_within_a_job() {
        let (runs, folds) = regions(&[0, 14, 28], &[1, 2, 15, 16, 29, 30]);
        let shapes: Vec<(Vec<i64>, Vec<i64>, bool)> = (runs.into_iter())
            .map(|run| (run.required, run.lookahead, run.merged))
            .collect();
        let expected = [
            (vec![0], vec![1, 2], false),
            (vec![14, 28], vec![29, 30], true),
        ];
        assert_eq!(
            shapes, expected,
            "the fold keeps only the lookahead past 28"
        );
        assert_eq!(folds, 1, "the fold is counted");
        let jobs: Vec<Demand> = (0..24)
            .map(|t| {
                let lookahead = vec![t + 1, t + 2, t + 15, t + 16, t + 29, t + 30];
                vec![(0u8, vec![t, t + 14, t + 28], lookahead)]
            })
            .collect();
        let mut readers = Model::new(8).with_budget(1_000 * F);
        let seen = drive_jobs(&mut readers, &jobs);
        let counted = (seen.merges, seen.folds);
        assert_eq!(
            counted,
            (0, 24),
            "each job's fold is counted, no merge: {seen:?}"
        );
        bounded(&seen, readers.merged_rewinds);
    }

    /// Step reader `id` once as its thread would, granting in full;
    /// returns what it was told.
    fn step_reader(readers: &mut Model, id: u64) -> Next {
        let next = readers.next(id, Duration::ZERO, false);
        match next {
            Next::Open { want } => readers.granted(id, Some(want), Duration::ZERO),
            Next::Close => readers.closed(id),
            Next::Retire => readers.exited(id),
            Next::Decode { .. } | Next::Wait { .. } => {}
        }
        next
    }

    /// Deliver reader `id`'s decode of `at` as a frame.
    fn deliver_ok(readers: &mut Model, id: u64, key: u8, (at, version, bytes): (i64, u64, usize)) {
        let value = frame(key, at);
        let fr = Fr {
            value,
            bytes,
            pinned: false,
        };
        let (back, _) = readers.deliver(id, at, version, Ok(fr), Duration::ZERO);
        readers.release(back.map_or(0, |back| back.bytes));
    }

    /// Step `id` until it waits, delivering each decode as a frame; the
    /// times it decoded.
    fn walk(readers: &mut Model, id: u64, key: u8) -> Vec<i64> {
        let mut times = Vec::new();
        for _ in 0..BUDGET {
            match step_reader(readers, id) {
                Next::Decode { at, version, bytes } => {
                    times.push(at);
                    deliver_ok(readers, id, key, (at, version, bytes));
                }
                Next::Wait { .. } | Next::Retire => return times,
                Next::Open { .. } | Next::Close => {}
            }
        }
        panic!("reader {id} did not settle within {BUDGET} steps");
    }

    /// Amendment R43 (re-review R41 BF3-2, Astra's counterexample): A's
    /// readers are at 4 and decoding 14 when a replan, posted mid-job,
    /// merges A to required 0 and 14 (lookahead 15, 16) beside B's 7 at
    /// P = 2. The merged reader rewinds 4 → 0 once. It does not pass 14,
    /// which the other reader's older decode holds: it waits. That decode
    /// fails (stale, dropped), 14 is admitted again, and the merged reader
    /// continues forward: 0, 14, 15, 16, one rewind counted. (Before R43 it
    /// read 0, 15, 16, then 14: a second rewind in one job.)
    #[test]
    fn a_mid_job_replan_with_a_failing_decode_rewinds_once() {
        let mut readers = Model::new(2).with_budget(1_000 * F);
        let first = plan_regions(&[(0u8, vec![4, 14], vec![])], readers.limit());
        let first = first.expect("two readers").regions;
        readers.post(first, (HashMap::from([(0, F)]), 0), Duration::ZERO);
        assert!(matches!(readers.admit(0), Admission::Ready { .. }));
        let (near, far) = (readers.slots[0].id, readers.slots[1].id);
        assert_eq!(walk(&mut readers, near, 0), [4], "the near reader is at 4");
        assert!(matches!(step_reader(&mut readers, far), Next::Open { .. }));
        let Next::Decode {
            at: 14,
            version,
            bytes,
        } = step_reader(&mut readers, far)
        else {
            panic!("the far reader decodes 14");
        };
        // The replan arrives while 14 is in flight.
        let demand = [(0u8, vec![0, 14], vec![15, 16]), (1, vec![7], vec![])];
        let planned = plan_regions(&demand, readers.limit()).expect("two readers");
        assert_eq!(planned.merged, 1, "A's playheads merge");
        let sizes = HashMap::from([(0, F), (1, F)]);
        let posted = readers.post_planned(planned, (sizes, 0), Duration::ZERO);
        readers.release(posted.dropped.0.iter().map(|fr| fr.bytes).sum());
        assert!(matches!(readers.admit(0), Admission::Ready { .. }));
        let merged = readers.slots.iter().find(|slot| slot.id == near);
        let plan = merged
            .map(|slot| slot.plan.clone())
            .expect("the near reader");
        assert!(plan.merged, "the near reader takes the merged region");
        assert_eq!((plan.required, plan.lookahead), (vec![0, 14], vec![15, 16]));
        let mut decoded = walk(&mut readers, near, 0);
        assert_eq!(decoded, [0], "it rewinds to 0, then waits at 14");
        // The older decode of 14 fails: dropped, and owed again (H-3).
        let error = MediaError::Backend("decode 0@14: injected".into());
        let (_, dropped) = readers.deliver(far, 14, version, Err(error), Duration::ZERO);
        assert!(dropped.is_some(), "a stale failure is dropped");
        readers.release(bytes);
        assert!(readers.unreserved(), "14 is owed again");
        assert!(matches!(readers.admit(0), Admission::Ready { .. }));
        decoded.extend(walk(&mut readers, near, 0));
        assert_eq!(decoded, [0, 14, 15, 16], "forward after its one rewind");
        assert_eq!(readers.merged_rewinds, 1, "one rewind, counted");
        // The far reader leaves; B's reader starts and the job resolves.
        assert_eq!(step_reader(&mut readers, far), Next::Retire);
        let spawned = readers.assign(Duration::ZERO).spawn;
        assert_eq!(spawned.len(), 1, "B's reader starts");
        assert_eq!(walk(&mut readers, spawned[0].0, 1), [7]);
        assert!(readers.resolve(&[(0, 0), (0, 14), (1, 7)]).is_some());
    }

    /// Amendment R43: lookahead evicted behind a reader within a job (the
    /// preview admits again for G owed, drains, and K-5 evicts the farthest)
    /// is not read again in that job: the reader continues past its
    /// position.
    #[test]
    fn evicted_lookahead_behind_a_reader_is_not_read_again_within_the_job() {
        let mut readers = Model::new(20).with_budget(10 * F);
        let demand = [(0u8, vec![0], vec![1, 2, 3])];
        let regions = plan_regions(&demand, readers.limit()).expect("a reader");
        let sizes = HashMap::from([(0, F)]);
        readers.post(regions.regions, (sizes, 0), Duration::ZERO);
        assert!(matches!(readers.admit(0), Admission::Ready { .. }));
        let id = readers.slots[0].id;
        let mut decoded = Vec::new();
        while decoded.len() < 3 {
            if let Next::Decode { at, version, bytes } = step_reader(&mut readers, id) {
                decoded.push(at);
                deliver_ok(&mut readers, id, 0, (at, version, bytes));
            }
        }
        assert_eq!(decoded, [0, 1, 2]);
        // 8f of G owed again do not fit beside 3f: 1f of lookahead goes.
        let Admission::Wait { evicted, .. } = readers.admit(8 * F) else {
            panic!("admission drains");
        };
        let values: Vec<u32> = evicted.iter().map(|fr| fr.value).collect();
        assert_eq!(values, [frame(0, 2)], "the farthest lookahead is evicted");
        readers.release(evicted.iter().map(|fr| fr.bytes).sum());
        assert!(matches!(readers.admit(0), Admission::Ready { .. }));
        decoded.extend(walk(&mut readers, id, 0));
        assert_eq!(decoded, [0, 1, 2, 3], "2 is behind it: not read again");
    }

    /// Amendment R43: a lookahead decode K-2 stops leaves its reader just
    /// before that time. Once admission is ready again the merged reader
    /// decodes it again, and that is no rewind.
    #[test]
    fn a_stopped_lookahead_decode_is_decoded_again_without_a_rewind() {
        let mut readers = Model::new(2).with_budget(10 * F);
        let demand = [(0u8, vec![0, 14], vec![15, 16]), (1, vec![7], vec![])];
        let planned = plan_regions(&demand, readers.limit()).expect("two readers");
        let sizes = HashMap::from([(0, F), (1, F)]);
        readers.post_planned(planned, (sizes, 0), Duration::ZERO);
        assert!(matches!(readers.admit(0), Admission::Ready { .. }));
        let merged = readers.slots.iter().find(|slot| slot.plan.merged);
        let id = merged.expect("A's merged reader").id;
        let mut decoded = Vec::new();
        loop {
            match step_reader(&mut readers, id) {
                Next::Decode { at: 15, .. } => {
                    decoded.push(15);
                    break;
                }
                Next::Decode { at, version, bytes } => {
                    decoded.push(at);
                    deliver_ok(&mut readers, id, 0, (at, version, bytes));
                }
                Next::Open { .. } => {}
                next => panic!("the merged reader walks to 15: {next:?}"),
            }
        }
        // 7f of G do not fit beside 4f live: its lookahead decode stops.
        let Admission::Wait { stop, .. } = readers.admit(7 * F) else {
            panic!("admission drains");
        };
        assert_eq!(stop, [id], "the lookahead decode stops");
        readers.stopped(id, Duration::ZERO);
        readers.release(F);
        assert!(matches!(readers.admit(0), Admission::Ready { .. }));
        decoded.extend(walk(&mut readers, id, 0));
        assert_eq!(decoded, [0, 14, 15, 15, 16], "15 is decoded again");
        assert_eq!(readers.merged_rewinds, 0, "and that is no rewind");
    }

    /// Amendment R43 (re-review R41 blocker 4): a reader is granted, then
    /// retires and exits (`forget`) before the preview applies the cancel
    /// it decided while the reader still waited: nothing stays. A cancel
    /// that precedes its reader's ticket still cancels it. Repeated, the
    /// book stays empty; cancels of live readers stop at the cap.
    #[test]
    fn a_cancel_after_its_reader_exited_leaves_nothing() {
        let mut book = PermitBook::new(4);
        book.join(1);
        book.enqueue(1, 2);
        assert_eq!(book.poll(1), Poll::Granted(2));
        book.forget(1);
        book.cancel(1);
        assert!(
            book.cancelled.is_empty(),
            "a tombstone of a departed reader"
        );
        book.join(2);
        book.cancel(2);
        book.enqueue(2, 1);
        assert_eq!(book.poll(2), Poll::Cancelled, "a cancel before the ticket");
        book.forget(2);
        for id in 3..1_000 {
            book.join(id);
            book.enqueue(id, 1);
            assert_eq!(book.poll(id), Poll::Granted(1));
            book.forget(id);
            book.cancel(id);
        }
        assert!(book.cancelled.is_empty() && book.live.is_empty());
        assert_eq!(book.in_use(), 0);
        let live = u64::try_from(CANCELLED_CAP).expect("cap") + 10;
        for id in 1_000..1_000 + live {
            book.join(id);
            book.cancel(id);
        }
        assert_eq!(book.cancelled.len(), CANCELLED_CAP, "capped");
        assert_eq!(
            book.cancelled.first(),
            Some(&1_010),
            "the lowest went first"
        );
    }

    /// Amendment R43: replans posted mid-job at random, between reader
    /// steps, decodes that succeed or fail (stale ones included), stops and
    /// admissions, at P = 2 (A's playheads merge every job) and P = 3.
    /// Every decode is checked in [`World::step`]: after its first decode
    /// in a plan version, a reader only moves past its position.
    #[test]
    fn mid_job_replans_rewind_at_most_once_per_job() {
        let (mut merged, mut stale_failures, mut rewinds, mut decodes) = (0, 0, 0, 0);
        for seed in 1..=300u64 {
            let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let mut below = |bound: usize| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                usize::try_from(state % bound as u64).expect("index")
            };
            let mut world = World::new([2, 3][below(2)]);
            let mut t = 0;
            for _ in 0..64 {
                let posts = world.render.is_none() && (world.job.is_none() || below(5) == 0);
                if posts {
                    t += i64::try_from(below(3)).expect("small");
                    let lookahead = vec![t + 1, t + 2, t + 15, t + 16];
                    let demand = [(0u8, vec![t, t + 14], lookahead), (1, vec![t + 7], vec![])];
                    world.post(&demand, 0);
                    merged += (world.readers.slots.iter())
                        .filter(|slot| slot.plan.merged)
                        .count();
                } else {
                    let events: Vec<Event> = (world.events().into_iter())
                        .filter(|event| !matches!(event, Event::Post(_) | Event::Shutdown))
                        .collect();
                    let label = world.apply(events[below(events.len())]);
                    stale_failures += usize::from(label.starts_with("finish-err×stale-required"));
                }
                world.check();
            }
            world.check_live();
            rewinds += world.readers.merged_rewinds;
            decodes += world.decodes.len();
        }
        let seen = format!(
            "{decodes} decodes, {merged} merged regions, {stale_failures} stale required \
             failures, {rewinds} merged rewinds"
        );
        eprintln!("{seen}");
        assert!(decodes > 500 && merged > 100, "{seen}");
        assert!(stale_failures > 10 && rewinds > 10, "{seen}");
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
