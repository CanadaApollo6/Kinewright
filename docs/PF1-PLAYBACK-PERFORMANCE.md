# PF1 — Playback performance

> Status: **promoted 2026-09-27** (revision 4 plus the rev-4 closure edits,
> after an Astra critic and three closure checks). Recipe v2. Binding: MO2 ME13–ME16,
> N19–N26, P3 (R1–R9), P5 (R11–R14), P7 (R15–R18; R15 supersedes R10), P8 (R19
> amends R16), the roadmap motion section, AW0 A1, `docs/PERFORMANCE.md`.
> Pinned: eframe/egui/egui-wgpu 0.35.0, wgpu 29.0.4, FFmpeg n8.0. Non-normative
> material (diagnosis, T1–T9, commands, provenance, checklist, rev-2/rev-3
> mappings, cache arithmetic): `docs/PF1-EVIDENCE.md`, `scripts/pf1-probe-replay.sh`.
> Symbols, not `file:line`; every rule has an ID and an owning stage (§13).

## Revision 4 changelog

| Finding (rev-3 closure edit) | Where it is resolved |
|---|---|
| R15; RB3 (edit 3); streaming deadlock | §5: the scheduler serves the preview raster only; K-3 synchronous fallback for sets > C or > R sources, counted, G17; streaming, banding and lazy `decoded_layers` deleted and deferred (D11); K-2 atomic admission kept, with a drain flag and headroom sized from actual frame bytes; K-6 proofs/export/thumbnails keep today's renderer |
| R16 (as amended by R19); RB2 (edit 2) | §6 H-5: only readers hold permits (pool = P); synchronous decoders keep min(P, 16), outside the pool, oversubscription stated; acquire at open, release at close; `shrink` only in inactive states; strict FIFO; P = 1/2/3/20; 10 + 10 with transitions; H-1 synchronous decoders closed before the preview parks |
| RB1 (edit 1) | §6 H-2 5 s quiescence deadline for every inactive reader state; H-3 failures matched by plan version, obsolete ones dropped before any stamp is attached |
| RB4 (edit 4) | §7 R-2: ordered drag images kept; superseded-seq errors suppressed; expired playback candidates never newly published; held vs stale images distinguished |
| R17; RB5 (edit 5); slot exhaustion; terminal retirement | §10 G-4 one rebind per epoch, bounded slot wait, retirement repaint; G-4/G-7 terminal "no future paint" handoff in `on_exit`/`Drop` |
| R17; RB8 (edit 7); false paint success; ineffective control | §7 R-5 marker from the egui-wgpu `paint` callback, acked once at a later root epoch; §2 Q-3 and §11 G16 clock-progress gate the freeze control fails |
| R18; RB6 (edit 6) | §8 S-2 indexed recovery anchor verified at the run's seek, valid decoded timestamps, fallback when unknown; reordered-timestamp and edit-list witnesses |
| RS5 (edit 8) | replay: exact anchor counts for every patch, including both `sed` substitutions; phases kept |
| RS7 (edit 9) | §7 R-4: agent jobs render synchronously (R15); bound counts intervening transport attempts; cancellation notifies; arrival suspends a paused FrameWait |
| RS8 (edit 10) | §4 X-5: `8 + R` reader bound from S2b-2 (permits before construction); live counter reports actual holders, synchronous ones included, throughout |
| RS6 (edit 11) | §13: S2b split into S2b-1…S2b-4, each with its own must-pass; budgets re-estimated (≈ 5,560 after closure edits, was 5,830) |
| rev-4 closure edits | (1) R19: header, H-5, I11, G13, G18, §13; (2) H-2/H-5 PermitWait, tickets, close-before-growth, prompt retirement; (3) S-2 shadow-seek anchor; (4) R-2/R-5/I18 finalised display cell; (5) X-5 bound at S2b-2 |

## 0 Goal and scope

**Goal.** Real-time preview playback and interactive seek/scrub on the three
production timelines (§3), without changing rendered bytes (CC1–CC8 S1, MO2
pins) and within stated memory ceilings. Today typical two-source 1080p plays at
1.8 fps on lavapipe and the RTX 3090 (evidence T4). **In scope:** exact SDR
conversion tables (§4), a hard-budget preview cache (§5), a preview thread and
readers under thread permits for the preview raster only (§6), stamped
presentation (§7), seek/scrub (§8), a clock that cannot drift (§9), a GPU SDR
display path without full-frame readback (§10). Proofs, export and thumbnails
keep today's synchronous renderer plus the exact tables (R15). **Out of
scope:** §15; HDR display paths belong to CC8 S3 (§12).

## 1 Diagnosis (summary; table and boundaries in evidence E0/E1)

Conversion costs 36 ms per 720p source frame; two sources thrash the cache;
rendering starves `fill_audio`; CPU monitor encode costs 19–26 ms; seeks always
reseek; the clock advances through underruns. The post-S1 model (≈ 4 ms per
source frame plus decode) is **provisional** until S1 re-measures it.

## 2 Measurement protocols, lanes and pins

**Q-1 [S0] Lanes.** CI-L (Linux, lavapipe) and CI-W (Windows, WARP) run only
deterministic tests (correctness, parity, cancellation, stepped audio, ledger,
counters, models). Timing runs only on LL (local lavapipe) and LH (local RTX
3090), `--ignored` release, and by hand on the Win11 WARP VM (ME14).

**Q-2 [S0] Protocols.** *P-comp* is ME13's compositor protocol, unchanged.
- *P-play:* three runs per workload after a 2 s warm-up, pinned seeds, the
  real-time simulated audio driver (V-5). The denominator is fixed by the
  timeline: duration × fps due frames (1,800 for 60 s; 7,200 for `talk_recut`).
  A run is valid only if the clock reaches the duration and the wall clock
  elapsed ≤ 1.02 × duration + 0.5 s; due frames the clock never passed count as
  dropped. Each run is reported and must pass alone. Metrics: due-frame
  outcomes (R-5), present-interval distribution, max held age, A/V offset at
  ack, max clock stall (longest span, sampled every 5 ms, with `position()`
  unchanged while playing), `underrun_frames`, peak RSS, ledger peak.
- *P-seek:* three runs of 200 random seeks, 200 forward steps (+1…+12), 200
  backward steps, then a 5 s drag of `request_frame` at 30 Hz, pinned seeds.
  Each latency runs from the transport call on the caller thread to the
  consumer's receipt of the first frame whose stamp is that call's or newer
  (harness), or to its ack (app). p95 per run, worst run reported.
- *P-rss:* a fresh process per measurement via `process_memory()`: Linux
  VmRSS/VmHWM and `/proc/self/task`; Windows `GetProcessMemoryInfo` and Toolhelp.

**Q-3 [S0] Controls** (each must fail its metric): *slowdown* (+50 ms per
preview render fails G1); *freeze* (publication stopped 1 s with the clock
running fails G14); *clock freeze* (clock stopped 1 s while playing fails
G16, whatever the validity allowance); *stall* (2 s `fill_audio` stall
registers `underrun_frames` > 0).

**Q-4 [S4] Pins (R5).** ME13 `BASELINES` and `r28_end_to_end_tracked`'s 5%
rule stay untouched and gating. `PF1_PINS` records workload, path, protocol,
machine (CPU, GPU, driver), build (commit, rustc, FFmpeg build) and metrics; a
paced pin never replaces an unpaced render baseline.

## 3 Workloads

**W-0 [S0] Fixtures.** `mo2_perf_fixtures`' builders move to a shared
`pub(crate) perf_fixtures` module (MO2 tests unchanged). Video comes from the
pinned tool with MO2's x264 arguments plus `size`, `-g` and optional
`noise=alls=10:allf=t`; audio uses `test_support::tone`/`wav_f32`, never lavfi
(AU6 11.0.1), and a music track keeps the ring live.

**W-1 [S0] Workloads** (30 fps), plus MO2's `typical_1080p` and
`blend_heavy_1080p` unchanged:

| Name | Document | Sources | Timeline | Mirrors |
|---|---|---|---|---|
| `explainer_16x9` | 1920×1080, 60 s | noisy 1080p GOP 250 presenter (60 s); three GOP 60 cutaways (testsrc2, smptebars, gradients; 8 s) | V1 presenter; V2 cutaway every 6 s for 3 s; PiP at 35%; two callouts, a title card; music | Firestore explainer |
| `reel_9x16` | 1080×1920, 60 s | two noisy 1080×1920 GOP 60; one 1080p scaled to fill | three video tracks, a cut every 1.5 s; four keyframed titles (Scale, OffsetX/Y, Rotation); one Screen blend; one adjustment clip; music | MockingBoard Reel |
| `feed_4x5` | 1080×1350, 60 s | as `reel_9x16` at 1080×1350 | same edit | MockingBoard feed |
| `talk_recut` | 1920×1080, 240 s | noisy 1080p GOP 250, 600 s, AAC tone | 120 spans of 2 s; span *i* starts at Σ jumps, jumps cycling 0.5/2/5 s (extent ≈ 540 s; same-GOP jumps included); one lower third | talk → essay |

**W-2 [S0] Coverage limits (AW0 A1).** *Native SVG code clips* re-rasterise
per frame; with none in the fixtures, their raster and cache gates are owed by
AW2 (D10); static ones hit U-2. *External-render clips* (Remotion first) are
cached, hash-addressed project media played by PF1's ordinary path; a fixture
joins W-1 once AW2 fixes the format (D10). AW2 owns external rendering,
invalidation, pending states and process cleanup; PF1 prescribes no live
producer. The PERFORMANCE heavy-4K, agent and desktop lanes are rerun at S4.

## 4 Exact conversion tables (SDR only)

**X-1 [S1] Input tables keyed by the conversion-determining fields.**
`ConversionKey` = matrix, range, bit depth and transfer: every field that
determines `rgba64_normalization_max`, the `expand_native_range` branch and
`decode_transfer`. `TransferTable` (65,536 u16 → f16-bit entries, 128 KiB) is
filled by running the **existing** per-pixel arithmetic for each code. Alpha uses
one static table (code / 65,535). Error behaviour is kept exactly: the
pre-loop checks ("managed source depth rejected", "…frame is too large", the
RGBA64 length mismatch) stay before any lookup and still fire for empty images;
the per-pixel errors ("…RGB range expansion failed…", "…colour decode failed…")
are descriptor-determined and value-independent, so a table build failure
returns the same text at the first pixel and empty images still raise none.

**X-2 [S1] Parity through the production dispatch (R1).**
`input_tables_match_every_accepted_descriptor` enumerates `ColorPrimaries` ×
`ColorWhitePoint` × `ColorMatrix` × `ColorRange` × `ColorBitDepth` (incl.
`Integer(8..=16)`) × `ColorTransfer` × assumption. That includes a
representative of every rejected class: `Unknown` in each field, and the
unsupported combinations. Each tuple goes through `select_conversion`, which is
`classify_source_with_assumption`, then `rgba64_normalization_max`, then the
`Conversion` choice. `Separable` tuples must be bit-identical to
`from_rgba64_le` for all 65,536 codes, `PerPixel` tuples must route to today's
f32 path, and rejected tuples must return today's error. CC8 S2's widened
acceptance is thereby exercised automatically (§12).

**X-3 [S1] One-pass fill.** `read_plane` + flip + rotate + lookup write the f16
buffer directly; equal to the unfused path for 4 rotations × flip.

