# Measured dialogue tools

The tools are built through the real compact dispatcher and remain unserved.
`dialogue_tools::SERVE_DIALOGUE_TOOLS` is false. The same switch controls the
capability catalog, native role manifests/admission, and provider-backed
transcript addresses. The nonempty legacy transcript golden pins the switch-off
bytes. Pure fixtures need neither `SessionStore` nor a bundle and compile for both
systems; Windows execution remains a CI gate. The separate Linux-only CLI correction is `a95f55d4`; the DQ3 review
follow-ups are `2bfdf65c`, already accepted and pushed by the lead.

## Audible-clock ruling

The executed 25-in-24 witness was real. Source 12..38 at 25 fps stores an end of
1,520,000 us, while its 24-frame project portion stops audibly at 1,480,000 us.
The old probe and exact sample-count calculation remain in `target/dq-tools/`.
It was not a PCM render or listening test. The lead resolved this in
`.review/arch/task-dq2-audible-clock.md`.

Core now defines `audible_span(source, asset_rate, project_length, project_rate)`.
It returns reduced exact source microseconds and known padded silence, using
`min(stored_end, source_start + project_duration)`. `ExactMicros` never uses
floats. A rational is `[numerator, denominator]`; an exceptionally large
numerator is a decimal string rather than a lossy JSON number. Equal rates,
25-in-24, 30000/1001-in-30 and padding have independent numeric controls.

`FrameGrid` generalises the existing Core frame-edge projection to an anchored
grid. Source proposals use `Dialogue::frame_edge`; project mapping uses that
same grid implementation. No second rounding formula lives in the Agent.
The audit now takes exact audible spans, guards their fractional edges without
flooring, and lists padding separately from measured quiet. Whole-microsecond
level/word statistics quantise each endpoint within one microsecond; the
safety guard does not. Fractional dip warnings round intrusion up, independently
of the guard. The audit borrows prepared dialogues rather than copying profiles.

The Agent calls the existing public `timeline_audio_segments` and feeds its
source/project ranges to Core. No Media source changed. Preview/export
conformance within one output sample, and the nonseamless mixed-rate split
follow-up, belong to the later Media job. The tools stay off until that gate.

## Provider and edit path

`DialogueMeasurements` selects the audio stream for a content identity and
returns `Arc<BoundDialogue>`. Each immutable value contains SHA-256 + byte length,
audio stream, recogniser/cache-build key, ordered `Word`s in source microseconds,
10-ms peak/RMS/voice hops with true duration, and the guard settings. The real
voice flag comes from earshot on the recogniser's 16-kHz mono signal. Constructor
validation and a streaming digest bind all of them. The provider is installed
through `McpServer::install_dialogue_measurements`; handler clones share the
service slot. One value is pinned per content for a call. The Media job must
supply the stream the renderer plays and build the cache adapter on the Agent
side of the dependency boundary; Media does not depend on Agent.

The address is `(timeline clip, word index)`, with that clip's binding key and
the expected document revision. Stale bindings, absent measurements, wrong
identity/stream, unavailable words and retiming refuse without an edit. Word
ranges address one clip; a batch spans recordings, as transcript deletion does.
The transcript groups consecutive words with one starting index, not a number
on every word. Its provider and switch are the tools' provider and switch.

A batch accepts 1..32 removals. Each resolves on its recording, proposes source
frames and maps toward keeping material on the anchored project grid. Real
operations build a scratch document. Its heard edges must pass the guard, or
move at most three project frames per side toward keeping more material.
Timeline-order construction subtracts the accepted ripple prefix, preserving
previous incoming anchors; a final pass checks every created edge. Minimum
source removal and project duration remain Core's minimum and two frames.
The scratch copies are intentional speculative edit states, not profile copies.

`apply_edit_plan` remains the single consent, scope, history and atomic Core
transaction. The additional observer receives Core's committed snapshot; it
never re-reads the pre-cut document. One Undo restores the complete document.
Other sync-locked tracks follow transcript deletion's isolate/delete/ripple
policy. An explicit overrule bypasses only acoustic/run-together policy and
still requires the existing edit consent, bindings, scope and timing bounds.

