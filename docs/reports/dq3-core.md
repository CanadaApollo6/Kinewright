# DQ3: dialogue cut self-checks

Base: DQ2/DQ4 review fixes `c8d15201` on `dq2`. This adds the pure Core `dialogue::audit` module. No caller, recogniser, document edits, existing Agent pacing, decoding or provider behavior changes. Scarlet `2cbab542` was read only; port headers name the source files and existing MIT notice.

The caller supplies a map of opaque recording ids to prepared `Dialogue` measurements, kept recording/source ranges in playback order, `AuditSettings`, and a word comparison function. `audit` returns an owned `Report` or a typed error before producing evidence for invalid inputs. `SourceRange` moved to Core and is re-exported by `cut_score`; its JSON is unchanged. Source-contiguous ranges coalesce; repeated playback and separate recordings keep their identities.

A report has duration, one policy snapshot, one dictionary of only referenced words, tagged items, totals and omitted counts. Items carry original recording/word references, source microseconds and playback microseconds. Default caps: 64 items, four sound runs per hole, 96 UTF-8 bytes per word name and 256 per phrase excerpt; truncation is explicit. Guard failures precede other warnings, then information, with playback order inside each priority. No raw profile arrays are serialized. Counts cover all findings even when the item cap omits them.

Joins distinguish four independent facts: (1) `edge_in_sound` exists exactly when the corrected Stage A guard refuses; (2) `short_air` warns below 60 ms outgoing or 40 ms incoming; (3) comparable quiet sides warn for a >=6 dB room-tone step when either exceeds -70 dBFS; (4) `dip_intrusion` names a measured dip inside a word timing, its nearer boundary and depth. A permitted closure can carry the fourth warning with no guard failure. A deep refused dip can carry both. A +12 dB quiet-to-quiet step warns while both guards pass.

Levels use 60-ms retained-side windows, 20-ms voice windows, time-weighted means of per-hop centidB RMS and maximum peak. These are averages of dB readings, not energy-mixed rendered audio. Both voice-aware and level-only air and nearest whole kept-word distances are retained; stretched ASR word timing is named separately. Hole reports state baseline, threshold, speaking-level duration and capped source runs, never infer speech from loudness. Quiet phrases include both original-source neighbor levels and bounded text.

Scarlet differences: integer microseconds/centidB throughout, retaining room steps instead of whole-dB rounding and weighting partial hops by measured duration; full finite profile rather than scan pages, with windows and air clipped to the actually kept sides; corrected Stage A guard defines only the sound flag. Word ownership uses half coverage per coalesced playback occurrence (the reviewed DQ4 rule), not midpoint inclusion. No protected planner pauses are supplied, so all long kept pauses are reported. Cross-recording repetitions retain both recording identities; a recording id is not evidence of a different speaker. Wordless playback gaps may cross source joins; each source portion keeps the common playback gap and its own recording measurements. UTF-8 truncation and capped priority output replace unbounded text. The minimal default key uses Unicode lowercase alphanumerics; callers may supply their own normalizer, independent of any recogniser. Unicode alphabetic combining marks can differ from JS Letter/Number matching. A zero-duration source test token is one microsecond here because Stage A requires positive word spans.

No addition to the Stage A profile is required: source-clock RMS, peak, VAD and true duration cover these checks, including quiet phrases. The future tool must authenticate recording/stream/revision identity, decode/cache profiles, map clip speed and source/project frames, account for simultaneously audible tracks, present/choose fixes and use existing consent and atomic edit paths. This report is evidence, not permission to edit.

Final evidence: **58 passed / 0 failed / 0 ignored** in `cargo test -p kinewright-core dialogue` (25 audit tests plus 33 guard tests), including the guard-equivalence property and independent warning controls. Complete Core **870/0/2**; scorer **26/0/0**, offline command **4/0/0**, Agent `session::tests::in2_` **8/0/0**. Workspace all-target clippy `-D warnings`, fmt, workspace build, workspace test compilation and slow-test lint all pass. Logs and command/load/result records remain untracked in `target/dq3/`: `focus-4`, `audit-core-full`, `audit-score`, `audit-in2`, `audit-clippy-4`, `audit-fmt`, `audit-build`, `audit-no-run`, `audit-lint`, `audit-pairs`.

| Deliberate wrong change | Red at named assertion | Restored green |
| --- | --- | --- |
| `room-tone-both-sides` | 0 passed / 1 failed, `DQ3_ROOM_TONE_USES_BOTH_SIDES` | 1 passed / 0 failed |
| `repeats-across-join` | 0 passed / 1 failed, `DQ3_REPEAT_ACROSS_JOIN_IS_LISTED` | 1 passed / 0 failed |

The same-side room subtraction reports 0 rather than 1200 centidB. Ignoring across-join repeats reports one rather than two findings, preserving the within-range control. Each mutation restores the full file SHA-256 and the complete checkout diff. Original source bytes are also saved under `target/dq3/pairs/` before mutation so an interrupted run is recoverable. Scarlet's read-only checkout still reports `2cbab542`; none of it was executed.

Size at `3de779d3`, before the follow-ups below: the synthetic 600,000,000-us source has 61 timed words, 61 kept ranges and 60 interior joins (plus head and tail). Compact JSON is **26,196 bytes**, below the fixture's **32,768-byte** assertion, with **64 listed items**, **40 omitted joins** and **78 omitted hole source portions**. The cap prioritizes flags; these figures are for this fixture, not a universal 32-KiB byte cap. Longer names and different findings change the size while the item/text/run caps still apply. No raw levels appear in the JSON.