**X-4 [S1] SDR only (R2).** Only `Conversion::Separable` reaches tables; the HDR
input adapter and any cross-channel transform select `Conversion::PerPixel`
(today's exact f32 path) by exhaustive `match`. No f16 foreign-space store (CE9).

**X-5 [S1] Table inventory** (computed at runtime; invalidation is by key).

| Table | Size | Instances | Owner, lifetime |
|---|---|---|---|
| Input alpha | 128 KiB | 1 | process `LazyLock` |
| Input RGB | 128 KiB | ≤ 8 in registry + one per live decoder holding an evicted key | registry `Mutex<HashMap<ConversionKey, Arc<_>>>` (leaf), `OnceLock` per key, so one build per key at a time; LRU drops membership only. The live-byte counter (build +, `Drop` −) reports the actual holders in stats from S1, synchronous decoders included throughout. From S2b-2, readers acquire permits before constructing decoders, so reader-held tables ≤ R; synchronous decoders (H-5) add one each |
| Monitor RGB, alpha (BT.709 SDR) | 64 KiB each | 1 | process `LazyLock`; CPU encode (G-1) |
| egui premultiply | 64 KiB | 1 per app | built by the app (G-3, S3a) |
| GPU monitor + premultiply | 192 KiB | 1 per device | `DisplayEncoder` storage buffer, ledger-charged, staged by ME16's atlas pattern, rebuilt on device recreation |

## 5 Preview cache budget and admission (R3, R15)

**K-1 [S2b-3] Accounting.** `PreviewBytes` counts every unique `WorkingFrame`
allocation (`shared_buffer_id`) owned by the scheduler path: source rings,
`title_cache` rasters, reader handoffs, in-flight conversions (by reservation)
and frames pinned by the current render. A `Reservation` is taken before
converting and travels into the frame's drop guard; it releases when the last
`Arc` drops, never under `Sched` (H-4). C = `FRAME_CACHE_BYTE_BUDGET` = 224 MiB
is hard for this path: **I12**, scheduler live bytes ≤ C at all times.
Synchronous renderers (K-6) report their live bytes through the same counter
type, in stats, under today's policy.

**K-2 [S2b-3] Atomic admission (preview path).** A job's required set is its
distinct required source frames at the proxy raster plus generated rasters, of
q bytes not yet resident. (1) If the whole set exceeds C, the frame takes K-3
at once. (2) Otherwise the reservation is atomic under `Sched`: if live + q >
C, the preview sets `draining` (no lookahead is admitted), evicts unpinned
lookahead (victims dropped after unlock) and cancels outstanding lookahead
conversions, whose readers check the cancel flag per row band and release.
(3) Only then does it wait on `ready`, and only for live ownership (pinned
frames, cancelled conversions draining), never on evictable work. `draining`
clears when the reservation is taken.

*Lookahead* for source *s* is admitted while ¬`draining`, live + f ≤ C − H and
*s*'s lookahead < share, where H = n_active · f is one full required set at the
actual frame size, share = (C − G − H) / n_active, and G is the generated
rasters. The ≥ 8-frame target is best-effort: a smaller share shrinks the
horizon, down to just in time, and counts `lookahead_starved`.

**K-3 [S2b-3] Synchronous fallback (R15).** A frame whose required set exceeds
C, or needs more distinct sources than R readers (H-5), is rendered as today by
the preview's synchronous `FrameRenderer` (its own cache, the S1d window), after
the preview drops all unpinned lookahead; the result carries the job's stamp
(R-2). Each counts `sync_fallback_frames{reason}` (Amendment R47 adds a
third reason, a set detained by detached readers); G17 requires it rare on
every W workload (the largest W set, 4 sources + 4 titles, is ≈ 80 MiB at
1024×1280, so expected 0). Bounded streaming, banding and a lazy
`decoded_layers` are deferred (D11).

**K-4 [S2b-3] Arithmetic** (evidence E8): lookahead 13 frames for typical, 8
for `reel_9x16`, 5 for `feed_4x5` and 2 for 8 sources (the last two starved).

**K-5 [S2b-3] Eviction by distance.** Within a ring the victim is the frame
farthest from every demand region on that source (two readers → two regions),
behind-travel first. Across rings, inactive sources go first, then sources over
share. Pinned frames are never evicted. On a job change the preview drops
obsolete lookahead **before** it waits.
*Travel (R29, review B F4; S1d implements it per source cache):* behind is
below the nearest demand point travelling forward, above it travelling
backward. The direction follows the demand point that moved least (the
smallest non-zero step to the nearest previous point, forward on a tie
both in choosing that nearest point and across points: R30),
so a reversal flips it at once and a discontinuous seek is a step in the
jump's direction (what the jump left behind goes first); an unmoved or
cleared demand keeps it; a source's first demand is forward. Capacity
eviction (the 32-entry ring) also skips pinned frames (review B F3).

**K-6 [S1] Proofs, export, thumbnails (R15).** `monitor_proof_for_document`,
export and agent thumbnails keep today's synchronous `FrameRenderer`, cache and
policy, gaining only X-1…X-5 and live-byte reporting. S1d makes the *preview*
renderer's Sequential window clamp(⌊(C − G) / (n·f)⌋, 1, `PREFETCH_FRAMES` +
1), fixing the thrash (G5) before readers exist; that renderer later serves K-3.
Thumbnails use `DecodeStrategy::Seek` (no window). S1 claims no I12.

## 6 Preview thread, readers, permits, shutdown

**H-1 [S2a/S2b-1] Threads.** The *media worker* keeps `Control` handling,
`fill_audio`, the cpal stream (not `Send`), `SharedClock` and transport, and
never renders (S2a). The *preview* (`kinewright-preview`) owns the scheduler
compositor, the display slots and one synchronous `FrameRenderer`, which serves
every preview render in S2a and only agent jobs (R-4) and K-3 from S2b.
*Readers* (`kinewright-decode-{n}`, S2b-1) each own one `VideoDecoder` (≤ R,
≤ 2 per `VideoSourceKey`) and follow the preview's `DecodePlan` (required times,
then lookahead; clips starting inside the horizon are pre-rolled).
*Synchronous decoders* belong to export, proofs and the preview's renderer; the
preview clears its renderer's `video_sources` after a synchronous job unless
another is queued, so none outlives a park (G15).

**Amendment R37 [S2b] Regions (G1/G14 (iv), review B F3).** A reader's plan
is one *region* of its source's demand. Regions merge only across real decoder
continuation: a reader decodes its required times, then its lookahead, each
ascending, and the decoder continues without a seek only at exactly the next
frame (`decode_window_sequential`). So a new region starts at a time more than
one frame after the previous one (`REGION_GAP` = 1, was 16) and at a required
time after lookahead. Two same-source playheads (`typical_1080p`'s layers 14
frames apart) are therefore two readers, each decoding forward with one seek
(its open); the R36 probe measured ≈ 184 seeks per LH run, was ≈ 2,470.
*Residual (H-1's cap; Amendment R41):* a source keeps ≤ 2 readers (H-1), so
every region past the second folds into the second, a fallback region like the
reader limit's merge below. It keeps only the lookahead past its last required
time, so within a job its reader seeks forward between its playheads, and each
fold is counted in `PlaybackStats::regions_folded`. No W workload has three
simultaneous playheads on one source; a same-source cut's pre-roll inside the
horizon folds routinely. *Reader limit (R38, review B F3):* when
all sources' regions exceed the reader limit R, lookahead-only regions go
first. Only as a last resort are two required regions of one source merged,
the nearest pair first (the later region's first required time less the
earlier's last), until the regions fit; a merged region keeps only the
lookahead past its last required time, so within a job its reader decodes
forward only (it seeks forward between the merged playheads). Across jobs it
does not: see Amendment R41. Each such merge is counted in
`PlaybackStats::regions_merged`. More required sources than R remain K-3's
synchronous fallback. *G14 margin:* the probe's held maximum was 91.1 ms
against 100 ms; S4's pinned run is the verdict.

**Amendment R41 (proposed) [S2b] Fallback regions rewind across jobs
(re-review BF3-2).** A *merged* region puts two or more of one source's regions
on one reader: the reader limit's last-resort merge (`regions_merged`) or H-1's
fold (`regions_folded`). Within a job it decodes forward only: its required
times ascending, then only the lookahead past the last of them. Across jobs it
cannot. When the playheads advance, the earlier one's next frame lies behind
the reader, which must seek back to it. At P = 2, with A requiring 0 and 14 and
B requiring 7, A's reader decodes 0, 14, 15 and 16; in the next job it seeks
back to 1, then decodes 17 (15 and 16 are already held). Decoding the gap
forward instead would cost as much as the seek, so no order is cheaper. This is
the degraded mode, and the documented cost of exceeding R or H-1's two readers
per source. A merged region's reader rewinds at most once per job. A *rewind*
is a decode at or before the reader's previous decode. Each rewind is counted
in `PlaybackStats::merged_rewinds`, which the timing lanes print as
`engine_merged_rewinds` beside `engine_regions_merged` and
`engine_regions_folded`. R37's rule, that regions merge only across real
decoder continuation, governs every region that is not such a fallback. With
enough readers, no reader rewinds as its playheads advance.

**Amendment R43 [S2b] A reader walks its plan in order (re-review R41
RS-3).** R41's bound failed when a replan was posted while another reader
was decoding: the merged reader passed the in-flight time, and when that
older decode failed stale and the time was admitted again, it sought back a
second time in one job. Now a reader decodes its *first* required time that
has neither a frame nor a failure, once reserved. Until then (another reader
has it in flight, or it waits for admission again) it waits rather than
passing it, and a delivery another reader waits for wakes the readers.
Lookahead follows only once every required time has a result, and only past
the reader's position in the current plan version (its *floor*, reset at
each post), so a frame evicted behind it is not read again within the job.
A stopped decode leaves the reader just before that time, so decoding it
again is not a rewind. Hence only a reader's first decode in a plan version
can rewind: **at most one scheduled-frame rewind per reader per plan
version**, and a job posts one plan version. A replan posted mid-job is the
next job's version. The bound counts the scheduler's decodes, not FFmpeg's
seeks: a decode retried after a failure or a reopen may seek again, and it
does not bound those (Amendment R47). The exhaustive and seeded models check
this on every decode.

**H-2 [S2b-1] `Sched`: shared state and participant states.** A
`Mutex<SchedState>` with condvars `work` (readers) and `ready` (preview) holds
`shutdown`; the single-slot `transport: Option<Job>` + `version`; `agent:
VecDeque<AgentJob>` (≤ 8); per reader `plan {version, required, lookahead}`,
`state`, `shrink`, `inactive_since`; the ring index; `failures: Map<(SourceKey,
time), Failure{plan_version, error}>`; `PreviewBytes`, reservations and
`draining`; and `preview_failure: Option<(FrameStamp, MediaError)>`.
Predicates are evaluated under the lock (spurious wakeups are harmless); each
participant is in exactly one state. The *inactive* reader states (Idle,
BudgetWait) time out at `inactive_since` + 5 s into Retiring (the quiescence
deadline); PermitWait retires only once its demand is obsolete (H-5), so a
required request is never lost:

| Who | State | Waits on | Leaves when |
|---|---|---|---|
| Reader | Idle | `work`, timed | plan version changes; `shrink` (→ close, reopen); `shutdown`; timeout |
| Reader | Runnable → Decoding | nothing (outside the lock) | frame done (insert per H-3), cancel flag, `shutdown` at a packet boundary |
| Reader | BudgetWait (lookahead only) | `work`, timed | admissible (K-2); plan change; `shrink`; `shutdown`; timeout |
| Reader | PermitWait (holds 0 permits, no decoder) | `Permits` cv | granted at the FIFO head; demand obsolete (ticket removed, notify); `shutdown` |
| Reader | Retiring | nothing | closes its decoder outside all locks, returns permits, exits |
| Preview | Parked | `ready`: untimed if paused, timed to the next due time if playing | new `version`; agent push; `shutdown`; due time |
| Preview | Rendering / Staging / SyncRender | nothing (outside the lock) | done |
| Preview | FrameWait(job) | `ready`: untimed for paused jobs (R49: timed to `RETIRE_DEADLINE` once a drag's newer job waits), timed to the deadline for playback | each required (source, time) has a result or matching failure; superseded (R49 for paused jobs); agent push (paused only, R-4); `shutdown` |
| Preview | Holding(frame) | `ready`, timed to `at` | due; superseded; `shutdown` |
| Preview | ReservationWait | `ready` | a live-owned reservation released (K-2); superseded; `shutdown` |

The worker never waits on `Sched`: it locks only to post a job or read
`preview_failure`, O(1) each.

**H-3 [S2b-1] Versioned results and failures; notifications.** A reader result
carries (`VideoSourceKey`, time, plan version, reservation). Frame bytes are
determined by (key, time), so a result from an older plan version is inserted
pinned if the current job requires it, as lookahead if admissible, and
otherwise dropped after unlock. A failure carries its plan version. It is
recorded only if that version is the reader's current plan version and (key,
time) is currently required; any other failure is dropped (the time is
re-demanded if still needed). An obsolete failure can therefore never acquire a
current job's stamp. A job consumes only failures for its own keys and plan
version, surfaced with its own stamp (R-2). *Notify:* job posted, agent pushed or
cancelled → `ready` (and `work` for jobs); plan changed or `shrink` → `work`;
result or failure → `ready`; eviction, reservation release, cancel drain,
`draining` cleared → both; `shutdown` → `notify_all` on both. Required frames
never wait on speculative work (K-2), so no capacity/frame wait cycle persists.

**H-4 [S2a+] Lock order.** No thread holds two of {`Sched`, `Permits`,
`Coalesced`, `DisplaySlots`, the table registry, the ledger}. Nothing that could
lock is dropped under `Sched`: evicted `Arc`s, reservations, reply `Sender`s
and decoders move into a local `Vec` dropped after unlock, and the drop guard
debug-asserts that a thread-local "holding `Sched`" flag is clear. No lock is
held across FFmpeg calls, wgpu submit/poll/map, channel sends or conversion.

**H-5 [S2b-2] Thread permits (R4, R16, R19).** Only readers hold permits.
- *Synchronous decoders* (export, proofs, thumbnails, K-3) keep today's min(P,
  16) frame threads outside the pool, never revoked or waited on. **I11**:
  reader frame threads ≤ P (`available_parallelism`). Export or a proof during
  playback oversubscribes by min(P, 16) per synchronous decoder: allowed,
  stated, not a deadlock risk, counted as `sync_decoders`; G18 guards export.