Results keep each removal's original `cut` frames under `from_revision`; edge and
join frames name the committed `revision`. Compact edge tuples carry the
recording, exact heard instant, placement and optional dip. Adjacent removals
share the surviving join; its outgoing and incoming guards/overrules belong
to the removals that own those outer sides, independently. Resolved batches
list every removal with its outcome; an acoustic refusal lists its reason and
neighboring quiet point. Revision/identity preflight refusals are request-wide
before resolution. The cut readback uses those same pinned values and lists
only joins created by this batch. The read-only check defaults to flags, retains
omission counts, supports all-detail/range, names unsupported clips and counts
other audible clips without judging their mix.

## Evidence

Logs and mutation proofs stay under `target/dq-tools/`; none is paid evidence.
The 11 dispatcher tests cover exact Undo, pause air, a refused middle removal,
consent, identity/stream/revision and changed bindings, absence, retiming,
mixed-rate guards, both sides' keep-rounding, ten NTSC removals, padding,
60 joins, two recordings and music counts, discovery, and legacy transcript
bytes. The mixed-rate control distinguishes the passing stored end from the
refused heard end, then checks the committed replacement edge.

The original ten-minute 60-join fixture measures 1,003 bytes flags-only and
21,170 bytes all-detail in the Agent wrapper (bounds 2,048 and 32,768). Core's
embedded reports are 93 and 20,260 bytes; the wrapper/provenance adds 910 bytes.
An applied removal is 106 bytes in compact JSON, below the 200-byte target.
For 1,800 words over twelve minutes, one transcript group's complete address
metadata costs 87 bytes: 10 for its starting index and 77 for its binding key.
That synthetic grouped transcript is 79,102 bytes in total; this is address
cost evidence, not a new claim about transcript pagination or output admission.

The five deliberate breaks are run against the completed implementation:

| Pair | Named red assertion | Red / restored green |
| --- | --- | --- |
| ignore-guard | `DQ_TOOLS_ACOUSTIC_GUARD_REQUIRED` | 0/1 at named assertion / 1/0; exact restore |
| remove-side-rounding | `DQ_TOOLS_ROUND_KEEP_OUT` | 0/1 at named assertion / 1/0; exact restore |
| precut-readback | `DQ_TOOLS_FRESH_COMMITTED_READBACK` | 0/1 at named assertion / 1/0; exact restore |
| partial-refused-batch | `DQ_TOOLS_WHOLE_BATCH_REFUSED` | 0/1 at named assertion / 1/0; exact restore |
| stored-end-guard | `DQ_TOOLS_GUARD_AT_AUDIBLE_END` | 0/1 at named assertion / 1/0; exact restore |

The rounding mutation reverses both the proposal rounding and its independent
final keep-side validation, preserving minimum duration. Each script run checks
the named assertion, exact 0/1 red and 1/0 green counts, original source SHA-256,
and a byte-identical tracked diff after restore. Preliminary runs before the
adjacent-edge ownership correction are retained separately, not counted again.

The final workspace build passed (52.83 s wall, including lock wait). The final
workspace fast run exited 0 in 2,450.64 s wall; its 90 summaries total 5,416
passed / 0 failed / 265 ignored, including doctests:

| Crate | Passed | Failed | Ignored |
| --- | ---: | ---: | ---: |
| Agent | 1,717 | 0 | 165 |
| App | 1,082 | 0 | 10 |
| Core | 884 | 0 | 2 |
| Hash | 6 | 0 | 0 |
| Job sys | 11 | 0 | 11 |
| Media | 1,233 | 0 | 73 |
| Project | 483 | 0 | 4 |

All 11 new dispatcher tests passed in 0.12 s. Workspace all-target clippy passed
with `-D warnings`, both default and `te1-eval`; fmt passed, including a separate
rustfmt check of the include-file tests. The actual measurement-pin selector
ran 2 tests, both passed (48.32 s test time). An earlier zero-test selector is
kept under a `zero-selector` log name and is not counted as evidence.

Only `arm_bridge_sha256.mcp_observation` changed in the pin: server SHA-256
`3e941ede35386a22b0722b69eea8a2a6074f92036ce6894638044843128962b9`
→ `5f0e4dd2df3cc3f5788b8bc9d04e4cd394181465d1d3a2ae933a2e8390c370e5`.
A recursive comparison against `2bfdf65c` confirms that single leaf; model-visible
hashes are unchanged. Parser selftests passed 52 and shard selftests passed 23.
The five final pairs took 1,512.32 s wall, including shared-lock waits.

