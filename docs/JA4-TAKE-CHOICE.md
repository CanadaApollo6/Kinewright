# JA-4: script alignment and durable take choice

Status: **design accepted 2026-10-20** (revision 2). Implementation has not started. Every runtime,
transcription, sync, resource, native Windows, connector and human gate is NOT RUN. Numerical limits
are proposed targets until measurements replace them.

Part of job A (talking head) in the [product brief](PRODUCT-BRIEF-2026-10.md#programme-mapping).
Its records, receipts and occurrence lineage belong to the [JA-1 Studio host](JA1-STUDIO-ORCHESTRATION.md).

Review trail, under `target/review/ja/`:
- Astra critic: `ja4-take-choice-critic.md` (revise; 5 Major, 1 Minor).
- Round 2 on rev2: `ja4-take-choice-critic-r2.md` (accept; all six closed, no new findings).

Binding inputs are `task-ja4-rev2.md`, `task-ja-common.md`, `task-ja4-take-choice-design-draft.md`, and `target/review/PITFALLS.md`. Read with them: the complete
October product brief, JA-4 rev1 and critic, JA-3 §§2–3, E1 and §§10–13, transcription/assembly/caption sources, and JA-1 revision 3. **Accepted
`docs/JA1-STUDIO-ORCHESTRATION.md` is the persistence and lineage authority.** It supersedes JA-3's older JA-1 references and the draft's pre-promotion loudness
row. Bare review filenames below are under `target/review/ja/`. Repository claims cite source locations or document sections. Missing behavior deduced from source
is labelled inferred. All six critic findings and recommended answers Q1–Q5 are accepted.

## 1. Outcome, scope and ownership

Riel imports a script and freely recorded talking-head footage, including false starts, repeats, and lines recorded out of order. The editor produces one complete
intended delivery per line, in script order, as ordinary editable clips. Missing speech stays missing. The first job is the tight-end draft-value video with
Songbird graphics. Actual script, exports, and practice recordings are pending inputs, not invented fixture contents (`docs/PRODUCT-BRIEF-2026-10.md`, "The first
video"; §15.2 below).

JA-4 owns script identities, complete transcript-path discovery, eligibility evidence, durable take choices, pins, pause protection, and verified
separate-mic/camera mappings. Deterministic code establishes eligibility; the editor agent chooses among eligible performances and records a short reason. JA-1 owns
records, writer authority, history, branches, occurrence lineage, feedback, ClaimBinding, and delivery. JA-3 owns editorial consumers, charts, captions, look, and
audio. JA-2 provides optional face geometry. No second store, writer, scheduler, merge engine, or global Core clip-ID scheme is added (JA-1 §§1, 4–9; JA-3 §§2–3).

Non-goals are forced acoustic alignment, semantic paraphrase selection, word splicing across takes, learned prosody/emotion/gaze, diarization, multicam,
fractional-rate drift correction, and discontinuity repair. No new inference model, bundled Node/Python, AI images/video, voice synthesis, or Tesseract content. The
product's 30–60 minutes measures Riel's time, not analysis latency. Job A and pending H-T retain their existing order (`docs/PRODUCT-BRIEF-2026-10.md`, "Budgets";
`docs/ROADMAP-AND-WORKFLOWS.md:892–910`).

## 2. Person-visible workflow

1. Import the script, recordings, and supplied facts. Show a short plan to match speech, choose clean takes, tighten the talk, and add graphics/captions. No
   mandatory picker or take-selection interview.
2. Work unattended. Progress distinguishes completed transcript coverage, partial search, uncertain wording, delivery review, missing material, and sync readiness.
   The agent handles ordinary issues from evidence.
3. Watch the reviewed cut in Studio. Optional line details show "Take 2", source preview, alternatives, and a reason such as "Complete wording; cleaner ending than
   take 1".
4. "Use take 1 for this line" addresses the displayed version and exact line/candidate. A successor edit pins that choice and replans its dependency closure. Source
   audition alone never changes the cut.
5. Reopening and unrelated revisions preserve the pin. Missing evidence or ambiguous script correspondence leaves the original pin unresolved, with its context
   available. The agent never silently substitutes.

After complete search with no exact path, show "No complete matching transcript evidence" and the missing or differing words. This is not a claim that the
microphone never recorded them. A complete delivery with an ambiguous long pause instead shows "Delivery needs review". Search exhaustion says "Search incomplete".
Labelled drafts remain inspectable. Required unresolved speech, sync, fact, or review evidence blocks the affected render or delivery as specified below. Only
indispensable absent material joins JA-1's one batched intake prompt (JA-1 §§3, 9–10, 14.1; product brief, "How it works").

## 3. Existing foundation and the additions

| Inspected foundation | Consequence |
| --- | --- |
| `docs/TRANSCRIPTION.md:3–37`; `crates/kinewright-media/src/transcript.rs:195–249` | Reuse pinned CPU Whisper small and the cache before inference. A cache miss currently decodes the whole source. |
| `crates/kinewright-core/src/media.rs:1603–1624`; `transcript.rs:339–381,469–496` | Words have text, source-frame bounds, and optional speaker. Fallback timing exists. Durable WordIds, timing provenance, and acoustic features are additions. |
| `crates/kinewright-agent/src/server.rs:7693–7708,7771–7803` | Media search uses bounded name/word substring hits. Inferred: it cannot certify complete script-line wording or retrieval. |
| `server.rs:8564–8667,14307–14339` | Assembly accepts ordered per-occurrence envelopes and repeated assets. Inferred: it consumes choices rather than selecting retakes. |
| `server.rs:8632–8649,14363–14379,14564–14645` | Ordered `selections` already contains `asset_id` and `kept_source_ranges`. Reuse this cleanup/result; add associations, protection, cumulative boundaries, and paired routing. |
| `crates/kinewright-core/src/captions.rs:336–385` | Script-authored captions redistribute words over existing cue timing. Inferred: they cannot certify spoken wording. |
| `crates/kinewright-media/src/audio.rs:4651–4666,4783–4828`; `decode.rs:1947–1949,2662–2673` | Audio and video subtract their own stream origins. Dense PCM does not certify internal timestamp continuity. |
| `crates/kinewright-core/src/operation.rs:165–180,236–243,2696–2704` | Same-asset patching links routes with one edit. Link-follow closure belongs to the caller's batch. Separate assets need one compound adapter. |

Throughout this document, unqualified `server.rs` is under `crates/kinewright-agent/src/`; `transcript.rs`, `audio.rs`, and `decode.rs` are under
`crates/kinewright-media/src/`. These are source observations, not runtime acceptance. JA-1's bundle is accepted design work, not landed persistence (JA-1 status
and §§4–6; `crates/kinewright-project/src/project.rs:240–245,406–412`).

## 4. Lossless script revisions, lines and beats

### 4.1 Speech and cues

Own exact UTF-8 input bytes, retained BOM, encoding, and SHA-256 through JA-1's input manifest. Reject invalid encoding without lossy replacement. Store parser
version and raw-byte spans for derived material. Markdown soft wraps become speech spaces. Paragraphs, list items, and hard breaks delimit beats. Pinned English
sentence rules handle abbreviations/decimals and delimit speech lines within beats. Plain text uses paragraphs and explicit line breaks. A long sentence remains one
logical line.

Parse escapes, fenced/indented code, inline code, Markdown links, quoted spans, and then cue brackets. Inline code and quotations remain literal speech, including
`a[0]` and quoted `[chart: literal]`. Link labels remain speech and URLs remain source metadata. Code blocks retain a needs-role issue until the agent classifies
them as quoted speech or supplied card/code material. Never silently discard them. Outside literal contexts, standalone `[chart: ...]`, `[card: ...]`, `[pause]`,
and `[pause: duration]` are nonspoken cues. A bare pause preserves the recorded hold. An explicit duration requests a hold, not narration. Store cue anchors
before/after speech tokens or between beats, including cue-only beats.

Unknown, nested, or unbalanced brackets and ambiguous quote/link parses retain both interpretations and exact spans as `needs_classification`. The agent records
classification against the script hash. Unresolved spans block affected lines. No regex strips arbitrary brackets. Cue instructions never enter spoken targets or
caption text. Holds without usable source media remain unresolved (§9).

### 4.2 Correspondence requires unique evidence

`ScriptRevision` contains parent, bytes, parser/profile, classifications, and ordered allocated `BeatId` and `LineId` records. IDs are identities, not text hashes
or row numbers. Lines retain speech, token IDs, raw spans, and cue anchors. Only Markdown soft-wrap whitespace is normalized for unchanged-text comparison.

An automatic old/new correspondence is allowed only when it is unique under the recorded revision evidence. Unchanged unique text, uniquely identifying context, or
a structured edit naming the surviving LineId may establish it. Order or proximity alone never breaks an indistinguishable duplicate tie. For old `A, X₁, X₂, B` and
new `A, X, B`, raw bytes cannot establish which X survived. Both possible histories remain represented as an unresolved correspondence unless structured provenance
names it. Inserting X before a pinned X, duplicate moves, changed cue/beat context, and ambiguous splits/joins receive the same test. A cue-only change preserves a
uniquely identified speech line but invalidates dependent pause/cue validation. It cannot establish an otherwise ambiguous duplicate correspondence.

Preserve unambiguous unchanged LineIds, including uniquely evidenced moves. Changed speech gets a successor ID and ancestry. Splits/joins retain explicit ancestry
without automatic pin transfer. An unresolved pin keeps its original LineId, BeatId, script revision, candidate, source words, cue/context, and displayed head. Keep
it reachable even when that line is absent from the new script. The lead inspects the retained old/new context and records an addressed resolution through JA-1's
existing command/receipt path (§7). Do not build a second store or general merge engine. Outline-only beats cannot authorize invented speech.

## 5. Complete exact discovery and separate approximate diagnostics

### 5.1 Normalized eligibility language

The complete-discovery pass is a CPU-only streaming matcher over **every declared transcript**, independently of recording order or script order. It accepts every
path consisting of the whole script line's exact normalized words plus permitted isolated non-script `um`/`uh` insertions. Anchors and windows do not control
eligibility retrieval. Approximate DP supplies explanations only (§5.4).

Retain a reversible normalized-token-to-original-WordId/byte map. Case folding, typographic apostrophes, and nonsemantic punctuation normalization cannot erase
negation, signs, decimals, units, or comparisons. A pinned finite English number grammar gives unambiguous written/spoken forms the same exact value/unit
representation. `18%` and "eighteen percent" may match. "Eighty percent", "eighteen points", and ambiguous "one twenty" cannot. Names/acronyms require exact
normalized words or source-supplied versioned aliases. No fuzzy names, automatic `for`→`four`, or critical-token edit tolerance. All lexical tokens are required.

Implement normalization as bounded, deterministic token transitions with original word spans. Number/alias expansions retain every consumed word, filler, and
critical-value check. Never discard fillers globally: authored "um" is required. Ambiguous number parsing is an explicit issue, not an arbitrary accepted value. A
permitted filler is one isolated `um` or `uh` between required speech tokens, flanked by required speech, with no consecutive filler insertions. No leading/trailing
substantive speech can enter the envelope. When a token can be authored speech or filler, keep both legal paths until full-line validation distinguishes them.
Equivalent paths may merge computation only if all distinct WordId associations remain recoverable.

### 5.2 Streaming state and completeness invariant

Start a line-match state at every transcript token boundary. A state names the line/profile, start word, next required token, filler permission, and predecessor
evidence. Consume a required exact transition or a permitted filler transition. Reject a path on substantive insertion, substitution, or omitted word. Emit every
accepting path with its complete original WordIds and interval. Continue after an acceptance so later takes and overlapping alternatives remain discoverable. A
silence changes no lexical state. No match is spliced across recordings, snapshots, substantive intervening speech, or a demonstrated restart.

Read transcripts in bounded chunks and carry active states, normalization state, and predecessor references across chunk boundaries. Exact matching operates on the
complete logical line, including long lines; the 96-token diagnostic unit is not an eligibility limit. No fixed overlap or anchor stride is needed. Canonical
ordering is manifest order, first/last word, then lexical path. Computation may share identical states, but output deduplication removes only identical full
word-association paths, not distinct deliveries.

Invariant: after processing each transcript prefix, the frontier represents every legal unfinished path that starts in that prefix, and every completed path in the
prefix has an emitted record. Initially the frontier is empty. At each boundary a new start covers all new paths. Advancing every legal transition preserves every
prior path, and emitting every acceptance preserves completed ones. Chunk transfer keeps the frontier unchanged. By induction, processing EOF for every snapshot
discovers every eligible lexical path in the declared scope. Acoustic/boundary validation may subsequently reject paths, but cannot recover paths hidden by
retrieval. This proves completeness for the stated language, not acoustic ASR recall.

### 5.3 Coverage, cumulative accounting and candidate identity

Freeze a scope manifest with script/profile hash, complete transcript/snapshot list, word counts, and declared ranges. Default scope is the whole of every declared
recording. Narrowing creates a new scope and is visible; completing a subset never certifies the omitted recordings. Missing snapshots are uncovered. Store
per-snapshot token frontier, completed line-set digest, active-state/normalizer checkpoint, emitted page refs, and cumulative counters through JA-1. Scope identity
cannot change under a resume cursor.

`coverage_complete` means every declared token boundary reached EOF with all line transitions processed, all accepting paths durably registered, and no unmatched
pending publication or quota/cancellation issue. Report validation-complete separately from discovery-complete. Only completed discovery may report that no lexical
path exists. A line with review-only paths is not "missing". A cancelled/paused pass remains incomplete.

Count exact transition evaluations as matcher cells and diagnostic recurrence evaluations as DP cells. Record both cumulative totals, unique covered tokens, actual
replay/recompute work, bytes written, elapsed active/wall time, and peak resident bytes. Resuming never resets the job's consumed work allowance or adds replayed
progress twice. Freeze cursor, counters, frontier, and emitted refs together at work boundaries. Bound slices to 200 million transition/DP evaluations each,
reported separately. A slice is scheduling, never completion. Yield after at most 65,536 evaluations and check cancellation at that boundary.

Exact candidate pages have their own reserved quota. Remove rev1's 64-delivery discovery cutoff: exact matches page until the admitted metadata quota is reached. At
that quota, stop with `search_incomplete`, retain cursor/evidence, and require an explicit scope/quota decision. Never silently evict, claim absence, or publish a
completed selection map. Near-misses consume no exact registry slots or reservation, and diagnostic saturation cannot stop a scan before a late valid take. Never
discard valid paths for ranking.

Candidate identity binds immutable snapshot, script line/text revision, complete word-association path, and interval. Stable "Take N" labels come from an initial
manifest/time-ordered registry, not ranking. Register exact paths first, then optionally merge separately bounded diagnostics before freezing initial labels. Later
discoveries append labels without renumbering. Diagnostic labels remain explicitly ineligible. A corrected snapshot produces successor evidence, never replacement
WordIds or retargeted labels.

### 5.4 Approximate DP explains failures

Only after exact discovery completes, use bounded semi-global affine-gap DP to explain partial lines, substitutions, restarts, or cue speech.
Prefix/suffix transcript costs are free, script ends are not. Match costs 0, substitution 6, gap-open 5, gap-extension 1, and isolated filler insertion 1. Ties use
fewer edits, shorter interval, source order, and lexical edit path. Costs are diagnostics, not confidence. Diagnostic windows have at most 256 source tokens and 96
script tokens, with at most 128 windows/line/slice. The cumulative diagnostic limit is 200 million DP cells/job, 64 stored near-misses/line, and 4 MiB/project
inside the metadata limit. Exhaustion returns `diagnostics_truncated` without changing exact coverage. No DP score can nominate an eligible candidate without the
independently complete exact-path guard.

## 6. Eligibility, long pauses and editorial choice

### 6.1 Hard validation and honest speech evidence

Require every intended lexical token in order exactly once, with no substitution or deletion. Numbers, names, units, negations, and comparison words receive the
full-token guard, never a shared-phrase waiver. Only the permitted filler insertions may be removed, and cleanup must retain every required WordId. Substantive
intervening speech or evidence of a real restart makes that attempted delivery partial/restarted. A later complete repeat may independently qualify. Never join a
prefix to another take's number.

Validate owned source/stream identity, audio availability, monotone nonempty word bounds, media bounds, and exact frame/sample conversion. Begin with 100 ms word
protection and inspect 250 ms outer context. Refine boundaries only from measured safe quiet spans. Overlapping speech, corrupt/silence-ASR timing, or insufficient
timing/context returns `boundary_review_required`. New transcript evidence records timing basis; legacy provenance remains `unknown`
(`crates/kinewright-media/src/transcript.rs:367–378,481–495`). Measure candidate RMS/peak in 20 ms windows with 10 ms hops, near-full-scale runs, silence
intersections, and boundary discontinuities under a pinned profile. Initially flag ≥3 samples at absolute value ≥0.999. This is a clipping proxy. A recorder clip
flag or unresolved boundary/clipping problem blocks automatic choice until repaired/revalidated. Keys include source hash, stream, range, sample rate, and profile.

Transcript-exact is **not heard-and-confirmed**. A disputed critical word requires a separate source-bound human or certified-audio observation naming words/value,
source interval, observer, and evidence capability. Keep original ASR and correction evidence. Script/Songbird agreement cannot certify what was said. Without that
observation, `critical_speech_unresolved` blocks the affected render. Optional model listening remains optional under JA-1 §10; no connector is certified by this
design/source review.

### 6.2 Silence preserves a complete delivery

An internal silence never splits a delivery by itself. Exact speech before and after an unprompted 2.2 s pause stays one complete candidate with retained WordIds
and measured `long_gap` evidence. Initially flag gaps over 2 s for pacing/context review. A previously authored pause cue protects an intentional hold. The existing
cleanup may shorten an unprotected hesitation only after word/boundary validation (§9). Actual restart evidence or intervening substantive speech makes the
attempted path partial. If context cannot distinguish a continuous delivery from a restart, return `delivery_review_required`, retaining the complete lexical path
and original words. Do not report missing words or require a pickup from silence alone.

### 6.3 The agent chooses among valid takes

Use JA-3 §3's priority order, without a weighted score: complete words/facts, usable restart/clipping/cut evidence, natural pauses/rate with fewer hesitations, then
measured delivery proxies. Report words/minute, pause durations, and hesitation counts. The initial 90–220 wpm flag prompts review, not rejection by taste. Windowed
RMS and rate variation are proxies, not confidence, emotion, or proof of better delivery. Report channel, normalization, windows, and unavailable measurements with
those proxies. The editor may prefer a quieter natural take. Record one short reason and evidence refs. Commit rechecks eligibility, pins, occupancy, and identity.
An unavailable agent leaves an unchosen deterministic proposal.

Inspect leading alternatives and chosen imagery through a certified image/clip connector (JA-1 §10.1). JA-2 geometry can help locate a face, but five landmarks at 5
Hz cannot certify gaze or eyes-open (`docs/JA2-PERSON-SEPARATION.md:487–495`). Source images/clips may support a sampled visual observation with exact times.
Missing geometry uses imagery; missing certified imagery leaves visual selection pending. No claim that the agent watched follows from landmarks or a path string.
Final JA-1 review stays mandatory. Reserve pins first. Two line occurrences cannot accidentally consume overlapping speech words. Repeated script lines require
distinct deliveries or separately authored explicit replay intent.

## 7. JA-1 records, pins, history and lineage

JA-1's project/store owner owns the coordinated v3 schema and publication seam. JA-4 supplies typed records inside that format, not a new file/store lifecycle (JA-1
§§4.1–4.3, 6, 7.3). Each acknowledged branch/head and history checkpoint references its exact `TakeMapRevision`, so undo/redo restores the matching map/pins and
consumer bindings. Delivered versions freeze that reference. Generic save cannot strip these records.