- *Readers.* At most R = clamp(P, 1, 8). A reader acquires at open and releases
  at close, its thread count fixed for its decoder's life. It wants w =
  clamp(⌊P / n⌋, 1, 16), n being the readers the current plan needs (the
  preview opens a plan's readers together).
- *FIFO grants.* `Permits` is a monitor with a FIFO of tickets; only the head is
  granted, min(w, free) if ≥ 1, and new requests never bypass waiters. A
  retired or cancelled request removes its ticket and notifies the `Permits`
  cv, so the next head is re-evaluated.
- *Rebalancing only at open, close and idle retirement.* While a ticket waits
  or a reader is short (granted < w), every reader holding more than the
  current w gets `shrink`, acted on only in an inactive state, never while
  Decoding: it closes and releases, then queues a new ticket at w and reopens
  (one seek from its cursor; frames exact). A short reader likewise closes and
  releases at an inactive boundary *before* queuing for growth, so no ticket is
  held with permits. While a ticket waits, inactive readers whose demand is
  obsolete retire at once instead of at the 5 s deadline. Readers go inactive
  whenever their plan is satisfied or budget-blocked, so these land within one
  decode run. No reader waits holding permits and synchronous decoders never
  wait, so there is no deadlock; FIFO prevents starvation.
- *Small P:* P = 1: R = 1, one thread; P = 2: R = 2, 1 + 1; P = 3: R = 3, 1 + 1
  + 1 or 3 alone. A plan needing more than R readers renders by K-3.
- *Arithmetic (P = 20, R = 8).* One source gets 16 (the cap). A second source
  wants w = 10 with 4 free: granted 4, short. Reader 1 gets `shrink`; at its
  next inactive point it closes (16 free), re-queues and is granted 10; reader
  2 closes at its inactive point and is granted 10: 10 + 10 after one reopen
  each. Three sources: 6 + 6 + 6 (2 spare); four: 5 × 4 = 20 (today 64); a
  same-source jump cut: 10 + 10. Export runs beside them at 16 per decoder.

**H-6 [S2a/S2b-1] Shutdown** (the control channel disconnects). (1) The
worker locks `Sched`, sets `shutdown`, moves the agent queue out, notifies all,
unlocks, then replies `Backend("media worker stopped")` to each moved job and
wakes `Permits` waiters. (2) It stops audio, as today. (3) Readers leave any
state at their next check, close decoders, return permits and exit. (4) The
preview replies "media worker stopped" to an active unreplied agent job,
finishes any ME16 deadline-free wait, runs G-7 teardown (no epoch needed, G-4)
and exits. (5) The worker joins readers, then the preview, without timeout; it
stays detached from the app, as today, so the UI never blocks, and a GPU wait
that never completes keeps its thread and charges (ME16's stance). (Amendment
R47: the preview's join of its readers ends at `RETIRE_DEADLINE`, detaching a
reader stuck in IO.)

**H-7 [S2b-4] Idle.** Engine constructed, no document: +0 threads. First render
(`set_document` → `present(0)` → a paused job): +1 preview, plus readers of the
visible sources. Settled: readers retire 5 s after going inactive, closing their
decoders; the preview stays parked (+1 thread) with no synchronous decoder.
**I10** checks this on CI with an injected clock; RSS is a pinned local gate
(G15). AW1 B5 proxy mode still constructs no engine.

**Amendment R41 (proposed) [S2b] A removed source closes its decoders
(Windows CI run 36528931046; re-review U-1).** Idle readers keep their
decoders open until they retire, and on Windows an open decoder holds its file,
so a source moved right after it left the document failed to rename (os error
32). Now the first render of a new generation retires the readers of every
source not in its media pool. It stops them at their next check or packet
boundary, cancels their tickets, and waits on the preview thread, never the
UI's, until each has closed its decoder and exited, before the frame renders. So
once a frame of the new document is delivered, no decoder is open on a removed
source: reader or synchronous (`bind` clears the renderer). A cache clear with
no job waiting retires every reader the same way. Per-source memory between
jobs (each source's measured frame size and its travel) forgets the sources a
new generation removes, and keeps only the plan's sources on a cache clear. It
is capped at 256 sources, forgetting the least recently used; a forgotten size
is measured again, and a forgotten travel restarts forward. (Amendment R43
bounds the wait and narrows the claim to readers and the preview.)

**Amendment R43 [S2b] Retirement is bounded, interruptible and panic-safe
(re-review R41 RS-1, RS-2, RS-4, notes 1 and 2).**
*Interrupt.* Every reader's input carries an FFmpeg `AVIOInterruptCB` that
reads the reader's stop flag, so a retired reader's open (probing included),
seek or packet read ends at the file's next read with `AVERROR_EXIT`. This
covers local-file IO on the reader's own thread (Amendment R47): a protocol
that reads on a helper thread, such as FFmpeg's `async`, is not claimed. The
reader's open checks the flag before the file opens, and after (note 1). The
packet loop reads one packet per turn, checking the flag first.
`ffmpeg-next`'s `packets()` retried an interrupted read forever.
*Deadline.* The preview waits for retired readers for at most
`RETIRE_DEADLINE` = 200 ms, and less on shutdown or when its job is
superseded. The flag and the interrupt cannot end one libavcodec call and
one frame conversion already running. By E0's figures that is tens of
milliseconds: a whole keyframe-to-target seek took 12–41 ms, and the
pre-S1 1080×1920 conversion 81 ms. 200 ms covers that with room, and stays
under R43's 250 ms ceiling for the stall after an edit that removes a
source. Past the deadline, the readers still alive
are *detached*. Later retirements do not wait for them, the frame renders, and
`PlaybackStats::retire_overruns` counts the wait. A detached reader keeps its
slot, its permits and its bytes in flight until it exits (Amendment R47
withdrew R43's return of its permits at the deadline: a reader past 200 ms
may still be decoding or converting).
*Degraded case.* A detached reader closes its decoder when its blocked call
returns. Until then its file stays open, so on Windows it stays locked.
*Exit guard.* A reader's exit is an RAII guard that runs on return and on
unwind. Once the decoder has closed, it forgets the reader's permits (once),
removes its slot and wakes the preview. A reader that panicked also fails its
required times, so no job and no retirement waits on it.
*Tickets.* A cancel of a reader that already exited is a no-op, so the
`Permits` book keeps cancels only for live readers, capped at 64.
*Scope of the file-release claim (note 2).* Readers and the preview's
synchronous renderer close a removed source's files before the new
document's first frame (unless the retirement overran). Other holders keep a
removed file until their own job ends, bounded by that job:
- visual-asset jobs (`analysis.rs`, such as a waveform's audio decode);
- proof renderers (`monitor_proof_for_document` in `engine.rs`).

**Amendment R47 [S2b] Detached readers never stall a frame, and H-5's bound
stays exact (re-review R43 blockers 1–3).**
*K-3 for a detained set.* A detached reader may never exit. A job's
required set is *detained* when it cannot be admitted until detached readers
exit: its required regions need more slots than R leaves beside them (or
more readers of a source than H-1 allows), the detached readers hold the
whole permit pool, or the set does not fit in C beside their bytes in flight.
Detached readers can only exit while the job waits, which frees, so the
preview decides once, before it posts the plan: a detained frame renders by
K-3's synchronous fallback, counted `sync_fallback_frames` and
`detained_fallback_frames` (the timing lanes print
`engine_detained_fallback_frames`). Each later job decides again, so readers
serve again once the detached readers exit.
*H-5, exactly.* Accounted reader permits never exceed P: a detached reader
keeps its permits until its exit guard forgets them, so a stuck reader
reduces the readers' capacity, and K-3 serves the frames it detains.
Including detached readers, accounted permits and configured reader
codec-thread allocations are bounded by min(R × min(P, 16), P): a detached
reader's allocation is part of P, not added to it. Detached work may remain
busy indefinitely. Synchronous decoders (K-3's included) remain outside this
bound, each adding up to min(P, 16) configured codec threads. This is a bound
on configured allocations, not on the process's total threads.
*Shutdown.* The preview's join of its readers (H-6 (5)) is bounded by the
same `RETIRE_DEADLINE`. A reader still alive then is detached: its thread is
left to exit on its own (it owns a reference to the lane), and
`PlaybackStats::shutdown_detached_readers` counts it (the timing lanes print
`engine_shutdown_detached_readers`).
*Windows-safe witnesses.* A witness whose reader holds a file drops the
source from the document first and deletes the file only after the reader
exits, since Windows cannot delete a file a reader holds open.

**H-8 [S2b-1…4] Scheduler tests.** Each S2b commit extends an exhaustive
(event × state) model of `SchedState` + `Permits`: quiescence timeout in every
inactive state; stale result and stale failure; `shrink` while Idle,
BudgetWait and Decoding (deferred); a short grant; a request behind waiters;
shutdown in every state; agent push, cancel and suspend in a paused FrameWait; a
fallback frame; P ∈ {1, 2, 3, 20} with an idle retained reader, two concurrent
synchronous jobs and a plan growing 1 → 2 → 4 sources. A seeded 10,000-sequence
stress test runs under a watchdog; race and kill tests run on Opus.

## 7 Stamps, jobs and presentation

**R-1 [S2a] Stamps at issue (RB4).** Core adds `FrameStamp { epoch, seq }`.
Every `FfmpegMediaEngine` transport call takes a stamp on the caller's thread:
`seq` increments on every call, `epoch` also for `seek`, `play`, `pause` and
`set_document` (not `request_frame`). Controls carry their stamp. The coalesced
seek/frame atomics become one leaf `Mutex<Coalesced>` of `(target, stamp)`
pairs. The worker posts a coalesced request only after applying every control
whose epoch ≤ the request's (else it waits one loop), so each job binds its
stamp to its target and that epoch's document and LUT; queued work is never
relabelled. Worker-initiated stops (EOS, audio failure) take no stamp.

**R-2 [S2a] Validation at the final consumer.**
- *Channel:* `Playback::frames()` becomes `Receiver<PreviewFrame { at, stamp,
  texture }>`; `Playback::stamp()` is added (default zero); the twelve
  implementors change mechanically.
- *Selection:* `poll_background` only collects candidates. `finalize_preview`,
  the last step of `App::ui` (after every keyboard, timeline and transport call
  in the pass), reads `stamp()` and publishes the newest candidate with the
  current epoch and seq ≥ the shown seq. For playback the candidate must also
  satisfy position() − 1 frame < at ≤ `position()`. An expired candidate is
  never newly published; it counts dropped. `TextureHandle::set` (CPU path) or
  the stable `TextureId` re-point (G-4) happens here, and in the same step the
  shared `DisplayCell { stamp, at, frame_id, stale }` is written to describe
  what is now bound. A deferred rebind (G-4) leaves both unchanged. Layout uses
  the previous size (a one-pass lag on aspect change).
- *Held vs stale:* with no qualifying candidate, an image of the current epoch
  stays up as *held* (its held age grows). An image of an older epoch stays
  visible only as *stale* (`stale` set, visibly marked, P8), so a seek does not
  flash to blank. It is never acked and never counts as held or on time for the
  new epoch; held age and L-* run from the epoch change. `set_document(None)`
  clears it.
- *Drag:* intermediate `request_frame` results of one epoch are exact and show
  in seq order; release calls `seek`, whose epoch bump excludes them.
- *Stamped errors:* a preview-path failure carries its job's stamp. The worker
  calls `fail` only if that epoch is current **and** its seq ≥ the latest
  issued transport seq, and emits the new `MediaEvent::StampedError(FrameStamp,
  MediaError)`, which the app treats as `Error` (stop, incident) under the same
  test. Superseded or old-epoch errors only count `stale_errors`. I8 runs 1,000
  seeded interleavings of drags, seeks, play/pause and edits on both paths.

**R-3 [S2a] Jobs.**

| Kind | Posted on | Exact | Waits | Replaces |
|---|---|---|---|---|
| `Paused(at)` | coalesced seek/frame, `set_document` | yes | untimed | pending `Paused`/`Playback` |
| `Playback` | `play` | each shown frame exact | to deadline | the same |
| `Agent` | `Thumbnail`, `PreviewCacheStats`, `ClearPreviewCache` | yes | never (synchronous) | never (FIFO) |

Playback renders at = clock + lead (EWMA of render time, ≤ 2 frames), holds the
result until `position()` ≥ at (never early), and drops it, counted, once the
clock passes at + 1 frame. A frame missing a required layer at its deadline is
**held**: the previous image stays and its held age grows.

*Amendment R37 (start-up).* A playback version's first target is its start
frame (`from`, or the clock if it has already passed `from`), not clock +
lead; later targets are clock + lead as above. A `Paused` job whose stamp's
epoch precedes the newest issued `play` is superseded by it in issuance order
(R-1's `take_requests_before`, applied at the preview): the preview drops it
untaken; once taken, it posts no plan; and a FrameWait it abandons withdraws
its demand (an empty plan, as K-3's) and stops the readers' decodes at the next
packet boundary, so no reader seeks to or keeps decoding its stale frame. The
`play` records its epoch on the lane as it is issued, before the worker
applies it. The clock (V-1/V-2) and the R34/R35 accounting are unchanged; frame 0
is due, registered and counted like any other.

**R-4 [S2a] Agent lane (RS7).**
- *Push:* the worker resolves `document: None` to its current document, binds
  that document's own LUT and calls `try_push`; a full queue gets an immediate
  `Backend("preview-thread: agent queue full")`. The worker never waits.
- *Execution (R15):* an agent job renders on the preview's synchronous
  `FrameRenderer` with `DecodeStrategy::Seek`, outside all locks, byte-exact
  (C-5); it never enters FrameWait.
- *Fair selection:* after each transport *attempt* (a render published, held or
  dropped at its deadline) the preview runs one queued agent job; with no
  transport job pending, back to back. An agent push during a **paused**
  FrameWait suspends it (demands stay posted, readers keep decoding), runs the
  agent job, then re-evaluates the predicate; a **playback** FrameWait first
  ends at its deadline (≤ lead + 1 frame ≤ 3 frames). *Bound:* the job at queue
  position q starts after ≤ q agent renders + q transport attempts + one
  playback wait. Long-GOP agent seeks interrupt playback; `dropped_agent` is
  recorded.
- *Cancellation:* `thumbnail_*` hold a `CancelOnDrop` guard whose drop sets the
  job's flag under `Sched` and notifies `ready`; the preview discards a
  cancelled job at dequeue, and a running render finishes with its reply
  dropped. No wait predicate involves an agent job.
- *Exactly once:* the reply `Sender` moves with the job and sends once: the
  render result (errors included, never `Worker::fail`, as today), "agent queue
  full", or "media worker stopped" (H-6). `monitor_proof_for_document` keeps its
  caller-thread renderer (K-6).

**R-5 [S2a] Stats and acks (RB8, R17).** `Playback::stats()` (default impl):
due-frame outcomes, `max_held_ms`, `max_av_offset_ms`, `max_clock_stall_ms`,
`underrun_events`/`_frames`, `lookahead_starved`, `sync_fallback_frames`,
`sync_decoders`, `slot_starved`, `stale_errors`, permits in use. *Paint
marker:* over the preview image's rect and clip the app adds an
`egui_wgpu::Callback` holding the `DisplayCell`, never a copied stamp, so
layout (A) followed by a binding (B) or a deferred choice (C) is still marked
as B. Its `paint` (not `prepare`) reads the cell and stores `PaintMark { stamp,
at, frame_id, epoch n }`, unless `stale` is set or the cell's epoch is older
than `stamp()` (a seek issued before paint); both record nothing. In egui-wgpu
0.35, `Renderer::render` calls `paint` only for a non-empty clip and only after
the surface texture is acquired, then finishes and submits with no early
return; an abandoned paint (surface error, invisible viewport, zero clip) never
calls it. `App::logic` at a later root epoch m > n takes the mark and calls
`Playback::ack_presented(stamp, at)` (default no-op) once per `frame_id`, so a
held frame repainted over many epochs is acked once: *painted and submitted*,
not physically presented (unobservable here). Per due frame: *on time* if acked
within [due, due + 1 frame + 1 epoch], *late* if later, *dropped* if never. Held age is the time since the last ack of a newer frame,
sampled every 5 ms (harness) or at each `App::logic` (app).

*Amendment R34 (single-writer accounting; re-review 2 of S2a, D1–D4).*
1. *One writer.* The worker is the only writer of due-frame outcomes. It
   registers due frames only from its own applied transport: the position
   of the samples its audio callback consumed (never the shared clock,
   which a caller's `seek`/`play`/`set_document` and the worker's own
   transitions write). It registers at every tick (a 5 ms receive
   timeout: a nominal period, not a bound, since fills, controls and
   scheduling can delay a tick) and at each of its clock writes. *R35:* a
   transition (the terminal stop, a pause, a new document, a playing seek,
   a `play`) first stops the outgoing stream, then reads the frame its
   callbacks consumed through, at the stream's own rate and the outgoing
   document's fps, closes the epoch there (the terminal stop clamps it to
   the duration), and only then replaces or resets. What the terminal
   check or a caller's call wrote to the clock never feeds R-5.
2. *An ack only hands over.* `ack_presented` sends (epoch, frame, paint
   instant, expired) on a bounded channel (256) with `try_send`: no lock
   and no wait (R35). It samples no clock and registers nothing. A full
   channel *drops the newest* ack (the sender cannot drop the oldest) and
   counts `acks_overflowed`; the UI never blocks.
3. *Settlement.* The worker moves the channel's acks into its counters
   and drains them every tick and at every
   transport change. An ack settles its due record: on time if painted
   current within due + 1 frame, else late. An ack ahead of the
   registrations in the open epoch is held until a registration reaches
   its frame or the epoch closes. An ack that matches nothing (a duplicate,
   a frame never due, a closed epoch, history past the caps) is dropped
   and counted in `acks_unmatched`. The A/V offset is the whole frames the
   paint trailed its frame's due instant (at least one if expired at
   paint).
4. *Agent attribution.* A due frame the worker registers in an agent job's
   playback epoch while the job runs, past the newest frame the preview
   had published before it, is charged to `dropped_agent`; a later paint of
   it settles it and removes the charge. `dropped_agent` is therefore the
   job-window frames never painted.
5. *Hard caps.* Due records 65,536 (the oldest is evicted, keeping its
   identity); evicted epochs 8; settled ranges per evicted epoch 64 (the
   two oldest merge); the ack channel 256, and the acks the worker holds
   ahead of its registrations 256 (both drop the newest). History past a cap stays in the
   aggregate counters. The approximation: a forgotten epoch's evicted
   frames, and the frames inside a merged gap, stay dropped even if acked
   later (the ack is unmatched); an evicted frame charged to an agent job
   stays charged; a due instant is the registering tick's, which follows
   the frame becoming current by the tick's delay (nominally under 5 ms,
   unbounded under load). The A/V offset is taken from that stored
   instant: a paint that precedes a delayed registration reads zero
   elapsed, and `expired` then supplies only one frame, so the offset is
   a proxy that can under-report, not the consumed-clock offset.

## 8 Seek and scrub

**S-1 [S2a] Coalescing.** The paused slot keeps the newest stamp. At most one
paused render is in flight, and the final target is always rendered (L-6).

**Amendment R49 [S2c] (lead ruling 2026-10-05, deadline-bounded) A drag
finishes the paused render in flight.** The S2c-1 probe (E13) refuted reader
churn: on a 30 Hz drag 62–90 % of taken paused jobs were abandoned in H-2's
FrameWait by the next `request_frame` and 40–68 % of decodes were stale at
delivery, so few frames ever published. A paused FrameWait is therefore
superseded by a `play` (R37), by a newer epoch (`seek`, `pause`,
`set_document`), by any other post (`None`, playback) and by shutdown, at
once, as before (L-6). It is not superseded by a newer paused job of its own
epoch (a drag's `request_frame`) **only while it has waited less than
`RETIRE_DEADLINE`** since it was taken (the existing constant, no new
tunable); past that the newest `request_frame` supersedes it, as before, so a
reader stuck in IO cannot hold a drag until the next epoch change. A kept wait
wakes at that deadline. It finishes, publishes, and the preview then takes the
newest job (S-1's slot). A drag call's latency is at most min(two paused
renders, `RETIRE_DEADLINE` + one render). S-1, L-6, P, C, K-2, H-5's FIFO order
and R37's region rule are unchanged, and no plan is posted for the newer job
while the older one finishes. S-2 is the cost fix. P-seek prints
`drag_paused_abandoned` and `drag_seeks` (`PlaybackStats::paused_abandoned`
and `reader_seeks` over the drag). The brief's item-2 bounds (decoder kept
across plan changes, no permit resize while dragging) and its reopen witness
are withdrawn: the probe saw no reopens or resizes to remove.

**S-2 [S2c] Forward continuation, only inside a demonstrated domain (R12,
R18).** For a paused target t with reader cursor c, the reader decodes forward
from c instead of seeking only if **all** of these hold; anything unknown uses
today's seek.
1. *Anchor = the actual seek's result.* Today's seek calls
   `avformat_seek_file(-1, …, ts)`: `av_seek_frame` selects
   `av_find_default_stream_index` and rescales ts to it, and `mov_read_seek`
   then subtracts that stream's `min_corrected_pts + dts_shift` before its
   backward index search. Those fields are private to `MOVStreamContext`, so a
   lookup table cannot reproduce them. Instead each reader keeps a demux-only
   *shadow* context of the same file, and A(t) is the first packet of the
   selected stream after the identical `seek` call on the shadow: (`pos`,
   DTS, key flag). If the selected stream is not the decoder's video stream, or
   the shadow fails, the correction is unknown and the reader seeks normally.
2. *Matching anchor:* at the run's seek to t0 the real context's first packet
   of that video stream must equal A(t0) (`pos`, DTS, key), else continuation
   is disabled for the decoder. A(t) must equal A(t0), and the decoder must
   have been fed continuously from A(t0) (Amendment R51 withdrew the clause
   that no key packet may have been read since it).
3. *Valid decoded timestamps:* every frame produced since the anchor has a
   `best_effort_timestamp`, strictly increasing; selection uses them, never
   `fallback_index`; c < t ≤ c + 12 frames.
4. *Witnessed pair:* the demuxer/codec pair is on the list (`mov`/H.264 to
   start, fragmented MP4 excluded: the lead's ruling with R51/R52), extended
   only by adding witnesses.

*Open GOP: the earlier-key retry (S2c-5, R39 item 4 (b), lead rulings of
2026-10-06).* On a witnessed pair (rule 4's list, the same one), a
`decode_window` whose seek landed on a key packet past the stream start, and
whose first decoded frame presents after `start`, seeks once more, to just
before that key's DTS. It then decodes from the earlier key, which holds the
open-GOP leading frames' references. This applies to every Seek render
(preview, agent thumbnails, playback region starts), where these frames were
`no_frame` errors before, and that change is the intended fix. A retried run
has no anchor (no A(t0)): rule 2 cannot hold, so the next paused frame seeks,
and the rule-2 latch is not set by a retried seek. Its first packet is the
earlier key's, not A(t). Other demuxers keep today's single seek, so
AviDtsGuess's misses stay on the backlog ((c)).

Here a seek to t would feed the same packets from the same flushed state; the
witnesses must show `pending`, `lookahead`, `continuation_at` and `eof_sent`
equal the seek path's. Cancellation is checked between produced frames; the
decoder resets on a `VideoSourceKey` change, `shrink` reopen or error.
*Witnesses* compare with a fresh `Seek` at 50 seeded targets each: default x264
B-frames; reordered timestamps (`-bf 3 -b_pyramid normal`, negative
composition offsets); an MP4 edit list (`elst` start offset); `open-gop=1`
(exits at its key packets); VFR; missing PTS (falls back); targets at c + 1,
just before and at a key packet, and ±1 tick around each corrected anchor
boundary (key index timestamp + `min_corrected_pts + dts_shift`, in the
edit-list and negative-offset files); an edit; a relink; a same-source jump
cut; cancellation mid-run.

*Implementation [S2c-2].* A reader decodes a paused plan's frame with
`VideoDecoder::decode_paused(cursor, t)`; a playback plan keeps
`decode_window_sequential` (rule 2 would seek at every GOP and change
open-GOP leading frames there). The decoder tracks, since its last seek, the
first video packet and whether every frame's timestamp was present and
increasing; a running continuation that meets a bad timestamp before t is
complete is abandoned for `decode_window(t, t)` (a key packet read no longer
abandons it: Amendment R51). After each paused seek the shadow's A(t0) is read
and compared with the real first packet (a mismatch latches continuation off
until a reset: an error, or a new decoder). The shadow is opened on first
use, with the reader's interrupt; a second demuxer context (its own copy of
the index) per reader that has rendered a paused frame is the memory cost.
Rule 1's stream test is read at open: the decoder's stream must not be an
attached picture or discarded, and no other stream may be a video stream
that is not an attached picture (`av_find_default_stream_index`'s scores).
While `decode_paused` runs, `receive_frames` checks the stop flag after each
received frame. The merge of C-4 (1602761) also restores a stop check
before each packet read (R43 had moved it to the top of the loop), so a stop
raised while decoded frames drain reads no further packet.

**S-3 [S2c] Bounded backward window.** On a backward paused step to a
non-resident target t: B = clamp(share in frames, 1, 16), start = max(source
start, clip in-point, t − B + 1). The reader seeks and decodes exactly as a
fresh paused seek to t (Amendment R54): the window is the decoded frames that
land in [start, t − 1] on that decode, a demand region (K-5); it never seeks to
an earlier key to fill the window, so near a GOP start the window is smaller.
A hit is one render (L-4a); a refill is t's own seek and decode, plus t's
conversion (L-4b: time to t). GOPs over 250 are recorded, not gated (D4).

*Implementation [S2c-3].*
- **Scope.** A paused job's sources each with one required time t go to
  `Readers::backward`, under the post's lock.
- **Refill.** t refills when its frame is not in the ring and it is a step
  back: last − B < t < last, with `last` the source's last posted required
  time (K-5's travel memory, source space) and B the window's own size
  (Amendment R52; a longer jump back seeks alone, as before S-3). B is
  ⌊(C − G − H) / n / f⌋ clamped to 1…16, with H and G the job's set. The
  window is [start, t], start = max(the clip's in-point, t − B + 1), and
  [start, t) joins t's required times. K-2 therefore reserves the whole
  window, and B·f ≤ share keeps the set within C. Amendment R54: the
  reader seeks and decodes as a fresh paused seek to t (the same anchor and
  S2c-5 retry), keeping the decoded frames that land in [start, t − 1]; the
  window is then clipped to the frames kept (the times below them stop
  being required and their reservations return). A later step below the
  anchor is an ordinary refill in the earlier GOP. A window frame's
  reservation is max(f, d), d its decoded size (and B uses max(f, d) for
  f), so the kept frames are inside K-1.
- **t first (Amendment R53).** The refill's reader converts only t; it
  keeps the decoded (unconverted) frames of the window and delivers t to the
  preview ring ("published" in R53 means delivered to the ring, not rendered
  or shown). The window's conversions then follow in the same job, one plan
  step each, descending from t − 1; under R56 they deliberately run while
  the preview renders and shows t. A newer post that requires a time of that
  source outside the window cancels the rest between frames (Amendment
  R54); the frames already converted stay held. A step inside the window
  keeps it converting: the step's own frame first, then the remaining kept
  frames below it, descending (frames above the newest t are dropped:
  Amendment R57). A step to a window frame not yet converted
  keeps the window (no refill) and goes to the reader that kept its decoded
  frame, which converts it without decoding anything. A step to the frame
  whose conversion is already in flight keeps the window too and waits for
  that conversion. If no reader keeps it (the reader closed or decoded
  elsewhere), the step is today's: a refill or a seek.
- **Hold.** The post holds each source's window. Its ring frames survive
  later posts while the source stays planned and the playhead stays inside
  the window. A step inside the window to a frame already in the ring, in
  either direction, is a hit and decodes nothing. Since Amendment R57 the
  window converts only below the newest t: kept frames above it are dropped
  unconverted, so reversing above the newest t to a frame that was never
  converted is not a hit and may need a re-decode (a refill or a seek).
  Held frames that the job does not require are lookahead
  to K-5: a drain evicts the frames behind the travel first, so after a
  reversal those below the playhead go first.
- **Exclusions.** Playback jobs, forward steps, jumps back of B or more
  (R52) and sources with several required times hold no window. Agent
  renders (render-for-agent) never reach the readers (`run_agent` renders
  synchronously with `Seek`), so they neither refill nor move the travel
  memory.

## 9 Audio clock and A/V sync

**V-1 [S1] The clock counts programme frames popped (R6).** `render_output`
pops whole interleaved frames only (while `consumer.slots() ≥ channels`); the
rest of the callback is silence, preserving channel alignment. `SharedClock`
advances by frames popped; unpopped frames go to `underrun_frames`. Content is
never inspected: authored silence, muted mixes, zero-track timelines and
`next_chunk_limited` padding are programme. The amended
`callback_consumes_ring_then_writes_silence_and_accounts_frames` expects
position **11** (was 12), output `[0.25, -0.5, 0.0, 0.0]`, `underrun_frames` =
1; a partial frame (3 samples, 2 channels) pops one frame; an empty ring leaves
the position unchanged (G9).

**V-2 [S1] Drained end and terminal stop (RB7).**
- *Drained predicate* (each tick while playing): `fill_ring` has returned
  `false` (latched by `fill`) ∧ `pending` fully pushed ∧ ring empty ∧
  `position_samples` ≥ `fed_position_samples()` (the callback pops before it
  updates the position, so this holds only once all popped frames count).
- *Terminal stop:* the drained predicate, or today's `position ≥ duration`,
  calls the new `Worker::stop_at_end`, never `pause()` (which would read
  position 0 in the 1601-sample case and truncate loudness there). It (1)
  pauses the stream; (2) calls `loudness.pause_at(duration)`, publishing and
  truncating at the terminal position; (3) drops audio and installs empty mix
  meters; (4) stores `fallback_frame` = duration, then `sample_rate` = 0, so
  `position()` switches directly to duration; (5) sets `playing` = false and
  emits `PlaybackStateChanged(Paused)` and `Position(duration)`.
- *Drained is the ring, not the device (R29, review B S1).* The clock
  advances inside the output callback, before the host API takes the
  buffer (ALSA `writei`, WASAPI `ReleaseBuffer`), so "drained" means the
  programme has left the ring, not that its last sample has sounded; the
  terminal stop can cut the device's queued output. S1 claims no device
  drain; an output-drain acknowledgment is deferred with D9.
- *Races (R29, review B F1/F2).* A seek pending when the programme drains
  is applied first, still playing; a seek published before the stop's
  `eos_generation` bump resumes playing after it. A `Pause` handled once
  the ring has drained (the samples the callback consumed, never the
  requested position a `seek` sets), before the tick and with no seek
  pending, takes the terminal stop (R30, re-review D1). Continuous seeking
  defers the stop; a finite burst is consumed on the next pass.
- *Tests:* a one-frame 30000/1001 timeline at 48 kHz (ends at sample 1601)
  and a long programme both finish with `position()` = duration, loudness
  truncated there and the stopped state; the 2 s stall control does not
  complete. AU2, AU3 and AU4 suites run unedited.

**V-3 [S2a] Fill independence.** `fill_audio` runs on the worker at its own
cadence, since the worker no longer renders.

**V-4 [S0] Observables before S2a.** S0 computes due-frame outcomes, held age
and offset consumer-side from today's `frames()` arrivals and `position()`, so
baselines need no new API. The cpal playback timestamp minus the callback
timestamp is recorded as `device_latency_ms`, not compensated (D9).

**V-5 [S0] Device-free audio driver (R7).** `AudioRuntime` gains
`AudioOutput::{Cpal(cpal::Stream), Simulated(_)}`. The simulated output calls
the same `render_output`, stepped by `advance(frames)` in CI or paced in real
time (1,024-frame callbacks at 48 kHz) for P-play, via a harness-only engine
option. One LH run per workload cross-checks on the real device.

## 10 GPU display path and upload (SDR only)

**G-1 [S1] CPU monitor table.** `readback_for`'s SDR BT.709 branch of
`encode_monitor_rgba8_for_description` uses `MonitorTable` (65,536 entries
indexed by f16 bits). Its doc comment rejecting a **4,096-entry interpolated**
LUT is amended: this table has no interpolation and is exhaustively equal.
Test: all 65,536 patterns equal, including NaN → 0, ±Inf, and every denormal and
±0 → 0.

**G-2 [S3a] GPU encode pass** (SDR BT.709 monitoring only, R2). A compute pass
reads the Rgba16Float output with **unfiltered** `textureLoad`; the f32 of an
f16 texel is exact and `pack2x16float` recovers the bits (a denormal-flushing
backend yields ±0, which maps to 0 like every denormal; all NaN payloads map to
0). It looks up the 192 KiB storage buffer (X-5) and writes an Rgba8Unorm slot
with exactly the bytes egui uploads today; other monitoring stays on the CPU.

**G-3 [S3a] Ownership.** The engine already runs on eframe's device and queue
(`GpuContext::new_with_adapter_info` → `new_with_gpu`), so compositor, display
pass and egui submit to one ordered `wgpu::Queue`. The app passes the
premultiply table (from `Color32::from_rgba_unmultiplied`) and the `repaint`
hook in `DisplayConfig`; media returns `wgpu::TextureView`s, with no egui
dependency.

**G-4 [S3a] Slot fence: root full-frame epoch (R11, R17).** e =
`ctx.cumulative_frame_nr_for(ViewportId::ROOT)`, read in `App::logic` and
`App::ui`; the app creates no other viewport (debug-asserted). One stable
preview `TextureId` is re-pointed by `finalize_preview`
(`update_egui_texture_from_wgpu_texture_with_sampler_options`, LINEAR). A slot
unbound during epoch e becomes Retiring(e): egui samples it at most in epoch
e's paint, which is submitted or abandoned before the next root epoch, so
`App::logic` at any m > e frees it (it runs even when minimised).
- *One rebind per epoch:* after one Bound → Retiring(e) in epoch e, later passes
  of e (`request_discard`) keep the binding; the newer Ready binds next epoch.
  The app thus holds ≤ Bound + one Retiring, and the preview always owns the
  third slot (Free, Writing or its unbound Ready).
- *Retirement repaint:* each rebind calls `DisplayConfig`'s `repaint` hook
  (`ctx.request_repaint`), so a later epoch soon frees the Retiring slot.
- *Bounded wait:* a preview finding no slot (a debug-asserted backstop, e.g.
  mid-resize) waits ≤ 2 frame intervals and calls `repaint`; on timeout it
  counts `slot_starved`, drops a playback frame, or retries a paused one on the
  next release.
- *Terminal handoff:* `App::on_exit` (called by eframe's `save_and_destroy`
  after the last paint, before `painter.destroy()`), or `Drop` if it never ran,
  marks the display **Terminal**, "no future paint": Bound and Retiring slots
  are released through G-7's token without another epoch.

| State | Owner | Next |
|---|---|---|
| Free | preview | a write starts → Writing |
| Writing | preview submission | flags clean → Ready(stamp); flags set → Free + `StampedError` |
| Ready(stamp) | preview, complete (G-5) | bound (previous Bound → Retiring(e)); stale or newer Ready → Free; overwritable |
| Bound | app (stable id) | rebind → Retiring(e); Terminal → token release |
| Retiring(e) | app | `App::logic` at m > e → Free; Terminal → token release |

**I16** (fake epochs): A → B → C bound in one epoch's passes (one rebind, no
stall), a skipped paint, 100 minimised epochs, a mid-epoch resize, the wait
timeout, and Terminal teardown with no further epoch; plus a validated GPU run
on CI-L.

**G-5 [S3a] Non-finite refusal kept (B6).** The display submission also copies
the compositor's validity flags (`frame.validity` or `pooled_flags`) into the
slot's small charged `MAP_READ` buffer; the preview waits ME16-style
(`frame_poll`, no deadline). A set flag yields a stamped
`MediaError::NonFiniteRender { layer, .. }`, as `for_each_linear_pixel` does,
the slot returns to Free and nothing is published (I6 on MO2's non-finite
fixture; `app_playback_does_no_readback` counts full-frame readbacks = 0).

**G-6 [S3a] Self-check with fallback.** `enable_display` encodes a 256×256
texture of all 65,536 f16 patterns (with alpha sweeps) and compares it with CPU
encode + premultiply; on mismatch or a missing feature it keeps the CPU
`frames()` path, logged once, never adopting a tolerance. Headless AW1, tests
and agent jobs always use the CPU path.

**G-7 [S3a/S3b] Completion-owned resources,** each charged to `GpuLedger` at
its actual API size.

| Resource | Reusable / releasable when | On error, cancel, device loss |
|---|---|---|
| Display slots (3 × w×h×4: 10.55 MiB at 1280×720, 15 MiB at 1024×1280) | reuse: Free via G-4. Uncharge/destroy (resize, disable, teardown): once Free or Terminal, the preview calls `queue.submit([])` → `SubmissionIndex` and uncharges when `device.poll(Wait{that index, timeout: None})` or `frame_poll` observes it | charged until observed |
| Flag buffers | per slot, after G-5's wait | ME16 retirement |
| GPU table buffer + staging | ME16 atlas pattern | rebuilt per device |
| Upload staging ring (`MAP_WRITE`, ≤ 2 frames × layers, S3b) | its frame completed (G-5 or readback wait) **and** `map_async(Write)`'s callback ran | ME15 `retired` under the completion flag; device loss drains callbacks |
| Resident layer textures (U-2, S3b) | evicted and last using submission completed | retired, charged |

The preview thread is the completion owner: it polls via `frame_poll` and, at
teardown (H-6 step 4), waits deadline-free on its final token, so callbacks run
even when egui submits nothing more. The Terminal handoff (G-4) frees the
stable id through the app's stored `RenderState` and hands every slot to that
token. Its `queue.submit([])` follows egui's last submission in queue order, so
no epoch is needed.

**U-1 [S1/S3b] Upload copies.** S1e (conditional): if S0 finds
`upload_bytes`'s extra full copy material, pixels go straight into mapped
staging (exact byte cast); S3b adds the staging ring. *Amended (R28, S1):*
S0 never measured the copy. S1 measured it at 1.83 ms per 1280×720 layer,
against 0.25 ms for a plain copy. G3 passes without U-1, and the exact cast
needs a `zerocopy` dependency because the workspace forbids `unsafe`. U-1
therefore moves wholly to S3b, beside the staging ring, and the dependency is
decided there.

**U-2 [S3b] Residency.** Cache keyed by (`VideoSourceKey` or `TitleCacheKey`,
frame time, `shared_buffer_id`), ≤ 64 MiB, charged, idle eviction after 2 s;
layers stay Rgba16Float. Test: a repeated paused frame uploads 0 bytes.

**P-1 [S1] Monitor long-edge cap.** Only `present` changes: max_width =
min(`PREVIEW_MAX_WIDTH`, ⌊1280·w/h⌋). 9:16 → 720×1280 (the 720p pixel count);
4:5 → 1024×1280, 42% more pixels than 720p. `RenderScale`, thumbnails, agent
replies, `media_status`'s `max_width`, proofs and export are unchanged.
Portrait sources inside landscape documents still decode up to 1280 wide (D8).

## 11 Gate registry

*Inv* must pass from its stage onward; *Gate* must pass at its stage (later
ones are run and recorded); *Rec* is recorded only. **D**: today's value is
direct evidence on the gate's protocol; **PB**: pending an S0 baseline.

| ID | Invariant | Lane | Stage |
|---|---|---|---|
| I1 | X-2 input tables and G-1 monitor table exhaustively exact | CI-L, CI-W | S1 |
| C-5 | Existing pins unchanged: CC1–CC8 S1, MO2 byte and solo pins, export, `preview_frame`/`get_frame_at`, `preview_solo`. Export changes only via X-1 (proven by I1); G-1 is monitor-only. Only the V-1 test is edited, plus the S0 harness tests that model the V-1 clock (R28) | CI | S1+ |
| C-3 | Monitor pixels change only via P-1 (new pins at the capped raster) | CI | S1 |
| I3 | Ledger ceilings (384 / 1,536 MiB) and release (`r28_ledger_*`); every new resource charged | CI | S1+ |
| I4 | ME13 `BASELINES` untouched; `r28_end_to_end_tracked` 5% rule | LL, LH | all |
| I9 | V-1/V-2 tests; `position()`/loudness/state after terminal stop; AU2–AU4 unedited | CI | S1 |
| I8 | R-2 interleavings: no old-epoch or expired frame published, no superseded error stops playback, release target shown | CI | S2a |
| I18 | R-5 marker: abandoned, zero-clip, discarded-pass and stale paints never acked; one ack per `frame_id`; witnesses A-layout/B-bind/C-deferred and seek-before-paint | CI | S2a |
| I13 | Agent lane: FIFO, bound, cancel, paused-wait suspension, exactly-once replies incl. shutdown, errors only via reply | CI | S2a |
| I15 | H-8 model per S2b commit; stress test at S2b-4 | CI | S2b-1+ |
| I11 | Reader frame threads ≤ P; synchronous decoders min(P, 16), outside the pool; FIFO tickets removed on retire/cancel; no deadlock or lost required request for P ∈ {1, 2, 3, 20} | CI | S2b-2 |
| I12 | Scheduler live bytes ≤ C always; oversized sets take K-3 | CI | S2b-3 |
| I10 | H-7 thread counts, quiescence retirement (injected clock), no synchronous decoder after a park; AW1 B5 | CI-L, CI-W | S2b-4 |
| C-4 | Continuation = `Seek` in its domain, and falls back outside it (S-2 witnesses) | CI | S2c |
| I1b | Premultiply table = `Color32::from_rgba_unmultiplied` for all inputs | CI | S3a |
| I5 | GPU display = CPU + premultiply, zero tolerance: all-pattern texture + 10 frames per workload | CI-L, CI-W, LH | S3a |
| I6 | Stamped `NonFiniteRender` on the display path; charges back to baseline | CI | S3a |
| I16 | G-4 fence model (one rebind, bounded wait, Terminal) and G-7 lifetimes, teardown to zero charge | CI | S3a |
| I17 | Staging ring reuse only after completion and map callback; ME15/ME16 retirement; 0-byte repeated paused upload | CI | S3b |

| ID | Target | Protocol, lane | Stage | Today | Ev. |
|---|---|---|---|---|---|
| G3 | `blend_heavy_holds_floors_on_hardware` (ME13, 60 fps) | P-comp, LH | S1 | 27.4 fps | D |
| G9 | The clock does not advance on underrun | stepped, CI | S1 | advances | D |
| G12 | `reel_9x16` monitor raster 720×1280 | CI | S1 | 1080×1920 | D |
| G5 | 0 seeks per output frame after warm-up, two continuous sources | counter, CI-L | S1 | — (0.88 is the cut workload) | PB |
| G10 | Offset ≤ 33 ms within 5 s after a 2 s stall | stepped, CI | S2a | — | PB |
| G11 | 0 underruns over 60 s of `blend_heavy_1080p` | P-play, LL | S2a | — (T4 render loop) | PB |
| G16 | Max clock stall ≤ 100 ms in every run; the clock-freeze control fails it | P-play, LL, LH | S2a | — | PB |
| G13 | Four active sources: reader threads ≤ P = 20 | counter, CI | S2b-2 | 64 (min(P, 16) × 4) | D |
| G18 | Export wall time on the PERFORMANCE export lane ≤ S0 + 5% (no regression) | LH | S2b-2 | — | PB |
| G17 | `sync_fallback_frames` ≤ 0.1% of due frames in every run of every W workload (expected 0) | P-play, LL | S2b-3 | — | PB |
| G1 | `typical_1080p`: every run ≤ 1% late/held/dropped; p95 present interval ≤ 50 ms | P-play, LH | S2b-4 | — (T4: 1.8 fps render loop) | PB |
| G6 | typical p95/p50 present interval ≤ 3 | P-play, LL, LH | S2b-4 | — | PB |
| G14 | Max held age ≤ 100 ms in every run; the freeze control fails it | P-play, LH | S2b-4 | — | PB |
| G15 | Settled-idle RSS ≤ S0 baseline + 4 MiB for every workload and a title-only document | P-rss, LL pinned | S2b-4 | — | PB |
| G8 | L-1 ≤ 40 ms, L-2 ≤ 110 ms (one source; provisional, fixed from S0 and an S2b-2 run under permits), L-3 +1 step ≤ 20 ms, L-4a backward hit ≤ 20 ms, L-5 ≥ 10 / 7 distinct stamped frames/s received during the 30 Hz drag at GOP 60 / 250, L-6 release target shown (S2a) | P-seek, LL, LH | S2c | — (T3 decoder-level) | PB |
| G7a | 0 full-frame readbacks on the app display path | counter, CI | S3a | 1 per frame | D |
| G7b | GPU encode ≤ 2 ms per frame | LH | S3a | — (19 ms is CPU encode) | PB |
| G2 | G1's criterion for `explainer_16x9`, `reel_9x16`, `feed_4x5`, `talk_recut` | P-play, LH | S3b | — | PB |
| G4 | typical, `explainer_16x9` ≥ 24 fps; `blend_heavy_1080p`, `reel_9x16` ≥ 15 fps (provisional) | P-play, LL | S3b | — | PB |

**Amendment R48 [S2b] G3 on LH is environment-blocked until S4's pinned run
(Riel, 2026-10-05).** On an idle desktop the RTX 3090 stays mostly at P8
through the G3 benchmark, and every binary reads 42–65 fps: S2b's `10a2d18`,
R37's `712d108` and R47 (E12.18). The earlier ~71 fps passes were taken while
the screensaver held the clocks up (E12.13.4). G3 therefore measures the
driver's power management, not the compositor, on an unpinned card. It is
recorded as **blocked (environment)**, not as a pass or a regression: no stage
closes on it until S4 fixes and records the GPU state, and S4 rules on the
floor then. G3's LH figures are still taken and reported in every timing run.
The Omarchy bar and its widget pollers are baseline load, not contamination.

**Amendment R50 [S2c] Timing runs on the machine as it is (Riel, 2026-10-05).** No timing waits for a quiet window
("you should not be banking on that"). Every timing run from S2c on follows these rules.
1. **Paired.** Every timed lane interleaves the candidate with a reference binary in the same run (A B A B, at least
   two pairs), so both see the same load. The reference is the last closed stage's binary (`ad8f896` for S2c), or S0
   for G18. Relative claims are judged on the per-pair ratio, median across pairs: G18 ≤ S0 + 5%, I4's 5% rule, and
   any "this stage improves X".
2. **Absolute gates** (G1, G6, G8, G14, G16) are still measured and reported against their thresholds.
   - A miss counts against the candidate when the reference passes in the same run, or when the candidate is worse
     than the reference by more than the spread between pairs.
   - A miss both binaries share is recorded as *environment-limited at load L*, with the load record. The stage
     closes on the paired comparison.
   - Any later run whose load record is low supplies the absolute verdict. Runs are not scheduled to wait for one.
3. **Mechanism claims gate on counters that do not depend on load:** frames decoded per frame converted, seeks,
   `drag_paused_abandoned`, `sync_fallback_frames` (G17), underruns (G11) and RSS (G15). These are the primary
   evidence for S2c.
4. **The load record stays but no longer gates.** It keeps `uptime`, the top five processes, the GPU state and the 5 s
   sampler. Contamination lines are annotations, and the runner's quiet-wait is removed. The run must not overlap
   the worker's own cargo builds: build everything first, then time.
5. G3 stays environment-blocked under R48.

**Amendment R51 [S2c] Rule 2 is the anchor test alone (lead ruling, 2026-10-06).** The S2c timing (E13.4) showed that
abandoning continuation on *any* key packet read, read-ahead included, makes a refill cost about 6 seeks and 270
decodes and makes +1 steps near a key seek again (the L-1/L-2/L-3 regressions). Rule 2 becomes: A(t) must equal
A(t0), and the decoder has been fed continuously from A(t0). The key-packet clause is withdrawn.
- *Why it is safe:* with the same anchor and an unbroken feed, a fresh seek to t feeds the same packets from the same
  flushed state. A key that presents at or before t gives a different corrected A(t), and the shadow check rejects
  it. This covers B-frame reordering, open GOP and edit lists, because the shadow uses the same `seek` call.
- *The condition:* `pair && stream && !disabled && continuation_at == c + 1 && c < t ≤ c + 12 && !stamps_broken &&
  run.is_some() && first == run && shadow_anchor(t) == run`; `key_read` and the key branch of `on_packet` are gone.
- *Order:* the C-4 oracle (`Model::expected`, the positive control's rules) changed first, in its own commit
  (`f967bae`), then the implementation (`700b729`). Evidence: E13.5.

**Amendment R52 [S2c] S-3 refills only on steps (lead ruling, 2026-10-06).** A backward paused target t refills
only when last − B < t < last. `last` is the source's last posted required time (the existing travel memory, in
source space, not the timeline playhead); B is the window's actual size (≤ 16, `WINDOW_FRAMES`). A larger backward
jump seeks as `ad8f896` did, so a click-seek back no longer pays a window (L-1/L-2). Hits inside a held window stay
unconditional. Agent paused jobs neither refill nor update the travel memory: they render synchronously outside the
readers. Implementation `de7a00f`; evidence E13.5.

**Amendment R53 [S2c] A refill shows t first (lead ruling, 2026-10-06).** A backward refill converts and publishes t
before converting the rest of its window (E13.5.7: about 16 conversions of about 5 ms each made the refill's extra
cost). *Published* here and in R55/R56 means **delivered to the preview ring** (Amendment R61, item 7): t is
converted and delivered to the ring before any window conversion starts. It does not mean rendered or shown; under
R56 the window's conversions deliberately run while the preview renders and shows t.
- The window stays **required** and reserved under K-2, so the budget accounting is unchanged. B stays
  clamp(share, 1, 16).
- The window's conversions continue after t is published, in the same job, in descending order from t − 1. They
  are cancellable between frames; frames already converted stay held.
- A step that arrives for a window frame not yet converted waits for that conversion; it is not re-decoded.
- The reader keeps the window's decoded frames (at most B − 1, at the source's decoded size) until it converts
  them, decodes anything else, or closes. Amendment R54 charges them inside K-1 (below).
- L-4b is gated as *time to t on a refill*: no worse than 1.25 × the reference's step mean, paired. The time to the
  full window is recorded.
- Witnesses, each with a mutation: t is delivered to the ring before any window frame is converted (the witness,
  `a_refill_publishes_t_before_converting_its_window`, checks conversion order against delivery, not against the
  preview's render); a newer job cancels the
  remaining window conversions; a step into the not-yet-converted part of the window decodes nothing new.
  A fourth covers a step to the frame whose conversion is in flight (it waits; no refill).

**Amendment R54 [S2c] A refill decodes what a fresh seek decodes; steps inside the window keep it converting (lead
ruling, 2026-10-06).** E13.6.4: under R53 L-4b was 1.52–1.59× (a refill decoded 46 frames against 30) and the hit
p95 23.2–26.7 ms (no window was ever fully converted).
- **The window is clipped to t's own decode.** A refill seeks and decodes exactly as a fresh paused seek to t (the
  same anchor, and the same S2c-5 retry on its pair). The window is the decoded frames that land in
  [max(t − B + 1, clip in-point, source start), t − 1]. A refill never seeks to a key before t's anchor to fill the
  window, so its decode count equals the reference step's; near a GOP start the window is smaller. A step past the
  anchor is an ordinary refill in the earlier GOP.
- **Kept frames are references where safe code allows.** Each frame is received into a fresh `frame::Video` (a
  reference to the decoder's buffer); a kept frame is moved out of the pending slot, not copied, except a frame
  that also covers t. The cost of keeping is measured and recorded in E13.6.
- **Kept decoded frames count inside K-1.** Each unconverted window frame's K-2 reservation is max(f, d), d the
  source's decoded frame size; it shrinks to f on conversion and returns on drop or cancel. I12 (live ≤ C) covers
  the kept frames.
- **Cancellation applies only to jobs outside the window.** A newer job cancels the window's remaining
  conversions only when, for that source, it requires a time outside [window start, t]. A step inside the window
  has its own frame converted first, if it is not already, and then the descending conversions continue.
- **L-4a and L-4b stay as defined** (hit p95 ≤ 20 ms; time to t ≤ 1.25 × the reference's step mean, paired).
- Witnesses, each red first and each with a mutation: a refill's decode equals a fresh seek's and its window stops
  at t's anchor; I12 holds with a window kept (the mutation leaves the kept bytes uncharged); after a step inside
  the window all of its frames end up converted. The R53 "newer job cancels" witness targets a time outside the
  window.

**Amendment R55 [S2c] Window conversions outrun the stepper (lead ruling, 2026-10-06).** E13.6 (r54a): L-4b passed
(1.14 / 1.15×) but the hit p95 was 23.3 / 26.4 ms, with 18 / 21 of 125 hits pre-converted and 11 of 75 windows
filled. P-seek posts each step as soon as the previous one shows, and one window conversion costs about a step,
so a hit almost always waits for one conversion. Before R53 a hit was a ring lookup (E13.5: p95 13.8 / 18.3), so
the regression is this design's, not the environment's. The gates and P-seek's pacing do not change.
- **Measure first** (no behaviour change, its own commit): the per-frame conversion time for t and for window
  frames, LL and LH; whether the converter is threaded today; and a hit's time from post to frame ready, split
  into waiting for a conversion and the render. If a hit's wait is not mostly conversion, stop and report.
- **Then cut the per-hit wait,** in this order: (a) threaded conversion (FFmpeg 8 swscale or slice threading) in
  safe code for window frames, and for t if it speeds t too, bounded at min(4, available parallelism − 2)
  threads, with CPU time per refill recorded against r54a; (b) if (a) is unavailable or not enough, a second
  conversion lane beside the job thread, at most 2 conversions in flight per source; (c) never convert past what
  K-2 has reserved. B, I12 and the R54 charges do not change.
- **Invariants, each with a witness (and a mutation where new):** t is published before any window conversion
  starts; a newer job outside the window stops every lane's remaining conversions (in-flight ones may finish and
  stay held); a step inside the window converts its own frame first unless a lane already has it, and no frame
  is converted twice; converted bytes stay inside K-1 with any number of lanes in flight (I12); frames equal
  fresh seeks (the C-4 oracle).
- **Gate:** the paired L-4 check (3 pairs, LL and LH, `seek_gop60`): hit p95 ≤ 20 ms and L-4b ≤ 1.25×, with the
  pre-converted hit share, windows filled, CPU per refill and the backward mean reported.

**Amendment R56 [S2c] The window converts continuously after t (lead ruling, 2026-10-06).** R55's measurement
(E13.6.7) stopped it: a hit's wait is about a third of the hit and a conversion is 80% the working-frame step, so
(a) and (b) miss the cost. But a window conversion costs 4.4–6.4 ms against a step every 17–20 ms, and only 6–28
of ~124 hits were pre-converted, so something keeps the conversions from running ahead between steps. The gates
and P-seek's pacing do not change; the working-frame step and swscale threading are not touched (D12).
- **Trace first** (test builds, its own commit): each window frame's conversion queued, started and finished,
  against the step posts and the hit renders; one pair per lane. E13.6 names the cause with numbers. If the
  conversions really do run back to back and still fall behind, stop and report.
- **Then the scheduling:** from t's publish until the window is done or cancelled (R54), window conversions
  proceed without waiting on hits. A post inside the window only re-prioritizes: it never restarts, cancels or
  re-plans the pass. A hit on a converted frame never waits on a conversion. If the trace shows it, conversions
  run off the thread that serves hits: at most one extra thread per source, no new dependency.
- **Invariants (R53/R54) hold**, each behaviour change with a witness and a mutation, red first: t first, an
  out-of-window cancel, no frame converted twice, I12 and K-2's charges, frames equal fresh seeks. The new
  witness: a stepper posting faster than a hit but slower than one conversion per step has every in-window step
  after the first k served pre-converted.
- **The R54 gap is accepted as bounded** (one window per reader, at most B − 1 decoded frames, D13).
- **Gate:** the paired L-4 check as for R55.

**Amendment R57 [S2c] A row-wise table fill, and the window converts only below the newest t (lead ruling,
2026-10-06, revised).** R56's trace (E13.6.8): the reader converts back to back, but P-seek's backward steps
jump 1–12 frames, so 80% of window conversions are of frames never shown, a third of them above the newest t.
The hit pays one conversion in flight and its own, and the working-frame step is most of a conversion. That step
is already table-driven (X-1/X-3: `Conversion::Separable`, `fill_managed_plane`); its cost is the fused loop
itself, a per-pixel rotation `match`, bounds-checked byte reads and `extend`. The gates and P-seek's pacing and
jump sizes do not change; no conversion lanes and no swscale threading this round.
- **A fast path in `fill_managed_plane`** for an unrotated, unflipped frame: row-wise, the output preallocated,
  each row sliced once, no per-pixel `match`; safe code, bit-identical to the fused loop, which rotated and
  flipped frames still take. X-1's checks and errors run first, unchanged. Witness: equality with the fused loop
  on every separable description the fixtures use, an odd width, a stride wider than the row and a pixel count
  that is not a multiple of 8, with rotated and flipped frames still equal to the fused loop; mutation: an
  off-by-one in the row stride. The X-2 parity tests and every pin stay unchanged. The working-frame ms is
  recorded before and after, LL and LH, under the harness.
- **The window converts only below the newest t.** After a post at t′ inside a held window, the pass converts the
  kept frames below t′, nearest first, and never one above t′; the conversion in flight may finish and stay held.
  The kept frames above t′ leave the plan and the reader drops them before its next decode; their K-1 charges are
  held until it has (no new uncharged interval). This supersedes R54's "the descending conversions continue"
  for frames above t′, so R54's in-window witness now expects the window at and below t′ converted and nothing
  above it after the post. Witness: after a jump of k frames, no frame above the new t is converted after the
  post except the one in flight; mutation: frames above t′ stay in the pass. The other R53/R54 invariants and
  witnesses stay.
- **Gate:** the paired L-4 check, run as a measurement. If L-4a misses: the usual counters, and estimates of the
  hit p95 with one and two extra conversion lanes per source, lavapipe's render slowdown under that contention on
  LL, and the extra CPU per refill, for the lead and Riel.

**Amendment R58 [S2c] L-4a on LH is environment-limited by GPU idle clocks; S4's pinned run re-checks it
(Riel, 2026-10-06).** The R57 check (E13.6.9) found the following.
- L-4a passes on LL at 17.6 ms and misses on LH at 22.4 ms.
- An LH hit's wait is 1.6–3.0 ms; its render is 13.0–13.3 ms.
- The R56 binary renders in the same 13 ms in the same session. Under R55 the LH render was 9.8 ms, and on LL it
  is 7.6–8.0 ms.
- In 117 of 136 GPU samples taken during the LH runs, the GPU was at P8 (210 MHz).

The render code is the same in every one of these binaries, so the miss is not S2c's. Its cause is the one behind
G3 (R48) and the LH L-3 miss that both binaries share. **This is not a waiver:** L-4a on LH is recorded as
*environment-limited (GPU idle clocks), re-checked at S4's pinned run*, not as a pass. It joins G3 on S4's
checklist (§13, S4's row). No conversion lanes are added. S2c closes on the paired comparison and on L-4a on LL.

**Amendment R59 [S2c] No reader idles holding discard charges (lead ruling, 2026-10-06).** The closing plan at
`5fa62b4` (E13.6.10) found `explainer_16x9` backward steps timing out at 10 s, on the candidate only. The cause:
a refill below a held window discards that reader's kept frames above the new t′, and R57 keeps their K-1 charges
with the reader until its decoder drops them, at its next decode. When the new set does not fit beside those
charges, admission waits. The reader's own required frame is then unreserved, so it has no decode to start and
never drops them: a deadlock until the next post. E13.6.6 warned of this.
- **A discard-only job.** A reader holding discarded kept frames with no decode to start is told to drop them at
  once (`Next::Discard`, decided under `Sched`). It drops them outside the lock, then releases their charges, which
  wakes admission. A post wakes every reader, so the job runs at the post. K-1 stays exact, R57's text stays true,
  and no charge waits on a reader's retirement.
- **Invariant (test builds):** "a reader idle while holding discard charges" is counted and must be 0 in every
  harness run; a P-seek step that times out also fails the lane.
- **Witness:** two sources. Source 0's reader keeps a window's frames; a step refilling below them beside source
  1's frame does not fit beside their charges, and source 1 must be admitted without that reader retiring.
  Mutation: the discard-only job removed.

**Amendment R61 [S2c] The stage-close fixes (lead ruling, 2026-10-07).** Both Astra stage-close reviews said do
not close (E13.6.11). No R53–R60 semantics change except as stated here.
- **A refill completes only against its own plan.** A refill's completion (`Readers::refilled`) carries the plan
  version it was started under. It always clips the reader's own kept set to what its decoder kept. It clips the
  plan, the required set, the held window and the reservations only if that version is still current. A
  completion superseded by a newer post leaves the newer plan whole. A continued window time below what the
  decoder kept then stays required and is decoded normally (with a seek).
- **A continued window fits K-3.** When a post continues a held window (R54/R57), the continued times join the
  required set only while H + G ≤ C, nearest t first, in the scheduler's terms (max(f, d) for an unconverted
  window time). The rest are cut, down to t alone. The kept frames of the cut times are discarded like R57's frames
  above t′: their reservations move to the reader's discard charge until its decoder drops them (R59's
  discard-only job if it has no decode). R57's discard bound becomes a kept range [low, bound]: the decoder keeps
  only the times inside it. `low` is the lowest continued time kept when a continuation was cut, and unbounded
  otherwise, so R57/R59 behave as before when nothing is cut. A required set that cannot be admitted, an assert or
  an eviction of required frames is never the result.
- **The detached-reader fallback (R47) counts discard charges.** A detached reader holds its conversion in flight
  and its discard charges beside the set. Both count in the check that sends a plan to the synchronous renderer.
- **A decoded frame that covers several window times** (a VFR source whose frame shows at two or more grid times)
  keeps the set of times it still owes. Each conversion removes its time. While other times remain it converts a
  copy (transient, inside that time's max(f, d) hold). The managed converter takes the frame's buffers, so
  converting the kept frame itself would leave nothing for its next time. Its last time converts the frame itself,
  which releases it. A discard drops the times outside the kept range, and the frame goes with its last time. A
  decoded frame is never kept without a time that charges it.
- **Test harness:** the P-seek backward phase traces only when `PF1_TRACE` is set. Every setup and reposition seek
  counts toward P-seek's `timeouts`, and P-play's `valid=false` fails the lane.
- Each code fix has a witness, red first, and a mutation (E13.6.11).


**Amendments R62–R64 [S2c] One charge rule and the accounting model (lead rulings, 2026-10-07).** No R53–R61
semantics change except as stated here (E13.6.12).
- **One charge rule (R62 item 2, R63).** A required frame's reservation is its charge: max(f, d) inside a held
  window below its t, f otherwise; above it only while a reader keeps the time decoded (max(charge, d)). K-3, the
  post, admission and every release use it: K-3 counts a frame a reader keeps at max(f, d) (F7); a post raises a
  reservation below its charge (F1) and lowers one above it (R64); a dropped kept time shrinks to its charge, not to
  f (F5), when no other reader still keeps it (R62 item 1), on exit too (F9) and when a superseded refill clips it
  (R64). If t alone plus G cannot fit C, K-3's synchronous fallback (R15, §5 K-3 [S2b-3]) takes the frame.
- **A refill keeps only what the ring lacks (F2).** At dispatch a refill is told the window times to keep: its
  window less the times the ring holds. Its decode is unchanged (R54).
- **A refill's t waits for its window (F4).** A reader does not dispatch a refill until every window time is
  reserved or resolved, so the frames it keeps are charged.
- **A retiring reader's kept frames stay with it (F3).** `discard_outside` does not move reservations onto a
  retiring reader after detention decided.
- **A stopped result is a failure (F6).** The read loop's stopped path drops the decoder's kept frames, as
  `stopped` takes them as gone (R62 item 7's rule).
- **Keeper affinity (R64, F8).** At a post a region holding a time a live (neither retiring nor detached) reader
  keeps decoded goes to that reader first; a reader two regions want takes the one with more of its kept times
  required, and the other goes to a free reader or waits. Fallback B: a time its live keeper cannot take leaves
  through a discard, its charge riding it (R57/R59), and is decoded again by the reader it went to, counted
  (`kept_handoff_redecode`). The same holds for a time a busy keeper's pending discard dropped at an earlier
  post: its decoder keeps it until it serves the discard, and a redecode elsewhere is counted (F12). A retiring or
  detached keeper's kept times stay with it (F3; D13).
- **The model is the gate for scheduler accounting (R62 item 9, R63).** `sched::accounting` checks I1–I6 after every
  operation of 3,000 seeded sequences in the fast tier and 300,000 in the slow tier.

**Amendment R65 [S2c] Equivalence for small counters (lead ruling, 2026-10-07).** R62's counter gate, and the
S3/S4 counter gates after it: a counter whose pre median is ≥ 20 a run passes when its median cand/pre ratio is
within 5%. One under 20 a run passes when the pooled per-run means differ by at most max(1, 25% of pre's mean) and
cand's per-run range overlaps pre's. A counter that must be 0 stays 0. Under it the R62 gate passed (E13.6.12). F10,
F11 and F12 are accepted as same-rule fixes; D15 is deferred to S3.

**Amendment R66 [S2c] The one charge rule on three more paths (lead ruling, 2026-10-07).** No R53–R65 semantics
change except as stated here (E13.6.13).
- **Admission reserves a kept time at its kept charge (F13).** A missing required time that a reader still keeps
  decoded is reserved at max(charge, d), as K-3 counts it. R61's fit of a continued window counts every frame the
  same way, the job's own and the continuation's, so a continuation never fits that admission cannot admit.
- **A dispatched discard keeps its times until it is served (F14).** A reader's discard-only job keeps the kept
  times it drops (`dispatched`) until the reader is back from it, and on close, failure and stop they go; a post
  made meanwhile hands them off (B), and their redecode is counted. A refill's result clips the times a pending
  discard drops to what its decoder kept, and a reader retired while it refilled keeps only what its decoder kept.
  A retiring reader's failed result keeps nothing.
- **A panic exit releases kept charges (F15).** `fail_start`, which a reader's panic exit calls (R43), releases its
  kept times through `unkept`, as `exited` does (F9).
- **The model's oracle is its own (item 4).** I1's charges and I3's H come from the model's records (the windows it
  holds, f, d, and which reader keeps which time), never from the scheduler's helpers. The exact K-1 check's only
  exemptions are D13's kept-but-not-required gap, which ends when a plan requires the time again and admission
  reserves it, and F3's retiring reader.

**Amendment R67 [S2c] The windows-filled rise is F13's (lead ruling, 2026-10-07).** At R66's re-run of the R62 gate,
`seek_gop60`'s windows filled rose: cand/pre 1.057 and 1.125, median 1.091; pooled, 55.67 a run against 51.83
(+7.4%). Every work counter stayed within R65's band (E13.6.13). The rise is accepted as F13's behaviour change, not
judged a divergence:
- F13 and its fit reserve a kept time at max(f, d) where the old code reserved f, so less room is left and more
  windows stop full; R66 predicted that direction before the run, and the old build's lower count was in part the
  under-charge F13 fixes;
- pre's own two rotations differ by 10.4% (medians 53 and 48), more than the shift.

The attribution is by direction only; no run isolates it. The gate is PASSED under R65 and R67 with the change
recorded. S4's pinned run re-checks windows filled, with R65's admission waits.

**Rec:** L-1m/L-2m and L-4b; S-3's counters (Amendment R54): backward hits served pre-converted against hits
that waited for a conversion, windows fully converted, and frames decoded per refill; R55's per-frame
conversion time (t and window, LL and LH), converter threads, a hit's wait split into conversion and render, and
CPU time per refill; R56's trace (test builds) of window conversions against steps and hits; `dropped_agent`, `stale_errors`, `slot_starved`,
`sync_decoders`, `device_latency_ms`, and RSS per workload; WARP VM baselines at
S0 and the ratio at S4 (ME14's absolute 20 fps floor stays **owed**, D6); the
PERFORMANCE heavy-4K, agent and desktop lanes.

## 12 Coordination

- **CC8 S2 lands after PF1 (R1)** and reruns
  `input_tables_match_every_accepted_descriptor` (X-2); a newly accepted
  descriptor is `Separable` only if it passes parity, otherwise `PerPixel`.
- **CC8 S3 rebases onto PF1 (R2).** Its HDR adapter is `PerPixel` and its
  HDR → SDR monitor intent stays fused f32 on the CPU readback path; G-2 is
  disabled unless monitoring is SDR BT.709; its transform digest joins
  `VideoSourceKey`, `TitleCacheKey` and `ConversionKey`. Any GPU HDR path is
  S3's, under R22. CE9 is untouched.
- **AW1** keeps the CPU path (B5 constructs no engine); **AW2**: W-2, D10.

## 13 Stages, budgets and gates (R8, R13)

Lines are production source lines, harness and tests included (N19). S2b's
four commits each land alone with their must-pass green. Before S2b-2, readers
open with ⌊P / R⌋ threads; before S2b-3, each ring holds the S1d window.

| Stage | Scope | Must pass | Budget |
|---|---|---|---|
| S0 | Harness: `perf_fixtures`, W-1, P-play/P-seek/P-rss on today's APIs, clock-stall metric, V-4 consumer metrics, V-5 driver, `process_memory()`, controls, baseline run (WARP VM by hand) | I4; baselines recorded; every Q-3 control fails its metric | ~670 |
| S1 | Exact fixes, each commit landable alone: S1a `collect` removal (control); S1b X-1…X-5 incl. live table counter; S1c G-1; S1d K-6 preview window cap + distance eviction; S1e U-1 (conditional; moved to S3b by R28); S1f V-1/V-2 + `stop_at_end`; S1g P-1 | I1, C-5, C-3, I3, I9; G3, G9, G12, G5 | ~650 |
| S2a | Preview thread with its synchronous renderer, R-1…R-5 (core `FrameStamp`, `PreviewFrame`, `StampedError`, `stats`, `ack_presented`; `Coalesced`; 12 implementors), app `finalize_preview`, paint marker and `App::logic` acks, agent lane, S-1, V-3 | I8, I18, I13 + S1's; G10, G11, G16, L-6 | ~1,150 (core ~150, app ~260) |
| S2b-1 | `Sched`, readers, H-2/H-3/H-4/H-6, plan versions, quiescence retirement, model | I15 (model) + earlier | ~450 |
| S2b-2 | `Permits` (H-5, R19), tickets, `shrink`, close-before-growth, P witnesses | I11 + earlier; G13, G18 | ~200 |
| S2b-3 | K-1…K-5 admission, `draining`, K-3 fallback, eviction | I12 + earlier; G17 | ~250 |
| S2b-4 | H-7 idle, synchronous-decoder release on park, stress test, playback gates | I10, I15 (stress) + earlier; G1, G6, G14, G15 | ~150 |
| S2c | S-2 shadow-anchored continuation, S-3 backward window, witnesses | C-4 + earlier; G8 | ~560 |
| S3a | G-2…G-7 display correctness: encode, fence, rebind rule, bounded wait, Terminal handoff, flag readback, lifetimes, self-check, premultiply, app registration | I1b, I5, I6, I16 + earlier; G7a, G7b | ~950 |
| S3b | Staging ring, U-2 residency | I17 + earlier; G2, G4 | ~450 |
| S4 | `PF1_PINS`, evidence, docs, PERFORMANCE lanes; the GPU state pinned and recorded | all; on LH at the pinned run: G3 (R48) and L-4a (R58); D13's active RSS scenario (RSS during window cancellation, R61) | ~80 |

The total is about 5,560 lines (rev 3: 5,830): R15/R16 remove ≈ 450 from S2b;
the paint marker, Terminal handoff and shadow-seek anchor add ≈ 180.
*Per commit:* workspace build, clippy `-D warnings`, fmt, the affected crates'
tests (media never with `--test-threads=2`), `cargo build -p kinewright-app`
when core or app changes. *Per stage:* full workspace test; the Must-pass
column of that and every earlier stage; LL and LH runs; AW1 gates; one critic
and two Astra reviews (S2b once, after S2b-4), with race and kill tests on
Opus. *CI on push*, Windows included.

## 14 Errors and incidents

- **E-1 [S2a] Typed errors pass through unchanged** (`SourceColorForAsset`,
  `UnsupportedDecoderFormat`, `NonFiniteRender`, every variant). A transport
  job's failure is stamped and reaches `Worker::fail` as today, when a
  **required** frame fails, only if current and not superseded (R-2). A
  lookahead failure surfaces only if its (source, time) becomes required in the
  current plan version (H-3). Agent job errors return only via their reply.
- **E-2 [S2a/S3a] Only genuinely new failures are prefixed.** Thread spawn
  failure, agent queue full and display enable failure are `MediaError::Backend`
  with `preview-thread:`, `decode-reader:` or `display:` (`BackendUnclassified`),
  each asserted by a test. A display enable failure falls back to the CPU path
  (G-6) without an event. `MediaError::Cancelled` is internal, never surfaced.

## 15 Deferrals

| # | Deferred | Why | Owner | Revisit when |
|---|---|---|---|---|
| D1 | Hardware decode (hwaccel, software fallback) | Here, for 8-bit H.264, NVDEC and Vulkan decode were slower than 16 frame threads (T2; commands and memory domain in E2). After S1 decode is not the bottleneck; zero-copy needs wgpu interop. **Not** a general claim about hosts or codecs | HW1, after CC8 S3 | CPU saturation with ≥ 4 simultaneous 4K or 10-bit HEVC sources; HEVC/10-bit footage in the session |
| D2 | GPU YUV → RGB | swscale identity cannot be guaranteed today and is costly to establish; swscale costs 0.8 ms | HW1 | the D1 trigger |
| D3 | Parallel export, delivery-encode tables | export already gains X-1, K-3 and K-6 exactly | export-performance slice | export slower than 2× real time |
| D4 | Proxy media; seeks at GOP > 250 | media-management feature | media backlog | such a source in the session |
| D5 | Playback stats as an MCP tool | needs an AW design | AW programme | an agent workflow needing live health |
| D6 | ME14's absolute WARP 20 fps floor (owed) | WARP's passes dominate | lead with Riel, VM | the VM run |
| D7 | Reverse and > 1× shuttle | not needed for the three videos | MO backlog | a session request |
| D8 | Long-edge decode cap for portrait sources in landscape documents | changes pixels; needs a per-layer argument | MO backlog | such a source in W-1 or the session |
| D9 | Device output-latency compensation; an output-drain acknowledgment, so the terminal stop waits for the last sample to sound (V-2's drained predicate is the ring's) | AU-owned clock semantics | AU backlog | recorded `device_latency_ms` > 1 frame, or an audibly clipped programme end |
| D10 | Native SVG raster gates; cached external-render fixture | AW2 owns code and external clips (A1) | AW2 | AW2 design |
| D11 | Bounded streaming, row banding and a lazy `decoded_layers` for required sets > C (preview or full resolution) | R15: kept out of the scheduler core; K-3 falls back to today's path | export-performance slice (D3 owner) | G17 fails, or a full-resolution set exceeds C in the session |
| D12 | The working-frame step (RGBA64 to the working frame, after swscale) | Amendment R56: 3.6–5.2 ms a frame, about 80% of a conversion (E13.6.7). After R57's row-wise fill (E13.6.9, r57w): t 1.87–2.61 ms and a window frame 2.11–3.31 ms (×0.49–0.72), against whole conversions of 2.9–3.2 ms on LL and 3.2–4.2 ms on LH, so it is still most of a conversion. t, window frames and playback all pay it. Our own Rust code, not swscale | S3/S4 | S3/S4 planning |
| D13 | Charging a cancelled window's decoded frames until the reader drops them | Amendment R56: after a post outside a held window, that window's kept decoded frames stay in its reader until the reader's next decode or close, uncharged. Bounded: one window per reader, at most B − 1 = 15 decoded frames. Charging them could deadlock admission | S4 | S4's G15 RSS check, plus an active scenario (R61 item 11): RSS sampled *during* cancellation, not only at settled idle. A P-seek-style backward stepper on a 1080p GOP-60 source refills a window, then posts outside it (a jump far back, then forward) before the window converts, repeatedly, with RSS sampled every 100 ms. The peak RSS above the idle baseline is recorded against the bound (one window per reader, B − 1 decoded frames at d each) |
| D14 | Extra decodes after a superseded refill | Amendment R62 item 3: after a refill superseded mid-decode (R61's version check), the newer plan's continued times below the decoder's `kept_from` stay required though no reader keeps them, so the reader decodes them again, with one seek. Bounded: at most B − 1 = 15 required times plus the GOP pre-roll, once per superseded refill; their reservations hold their charge until each converts | S3 | the S3 backlog: a refill completion that hands the newer plan what the decoder kept |
| D15 | The exhaustive reader-model test's fast-tier time | Amendment R65: `sched::tests::the_reader_model_holds_for_every_short_sequence` (S2b, `72246d3`) takes about 120 s in the fast tier. It predates S2c and stays as it is until S3 | S3 | the S3 backlog: move it to the slow tier, or shrink it |