The default-build guard passed on 3 artifacts (115.41 s wall). The platform
script exited 0 in 535.03 s wall. Its three Linux commands exactly match CI:
`cargo build -p kinewright-agent --no-default-features --lib`,
`cargo build -p kinewright-app --no-default-features --bin kinewright-app`, and
`cargo build -p kinewright-agent --no-default-features --bin kinewright`.
All passed. MSVC workspace clippy and Agent slow/App default/App slow clippy
passed with `-D warnings`; the three isolated MSVC shipping **checks** passed.
These are not Windows execution. Feature-disabled builds/checks emitted unused
constant/import warnings in unchanged authentication, certification and task-name
files; the all-target clippy matrices were clean. Both disposable shipping
targets were removed on success (Linux 3.7 GiB, MSVC 907 MiB).

Final slow-test lint passed: 140 slow tests, 160 ignored entries, 27 equipped
skip sites. Final logical target size is 17,130,507,103 bytes (15.95 GiB).
At the no-cargo boundary after the workspace suite, retiring 173 unlinked
executables reduced logical target bytes from 38,685,584,285 to 15,510,379,885;
evidence logs, diagnostic patches, libraries and fixture-host links were kept.

## Handoff and boundaries

The Media job supplies the bound cache values above, installs the provider,
proves renderer conformance and flips the one serving switch. This does not
change the existing Media transcript contract. A future speaker field would be
needed to suppress repeats on authenticated different-speaker evidence; current
Core words carry no speaker and recordings are not guessed to be speakers.

`get_dialogue_pacing` / `pacing.rs` are untouched. The new check supersedes their
short/long gap classification and transcript-bound/merged-silence gap estimate
when served. It adds measured edge-in-sound, independent short-air, room-tone
and dip warnings, plus repeat and sounding-hole evidence. The older speaker,
punctuation and capitalization boundary hints are not new acoustic evidence;
authenticated speaker-aware repeat suppression remains a follow-up.

Additional explicit refusals: the cut planner requires an audible, enabled
media clip without an animated enable curve, and disjoint selections on one
dialogue track. The checker handles audible segment lists, lists retimed or
overlapping dialogue clips as not checked, and does not judge a mixture.

NOT RUN: real recogniser/profile extraction, renderer conformance, PCM/listening,
slow-tier execution, Windows execution, provider/paid/dry/interactive-app
launches. No push or subagents.

## Tool review fixes

Review targets `6adecfb8`; the serving switch remains false. The extent checker
tracks every split child back to its original clip, independently of project
position and of other clips sharing the recording. Every affected track is
compared against the original audible spans. Outside the requested intervals,
a measured loss is allowed only when **all** of its hops are raw measured quiet
and the total loss from that original segment is at most one project frame.
Growth and changed outer padding are guarded at the new heard edge. Each is
reported separately; an overrule cannot waive any of these checks. An unmeasured
sync track instead reports changed-extent count and largest microsecond change.
The planner tries other alignments before refusing, without publishing a trial.
The typed refusal names `clip`, `lost_micros`, and:
"frame rates differ; unselected audio is not proven quiet within one project
frame; choose another cut alignment".

A diagnostic disproved the note's whole-multiple shortcut: splitting source
0..200 at 60 fps in a 30 fps project at project frame 16 selects native frame
31. The right piece ends audibly at 3,316,666⅔ us, versus 3,333,333⅓ before.
The raw probe is saved under `target/dq-levels-guide/whole-multiple-inverse-phase-probe.rs`
and its output in `review-tests-first.log`. Therefore only equal rates skip
comparison. Integer multiples require **zero** outside change and search for an
alignment with the original phase; their randomized tool property covers 60/30
and 60000/1001 over 30000/1001. No Core split behaviour was changed. The old
mixed-clock heard-edge witness now supplies a quiet outer tail so that it still
isolates its intended join guard; the new sounding-tail witnesses separately
prove the additional refusal.