| Record | Required content |
| --- | --- |
| ScriptRevision | Bytes/hash, parent, parser/normalizer/classifications, ordered beats/lines/tokens/cues, and unique or unresolved identity correspondence. |
| TranscriptSnapshot | Source/stream/fps/model hashes, actual extraction/language/timing provenance or unknown, immutable words and original word hash. |
| CandidatePage / search checkpoint | Line/text/snapshot/profile key, exact paths and stable labels, bounds/issues/features, scope/frontier/cumulative counters. Diagnostics have a separate quota and coverage status. |
| TakeMapRevision | Parent/script, ordered choices/statuses/reasons, alternatives, actor/task, validation refs, and profile keys. |
| TakePin | Original line/beat/context, displayed VersionId/HeadId/comment, exact candidate/words/source, command, active/cleared/unresolved state, and any addressed resolution. |
| PauseIntent | Prior-authored cue/token/beat anchor, protected source gap or requested hold, duration/bounds, and provenance. |
| RecordingPair | Owned sources, selected streams/time bases/origins, decoder convention, A/V offset observation, fit/bounds, decoded coverage, all anchors, and admitted/refused ranges. |
| AssemblyBinding | Map/plan digest, head-scoped occurrences, retained WordIds/source-to-project segments, requested/applied cut errors, links, pauses, and consumer invalidations. |