The accepted 45-ms boundary dip and refused 155-ms deep dip both name their word and intrusion; only the latter has the sound flag. The quiet +12-dB room step has neither sound flag. Half-kept occurrences agree with the reviewed score policy, and disjoint tiny slivers do not become a played word. Pauses, immediate repeats, breath versus speaking-level holes, quiet phrases, multiple recordings, stretched words, malformed inputs, UTF-8 caps and omission totals have permanent controls with source test names in comments.

NOT RUN: real recordings or recogniser output, listening/rendering, media extraction, agent-facing tool integration, provider/paid/dry/app launches, Windows, full Agent/App/Project/Media or workspace runtime suites. Workspace tests were compiled. No subagents, push, compatibility work, timing waits, or original `pacing.rs`/`transcript_edit.rs` changes. The reviewed guard/matching source bytes remain those of `c8d15201`. Target: 33 GiB before cleanup, 7.1 GiB after removing superseded build output, 29 GiB after all gates; previous evidence and parked DQ3 backups are retained.


## Last-review follow-ups (2026-11-14)

Sound-run minimums now apply to the continuous playback stretch across a join,
then surviving portions are attributed to their own recordings and speaking
levels. Independent one-recording and two-recording controls retain both 80-ms
portions of a 160-ms run; an isolated 80-ms portion still disappears. Repeats
can cross recordings; only the first saying must be source-contiguous. `Word`
has no speaker field: suppressing different voices needs a globally comparable
speaker identity per word from the provider, not another recording-id guess.

Settings reject zero item capacity, empty repeat/context windows, impossible
level contrasts, and padding that consumes the minimum hole. Repeat lookahead,
phrase duration and neighbor windows are capped at **30 seconds**. Time alone
does not bound overlapping token density, so a saying also has a **128-word**
ceiling (including the short-repeat budget); `repeat_search_limited` reports
anchors beyond that search budget. The dense-token control exercises it.
Other numerical thresholds remain policy choices rather than extra switches.

The JSON keeps shared hole intervals once in `hole_groups`; portions carry a
group index, a two-number level without the duplicate source span, and omit
empty/zero fields. It omits `comparable`, zero counts and the default policy
snapshot. `detail: flags` filters information before the item cap, retaining
totals and omissions. The 60-interior-join fixture measures **20,260 bytes all**
(previously 26,196: **5,936 fewer**, 22.7%) and **93 bytes flags-only**. Its
flags-only result has no warnings/items and explicitly counts 62 omitted clean
joins and 120 omitted quiet-hole portions; this is not a universal byte bound.
The all-items 32-KiB assertion stays; flags-only adds a 1-KiB assertion.

Three sub-hop decisions: keep the edge-containing hop's retained prefix
(duration-weighted; up to 10 ms different from Scarlet); make stretched-word
naming use voice-aware air; keep the conservative crossing-word exclusion for
whole-word distance. A dip warning depends on the recording's guard settings
`dip_dbfs_hundredths`, `dip_reach_micros` and `word_margin_micros`, not just the
audit policy. An empty cut returns an empty report. No edge-match counts were
separated from the score's error quantiles; the new check is an audit, not a
standalone tolerance-count ranking.

The Linux-only Studio CLI fixture now puts its durable store in the workspace
target directory, independently of whether the process temporary directory is
memory-backed. Its imports and snapshot helper remain inside the gated test;
the other three CLI cases remain portable.

| Wrong change | Red assertion (0 passed / 1 failed) | Restored |
| --- | --- | --- |
| Fractional intrusion divided before comparison | `DQ3_FRACTIONAL_REACH` | 1 passed / 0 failed |
| Outgoing window reads removed audio | `DQ3_KEPT_WINDOW_IS_CLIPPED` | 1 passed / 0 failed |
| Exact pause excluded at equality | `DQ3_EXACT_PAUSE_THRESHOLD` | 1 passed / 0 failed |
| Sound run restarts at a source portion | `DQ3_SOUND_RUN_CONTINUES_AT_JOIN` | 1 passed / 0 failed |
| Second saying restricted to one recording | `DQ3_REPEAT_ACROSS_RECORDINGS` | 1 passed / 0 failed |

The three independent review probes are permanent house-named tests. The
mutation driver saves source bytes and checks both SHA-256 and the whole diff
on restore. Evidence: `target/dq3-review/pairs/` and `target/dq-tools/review-*`.
Gates from the logs: full Core **877 passed / 0 failed / 2 ignored**; full
Agent **1,706 passed / 0 failed / 165 ignored**, including four doc tests.
The CLI was rerun after its local import cleanup: **4 passed / 0 failed**.
Workspace all-targets clippy (`-D warnings`), fmt and slow-test lint passed
(140 slow tests / 160 ignored-list entries). The final size run passed 1/0;
all five mutations were rerun against the final source and restored green.
Each restored SHA also matches the current file. No Media source changed.
Target remains **29 GiB**.

NOT RUN here: real recogniser/profile extraction, listening/rendering, provider,
paid/dry/app launches, Windows execution, or serving the new tools. Tool drafts
are parked unlinked under `target/dq-tools/parked/` until this commit is green.