Segments beginning inside a clip are explicitly `not_checked`, including when
they would otherwise supply a neighbouring guard. The complete serialized MCP
check reply has one 4,096-byte flags budget or 32,768-byte all-detail budget.
Acoustic failures precede warnings, unavailable evidence, then information.
Provenance dictionaries share that budget; omissions retain counts and playback
range instructions. Small replies retain their bytes. Request refusal reasons
are bounded to 128 UTF-8 bytes on the wire and 128 characters in Display,
including when constructed directly by an injected provider; serialization copies
only the bounded prefix.

An overrule belongs to the removal owning the final heard edge. A following
overruled removal cannot lend consent to the kept sliver from a backed-off
preceding removal. Trials and the final batch use the same geometry rule. The
minimum removal is judged once in source time; the two-project-frame structural
minimum remains independent. Binding validation runs before the confirmation
prompt and again after consent, using the current provider slot both times.

### Scratch measurement and ownership

Before changing scratch application, five real ten-removal dispatches on a
3,000-clip document took 944,214 / 951,694 / 958,885 / 955,551 / 956,095 us,
median **955,551 us**; ten trials / thirty operations each. The worst batch took
8,347,806 us, 512 trials / 1,536 operations. Because this exceeded 100 ms, a
trial now owns one disposable scratch and applies validated operations in place;
on error the entire trial is discarded. This removes the three extra whole
snapshots per trial (worst approximately 2,048 → 512), not the atomic live apply.
The first isolated post-change measurement was median 946,773 us and worst
8,119,319 us: no material latency improvement is claimed. The checked-operation
and whole-document validation costs remain. Candidates are ordered by total
keep-side backoff, trying the nearest alignment first.

### Dependence on recogniser times

`resolve` still uses word starts/ends, word-window anchors and the configured
snap (120 ms), kept slack (50 ms), removed slack (300 ms) and word margin (20 ms).
The guard trusts measured long quiet even inside a stretched word; a short dip
still needs its 50 ms word-boundary reach. Thus a falsely early word end can
classify a dip as untranscribed and accept it, while the same dip inside a late
word is refused. These are timing-dependent policies, not word-alignment proof.
Pause-after-word removal now locates measured gaps between the addressed word
starts, measures air from sound, and refuses multiple distinct candidate gaps
rather than guessing. With actual sound ending at 500,000 us and the next sound
starting at 3,600,000 us, ASR ends 420,000 / 760,000 / 3,500,000 all leave the same
650,000..3,500,000 removal for 250,000 us retained air. The audit's measured air
is independent; the stretched-word warning still compares the word timing with
the kept edge. Existing pause fixture expectations moved by 60 ms because their
sound ended at 2.26 s while their ASR word ended at 2.2 s.

The engine seam job needs an exact source in-point (finer than a native frame),
propagated through split/trim operations, serialization, history and Undo, plus
live/export conformance to that clock, including enable-run origins and seeks.
The temporary quiet-loss allowance can then be removed.

The pair results and validation are recorded below. No Media source changes,
real provider commands, paid runs or interactive App launches.

### Review mutation evidence

Eight independent mutations, each run against its exact witness, then restored
byte-identically. Logs and SHA-256 restore proofs are in `target/dq-tools-review/pairs/`.

| Wrong change | Named failing assertion | Red → green (passed/failed) |
| --- | --- | --- |
| half-speed | `DQ_TOOLS_HALF_SPEED_NOT_CHECKED` | 0/1 → 1/0 |
| late-binding | `DQ_TOOLS_BINDING_RECHECK_AFTER_CONSENT` | 0/1 → 1/0 |
| one-frame | `DQ_TOOLS_TWO_PROJECT_FRAMES` | 0/1 → 1/0 |
| outside-extents | `DQ_TOOLS_UNSELECTED_TAIL_PROTECTED` | 0/1 → 1/0 |
| removal-cap | `DQ_TOOLS_33_REFUSED` | 0/1 → 1/0 |
| scoped-overrule | `DQ_TOOLS_SCOPED_OVERRULE` | 0/1 → 1/0 |
| sync-tracks | `DQ_TOOLS_SYNC_EXACT_GEOMETRY` | 0/1 → 1/0 |
| tail-overrule | `DQ_TOOLS_TAIL_OVERRULE_INSTANT` | 0/1 → 1/0 |