`WordId = (TranscriptSnapshot hash, original word ordinal)`. Text equality is not identity. Corrections keep explicit successor links. Range identity includes
stream, rational fps, and half-open source-frame bounds; audio evidence additionally binds sample rate/indices and timestamp coverage. Cache deletion cannot erase
owned snapshots. Relink/retranscription invalidates dependent validation while retaining unresolved pins.

All mutation uses JA-1 auth, scoped authority, and expected branch/HeadId/map. Publish clips, history, map, pins, resolutions, lineage, consumers, and receipt
atomically through B1–B5. Receipt lookup authenticates and matches ID/digest before stale-head checks. Lost replies reconcile the same command, never repeat a
ripple. Retain references through crash recovery, Save As, merge, undo/redo, and fenced adoption. Unknown required kinds refuse writes/resume. JA-1 §§4–6 owns these
rules; this design does not reopen its host decision.

Pins override automatic taste and unrelated replans. An invalid requested candidate retains its request and blocker but cannot become valid. A duplicate ambiguity
retains original context without transferring the pin to a new line. The lead's addressed resolution names original pin, old/new revisions, surviving or successor
LineId, candidate disposition, and evidence. It commits through the same JA-1 command/receipt path. Clearing/replacing a pin needs an addressed instruction. A
resolved correspondence still revalidates wording, pause context, source integrity, and range eligibility before reuse.

Replacement creates fresh JA-1 occurrences with semantic line/beat ancestry. Retained clips keep occurrences; splits retain exact range ancestry. Core IDs are local
and reusable (`crates/kinewright-core/src/operation.rs:4425–4434`). Old feedback stays original. Temporal beat mapping and spatial-region validity are independent
after a take change. Undo/redo chooses the head's lineage map; merge/adoption cannot invent script correspondence (JA-1 §§4.2–4.3, 5.1, 7.3).

## 8. Separate QuadCast/C920 sync

### 8.1 Fit in audio clocks, then convert into video coordinates

Use selected QuadCast audio for speech and selected C920 scratch audio for correlation. RecordingPair binds both owned hashes, selected stream indices, rational
stream time bases, raw start PTS and their known/unknown status, actual decoder origins, resampling delay/priming, timestamp fallback behavior, and decoder/profile
identity. Verify that playback/planning uses those streams. If current best-stream selection cannot bind the requested index, refuse that selection until the narrow
adapter exists. Same-file A/V needs the same evidence.

Let `m` be seconds relative to the mic decoder's recorded audio origin. Transcript frames convert to this clock using the snapshot's exact fps and extraction
convention. Let `c_a` be seconds relative to the camera scratch-audio decoder origin. The fit returns **camera audio-relative time**, `c_a = a + b*m`. It returns
neither container time nor video-relative time. Mic container start does not become camera start.

For the verified normalized-clock case, let `O_a = start_pts_a * time_base_a` and `O_v = start_pts_v * time_base_v`, both exact seconds in the camera container's
common epoch. Then:

```text
c_v(m) = a + b*m + (O_a - O_v) + delta_AV
video_source_frame = decoder_grid_ceil(c_v(m) * video_fps)
```

`delta_AV` is a separately evidenced capture offset, defined positive when the corresponding visual event needs a later video timestamp than the matching
scratch-audio event. Correctly muxed/verified capture may establish zero. The correlation fit cannot infer this offset because it sees only audio. Unknown is never
zero. Apply documented decoder delay/origin corrections before this formula, or return `sync_timing_unresolved` for unsupported conventions. Frame selection uses
the actual decoder grid/decoded PTS receipt, including rounding, not ideal nominal-frame arithmetic (`audio.rs:4651–4666,4783–4788`; `decode.rs:2662–2673`).

If camera audio starts 200 ms after video, zero audio-relative time maps to video-relative 200 ms before separate capture correction. At 30 fps the origin term is
positive six frames, not negative six or zero. Unknown origins, synthetic missing-PTS fallback, unsupported edit lists/resampling conventions, or unverified source
A/V alignment leave only the affected pair/ranges unresolved. Never silently substitute camera audio.

### 8.2 Correlation and measured timestamp coverage

Decode bounded mono windows at 8 kHz through the actual source decoder. Start with at most nine speech-rich 8 s anchors across overlap. Search offset within ±120 s;
a lead-scoped retry may extend to ±600 s under the same memory limit. Recording timestamps seed search only. Use 20 Hz log-RMS for coarse search, refine three
separated lags at 2 kHz within ±100 ms, then at 8 kHz within ±2 ms, with pinned filters/reductions. No whole-recording PCM or new model is needed.

Initial profile requires normalized waveform peak ≥0.60, runner-up margin ≥0.10 outside ±50 ms, at least three consistent anchors, |drift| ≤1,000 ppm, and inlier
residual ≤15 ms. These are provisional admission thresholds, not measured results or an interior error bound. Fit deterministic median pairwise slope/offset; retain
every measured anchor, lag interval, rejected anchor, and rejection reason. G8 must calibrate the profile.

Extend the existing decoder observation/result path to carry decoded source PTS intervals, output-sample mapping, overlap/gaps, padding, unavailable regions, and
priming/rounding for all selected audio/video ranges and analysis windows. Stream these receipts with bounded coalescing. The dense `Vec<f32>` currently
concatenates/resizes data and omits later internal gap insertion (`audio.rs:98–105,4807–4828`). Its length, nonzero samples, source hash, or successful endpoints
cannot certify continuous source-time coverage.

For the first video, **refuse discontinuous or unverified selected ranges before paired commit**. Do not compact, pad, interpolate, partition around, or repair a
discontinuity to make a pair pass. Missing PTS, unknown padding, and insufficient coverage are scoped `sync_coverage_unresolved`. A rejected/inconsistent anchor
inside a selected range invalidates it until an evidenced explanation and remeasurement establish coverage. Robust fitting cannot discard interior evidence and
still certify that range.

### 8.3 Conservative range admission and offset-only cuts

Continuity of timestamps is necessary but insufficient for sync. Require local lag evidence through each selected range, including endpoints and interior. Use
overlapping local windows up to 2 s, extending to 8 s for sparse speech only when the unchecked-span bound still holds. Store actual evidence spacing. Require
anchors bracketing both edges. The initial maximum interval between local evidence centers is 2 s, so any point is at most 1 s from an anchor. Shorter intervals may
be necessary. Unsupported silence/ambiguous windows create uncovered spans; actual edge distances count toward admission, with no assumed extrapolation.

Admission needs an explicit supported error model: continuous clocks with bounded affine-estimate error and a justified bound `K` on residual lag change per second
between verified local observations. A fitted residual or largest observed slope is not proof of that bound. Establish it from source-bound capture/clock evidence
or calibrated independently observed controls applicable to the recording, and retain assumptions. If no applicable bound or sufficiently dense evidence exists,
refuse the range. Do not claim protection against arbitrary hidden lag excursions from three anchors or a single slate.

For a kept interval of duration `L`, midpoint `q`, and fit reference `m0`, apply the constant local offset `a + (b-1)*q`, then the origin/capture conversion in
§8.1, at 100% speed. A conservative time-error bound is:

```text
U_offset(q) = U_fit0 + U_b * abs(q - m0)
E <= U_offset(q) + (abs(b - 1) + U_b) * L/2
     + U_anchor + K * h + U_clock + R_actual
```

`U_fit0` bounds fitted camera time at reference `m0`, and `U_b` bounds slope. Derive both from measured lag intervals and their admissible parameter ranges, not
standard error alone. `a` remains the derived intercept at mic time zero. `U_anchor` bounds local departure from the fit at verified anchors. `h` is the maximum
unchecked distance to local evidence, including edges. `K*h` bounds unobserved residual variation. `U_clock` includes stream-origin/capture-offset uncertainty and
unaccounted decoder/resampler delay. `R_actual` conservatively sums actual mic/sample, camera-frame, and cumulative project-boundary rounding. Record components and
exact applied bounds. Unknown or unbounded components force unresolved admission.

Require `E ≤ 1/project_fps` everywhere in the admitted interval and actual media bounds on both routes. The `L/2` term is drift over half an offset-only segment,
not a residual from the global fit. Long independent clocks cannot pass from one slate. A supplied slate/manual observation supplies offset evidence only unless its
accompanying drift/interior coverage also supports the full interval. Do not extrapolate coverage from it.

If verified continuous speech has too much representable drift, split only at an existing validated safe silence and recheck each resulting ordinary cut. This
handles bounded smooth drift, not discontinuity repair. Otherwise return `sync_drift_unrepresentable`. Keep affine evidence but apply offset-only clips; no hidden
time stretch or ppm-to-integer-speed approximation (`crates/kinewright-core/src/model.rs:473–483`). Independent decoded flash/clap/mouth landmarks test final
sign/error through the real paired planner and decoder. Output compared only with its own fitted map is not an independent oracle. JA-1 §10 output review remains
separate from source sync admission.

## 9. One cleanup result, protected words/pauses and atomic assembly

Feed selected mic occurrences in script order with exact envelopes to the existing assembly path. Repeated asset IDs remain distinguishable by input occurrence
order. Reuse `dialogue_keep_ranges` and its silence and filler analysis once per envelope. A shared typed internal result carries the existing ordered source kept
ranges plus optional durable line/candidate/WordId associations and pause evidence. Today's public assembly still serializes its `selections`; the paired planner
consumes the same typed result. No second cleanup computation or public kept-range service is added (`server.rs:8593–8649,14363–14379,14564–14645`).

New work is limited to durable associations, protected pause/word inputs, cumulative-boundary handling, and the separate-asset atomic adapter. Convert JA-3's
proposed 400 ms silence threshold and 100 ms retention with exact source fps, not fixed frame counts (JA-3 §4; `server.rs:8578–8585`). Protected required words
include authored fillers. Only permitted non-script fillers may be cut. Subtract word and PauseIntent protection from the final cut set after cut merging, so
merging cannot bridge a protected interval. Revalidate exact retained WordIds, margins, and decoded cut boundaries before applying.

Thus an unprotected 2.2 s hesitation may shorten without losing speech, while the identical pre-authored hold remains. Preserve actual holds. Longer requested holds
require ordinary explicit picture-hold/room-tone edits from JA-3 with usable evidence and review. They never manufacture words or become post-QA waivers. An unsafe
cut remains a review/refusal, not an excuse to mark complete speech missing.

For each exact kept range, consume accepted RecordingPair coverage once. Derive one shared project interval and prepare video-only C920 plus audio-only QuadCast
clips, linked in one compound ordinary-operation plan. Scratch audio is unrouted. Include complete ripple/overwrite/link closure on affected tracks. Never commit
mic first or run two independent insert planners. Same-asset dual-route patching does not represent two assets
(`crates/kinewright-core/src/operation.rs:165–180,236–243,2696–2704`).

Accumulate rational durations first and round shared cumulative project boundaries once. Validate actual representable source bounds/durations and protection on
both routes. Safe-margin adjustment is allowed only if revalidated and recorded; otherwise preparation refuses. No frame-per-line accumulation. Record requested and
applied bounds, rounding, A/V mapping, and downstream movement. One atomic commit and undo entry includes the paired clips, links, map, and consumer bindings. Every
sync refusal precedes that commit.

Replacement touches the addressed line's closure, adjacent cut/fade/pause boundaries, and dependent overlays. Shift later placements once if duration changes.
Preserve unaffected choices/pins/occurrences. Recheck head, map, and authority at commit. Captions consume retained words and script speech only after eligibility;
count/timing mismatches block or resegment rather than stretch a whole script over unmatched speech. Facts consume script, actual ASR, source-bound observations,
and selected ranges separately. Reuse JA-1 §9's Songbird v1 dataset/scope/completeness and ClaimBinding rules. A correct caption/chart cannot mask wrong spoken
words. Unresolved critical speech blocks affected renders. Every choice change invalidates caption, fact, and output-review keys, including equal-duration swaps.
Feedback/reels inherit explicit beat/occurrence ancestry and their own review. No new fact schema or truth detector is introduced.

## 10. Bounded agent capabilities

Use JA-1's existing compact registry, authorization, prepared-plan commit, and command envelopes (JA-1 §8). Capability names below are proposals, not an additional
unguarded mutation API.

| Capability | Inputs and bounded result |
| --- | --- |
| `ingest_script` | Owned bytes, parent revision, expected head, optional structured identity edit; returns revision, preserved/new/conflicted IDs and issue cursor. |
| `align_script_takes` | Frozen scope/script/snapshots/profile; returns job/progress or map proposal, cumulative counters, exact coverage, review/missing states, and pages. |
| `inspect_take_choices` | Exact map/version and line/beat IDs; returns stable labels, words, reasons, pin/context, diagnostics, evidence limits, and source-preview refs. |
| `analyze_recording_pair` | Owned streams, search scope, timing/capture evidence/profile; returns fit, clock conversion, coverage/bounds, accepted ranges, or scoped failure. |
| `plan_take_choices` | Expected head/map, up to 32 addressed choices/reasons, pin actions/resolutions, pause refs; returns one opaque plan, A/V closure, shifts, and invalidations. |
| Existing plan commit | Atomically commits through JA-1 and returns durable receipt/head/map. Delivered still requires JA-1 review/sealing. |

Analysis uses JA-1 reserve→write→seal→pin→retire, scoped jobs, and owner publication. Inspect at most 16 lines, eight expanded alternatives/line, and 128
words/page; cap JSON at 64 KiB. Identity-bound cursors expose more without dumping transcripts/PCM into chat. Replies bind actual head/map/script/profile/source
identities. States include `needs_classification`, `search_incomplete`, `diagnostics_truncated`, `word_mismatch`, `no_matching_transcript_evidence`,
`delivery_review_required`, `boundary_review_required`, `critical_speech_unresolved`, `pin_unresolved`, `sync_timing_unresolved`, `sync_coverage_unresolved`,
`sync_ambiguous`, `sync_drift_unrepresentable`, `stale_head`, `quota_exceeded`, and JA-1 durability states. Never retarget "latest".

## 11. Studio details and feedback