The first sliver fixture did not force A to back off and incorrectly judged a
B-owned edge; both red and green failed. Those attempts are archived in
`target/dq-tools-review/fixture-error/` and are not counted as evidence. The
corrected real-dispatch fixture forces A's native-frame backoff, and the mutation
then lets B lend its overrule to A's sounding edge. Restored code finds quiet.
A separate final-geometry unit control also pins that ownership rule.

The 72 generated whole-multiple-rate cuts all apply without outside extent
changes. The NTSC quiet-loss fixture reports 3,167 us, and its sounding-tail
control refuses. A separate growth fixture reports 1,867 us and refuses when
that newly heard tail is sounding, despite an overrule on the removal. The
unmeasured music bed reports two extent facts (source end and padding), largest
3,334 us; these count changes, not distinct clips. Large flags replies measure
4,096 bytes (3,000 retimed clips plus a late acoustic failure) and 4,088 bytes
(250 recording provenance entries). The sixty-join reply remains 1,003 bytes
for flags and 21,170 for all detail.

The five retained controls near the changed code were rerun as well:

| Retained wrong change | Named failing assertion | Red → green |
| --- | --- | --- |
| ignore-guard | `DQ_TOOLS_ACOUSTIC_GUARD_REQUIRED` | 0/1 → 1/0 |
| partial-refused-batch | `DQ_TOOLS_WHOLE_BATCH_REFUSED` | 0/1 → 1/0 |
| precut-readback | `DQ_TOOLS_FRESH_COMMITTED_READBACK` | 0/1 → 1/0 |
| remove-side-rounding | `DQ_TOOLS_ROUND_KEEP_IN` | 0/1 → 1/0 |
| stored-end-guard | `DQ_TOOLS_GUARD_AT_AUDIBLE_END` | 0/1 → 1/0 |

The rounding script was retargeted from the old boolean `keeps` to its current
typed refusal, and its expected marker changed from `KEEP_OUT` to `KEEP_IN`: the
incoming assertion is now the first to catch the same wrong rounding policy.
The first marker-mismatch attempt is archived under `target/dq-tools/entry-drift/`;
no witness assertion was relaxed. All thirteen unique pairs pass. The eight new
pairs took 980.82 s and the retained-rest wrapper 4,065.33 s wall, mostly waiting
for the shared lock. These are wrapper durations, not test execution times.

Final-source scratch measurement: median **984,288 us** for ten removals;
**8,530,389 us** for 512 failed trials / 1,536 operations. The complete benchmark
test took 13.53 s. The baseline median was 955,551 us: the final median is 3.0%
higher, so this is a copy-count reduction without a claimed latency gain.

An ordinary regression also makes the new tail instant quiet while the later
part of the lost stretch sounds. The instant guard alone accepts it; the
whole-stretch check refuses with `unselected_audio`, preserving revision,
document and history.

### Review-fix gates

At the review-fix source, `cargo test -p kinewright-core -p kinewright-agent`
passed: Core **897 / 0 / 2**, Agent **1,738 / 0 / 166** (passed / failed / ignored,
including integration and doc tests). Wrapper wall time: 1,131.52 s; Agent lib
alone: 1,007.98 s. Workspace all-target default clippy `-D warnings`, Cargo fmt,
the explicit include-file rustfmt check, and both measurement-pin tests passed.
The pin selector ran two tests, not zero. Slow-test lint: 140 slow / 161 ignored /
27 equipped skip sites; parser selftests 52, shard selftests 23.

Only `arm_bridge_sha256.mcp_observation` changed in the measurement pin:
`5f0e4dd2df3cc3f5788b8bc9d04e4cd394181465d1d3a2ae933a2e8390c370e5`
→ `1fc289a1db94b52253834d8d0ac13aa42ccb22fa7298fd52b4fe155a09013424`.
The pin tests verify the frozen model-visible leaves. Eval clippy and the default
build guard (three artifacts) passed too. The Windows gate passed all four
cross-clippy modes, three isolated Linux shipping builds and three isolated MSVC
shipping checks. Both shipping targets were removed on success (3.7 GiB Linux,
907 MiB MSVC). This is Windows compilation/lint, not Windows execution. The final
guide commit also receives the combined workspace fast suite and build.