Add collapsible "Script and takes" details grouped by beat in JA-1's plan/details view. Default rows show speech, selected take, and one reason. Expanded rows show
original script/cues, eligibility/evidence basis, coverage, source preview, alternatives in stable label order, and active/unresolved pins with old context.
Long-gap review is distinct from missing wording. Search/diagnostic coverage labels remain separate.

Preview exact immutable source/sync evidence or version with one active player. Return restores the prior view. Chat and "Use this take" send the same addressed
command. An old-version request retains its candidate identity and resolves lineage before replan; never reinterpret "take 1" as the first newly ranked result.
Ambiguous duplicates keep the bounded old/new context for lead resolution. Keyboard/text actions cover all choices. Show pending/saved/reviewing honestly; candidate
counts alone never produce a completion badge. Keep IO/analysis off the UI thread. JA-1 §§3, 7, 10 owns display identity and final review semantics.

## 12. Admission bounds and measured outputs

These are provisional targets/limits, **not results or whole-process RSS allowances**. JA-1 §12 owns aggregate accounting. Count transients and refuse/pause before
overflow. No quota clears a pin or drops an accepted path.

| Resource/scope | Initial target/limit and overflow |
| --- | --- |
| Script | 1 MiB UTF-8, 2,000 speech lines, 50,000 normalized tokens; refuse oversized ingest before parsing allocation. |
| Search manifest | 32 recordings, 120 recorded minutes, 100,000 original transcript words, 200,000 normalized source tokens; larger scopes need explicit batches with retained coverage. |
| Exact search | 200 million matcher evaluations/slice, cumulative counters never reset; yield/resume, never call a slice complete coverage. No 64-delivery cutoff. |
| Diagnostics | 200 million DP cells/job, 64 near-misses/line, 4 MiB/project reserved separately within metadata; mark diagnostics truncated on exhaustion. |
| Durable JA-4 metadata | 32 MiB/project within JA-1 quotas, pages ≤4 MiB, owner object cap applies; reserve exact pages first, pause incomplete before overflow. |
| Host JA-4 metadata | ≤16 MiB within JA-1's 64 MiB control cap; page reconstructible expansion, never evict durable pins/evidence. |
| Worker | One active job/one queued request, ≤128 MiB incremental buffers/frontier/predecessors/decode transients, no VRAM; quota refusal preserves incomplete coverage. |
| Warm exact alignment | Target ≤1 wall-second/recorded minute and first page ≤2 s, excluding ASR/IO/acoustic/visual work; measure full accepted workloads. |
| Features/sync | Targets ≤2 wall-seconds/selected minute for local features, ≤30 s/pair for initial global search; report source IO/decode-inclusive totals and full-coverage work separately. |
| UI/cancellation/payload | Warm details p95 ≤100 ms, analysis cancellation ≤100 ms at work boundaries, JSON ≤64 KiB, reason ≤256 bytes. Whisper cancellation remains safe-boundary. |

The search has no hidden total-work reset at each slice. Bound the whole job by the frozen input caps, resident/durable quotas, and JA-1's persisted run deadline;
hitting any bound yields explicit incomplete state. No automatic cap increase or scope reduction occurs. Script/snapshot/pages/counters use bounded grouped records.
Versions, histories, pins, and unresolved evidence retain their dependencies; owner-only GC handles unreachable objects. Reservation includes staging, next/previous
roots, retry state, and all live frontiers.

Reuse one owned transcript per source. Rewraps, reranking, pins, previews, and reopen cause no inference. Cache hashes do not establish missing language/extraction
provenance (`transcript.rs:195–208,625–675`). Explicit retranscription creates successor evidence and revalidates pins. The existing full-source 16 kHz mono f32
buffer alone is inferred to cost 3.84 MB/minute, excluding model/state (`transcript.rs:223–245`). The incremental worker bound neither includes nor fixes that
Whisper allocation. Meter real Whisper peak separately. Serialize heavy work through JA-1; no new daemon is added.

G11 runs the full Cartesian workload matrix, not samples or per-line extrapolation:

| Recorded minutes | Speech lines | Matcher cells / DP cells | Peak bytes | Coverage / latency |
| --- | --- | --- | --- | --- |
| 20 | 100 | NOT RUN; measured outputs required | NOT RUN | NOT RUN |
| 20 | 200 | NOT RUN; measured outputs required | NOT RUN | NOT RUN |
| 40 | 100 | NOT RUN; measured outputs required | NOT RUN | NOT RUN |
| 40 | 200 | NOT RUN; measured outputs required | NOT RUN | NOT RUN |

For each cold/warm lane retain input/binary/profile hashes, actual cumulative work, peak allocated/resident bytes, completed/declared tokens and snapshots, emitted
paths, validation coverage, p50/p95/full-job latency, and cancellation. Measure decode-inclusive sync, local features, reopen/override, CPU, process/child RSS, and
VRAM separately. Annotate other workstation load, preserve raw summaries, and use PITFALLS paired/serialized timing rules. No builds/tests overlap timing.
Linux/3090, native Windows, provider, and human lanes remain independent. Full workloads and every measured quantity above are pending, not asserted performance
passes.

## 13. Implementation stages by dependency

| Stage | Required dependency and result | Gates |
| --- | --- | --- |
| S0 contracts/fixtures | JA-1 owner freezes v3 map/history/input refs with JA-3 consumers, exact language, sync bounds, and compatible observers. Practice hash slots stay pending. | Prepare all witnesses. |
| S1 script/evidence | JA-1 S1 records/owned inputs; lossless parsing, conservative identity, frozen snapshots. | G1, G2, G7-storage. |
| S2 discovery/validation | S1; streaming exact matcher, separate diagnostic DP, acoustic boundaries and long-gap review. | G3–G5, G11-text. |
| S3 choice/persistence | S2 plus JA-1 S2 authority/history/receipts; choices, reasons, pins, exact head/map restoration. | G6, G7. |
| S4 sync/assembly | S3; stream/coverage receipts, admitted offset-only pairs, one shared cleanup result, pause/word protection, compound edit. | G8, G9, E1. |
| S5 Studio/consumers | S4, relevant JA-1 S3/S4, JA-3 captions/facts/QA, certified imagery; JA-2 geometry optional. | G10, G11-integrated; JA-1 G16/G17/G19 separate. |

Only mechanisms tied to these gates are in scope. JA-4 acceptance does not certify AW1, PF1/H-T, JA-2, graphics, native Windows durability, or the full practice
video's publishability.

## 14. Exit gates and discriminating witnesses

All deficiencies below are source-derived expected failures, **not observed reds**. Follow JA-1 §13.1's compatible observer rule: exercise current successful APIs
first, then assert missing semantics. A missing endpoint, compile/setup failure, unavailable media/provider, or disabled lane is NOT RUN, never a behavioral red.
Keep pending baseline seams explicit. During implementation retain literal baseline behavior, targeted mutation red, and restored green with identities/raw
summaries. Rerun only each mutation's affected witness.

**E1. Choose once, override once, S4.** Real frozen-word/transcription registration → aligner → existing assembly → JA-1 save/reopen. One recording has flub, clean
repeat, wrong statistic, later alternative, out-of-order line, and bracket cue. Baseline full-envelope assembly retains repeated speech; supplied correct envelopes
are the positive control. Assert one correct delivery per LineId, exact retained WordIds/source bounds, script order, and no cue speech. Pin alternate and
reopen/unrelated-edit; preserve it and change only its closure. Wrong-statistic/flub pin refuses without losing the valid pin. Mutate wording guard, pin precedence,
or map persistence. Synthetic text facts are test labels, never supplied football truth.

**G1. Lossless cues, S1.** Real ingest/record adapter with soft wraps, links, pause/chart cues, `a[0]`, quoted brackets, unknown/nested brackets, and code block.
Positive resolves literal speech and round-trips bytes. Baseline parser seam is pending. Mutate broad bracket stripping or cue-as-speech. Assert exact spans, token
inventory, ambiguity, and affected-line blocking rather than counts alone.

**G2. Unique correspondence, S1.** Real revision records with `A, X₁, X₂, B`. Delete first versus second X using indistinguishable raw revisions; both must retain
the original pin/context as unresolved. Insert X before a pinned X, change cue/beat context, ambiguously move duplicates, and split/join. Positive controls are a
unique unchanged moved/rewrapped line and a structured edit naming the exact survivor. Assert exact LineIds/BeatIds, ancestry, original candidate/WordIds, and pin
disposition. Repeat after reopen, undo/redo, and adoption with G6. Baseline identity seam pending. Mutate order-based duplicate choice or text-hash IDs.

**G3. Complete out-of-order discovery, S2.** Frozen-word input through the real matcher. Include C, A-flub, B, A-clean, C-repeat, and a long complete line. Add
critic counterexample 1: each selected multiword anchor is broken by isolated fillers in the valid take, while the exact anchors occur in a distant wrong/partial
take. Add counterexample 2: 96 required tokens plus 35 isolated fillers at source interval `[126,257)` across old `[0,256)` / `[128,384)` windows. An independent
exhaustive small-fixture enumerator labels all legal WordId paths. Assert exact equality of discovered sets under varying chunk boundaries/slice resumes. Positive
is unchanged exact speech and supplied-envelope assembly. Baseline lacks matching, pending seam. Mutate anchor-only search, fixed-window resets, path pruning, or
cross-take joins. Every valid path survives.

**G4. Full-token guard and late valid take, S2.** Use values, units, names, negations, and comparisons with shared wording. Wrong/partial-only input yields no
eligible path after complete coverage; exact equivalent written/spoken value/unit is positive. Add more early rejected shared phrases than the diagnostic registry
can hold, followed by a late valid take. It must be discovered with original WordIds despite diagnostic saturation. Interrupt a search slice: coverage is
incomplete, never missing/pass. Mutate critical equality, substantive-insertion rejection, shared diagnostic/exact quota, or completion-at-slice logic.

**G5. Boundaries, pauses and perception, S2.** Real bounded PCM decoder/validator with clean speech, clipped samples, truncated consonant, real restart/intervening
speech, silence-ASR, and unknown/even timing. Add the three pause fixtures: exact speech with safely removable 2.2 s silence and no cue, a true partial/restart with
intervening speech, and the same exact speech/pause protected by a cue. The first stays complete; the second cannot splice; the third retains its hold. Ambiguous
context returns delivery review. Repeat through G9 cleanup. Assert retained original WordIds and independent decoded cut landmarks, not duration alone. Baseline
validation seam pending. Mutate silence-splitting, timing upgrade, clipping/boundary checks, or landmark-to-gaze inference. No fabricated listening receipt may
pass.

**G6. Pin/history/occurrence identity, S3/S4.** Real plan/host commit → undo/redo → merge/adoption → feedback. Positive alternate replaces only its closure, shifts
downstream once, and preserves unaffected identities. Repeat every G2 duplicate deletion/insertion/context case after reopen, undo/redo, and adoption. Ambiguity
keeps the exact original pin unresolved; uniquely evidenced survivor/provenance keeps the correct pin. Commit the lead's addressed resolution, lose reply, and
reconcile once. Assert head/history/TakeMapRevision correspondence and independent temporal/spatial outcomes. Baseline map seam pending. Mutate local-ID identity,
guessed duplicate transfer, map restore, receipt precedence, or automatic unresolved-pin clearing.

**G7. Durable bundle, S1/S3.** JA-1 real writer/recovery/Save As observer. Positive reopen after cache deletion retains words, alternatives, original unresolved pin
context, pauses, counters, and sources. Crash/drop reply around publication and retry. Assert wholly old/new graphs and one committed choice/ripple. Mutate
sidecar-only storage, early acknowledgment, missing reachability, or duplicate command replay. Baseline document save is a control; new record seam pending. JA-1
owns storage-crash/native Windows proof; SIGKILL alone is insufficient.

**G8. Clock conversion and continuous sync, S4.** Actual selected-stream decoder/correlator → real paired planner → decoded output, with independent landmarks and
pre-commit operation/receipt counters. Clock fixtures are correctly muxed unequal start PTS with audio 200 ms after video, equal-origin control, and a real
flash/clap or independently observed mouth/audio landmark with a separately bound capture offset. Assert the positive origin-conversion sign and final audio/video
error ≤one project frame, not just PCM lag. Continuity negatives are an interior lag excursion returning at both endpoint windows, an internal PTS gap compacted or
padded by decode, and a rejected/inconsistent interior anchor. Each refuses before commit, even if global inliers/endpoints pass. Positive controls are a clean
continuous pair and a smooth nonzero-drift segment whose offset-only bound fits. Test every error-bound component and maximum unchecked span. Also refuse unknown
origins/capture offset, missing/ambiguous scratch, two peaks, out-of-search offset, unsupported timing, and a long independent-clock span with only one slate.
Baseline same-asset routing is a regression control, not sync evidence; new observation/admission seam pending. Mutate omitted origin, wrong sign, zero-for-unknown,
PCM-length continuity, endpoint-only checks, discarded interior anchor, unchecked-span/drift term, or rounding. Negative cases leave exact pre-state and zero paired
commits.

**G9. Shared cleanup and one paired edit, S4.** Existing assembly typed result → compound adapter → real Core and decoder. Positive no-protection inputs preserve
today's ordered `selections`/`kept_source_ranges`, including repeated assets. Run all three G5 pause fixtures: unprotected hesitation can shorten safely, protected
hold remains, true restart stays ineligible. Assert exact required WordIds, actual decoded cut boundaries, linked camera/mic ranges, and unrouted scratch audio. Use
unequal durations, awkward fps, many cumulative boundaries, and nonempty downstream sync-locked tracks. One undo/redo restores exact clips/map/links and one
downstream shift. Conflicts/sync failures make zero half-pair writes. Baseline already returns kept ranges; new protection/word/paired seam pending. Mutate pause
protection after merging, required-word protection, cumulative rounding, paired routing, or two-insert implementation.

**G10. Script, ASR, sound, data and captions, S5.** JA-3 consumers + JA-1 claim/render-start observer. Supply correct script/chart but wrong ASR value, and
separately wrong recorded audio while script and ASR agree. An independent source-bound spoken-value observation must expose the latter mismatch. If no independent
checker is available, that lane is unavailable/NOT RUN and critical speech stays unresolved. Positive uses a correct take with supported fact/spoken evidence.
Assert zero affected render starts for unresolved critical speech, exact caption text/timing from retained words, and invalidated review keys after override. Mutate
caption laundering, JSON-as-speech proof, cue inclusion, or stale review reuse. Studio old-version "take 1" must address the original candidate. JA-1 G16 remains
final output review.

**G11. Full search, reuse and resources, S2/S5.** Real matcher/transcription/decode counting, allocation/queue observers, and UI timing. Run all four §12 full
workloads cold/warm with out-of-order retakes, both G3 counterexamples, diagnostic saturation, late valid take, and unchanged exact controls. Independently label
coverage/path sets. Report measured cumulative matcher/DP cells, peak bytes, complete coverage, and full latency, not a slice or forecast. Cache reuse is a
regression control: rewrap/choice/reopen/unrelated edit causes zero new inference. Mutate per-line transcription, whole-PCM sync allocation, lost frontier, counter
reset, page leak, or cancelled publication. Verify typed overflow and retained pins. Measure real Whisper peak separately. Resource/native/provider/human lanes
remain distinct and unrun until exercised.

## 15. Risks, decisions and remaining lead work

Exact ASR can be acoustically wrong. Conservative equality may reject a usable paraphrase. Neither risk authorizes biased transcript repair or invented speech.
Room-mic correlation can be ambiguous, and independent clocks may lack a defensible interior bound. Refuse affected ranges rather than claim residuals prove sync.
Duplicate script edits can lose intent; retaining original unresolved context is safer than moving a pin. Complete discovery may exceed quotas; incomplete coverage
remains visible and cannot become absence.

Deferred work includes learned/multilingual/semantic alignment, new prosody/gaze models, cross-take comping, arbitrary script formats, generic merge/sync
frameworks, discontinuity repair, and continuous drift retiming. Required unavailable evidence is a blocker, not a deferral. No part of findings 1–6 is rejected.
The unused anchor-window alternative is dropped in favor of the lead's streaming choice.

### 15.1 Accepted answers to rev1's lead questions

1. Q1 is settled. JA-1's existing project/store owner coordinates schema/publication, including exact head/history-to-map references, pin reachability, Save As,
   merge, and fenced adoption (§7).
2. Q2 is settled as a fixture policy. Freeze supplied material and independent small adversarial fixtures. Practice files are pending below. Injected transcript
   words prove text behavior, not microphone wording.
3. Q3 is settled as an evidence policy. Certify the actual configured source-image/clip path before use. Geometry/RMS/rate are aids. Model listening remains
   optional. Disputed critical speech without a bound human/certified-audio observation stays unresolved and blocks the affected render (§6.1; G10).
4. Q4 is settled as scope. Keep provisional thresholds and offset-only cuts. Measure G8 before acceptance. Failed controls justify only an evidenced bounded
   correction or refusal, never loosened thresholds, hidden stretching, trusted timestamps, or silent camera-audio substitution (§8).
5. Q5 is settled as ownership. One existing assembly-planner owner shares cleanup/result; JA-3 consumes exact applied mappings for captions/facts/QA. No second
   computation/service. Measure accepted full workloads, decode-inclusive sync, and Whisper separately under JA-1 aggregate accounting (§§9, 12).

### 15.2 Pending practice inputs and numbered questions for the lead

Riel will supply the tight-ends script, exact Songbird JSON exports, and a C920/QuadCast practice take. This design does not claim to have inspected their contents
or expected word intervals. Freeze these slots:

| Pending input | Required immutable identity and oracle |
| --- | --- |
| Tight-ends Markdown/plain-text script | `script_sha256: pending`; original bytes, parser profile, speech/cue labels, expected line/beat correspondence. |
| Songbird v1 JSON exports | `songbird_export_sha256[]: pending`; per-file owned bytes and JA-1 dataset/scope/completeness bindings. Shipped format does not mean supplied practice exports. |
| C920 practice take including scratch audio | `camera_sha256[]: pending`; selected stream indices/time bases/origins, decoded coverage, independent A/V landmarks. |
| QuadCast practice audio | `mic_sha256[]: pending`; selected streams/convention, expected source WordIds/ranges, source-bound critical-word observations. |

Small independently labelled text fixtures remain separate from these pending acoustic/material lanes. They include wrong numbers/units/names/negations,
out-of-order takes, both retrieval counterexamples, long gaps, and duplicate insert/delete/context edits. Do not invent a practice inventory or factual values. No
new design questions go to Riel. The remaining numbered questions are implementation/evidence work:

1. Which named JA-1 schema owner and existing assembly-planner maintainer will freeze the exact checkpoint/map, protected-input, cumulative-boundary, and JA-3
   consumer contracts before S1/S3/S4?
2. When supplied, which hashes and independently labelled WordId/decoded-landmark oracles freeze the practice inputs above? What source-bound observations cover
   disputed critical words, including G10's audio mismatch?
3. Which configured connector has actual certified source-image/clip transport, and who records any human or certified-audio observation when critical speech is
   disputed? No certification is claimed here.
4. What measured G8 controls establish applicable clock/lag uncertainty and interior drift/coverage bounds for the real capture chain? Until established, those
   ranges remain unresolved under the fixed policy.
5. What are the measured full-workload cells/peak bytes/coverage/latencies and integrated Linux/Windows results? Keep runtime, resource, native durability, and
   human publishability acceptance separately pending.

## 16. Changes from rev1

| Critic finding | Disposition and addressing sections |
| --- | --- |
| 1. Major, incomplete retrieval | Accepted. Complete streaming exact/filler paths and proof, chunk state, cumulative coverage, separate diagnostic quota, no 64-delivery cutoff. §§5, 12; G3/G4/G11 cover both counterexamples, late valid take, and all full workloads. |
| 2. Major, destructive silence split | Accepted. Silence preserves complete candidates, long-gap evidence and explicit delivery review; cleanup removes only safe unprotected hesitation. §§6.2, 9; G5/G9 check all three cases, retained WordIds, and decoded cuts. |
| 3. Major, unnamed clock domains | Accepted. Bound streams/origins/convention and separate capture offset; compose audio-relative fit into decoder video frames. §8.1; G8 includes unequal/equal origins and real independent landmarks. Unknown timing refuses. |
| 4. Major, unproven sync coverage | Accepted. Timestamp coverage/gaps/padding, interior evidence, rejected-anchor refusal, explicit offset/slope/unchecked-span/rounding bound, and no discontinuity repair. §§8.2–8.3; G8 has three negatives and two positive controls before commit. |
| 5. Major, duplicate pin migration | Accepted. Unique revision evidence or named structured survivor only; otherwise retain original unresolved context and JA-1 addressed resolution. §§4.2, 7; G2/G6 check insert/delete/context after reopen, undo/redo, and adoption. |
| 6. Minor, duplicate kept-range work | Accepted. Reuse existing selections/cleanup through one internal typed result. Narrow additions and no public service. §§3, 9, 13; G9 retains existing selections as a regression control. Nothing rejected. |
