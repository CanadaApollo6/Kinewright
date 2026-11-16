# Dialogue measurements and renderer clocks

The checkpoint-1 work merges the dialogue tools base and supplies source acoustic
evidence in media. The dialogue-tool switch remains off. No protected agent
cut/check/server implementation or Core dialogue file is edited after the merge.

## M0

Merged `origin/dq2` at `9564e3777533d92d6097be5f5dd98ca8c1c9f0f6` into
`dq1`, with no merge repairs. Merge commit `5c59a5f0` passed media and Core tests,
workspace all-target clippy, formatting and the three isolated shipping checks.

## M1

The source decoder taps samples before downmixing. Core `ProfileBuilder` receives
all channels at the source rate. Earshot 1.2.2 receives the same 16 kHz mono
chunks as recognition; 256 samples form one detector frame. A score above 0.5
voices every overlapping 10 ms profile hop. The final detector frame alone is
padded; its interval is clipped to the real source duration. Nonfinite input or
scores and detector unavailability remain `NoVoiceInformation`, not quiet.
Panics use the existing transcript/recognition job boundaries.

Profiles are independent of recognition models and language requests. Their
identity includes content SHA-256/byte length, audio stream and measurement
contract/version. The contract reads Core's actual hop and the detector's actual
rate, frame size and threshold. A fast invariant checks the recorded detector
version against the exact Cargo dependency pin. The cache lives beneath the
existing derived-analysis family at
`derived-analysis/v1/sound-profiles/v2/`; its inventory and scoped clear include
profiles without changing the public cache-family schema. No cache migration is
provided. A transcript cache hit measures a missing profile without loading a
model. Media without a completed content identity is not measured.

One decode pass was selected. The recogniser session is unchanged, including its
existing frame-bounded input. The profile covers the real audio stream. Streaming
state consists of Core hop output, voice intervals, one decoded/conversion frame,
the 256-sample detector carry and the existing bounded recognition window.

Earshot is pinned to `=1.2.2`, Cargo checksum
`afea66474c4036a7e5fa4b072a3b0a09cd0b01f00134da3115d76b7cc5899f03`.
Its crate records upstream commit `c400f15bd232dda30a297eec90235bd09066cab7`.
The [MIT licence from that commit](https://raw.githubusercontent.com/pykeio/earshot/c400f15bd232dda30a297eec90235bd09066cab7/LICENSE-MIT)
is included in `LICENSES/earshot-MIT.txt`.
The native SDK/model pins and provisioning paths are unchanged.

### Measurements

Linux CPU, Rust test/debug code, Release native SDK, four recognition threads.
The measurement children ran at nice 19. Files were warm. RSS comes from Linux
`wait4` child-resource counters, with no sampling or system installation. Local
raw outputs and load records are in `target/dq-media/m1-final/`.

Five alternating `one, two` pass pairs on AMI8 produced identical profiles in
every pair. This measures decoding, Core levels and Earshot, without recognition
or durable-cache writes. The two-pass alternative uses separate decoder traversals
for source levels and mono detection.

| Passes | Median, s | Range, s |
| --- | ---: | ---: |
| One | 6.751196 | 6.738557–6.758114 |
| Two | 7.166382 | 7.121253–7.190557 |

The plan's earlier CLI decode-only medians were 0.047108 and 0.092637 s; those
excluded Core levels/Earshot and are not the pipeline timings above.

| Run | Pipeline time, s | Peak RSS, KiB | One-minute load before / after |
| --- | ---: | ---: | ---: |
| AMI8 measurement | 6.758486 | 56,284 | 2.58 / 2.49 |
| AMI30 measurement | 25.534032 | 57,692 | 2.49 / 3.27 |
| AMI30 recognition + measurement | 142.666358 | 986,468 | 3.27 / 4.06 |

The combined run loads and verifies the explicit pinned EN model and commits
recognition normally; it excludes durable-cache writes. It produced 3,381 words.
AMI30's serialized measurement is 3,155,479 bytes. The 30-minute profile has
180,000 hops; PCM is never retained for the recording. These are Linux
measurements, not Windows runtime results.

On AMI8, using hop-center membership in the union of human reference word
intervals and in the independently measured pauses:

| Region | Voiced hops | All region hops | Voiced share |
| --- | ---: | ---: | ---: |
| Reference words (1,026 words) | 22,944 | 34,337 | 66.8200% |
| Measured pauses (130 pauses) | 141 | 9,599 | 1.4689% |

There are 48,000 hops overall, 23,644 voiced. Any-overlap membership gives the
same counts for these annotations. The independent pause reference uses the
existing −80.0863 dB floor and −72.0863 dB threshold. No tuning was performed.

Three on-demand tests take explicit fixture/output paths; the combined test also
requires `KINEWRIGHT_ASR_MODEL_DIR`. They do not discover application data or
fetch models. The probe directory was read only. The combined launcher's initial
parent-directory mistake failed with `Model(Missing)` before loading; the
successful measurement uses the existing EN model directory.

## Remaining stages

M3 supplies the background cache adapter described below. The existing unit
`NotMeasured` refusal stays unchanged. A new final enum variant carries pending
measurement status. All endpoints share a 64 MiB resident budget.

M4 and M5 are not part of checkpoint 1. The switch remains off. At M5,
`get_dialogue_pacing` costs 1,882 bytes as a compact Tool definition, including
1,356 schema bytes and 360 description bytes. Remove it and its redundant
internal pacing implementation (zero direct model-visible bytes). Other transcript,
silence and assembly tools retain distinct purposes.

### M1 validation

Nine focused tests pass. They exercise native-rate stereo levels, unchanged
recogniser PCM, missing/failed voice information, detector dependency identity,
cache content/stream/contract misses, cancellation without publication, cache-hit
transcript jobs, and the production file driver with a recording backend. The
last test proves ASR sees its exact bounded prefix while the saved profile covers
the whole source. Its assertions are `DQ_MEDIA_ASR_PCM_AND_BOUND`,
`DQ_MEDIA_FILE_JOB_PROFILE_PUBLICATION` and `DQ_MEDIA_PROFILE_WHOLE_SOURCE`.
The real engine inventory/clear test includes profile files and preserves sources.

The final full media suite passes 1,301 tests with 83 ignored (1,047 s for the
media gate). The build, workspace all-target clippy, formatting, three
isolated shipping checks and slow-test lint pass. The local final gate log is
`target/dq-media/m1-final/summary.log`. The earlier prototype full gate also passed;
cache-family integration and the file-driver witness were then checked again in
full. No public transcript DTO or model-visible tool definition changes in M1.

## M2

The mixer now takes the source in-point from the canonical enabled run. A seek
adds the exact elapsed output samples to that in-point, without quantising a new
source-frame edit. Live shaping updates use the same bounds. This fixes the
reported 69,040 versus 67,200 source-sample start (1,840 samples) while preserving
ordinary seeks inside mixed-rate clips. Invalid playback windows stay refused.

Eight generated-PCM cases run through the production export and live mixer:
equal rates with a nonzero timeline start; 25 in 24; 30000/1001 in 30 with a
fractional-sample in-point; 60 in 30; nonzero source in-point; project padding
beyond stored source; an enabled run beginning inside a clip; and a middle seek.
Core `audible_span` supplies the independent clock oracle. Export pre-encode and
live PCM marker support must agree within one stereo output frame at 48 kHz,
with no extra marker read into padding. Both the product decoder and an
independent linear FFmpeg decode check the written AAC markers within one sample.
AAC ringing is not mistaken for an edit offset. No physical audio device opens;
video export uses only the software Vulkan/WARP adapter.

The first baseline caught the enabled-run PCM mismatch. A separate short-export
failure exposed discarded AAC preroll: the linear decoder retained sample 211 at
0.3256361 while the product's unconditional zero seek reduced it to 0.012923014
and put its strongest residual at sample 812. Opening at zero now preserves the
initial packets. The short-export test passes without reducing its marker
threshold. Word/voice caches advance to v5 and profiles to v2 to invalidate old
decoded inputs; no cache migration or compatibility path is added. Core required
no correction. The NTSC fixture was corrected to Core's nearest-rounded 60-frame
document length, rather than an assumed 61.

Three deliberate wrong changes were caught and fully reverted:

| Wrong change | Test | Failure |
| --- | --- | --- |
| Quantise seek offset onto source frames | `dq_media_clock_seek_inside_clip` | wrong PCM marker support |
| Ignore stored source end | `dq_media_clock_stored_source_padding` | two live markers instead of one |
| Restore zero seek | `dq_media_clock_stored_source_padding` | product decode loses marker 211 |

The restored focused suite passes 18 tests (three explicit on-demand benchmarks
remain ignored). The mixed-rate split seam remains outside this work. M3–M5 and
the dialogue-tool switch are untouched.

### M2 validation and checkpoint 1

The final full media gate passed 1,310 tests with 83 ignored: 1,277 unit tests
in 1,089.01 s and 33 integration/doc tests. Including compilation and process
startup, the media command took 1,138 s. The media build took 4 s; workspace
all-target clippy took 24 s; formatting passed. Workspace build (36 s) and
workspace test compilation (73 s) passed.

All three shipping builds ran separately, without workspace feature unification:

| Command | Build time, s |
| --- | ---: |
| `cargo build -p kinewright-agent --no-default-features --lib` | 25 |
| `cargo build -p kinewright-app --no-default-features --bin kinewright-app` | 26 |
| `cargo build -p kinewright-agent --no-default-features --bin kinewright` | 1 |

The corresponding isolated checks passed too. Windows media library/test
cross-compilation with `cargo xwin check` passed in 13 s. Windows execution was
not run locally; it remains a CI result.

After rebuilding the fixture host with `eval-harness`, 68 targeted agent tests
and 115 app tests passed. These include the separate public-API compiler probe,
protocol child, native invocation, version probe, full-reply-pipe cleanup,
studio runtime, admission/guardian/usage, voice, lease, dialogue and cut-score
checks; and app transcript, voice, feedback capture, manual values, C4/C7 feedback
and process-exit checks. Equipped/on-demand tests retain their normal ignores
(55 agent, three app). Every targeted command was checked for a nonempty result.

CI script self-tests passed: slow-test harness 52, NeMo provisioning nine,
Linux dependencies 13 and sharding 24. Slow-test lint and actionlint passed.
The slow-test harness emitted its existing nonfatal unclosed-file warning;
dependency self-tests deliberately exercised refused and timed-out connections.
No model was downloaded by tests. The final gate and mutation logs are under
`target/dq-media/m2/`. Recorded target sizes stayed below 40 GB and finished at
36.761 GB; `target/dq1/` was preserved, with over 420 GiB free on disk.

All launched build/test processes finished before this checkpoint. No push was
performed. M0 and M1 are `5c59a5f0` and `dc59b49d`; M2 is the commit containing
this section. The status decision was resolved for M3 below. No protected caller
was edited.

## M3

`CachedDialogueMeasurements` resolves content identity in its branch Core and
uses that project's admitted `SessionMedia`. Stream selection calls the same
media helper as playback and export. Source hash and byte length are checked
before cache evidence is accepted. Missing content identity remains `Identity`.
The media cache API also checks the selected stream itself.

The adapter reads the current request's recognizer recipe and exact integer
microseconds from the word cache. It does not reconstruct recognizer times from
the rounded UI transcript. Authored transcripts keep priority and are converted
from their exact source-frame rate, with explicit authored provenance. No public
transcript DTO changed.

| Missing cache half | `get` behavior |
| --- | --- |
| Words, with or without a profile | Queue the ordinary transcript job. It also fills a missing profile. |
| Profile, with words available | Queue profile-only work on the same transcript worker, without loading a model or replacing authored words. |
| Neither | Read both halves and return one immutable bound value. |

An evicted or cleared word file can be requested again even when its in-memory
status was completed. Admission retains that request's language/model recipe and
does not cancel an active request or replace authored words. Failures and
cancellations remain explicit. Profile-only requests reuse the existing worker,
admission, status-cap, cancellation, panic and shutdown mechanisms. Their separate
request table preserves the existing authored/ready transcript status.

The only change in `dialogue_tools/mod.rs` appends
`Refusal::MeasurementPending { status: MeasurementStatus }`. Its wire code is
`measurement_pending`; status is `queued`, `running` with optional
`progress_percent`, or `failed` with a reason capped at 128 Unicode scalar
characters. Known download and audio progress are retained. No inference or
acoustic measurement runs inline in the provider.

The process-wide 64 MiB least-recently-used cache charges words, string capacity,
profile arrays, provenance and conservative capacity for Core's derived interval
vectors. Tools hold shared immutable values. Only entries with no outside strong
reference can be evicted. Full pinned capacity produces a short retryable failure
instead of discarding a running tool's value. A changed word or profile value is
compared before reuse and produces a different binding. Rebuilding identical
evidence after eviction produces the same binding.

The app carries its admitted media source through initial, new-thread and
replacement branch-server construction. Each real endpoint installs the provider;
stub fixtures carry no provider. Construction is lazy and does not load models.
The global serving switch is still false.

### Dispatcher evidence and protected follow-up

The generated recording has separated tone bursts, silence, and two authored
words sharing one uninterrupted burst. Tests use the real provider, real decoded
levels and detector, an in-memory Core, the ordinary confirmation broker and the
blocking MCP dispatcher. Fixtures and cache files live under workspace `target/`
on both platforms; no durable project-store fixture is used.

`dq_media_provider_dispatches_pending_then_measured_cut_and_check` first holds an
unrelated transcript job before inference, forcing both dialogue tools to observe
queued measurement. It checks their typed status and serialized MCP wire status,
then releases the worker, reads bound words, refuses a cut inside the uninterrupted
burst, applies a quiet cut and reads the measured check report. No fake provider or
model download is involved.

Initial refusals survive both tools unchanged. One protected path still loses
status: `dialogue_tools/server.rs:114` converts a provider refusal during the
pre-commit recheck to text, and line 136 wraps it as `edit_refused` with a string
cause. Those lines are untouched and need worker 1's correction. Ready-cache
calls and initial pending calls are usable; a measurement becoming unavailable
during consent loses its typed status in that recheck path.

Other fast assertions cover a 128-character Unicode reason, exact 12,345/26,789
microsecond cache times, recognizer/language recipe misses, missing words with an
existing profile, preserved authored words, active-request protection, live-value
pinning, least-recent eviction, stable rebinding, changed-word invalidation and
sharing one resident value across independent endpoints.

### M3 validation

The full media suite passed 1,312 tests with 83 ignored, including 1,279 unit
tests in 1,021.54 s. The media command took 1,053 s. Nine final agent tests passed
in 1.14 s, including actual measured burst/quiet levels, serialized pending status,
and equality of the dispatcher's check report with Core's direct audit.
Twenty focused media tests passed. Existing dialogue, served-tool and role/schema
pins passed without changes. App branch-server and chat tests passed. Media build,
three isolated shipping checks, workspace all-target clippy and formatting passed.

Four deliberate breaks were caught and reverted:

| Wrong change | Assertion |
| --- | --- |
| Replace pending status with unit `NotMeasured` | `DQ_MEDIA_PENDING_CUT_STATUS` |
| Evict a bound value held by a tool | `DQ_MEDIA_PINNED_VALUE_MUST_NOT_BE_EVICTED` |
| Read every word cache with the English model and language recipe | `DQ_MEDIA_RECOGNISER_KEY_MISS` |
| Omit profile payload from resident accounting | `DQ_MEDIA_RESIDENT_COUNTS_PROFILE_AND_WORDS` |

An identity-only variation of the third break also failed, at the later missing
cache-read postcondition. The final variation ignored both recipe components and
hit the named miss assertion. All final positive checks used restored code.
Evidence is under `target/dq-media/m3/`. Cleanup removed incremental output and
113 inactive generated test executables before builds, preserving `target/dq1/`
and all media evidence. Target usage was 23.610 GB after cleanup and 24.722 GB
after the full gate. The checkpoint-wide shipping builds, Windows cross-checks
and external-compiler fixtures are recorded under M4 below.

## M4

Linux CI now builds the agent shipping library, app shipping binary and writer
host in three separate `-p` commands with defaults disabled, before workspace
clippy. This is a named Linux-only step and uses the same native setup and build
commands as Windows. No shipping target relies on workspace feature unification.

All five artifact-download sites in `ci-tests.yml` have two explicit attempts.
Each attempt has its own three-minute timeout. The initial attempt allows a
failure so a retry can run; the retry checks the initial step's `outcome`, skips
cancelled workflows, requests the identical artifact and fails the job if it
fails. The compiler-input condition remains on its initial attempt. Existing
job deadlines and pinned action revisions are unchanged.

Workflow self-tests require the three exact Linux shipping commands before
clippy, five matching first/retry pairs, the three-minute limits, and failure
propagation from the retry. Three wrong policy strings are rejected: a changed
deadline, disabling continuation on the first attempt, and checking `conclusion`
instead of `outcome`. All four CI self-test scripts passed locally (52, nine,
15 and 24 tests); actionlint and slow-test lint passed. The earlier draft was also
checked before being applied. `scripts/ci_shards.py` and `ci/slow-tests.txt` are
untouched.

Windows agent library and tests cross-compiled locally in 24 s. An additional
app cross-check could not reach Rust checking because the unchanged
`crates/kinewright-app/build.rs:33` requires `rc.exe`, which is unavailable on this
Linux host. No resource-compiler shim, installation or build-script workaround
was used. Local Windows app checking and Windows execution remain unverified;
the native Windows CI job supplies those checks. Its failure log is
`target/dq-media/m4/windows-app.log`.

### Checkpoint 2 validation

The final required media gate passed 1,312 tests with 83 ignored in 1,080 s.
Workspace all-target clippy (17 s), formatting, workspace build (29 s), workspace
test compilation (74 s) and the three isolated shipping checks passed. The three
actual shipping builds passed separately in 20 s, 26 s and one second for the
agent library, app binary and writer host respectively. Windows media library
and tests also cross-compiled in nine seconds.

After rebuilding the fixture host, 77 targeted agent tests and 115 app tests
passed, with the existing 55 and three equipped/on-demand ignores. Every current
test log was checked for a nonempty passing result. A local runner initially
looked for the new provider log under the prior checkpoint's directory; that
guard was corrected, all current logs were checked, and the remaining tests ran.
The final CI self-tests, slow-test lint and actionlint passed again. Their known
unclosed-file warning and deliberate loopback failures are unchanged.

This audit also corrects checkpoint 1's targeted-agent total above from 78 to 68;
the underlying passing logs are unchanged. Checkpoint 2 adds nine provider tests.
The final target directory is 36,506,008,080 bytes (36.506 GB), the maximum
recorded during this gate, with over 420 GiB free. Both evidence directories were
preserved. No build or test remains running. Local gate evidence is under
`target/dq-media/m4/`. M3 is `84b64584`; M4 is the commit containing this section.
No push was performed, and M5's serving switch remains false.

## R1: pinned dialogue review merge

Merged `517d7f4068bcc721b6b0e23501a35a8b14546f44` (review fixes and
the on-demand guide) with a plain merge. `dialogue_tools/mod.rs` merged both
sides automatically, retaining the new extent refusals and `MeasurementPending`.
The ignored-test list was the only textual conflict; both sets of entries stay.
The five protected dialogue-tool files and Core `dialogue/` are byte-identical
to the incoming revision. The serving switch remains false.

The provider end-to-end test initially rejected the check reply's newly added
`range` field. It now asserts that exact `[0,400]` range as well as the original
typed queued status. No assertion was removed. All nine provider tests and 35
dialogue tests pass (one on-demand dialogue test remains ignored). The original
measured quiet cut still applies and the burst cut still refuses: the new extent
rule did not refuse any cut this fixture expected to apply.

The merge gate passed media (1312 tests, 83 ignored), Core (897 tests),
workspace all-target clippy and formatting, workspace build and test compilation,
the three isolated shipping checks and actual builds, and Windows media cross-check.
The native-linking/protocol/provider/dialogue agent checks passed 101 tests;
the app checks passed 115. All four CI self-tests, slow-test lint and actionlint
passed. Evidence is `target/dq-media/r1/`; the largest recorded target size was
36,670,032,763 bytes. The build/test process exited successfully before this commit.

## R2: real-recording rehearsal

The production chain completed twice, including real recognition, sound profile,
the cached provider, the real dispatcher, confirmed Core edits, independent check,
and CPU export mixing. No cut setting, acoustic threshold, model setting or
protected file was changed. M5 remains off. The scripted editor uses no overrules.

`server::tests::dialogue_rehearsal_tests::dq_media_real_recording_rehearsal` is an
ignored on-demand test listed in `ci/ignored-tests.txt`. It requires all four
absolute environment paths: `KINEWRIGHT_REHEARSAL_AUDIO`, `KINEWRIGHT_ASR_MODEL_DIR`,
`KINEWRIGHT_REHEARSAL_DATA`, and `KINEWRIGHT_REHEARSAL_OUTPUT`. Output must be under
this worktree's `target/dq-media/rehearsal/`; the private data root must be inside
that output tree. There is no application-data discovery. The fixture verifies
and copies the explicitly supplied pinned EN model into its private installation,
then the real transcript worker uses normal verified provisioning from that
complete installation. The measured process ran under `unshare -rn`, so no model
or other network download was possible. The source clip and probe model stayed
read only. The feature-gated `rehearsal_support` wrappers call the existing model
verifier and production export mixer; they add no recognizer or provider substitute.

On this Linux host, force lavapipe with `VK_DRIVER_FILES` and
`VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json` and `WGPU_BACKEND=vulkan`.
The engine's otherwise-required compositor context is forced to the CPU fallback;
preview publication is disabled and no frame is requested. No physical GPU or
audio device is used. The WAV wrapper calls the production `export::mix_audio`
and writes its 48 kHz stereo float samples, without the video exporter.

After sourcing the native setup outside the lock, the on-demand entry is:

```sh
flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 \
  cargo test -p kinewright-agent --lib \
  server::tests::dialogue_rehearsal_tests::dq_media_real_recording_rehearsal \
  -- --exact --ignored --nocapture --test-threads=1
```

The measurement used the compiled test executable directly under the same outer
lock, with `nice -n 19 python3 -I -B target/dq-media/measure-child.py` and
`unshare -rn`, excluding Cargo compilation from wall time and peak RSS. Rust
was the unoptimized test profile, the native SDK its existing Release build,
and recognition used the unchanged four-thread identity and native clamp.
The warm run uses a fresh Core, dispatcher and provider with the original document,
while retaining the explicit fixture caches in the same process. Timing starts
inside the dispatcher caller and includes confirmation and reply serialization;
it excludes the prior model/measurement wait and WAV rendering. Approvals are
scripted in place of the person, not bypassed. No listener or model is involved
in issuing the editing requests.

### Inputs and selection

The clip is `/home/riels/.cache/kw-probe-asr/audio/derived/ami8.wav`, 15,360,078 bytes, SHA-256
`3565188c90972ee3bc6993345c6ecbd611b5957abe12c4d56fa7ffad21c5b4c0`. The model is the pinned Nemotron Speech Streaming
EN 0.6B Q8_0 from `/home/riels/.cache/kw-probe-asr/models/nemotron-speech-streaming-en-0.6b`. The observed adapter was
`llvmpipe (LLVM 23.1.4, 256 bits)` (`vulkan`, software fallback true).

The real recognizer emitted 906 words. Selection uses every merged Core
quiet/valley run strictly longer than 700 ms, including quiet hidden by stretched
ASR word ends; overlapping quiet and valley evidence is counted once. Each pause
asks `leave_micros: 300000`. Every standalone word accepted by the existing
`is_filler_word` matcher is selected: normalized `um`, `uh`, `erm`, `hmm`, `mm`,
`mhm`; emitted matches were `uh` × 14, `um` × 40.
No context-dependent words were added to that matcher.

There were 165 requested candidates: 111 measured pauses and
54 fillers. Five 32-removal calls and one
5-removal call sent all 165 candidates; all six batches were refused atomically. The script retried the surviving addresses individually in
timeline order. All 152 cut calls stayed within 32 removals. Every applied
call was followed by a fresh bound transcript read: 79 reads including the initial one.

| Kind | Requested | Applied as asked | Applied with edge movement | Individually refused | Atomic batch refused; retry address gone |
|---|---:|---:|---:|---:|---:|
| filler | 54 | 0 | 39 | 15 | 0 |
| pause | 111 | 1 | 38 | 53 | 19 |

Totals: 1 applied as asked, 77 applied after an edge moved,
68 individual typed refusals, and 19 refused batch members that
could not be retried once preceding successful cuts removed their original
left word. Those 19 are not counted as successful pause trims or as additional
tool-level typed refusals. No different word was substituted: a pause address
names adjacent original recording words. The chain completes with this explicit
addressing limitation rather than claiming that every requested pause was tightened.

Duration is **480.000 s → 338.966667 s**, a reduction of
141.033333 s (29.382%). In exact project frames this is
14400 → 10169 at 30 fps. Filler cuts removed
92.566667 s
including surrounding air; pause cuts removed
48.466667 s.

Transient typed refusals are separate from that terminal table. All 29 were
`conflict` outcomes in partially applied batches, with the conflicting earlier
member identified in the reply. Three examples follow; every transient outcome
is retained in the diagnostic JSON.

| Transient code | Count | Three examples at original source times, s |
|---|---:|---|
| conflict | 29 | pause 13.940–14.690 (`it / and`); filler 19.120–19.200 (`um`); filler 62.720–62.800 (`um`) |

### Edge movement

The comparison origin is the requested word's raw source interval. For a pause
it is the measured run minus 300 ms allocated with the unchanged production
3:2 air ratio (180 ms left, 120 ms right). Movement includes acoustic relocation
and frame placement. Fractions remain exact in the JSON evidence; this table
rounds to milliseconds. P90 and P95 use nearest rank; medians average the middle
two values where needed. These are magnitudes across both edges of every applied
cut, including zero movement, not a statement that a moved cut is perceptually good.

| Edges | Count | Zero | Absolute median, ms | P90, ms | P95, ms | Maximum, ms | Signed range, ms |
|---|---:|---:|---:|---:|---:|---:|---|
| filler | 78 | 0 | 673.333 | 2820.000 | 4413.333 | 6633.333 | -5273.333 to 6633.333 |
| pause | 78 | 10 | 6.667 | 26.667 | 950.000 | 2533.333 | -2533.333 to 16.667 |
| all | 156 | 10 | 153.333 | 1926.667 | 2820.000 | 6633.333 | -5273.333 to 6633.333 |

Magnitude bins: zero: 10, 0–one frame: 63, one frame–100 ms: 2, 100–300 ms: 13, 300–1000 ms: 36, over 1000 ms: 32.

### Refusals and unavailable retries

Times below are exact original source/timeline positions rounded to milliseconds;
the initial clip starts at source and timeline zero at unit speed. Every reason
with fewer than three occurrences lists all its occurrences.

| Reason | Count | Up to three examples, seconds |
|---|---:|---|
| acoustic: no measured quiet or sufficiently deep dip brackets this word boundary | 12 | filler 5.760–5.840 s (`um`); filler 36.240–36.320 s (`um`); filler 180.480–180.560 s (`uh`) |
| acoustic: more than one measured pause separates these word addresses; choose a narrower cut | 51 | pause 9.170–10.250 s (`so / that's`); pause 25.790–26.590 s (`friendly / um`); pause 27.350–28.300 s (`friendly / um`) |
| minimum_cut | 1 | filler 70.560–70.640 s (`uh`) |
| acoustic: the pause is already too short to leave the requested duration | 2 | pause 152.210–153.440 s (`family's / beagle`); pause 238.320–240.040 s (`allergic / to`) |
| acoustic: no project frame lies inside this edge's measured support | 2 | filler 239.840–239.920 s (`um`); filler 312.000–312.080 s (`um`) |
| Original left address no longer survives (script retry status) | 19 | 53.420–54.420 s (`um / so`); 68.970–69.720 s (`um / so`); 129.240–130.180 s (`um / high`) |

The disappeared left-address texts were `uh` × 1, `um` × 17, `way` × 1. These are address observations; the exported audio remains available for audition.

The minimum-cut refusal reports 66,666 us against a 100,000 us minimum.
The ambiguous-pause refusals were not overridden or narrowed by guessed source
edges. Full request positions, refusal evidence and exact resulting edges are
in `run1-results.json` and `run1-calls.jsonl`.

### Independent check

The final `check_dialogue_cut` requested `detail: all`. All selected clips were
checked; no `not_checked` or per-apply `check_failed` result occurred. The table
uses the complete `total` counters, not just the bounded displayed items.

| Check kind | Total | Detail omitted |
|---|---:|---:|
| joins | 80 | 60 |
| edge_in_sound | 0 | 0 |
| short_air | 60 | 43 |
| room_tone_jumps | 10 | 6 |
| dip_intrusions | 14 | 11 |
| pauses | 62 | 44 |
| repeats | 11 | 9 |
| holes | 181 | 157 |
| sounding_holes | 89 | 65 |
| quiet_phrases | 0 | 0 |

Joins are context; pauses, repeats, holes and quiet phrases are warnings. Guard
flags are independent. `sounding_holes` is a subset of `holes`. The wire budget
also omitted 16 other-audio rows; every observed row was zero on this single
audio track. No omitted detail is treated as a passing check.

### Latency, reply size and memory

These are ready-tool calls after the transcript/profile job, not transcription
latency. A check has only one observation per run, so its median equals its worst.
Bytes are the complete serialized MCP `CallToolResult`, excluding JSON-RPC framing.

| Tool | Calls/run | Run 1 median / worst, ms | Warm median / worst, ms | Reply median / worst, bytes |
|---|---:|---:|---:|---:|
| `get_timeline_transcript` | 79 | 31.383 / 33.316 | 31.497 / 34.133 | 42,038 / 47,827 |
| `cut_dialogue` | 152 | 153.319 / 214.408 | 152.572 / 211.927 | 1,972.5 / 5,339 |
| `check_dialogue_cut` | 1 | 66.184 / 66.184 | 66.832 / 66.832 | 24,087 / 24,087 |

Cold here means missing transcript/profile values, with the model installation
already verified and the operating-system file cache warm. Cold transcript/profile
readiness took 38.771671 s; warm readiness
took 30.066 ms. Whole-child wall time was 85.015147 s,
user CPU 175.422984 s, system CPU 25.795253 s. Peak RSS was
**1,058,308 KiB** using Linux `wait4`/`ru_maxrss`, for the entire test
process (both runs, model preparation, engine, reply recording and WAV export),
not a sampled inference-only figure. The launcher and child were nice 19.
One-minute machine load was 2.67 before and 3.72 after.

**Warm-cache replies are byte-identical: True.** This compares every
complete reply in order for all 232 editor/read/check calls, against a fresh
original document; timing metadata is outside the compared reply bytes. The raw
`run1-replies.jsonl` and `run2-replies.jsonl` compare equal with `cmp`. Measurement
progress/status polling is outside this ready-reply comparison.

The WAVs are `target/dq-media/rehearsal/original.wav` and
`target/dq-media/rehearsal/edited.wav`, both independently probed as 48 kHz stereo
`pcm_f32le`, 480.000000 and 338.966667 seconds. Raw traces, both resulting
document snapshots, result accounting, `summary.json`, `resource.json` and the WAV probes
are beside them. The source/model paths are explicit; no application cache or
probe output was used as a transcript substitute.

### Final-code rerun and validation

Two complete cold-transcript/profile plus warm-cache pairs were executed. The
preparatory pair is retained under `target/dq-media/rehearsal/preliminary/`.
After moving the fixture-only wrappers into their own feature-gated module and
finishing lint/format fixes, the final pair used freshly emptied fixture
transcript/profile caches and the same verified model installation. All tables
above use the final pair, rather than selecting a faster run. Its first-run
complete replies also compare byte-for-byte equal to the preparatory first run.

The final gate passed 1,313 media tests (83 ignored), all 15 focused
upload-witness tests, workspace all-target clippy and formatting, workspace build
and test compilation, and all three separate shipping checks and actual builds.
Windows media and agent library/test Rust cross-checks passed; Windows execution
was not run on this Linux host. Native-linking, protocol, provider and dialogue
checks passed 101 agent tests (57 existing/on-demand ignores), and the app checks
passed 115 tests (three ignores). All four CI self-test scripts, slow-test lint
and actionlint passed. The final on-demand rehearsal test then passed with both
runs and the two independently probed WAVs. Evidence is `target/dq-media/r2/`.

The largest observed pre-prune target snapshot was 38,105,147,582 bytes. After
all tests finished, inactive generated test executables were pruned again,
retaining `target/dq1/`, all `target/dq-media/` evidence and both WAVs. The final
target size is **25,714,339,913 bytes** (25.714 GB), below the 40 GB cap.
Protected dialogue-tool files and Core `dialogue/` remain byte-identical to
`517d7f40`; no cut policy was changed to improve the rehearsal result.
All started build and test processes completed before this commit.

## Ruling 6: decoder word ends

The pinned runtime already emits the best end alignment its decoders provide;
no native timestamp arithmetic or word span is changed here. The issue is the
meaning assigned to those spans. Nemotron EN does **not** expose acoustic word
ends. Its greedy RNN-T head has token-emission frame positions, not a duration
prediction. Parakeet's TDT head has predicted token skips, also not acoustic ends.

### Evidence and decision

The inspected source checkout is exactly NeMo-Speech.cpp
`20999cc921d2b14aa58708f61756a8b2e787ba20`, the repository pin. Source and models
were read from the probe without writing there. The two GGUF headers independently
confirm `asr.head_type=rnnt` for EN (no `asr.tdt.durations` entry) and `tdt` for
Parakeet (durations `[0,1,2,3,4]`). Both have encoder subsampling 8 and 10 ms input
hops: one encoder frame is 80 ms. The extracted headers are recorded in
`target/dq-media/word-ends/model-metadata.json`.

At the pinned revision:

- [`RnntModel::make_transducer_decoder`](https://github.com/NVIDIA/NeMo-Speech.cpp/blob/20999cc921d2b14aa58708f61756a8b2e787ba20/src/asr/model.cpp#L2005)
  selects `TdtGreedyDecoder` only when the duration table is nonempty, otherwise
  `RnntGreedyDecoder`.
- [RNN-T `present_token`](https://github.com/NVIDIA/NeMo-Speech.cpp/blob/20999cc921d2b14aa58708f61756a8b2e787ba20/src/asr/decoders/rnnt_greedy_decoder.cpp#L185)
  records the first token's emission frame and updates the end to the last
  non-punctuation token's frame plus one. A single token therefore spans exactly
  one encoder frame. Multiple tokens emitted at that same frame can also have
  an 80 ms span. Blank frames do not extend the word.
- [TDT decoding](https://github.com/NVIDIA/NeMo-Speech.cpp/blob/20999cc921d2b14aa58708f61756a8b2e787ba20/src/asr/decoders/rnnt_greedy_decoder.cpp#L425)
  reads the predicted duration index and uses the final lexical token's frame
  plus `max(1, skip)` as its end. Those durations already reach the word result.
- [`RecognitionStream::set_word_time_`](https://github.com/NVIDIA/NeMo-Speech.cpp/blob/20999cc921d2b14aa58708f61756a8b2e787ba20/src/asr/recognizer.cpp#L638)
  converts these frames to milliseconds and bounds them by input duration.
  [The C API](https://github.com/NVIDIA/NeMo-Speech.cpp/blob/20999cc921d2b14aa58708f61756a8b2e787ba20/src/asr/c_api.cpp#L499)
  returns that end directly. Our FFI converts milliseconds to microseconds; it
  does not insert an 80 ms duration.

The R2 transcript has 758 spans of 80 ms out of 906 words. Its other 148 spans
range from 160 to 880 ms in 80 ms increments. For example, `remote` is
4.320–4.480 s, `actually` 8.080–8.240 s, and `that's` 9.680–10.160 s.
These observations agree with decoder token-frame spans, not a Rust-wide fixed
word-length rule. The histogram is `target/dq-media/word-ends/r2-widths.json`.

Choice left open by the ruling: retain the existing decoder estimates, rather
than extend words through blanks, to the next onset, or through a VAD interval.
None of those would establish the requested word's real end. No model swap,
forced aligner, or acoustic-policy change is introduced. The recognition
provenance is now accurately named `DecoderTokenEstimates` for both backends,
replacing the incorrect blanket `TokenDurationsLagged` tag. Transcript and voice
transcript caches move to v6 so old payloads cannot retain the misleading tag;
the independent sound-profile cache is unchanged.

### Consumers and hole counts

Transcript rendering, the three transcript tool descriptions, and the bound
timeline reply now explicitly label spans as approximate transcript intervals.
The timeline descriptor no longer recommends these boundaries as precise cut
edges. The frame coordinates remain exact representations of approximate
alignments. Core's transcript DTO documentation states this distinction too.
Captions, search, word selection, multicam grouping, scoring and transcript-only
fallbacks still use those intervals for their existing temporal/addressing
purposes; they acquire no claim of measured speech ends. The dialogue resolver
already uses measured quiet/dips for edges; it is unchanged.

The hole audit **does depend** on the supplied ends:
`audit/gaps.rs::gaps` subtracts transcript intervals, then `sound_portions`
measures levels within the remainder. Thus `holes` is a count of sufficiently
long transcript-interval gaps, and `sounding_holes` counts those portions with
level evidence. A real word's tail beyond an 80 ms emission-frame span can
contribute to both. The R2 totals 181 / 89 therefore cannot be interpreted as
181 missing-speech gaps or 89 recognizer omissions.

The report now always includes
`hole_basis: "approximate_transcript_intervals"`, applying to total and omitted
`holes` / `sounding_holes` and their detail rows. This labels the counts rather
than replacing them with invented ends. The existing bounded-reply copier keeps
this report metadata, and the real provider/dispatcher regression asserts its
presence. No edit to worker 1's protected tool files or acoustic/extent policies
is needed.

`dialogue_audit_stub_ends_label_timing_dependent_hole_counts` holds audio and
onset fixed: sound is 100–900 ms, onset 100 ms. A supplied 180 ms end yields
`(holes, sounding_holes) = (1,1)`; a supplied 900 ms end yields `(0,0)`.
Both wire reports must carry the qualification. Deliberately suppressing that
serialized field fails `DQ_WORD_END_HOLE_BASIS`; restoring it passes. The real
provider test also asserts `DQ_WORD_END_DISPATCHED_TRANSCRIPT_BASIS` and
`DQ_WORD_END_DISPATCHED_HOLE_BASIS` through the real dispatcher.

R2's historical files and measures remain unchanged. This is not R3; that rerun
and M5 still await the lead's instructions after worker 1's merge.

The unpaid measurement-pin recorder was run to refresh observed source hashes.
Only four bridge hashes differ: MCP observation plus host, studio and runtime.
The latter three source files are unchanged from `c7d63a08`: their pinned hashes
were already stale after the earlier merge. All four now match actual file
bytes. Model/options choices and limits are unchanged; no provider was launched.
Decoder/accessor fragments were also checked directly against the pinned Git
blobs, recorded in `target/dq-media/word-ends/pinned-source-evidence.json`.

### Ruling 6 validation

The full Core suite passed 898 tests (two ignored); media passed
1,313 tests (83 ignored). The nine real-provider/dispatcher tests and
23 renderer tests passed, including the new qualifier assertions.
Workspace all-target clippy and formatting passed, as did workspace build and
test compilation. All three isolated shipping checks and actual builds passed.
Windows media and agent library/test Rust cross-checks passed; native Windows
execution remains CI's responsibility.

The targeted agent/native-linking/protocol/dialogue checks passed 101 tests,
and app transcript/voice/capture/manual/feedback/exit checks passed 115.
All four CI self-tests, slow-test lint and actionlint passed. Two exact wire
expectations were deliberately updated for the qualifier: Core flag/all-report
sizes increase by 48 bytes (93 → 141 and 20,260 → 20,308), and the legacy transcript
header now includes the timing warning. The existing 2 KiB / 32 KiB assertions
and cut/guard behavior assertions remain unchanged. Final clippy, formatting
and dialogue tests passed after those expectation updates; production code did
not change during that final test correction.

The largest recorded target snapshot was 38,538,375,459 bytes. After all tests
finished, inactive generated executables were pruned, preserving `target/dq1/`
and all `target/dq-media/` evidence. Final target size is 26,147,377,523 bytes
(26.147 GB). No started build or test remains running. Evidence is under
`target/dq-media/word-ends/`; no push, R3 rerun or M5 switch was performed.


## R3: calibrated real-recording rehearsal

The production chain completed twice on the corrected driver, but the quality
targets are **not met**. There are 48 distinct qualifying sound hops, and 18 filler
cuts exceed the envelope built from approximate ASR spans. These results do not
clear M5. The acoustic-span alternative below has zero measured violations but
one unavailable span; it cannot replace the requested ASR-span diagnostic or
establish semantic word boundaries.

R3 runs on merge `14edbdaa`, including worker 1's rulings 1–5 from `2cedfa64`.
The merge's two integration repairs keep `word_timing` on ranged dispatch and give
the storage-budget fixture a voiced calibration reference. The measurement pin
was regenerated from merged source; model-visible manifests remain unchanged.

The input, pinned EN model, native settings, confirmation path, unit source clock
and CPU export path are those of R2. No acoustic or recognition setting was tuned.
The ignored on-demand test remains the sole entry point. All four paths are
explicit; the private R3 data root is inside its output tree, and neither the
probe directory nor application data is written. M5 remains off.

The scripted editor initially pages the transcript in 30-second project ranges,
de-duplicates recording word indices at page boundaries, and selects the production
`gap_runs()` longer than 700 ms plus every standalone filler. Pauses request
300 ms; word removals use the new natural-pause default, without `leave_micros`.
Batches contain at most 32 members and use ready-member application. Only typed
intra-batch conflicts retry singly on the new revision. A missing original anchor remains explicit; no different word is substituted
for a pause or filler request. Missing context uses bounded project ranges,
never a full reread. The test asserts actual neighbour-address reuse. Exact
committed source intervals, addresses used, arguments and every reply are retained.

R3 again emitted 906 words; filler matches were `um` × 40 and `uh` × 14.
The matcher and its six normalized spellings are unchanged from R2. The calibrated
gap evidence changes eligible pauses from 111 to 67; the >700 ms criterion is
unchanged. All 121 candidates have one terminal result. There were 144 submitted
member attempts across 28 cut calls, including conflict retries.

| Kind | Selected | As asked | Moved | Typed refusal | Original address unavailable |
|---|---:|---:|---:|---:|---:|
| filler | 54 | 0 | 19 | 34 | 1 |
| pause | 67 | 0 | 51 | 11 | 5 |

### Outcome and Proof comparison

The new diagnostic columns for R2 are recomputed from its historical cuts, not a
new R2 edit run. Both passes use R3's full-source calibrated gaps and stored noise
floor as one common diagnostic basis. R2's original artifacts are unchanged.

| Measure | R2 | R3 |
|---|---:|---:|
| Selected removal candidates | 165 | 121 |
| Applied cuts | 78 | 70 |
| Fillers applied / terminal refused or unaddressable | 39 / 15 | 19 / 35 |
| Pause trims applied / terminal refused or unaddressable | 39 / 72 | 51 / 16 |
| Whole-batch refusals | 6 | 0 |
| Unrequested words with ASR start inside a cut | 21 | 9 |
| Among those, words with a qualifying sound hop | 14 | 4 |
| Distinct 10 ms grid hops with qualifying sound (all word windows) | 321 | 48 |
| Applied filler length median / p95 / max, s | 1.667 / 7.300 / 9.767 | 1.600 / 5.833 / 5.833 |
| Filler cuts over ASR span + shorter flank + 300 ms | 37 | 18 |
| Filler cuts over acoustically bounded span + shorter flank + 300 ms | 1 | 0 |
| Filler acoustic span unavailable (dip/boundary or same quiet run) | 27 | 1 |
| Transcript reply bytes / applied cut | 42,409.2 | 1,178.0 |
| Duration before → after, s | 480.000000 → 338.966667 | 480.000000 → 389.133333 |
| Peak whole-child RSS, KiB | 1,058,308 | 1,060,784 |
| Warm complete replies byte-identical | True | True |
| Returned neighbour-address uses | 0 | 29 |

The stored speech RMS is **-4468 centidBFS (-44.68 dBFS)**; the stored unvoiced tenth-percentile floor is **-8238 centidBFS (-82.38 dBFS)**. R2 did not store these calibration fields.

R2 has 21 lexical word indices at 20 distinct start times: `the` and `way`
share 260.88 s. The table counts indices, explaining the difference from the
lead's 20-position observation. The complete R2 word list and every R3 hit are in
[the diagnostic evidence](dq-media-r3-diagnostics.json).

### Sound inside cuts and word timing

The independent diagnostic reads the original 16 kHz mono PCM16 take. It computes
RMS on a source-aligned 10 ms grid, clipping boundary hops to samples actually
inside the committed cut and the word's 300 ms window. Samples before a cut or
outside that window cannot contribute. A hit is strictly more than stored floor
plus 15 dB. All unrequested word windows are checked, including words whose ASR
start is before the cut; start-inside-cut words are also counted separately.
Overlapping word windows can name one grid hop more than once; the table counts
distinct (cut, grid-hop) pairs. These are acoustic flags, not a human word audit.

Levels are compared in recording-source units, matching the stored profile.
The production mono-centre mix attenuates the exported stereo channels by
3.0103 dB: the same 31.75 s hop is −38.485369 dBFS in the original export.
The corresponding export-domain floor would also shift by −3.0103 dB;
comparing that attenuated signal to the unshifted source floor would mix units.

Filler lengths are exact committed cut lengths. For the ASR-span envelope, the
filler span is its approximate transcript interval. The shorter flank is the
shorter full-source calibrated gap supporting the two committed edges; a missing
quiet flank contributes zero. The acoustic alternative uses the interval between
two distinct flanking gaps. It is unavailable for a dip/boundary or a filler inside
one quiet run. Neither alternative claims to measure semantic word ends.
The check's `holes` and `sounding_holes` retain
`hole_basis: approximate_transcript_intervals`; their counts do not establish
measured missing speech. Median averages the middle pair; p95 uses nearest rank.

| R3 unrequested word | ASR start, s | Cut source span, s | Qualifying hops |
|---|---:|---|---:|
| that's | 9.680 | 9.433–10.100 | 1 |
| in | 40.480 | 39.233–40.600 | 0 |
| your | 40.560 | 39.233–40.600 | 0 |
| from | 136.960 | 136.400–136.967 | 0 |
| I | 288.080 | 287.500–288.367 | 6 |
| see | 288.320 | 287.500–288.367 | 1 |
| now | 292.720 | 291.233–292.733 | 0 |
| it | 304.080 | 304.033–305.033 | 9 |
| okay | 365.360 | 363.867–365.367 | 0 |

Every qualifying R3 word/hop hit is listed below, in source seconds. The JSON evidence retains unrounded windows and levels.

| Word | Hop start → end, s | RMS, dBFS |
|---|---|---:|
| that's | 9.810000 → 9.820000 | -65.50 |
| were | 31.690000 → 31.700000 | -61.66 |
| were | 31.700000 → 31.710000 | -62.70 |
| were | 31.710000 → 31.720000 | -56.88 |
| were | 31.720000 → 31.730000 | -39.84 |
| were | 31.730000 → 31.740000 | -37.09 |
| were | 31.740000 → 31.750000 | -35.76 |
| were | 31.750000 → 31.760000 | -35.48 |
| were | 31.760000 → 31.770000 | -35.86 |
| were | 31.770000 → 31.780000 | -36.22 |
| were | 31.780000 → 31.790000 | -36.23 |
| were | 31.790000 → 31.800000 | -36.01 |
| were | 31.800000 → 31.810000 | -35.91 |
| were | 31.810000 → 31.820000 | -36.15 |
| natural | 241.240000 → 241.250000 | -64.99 |
| natural | 241.250000 → 241.260000 | -66.40 |
| choice | 241.240000 → 241.250000 | -64.99 |
| choice | 241.250000 → 241.260000 | -66.40 |
| choice | 241.270000 → 241.280000 | -66.50 |
| choice | 241.280000 → 241.290000 | -64.70 |
| choice | 241.310000 → 241.320000 | -63.47 |
| choice | 241.320000 → 241.330000 | -67.18 |
| choice | 241.330000 → 241.340000 | -66.92 |
| choice | 241.340000 → 241.350000 | -64.54 |
| choice | 241.350000 → 241.360000 | -65.18 |
| about | 255.470000 → 255.480000 | -66.69 |
| about | 255.490000 → 255.500000 | -64.36 |
| about | 255.500000 → 255.510000 | -57.17 |
| about | 255.510000 → 255.520000 | -67.19 |
| about | 255.520000 → 255.530000 | -66.05 |
| about | 255.530000 → 255.540000 | -65.12 |
| about | 255.540000 → 255.550000 | -65.62 |
| about | 255.550000 → 255.560000 | -66.06 |
| about | 255.650000 → 255.660000 | -67.29 |
| I | 288.120000 → 288.130000 | -66.67 |
| I | 288.180000 → 288.190000 | -67.19 |
| I | 288.200000 → 288.210000 | -67.06 |
| I | 288.260000 → 288.270000 | -67.21 |
| I | 288.310000 → 288.320000 | -65.02 |
| I | 288.350000 → 288.360000 | -66.95 |
| see | 288.350000 → 288.360000 | -66.95 |
| is | 304.180000 → 304.190000 | -64.90 |
| is | 304.190000 → 304.200000 | -62.38 |
| is | 304.200000 → 304.210000 | -58.99 |
| is | 304.210000 → 304.220000 | -61.43 |
| it | 304.180000 → 304.190000 | -64.90 |
| it | 304.190000 → 304.200000 | -62.38 |
| it | 304.200000 → 304.210000 | -58.99 |
| it | 304.210000 → 304.220000 | -61.43 |
| it | 304.260000 → 304.270000 | -65.38 |
| it | 304.280000 → 304.290000 | -63.49 |
| it | 304.300000 → 304.310000 | -65.93 |
| it | 304.310000 → 304.320000 | -67.02 |
| it | 304.340000 → 304.350000 | -66.86 |
| and | 322.033333 → 322.040000 | -66.61 |

One concrete hit is the `um` request at 32.48 s. It cuts source
31.666667–33.200000 s. The unrequested `were` has ASR start 31.52 s, before that
cut, and its 31.75–31.76 s hop measures −35.48 dBFS inside the cut. An independent
FFmpeg `astats` check gives −35.475026 dBFS. This is why the diagnostic also checks
windows beginning before a cut. Every reported cut interval was independently
verified against the complement of retained source ranges in the committed
result document, for both R2 and R3.

### Terminal refusals and pause-trim outcomes

| Code / reason | R2 | R3 | Three R3 examples at original source times, s |
|---|---:|---:|---|
| acoustic: more than one measured pause separates these word addresses; choose a narrower cut | 51 | 0 |  |
| acoustic: no measured quiet or sufficiently deep dip brackets this word boundary | 12 | 25 | filler 5.760–5.840 (`um`); filler 36.240–36.320 (`um`); filler 70.560–70.640 (`uh`) |
| acoustic: no project frame lies inside this edge's measured support | 2 | 8 | filler 22.400–22.480 (`um`); filler 53.040–53.120 (`um`); filler 178.560–178.640 (`um`) |
| acoustic: the measured edges do not bracket one forward removal | 0 | 1 | filler 28.000–28.080 (`um`) |
| acoustic: the pause is already too short to leave the requested duration | 2 | 11 | pause 13.940–14.690 (`it / and`); pause 81.060–81.920 (`good / alright`); pause 90.680–91.420 (`so / this`) |
| minimum_cut | 1 | 0 |  |
| no_surviving_address | 19 | 6 | pause 129.350–130.150 (`um / high`); filler 275.600–275.680 (`um`); pause 301.850–302.910 (`uh / what`) |

The 16 terminal pause failures are 11 `acoustic` / already-too-short refusals
and five missing original addresses. The latter are client statuses, not tool
refusals or successful trims. Twenty-nine initial conflict outcomes deferred;
24 were retried singly and five no longer had their original anchor. No later
member was silently reassigned to an unrelated pause.

### Edge movement

All 70 applied cuts moved at least one edge. Movement is from the original ASR
span or requested measured pause minus the unchanged 3:2 air split, including
frame alignment. These magnitudes do not establish perceptual quality.

| All edges | R2 | R3 |
|---|---:|---:|
| Count | 156 | 140 |
| Absolute median / p90 / p95 / max, ms | 153.333 / 1926.667 / 2820.000 / 6633.333 | 13.333 / 2816.667 / 3773.333 / 5546.667 |
| Signed range, ms | -5273.333 to 6633.333 | -5546.667 to 5536.667 |

### Call costs and independent check

The next table reports the first editing run after readiness, in the unoptimized Rust test profile with the existing Release native SDK. Timings include scripted consent and complete reply serialization. They are descriptive observations, not a throughput benchmark.

| Tool | Calls R2 / R3 | R2 median / worst, ms | R3 median / worst, ms | R2 median / worst bytes | R3 median / worst bytes |
|---|---:|---:|---:|---:|---:|
| `get_timeline_transcript` | 79 / 24 | 31.383 / 33.316 | 28.491 / 32.029 | 42,038.0 / 47,827.0 | 3,068.5 / 11,850.0 |
| `cut_dialogue` | 152 / 28 | 153.319 / 214.408 | 150.235 / 572.108 | 1,972.5 / 5,339.0 | 1,643.5 / 16,719.0 |
| `check_dialogue_cut` | 1 / 1 | 66.184 / 66.184 | 65.210 / 65.210 | 24,087.0 / 24,087.0 | 19,466.0 / 19,466.0 |

| Check total | R2 | R3 |
|---|---:|---:|
| joins | 80 | 72 |
| edge_in_sound | 0 | 0 |
| short_air | 60 | 24 |
| room_tone_jumps | 10 | 4 |
| dip_intrusions | 14 | 1 |
| pauses | 62 | 91 |
| repeats | 11 | 11 |
| holes | 181 | 238 |
| sounding_holes | 89 | 110 |
| quiet_phrases | 0 | 0 |

All counts use complete totals, not the bounded displayed items. Guard flags and warnings are distinct; joins are context. Full omitted counters and reports are retained in the JSON evidence.

R2 whole-child wall time 85.015 s; load before / after 2.67 / 3.72. RSS uses Linux `wait4` for the entire child, including both runs, model preparation, engine and WAV exports.

R3 whole-child wall time 55.921 s; load before / after 1.47 / 2.17. RSS uses Linux `wait4` for the entire child, including both runs, model preparation, engine and WAV exports.

Cold transcript/profile readiness took 37.035445 s; warm readiness took
28.325 ms. The complete child used 138.050381 s user CPU and 9.285309 s system
CPU, across both runs and exports, on this 20-logical-CPU host. No per-apply
`check_failed` occurred, and the final check returned all selected segments.
Original WAVs compare byte-for-byte equal between R2 and R3. The two complete
R3 ready-reply files also compare byte-for-byte equal. Both exports are 48 kHz
stereo `pcm_f32le`; the new edited WAV is 389.133333 s.

A preparatory pair exposed an unsafe client readdressing rule: the missing pause
anchor at 301.85 s was replaced with a kept word whose available clip selected a
different gap around 296.2 s. That pair is retained under
`preliminary-retarget/`, and is excluded from these tables. The final driver
refuses to substitute any missing original anchor. Its private transcript and
profile caches were emptied before the reported final pair; the verified model
installation was retained. This was a client correction, not cut-setting tuning.

### Artifacts and validation

R3 evidence is under `target/dq-media/rehearsal/r3/`: `measurement.json`,
`run{1,2}-calls.jsonl`, complete `run{1,2}-replies.jsonl`, terminal results,
document snapshots, `summary.json`, `comparison.json`, `resource.json` and both
WAVs. `original.wav` and the new `edited.wav` use the production export mixer,
with no GPU or audio device. The direct test ran under `unshare -rn`, so no model
or other network download was possible. The explicit model was hash-verified
and copied to the private installation through the existing provisioning path.
The warm comparison uses a fresh Core/dispatcher/provider and the original
project, retaining only the private caches. Status polling is outside the
ready-reply byte comparison.

Warm-run latency observations (the complete reply bytes are identical):

| Tool | R2 warm median / worst, ms | R3 warm median / worst, ms |
|---|---:|---:|
| `get_timeline_transcript` | 31.497 / 34.133 | 28.330 / 29.287 |
| `cut_dialogue` | 152.572 / 211.927 | 147.770 / 561.530 |
| `check_dialogue_cut` | 66.832 / 66.832 | 65.552 / 65.552 |

### Final gate

The merge gate passed the full default-tier workspace suite (**5,585 passed,
277 existing ignores**), a separate `--test mcp_server` run (**76 passed,
10 ignored**), and all **13** unpaid measurement tests, including both pin
checks. The pin generator changed only the merged server's source hash;
model-visible manifests and the accepted registry byte pins stayed unchanged.
The merge repairs have red/green witnesses in `target/dq-media/r3-merge/`.
Workspace build and test compilation, the four CI self-test scripts, actionlint
and slow-test lint passed. After the test-helper length repair, the focused
provider checks and warnings-denied workspace clippy passed again.

The final R3 ignored test passed with both real-model runs. Its final code also
passes workspace all-target clippy with `-D warnings`, Cargo fmt, explicit
included-file rustfmt, slow-test lint and the three separate shipping builds:
`kinewright-agent --no-default-features --lib`,
`kinewright-app --no-default-features --bin kinewright-app`, and
`kinewright-agent --no-default-features --bin kinewright`.
The shipping builds retain an existing feature-disabled dead-code warning;
warnings-denied clippy is clean. No protected product file was changed for R3.

Windows execution was not run on this Linux host. Optional MSVC cross-checks
were cancelled while still queued, before either compiler command started;
they are not claimed as passing evidence. The required Linux gates passed.
M5 remains off. All started builds and tests completed before this commit.

The largest observed target snapshot was 37,343,152,336 bytes. Final `target/`
is **28,829,687,984 bytes**, below 40 GB, retaining `target/dq1/` and all
`target/dq-media/` evidence, including the preparatory pair and final WAVs.

## R4: nearest edges and measured sound ownership

This rehearsal uses the same AMI take, pinned EN model, driver source and client
settings as R3. Only the merged production resolver/evidence/audit rules changed.
M5 remains off. The script still requests pauses longer than 700 ms to leave
300 ms, removes the same standalone fillers with natural default air, sends
ready-member batches of at most 32, retries only conflicts, and uses ranged
reads and returned neighbour addresses. Missing original anchors are not replaced.

The merge is `d159e7b1` (dq1 R3 `0b43d5fc` plus dq2 `91de5baf`). The driver
SHA-256 is `31c49d363eecd7bd45cf6dd52fa99b0f6deac6ba0991e9f183d72b44ef3f14fc`,
unchanged from R3. Audio identity is
`3565188c90972ee3bc6993345c6ecbd611b5957abe12c4d56fa7ffad21c5b4c0`
(15,360,078 bytes). Both runs have the same 906 transcript words, measured
profile and gap runs. The standalone filler matches are **40 `um`, 14 `uh`**.
All remaining settings compare equal after removing the obsolete dip field;
its replacement is the merged calibration rule, not a client adjustment.

### R4 proof beside R3

| Measure | R3 | R4 |
|---|---:|---:|
| Stored speech / noise floor, dBFS | -44.68 / -82.38 | -44.68 / -82.38 |
| Selected removal candidates | 121 | 121 |
| Applied cuts | 70 | 56 |
| Applied exactly as requested / after moving an edge | 0 / 70 | 0 / 56 |
| Unrequested ASR starts inside cuts | 9 | 0 |
| ASR starts not selected by that particular cut (stricter control) | 10 | 0 |
| Sound-hit grid hops above calibrated quiet | 21 | 0 |
| Unrequested words with an above-quiet hit | 5 | 0 |
| Among start-inside words, words with above-quiet sound | 1 | 0 |
| Floor +15 dB hit hops (context) | 48 | 0 |
| Acoustic filler envelope: over / checked / unavailable | 0 / 18 / 1 | 0 / 6 / 4 |
| ASR-span envelope count (context only) | 18 | 7 |
| Applied filler cut length median / p95 / max, s | 1.600 / 5.833 / 5.833 | 0.617 / 1.233 / 1.233 |
| Whole-batch refusals | 0 | 0 |
| Transcript reply bytes / applied cut | 1,178.0 | 1,430.2 |
| Edited duration from 480 s, s | 389.133333 | 429.233333 |
| Peak whole-child RSS, KiB | 1,060,784 | 1,064,748 |
| Warm complete replies byte-identical | True | True |
| Returned neighbour-address uses | 29 | 26 |
| `were` source 31.72–31.82 s fully kept | False | True |

The effective dip limit changes from **−79.68 to −64.69 dBFS** under the merged
floor/speech rule; no client setting is changed. The derivation and the 25 R3
refusal-window measurements are in `dq-agent-tools.md`, R8.
The primary sound threshold is **−64.69 dBFS**, the prepared recording's calibrated
quiet RMS limit. **−67.38 dBFS** (stored floor +15) remains context. Both passes
are re-audited with both thresholds using the same raw recording and unchanged
10 ms grid, with boundary hops clipped to the cut and the unrequested word's
300 ms forward window. Source units match the stored profile, before the
production export's mono-centre gain. Counts de-duplicate (cut, grid-hop) pairs.
The main count preserves R3's meaning: a word not requested by any filler
removal in the script. The stricter control excludes only the filler selected
by that particular member; a pause trim selects no words. Both controls use
half-open removed source intervals. R3's extra per-member start is the `um` at
275.60 s, inside another member's 275.133333–276.466667 s cut; it adds no
above-quiet hit hop. Every reported cut
interval is verified against source coverage actually missing from the committed
result document. No unrequested word is exempted merely because it starts before
the cut. ASR ends remain approximate and do not establish speech ownership.

The acoustic filler envelope is the interval between two distinct calibrated
quiet flanks supporting its committed edges, plus the shorter full-source flank
and 300 ms. Dip/boundary or same-gap cases without two distinct flanks remain
explicitly unavailable, never counted as passing. The old ASR-span envelope is
context only; an 80 ms stub does not measure acoustic filler duration.

The acoustic-envelope target is verified for **six** applied fillers, with zero
overruns. **Four remain unmeasurable under this definition**; zero measured
overruns is not a proof for all ten. Their full evidence is retained in the
JSON, with no substitute ASR-duration estimate:

| Filler source start | Applied source cut, s | Missing evidence |
|---:|---|---|
| 28.000 (`um`) | 27.666667–28.033333 | Both edges sit in the same 27.52–28.18 quiet run |
| 309.200 (`um`) | 309.200000–309.633333 | Start edge lacks a quiet flank; end has 309.25–309.75 |
| 409.920 (`uh`) | 409.800000–410.233333 | End edge lacks a quiet flank; start has 409.67–409.84 |
| 472.880 (`uh`) | 472.900000–473.133333 | Both edges sit in the same 472.84–473.25 quiet run |

### Fillers and pauses

| Kind | R3 selected / applied / refused / unavailable | R4 selected / applied / refused / unavailable |
|---|---:|---:|
| filler | 54 / 19 / 34 / 1 | 54 / 10 / 44 / 0 |
| pause | 67 / 51 / 11 / 5 | 67 / 46 / 19 / 2 |

| Kind and terminal code | R3 | R4 | Up to three R4 examples, original source seconds |
|---|---:|---:|---|
| filler: acoustic: no measured quiet or sufficiently deep dip brackets this word boundary | 25 | 21 | 5.760–5.840 (`um`); 36.240–36.320 (`um`); 70.560–70.640 (`uh`) |
| filler: acoustic: no project frame lies inside this edge's measured support | 8 | 1 | 22.400–22.480 (`um`) |
| filler: acoustic: the measured edges do not bracket one forward removal | 1 | 1 | 299.040–299.120 (`uh`) |
| filler: no_surviving_address | 1 | 0 |  |
| filler: selection_evidence/unattributed_sound | 0 | 14 | 19.120–19.200 (`um`); 53.040–53.120 (`um`); 68.640–68.720 (`um`) |
| filler: selection_evidence/unrequested_sound | 0 | 6 | 32.480–32.560 (`um`); 126.720–126.800 (`uh`); 234.640–234.720 (`um`) |
| filler: selection_evidence/unrequested_word_start | 0 | 1 | 40.320–40.400 (`um`) |
| pause: acoustic: the pause is already too short to leave the requested duration | 11 | 0 |  |
| pause: no_surviving_address | 5 | 2 | 373.060–374.830 (`um / okay`); 381.310–382.050 (`um / so`) |
| pause: selection_evidence/pause_limited | 0 | 13 | 13.940–14.690 (`it / and`); 81.060–81.920 (`good / alright`); 90.680–91.420 (`so / this`) |
| pause: selection_evidence/unrequested_word_start | 0 | 6 | 9.260–10.220 (`so / that's`); 136.230–137.080 (`affection / from`); 274.960–276.590 (`home / um`) |

### The named `were` case

The 31.72–31.82 s source interval is tested against **all** applied cuts, not just
the associated filler request. Its source coverage result is shown above. The
terminal result for the `um` request at 32.48 s is retained verbatim in the
[R4 diagnostic JSON](dq-media-r4-diagnostics.json). No replacement anchor or
overrule is used. **`were` is kept**, while that `um` removal is refused with
`selection_evidence/unrequested_sound`: component 31.69–32.45 s, kept word 64.
The kept source interval maps to **29.053333–29.153333 s in `edited.wav`**.
Its RMS is −36.320 dBFS in the source and −39.329 dBFS across the stereo export,
a −3.009 dB difference consistent with the production mono-centre gain. This
checks the written audio as well as the final document's source coverage.

### Sound and start evidence

Every above-quiet R4 hit is listed below. Floor+15-only windows and all exact
source intervals are retained in the diagnostic JSON.

| Word | Hop start–end, source seconds | RMS dBFS |
|---|---|---:|
| None | | |

Unrequested ASR starts inside R4 cuts:

| Word | Source start, s | Cut, source seconds |
|---|---:|---|
| None | | |

### Audit semantics and call costs

R4's holes use `hole_basis: {kind: measured_voice_without_nearby_word_start,
after_word_start_us: 1000000}`. This measures VAD voice outside the one-second
forward support of known word starts, with a 100 ms playback minimum; sounding
uses calibrated quiet RMS. It reads no supplied word end. Long tails and VAD/onset
errors can still warn, so it is not proof of omitted speech. R3's approximate-word-
interval counts have a different meaning and are shown only for context.

| Check total | R3 | R4 |
|---|---:|---:|
| joins | 72 | 58 |
| edge_in_sound | 0 | 0 |
| short_air | 24 | 3 |
| room_tone_jumps | 4 | 5 |
| dip_intrusions | 1 | 1 |
| pauses | 91 | 110 |
| repeats | 11 | 10 |
| holes | 238 | 86 |
| sounding_holes | 110 | 79 |
| quiet_phrases | 0 | 0 |

| Tool | R3 calls | R4 calls | R4 median / worst, ms | R4 median / worst reply bytes |
|---|---:|---:|---:|---:|
| `get_timeline_transcript` | 24 | 21 | 29.317 / 30.497 | 3,116.0 / 11,850.0 |
| `cut_dialogue` | 28 | 25 | 33.046 / 405.619 | 688.0 / 14,040.0 |
| `check_dialogue_cut` | 1 | 1 | 57.019 / 57.019 | 16,651.0 / 16,651.0 |

The changed mix of applied and refused calls limits timing comparisons; R4
applies 56 cuts rather than 70.

These are one cold/warm pair in the same unoptimized Rust test profile and
Release native SDK as R3, with scripted consent and full reply serialization.
They are descriptive observations, not a throughput benchmark. `wait4` RSS covers
both runs, model preparation, engine and WAV exports. R4 load averages before /
after were 2.76/3.35/4.05 and 2.81/3.31/3.99. Model/profile readiness,
whole-child CPU/wall/load and warm byte equality are retained in the evidence.
The child runs under `unshare -rn` with networking disabled. Model and private
cache paths are explicit; it does not discover application data. The explicit pinned model uses the existing hash-checked provisioning
path. Audio uses the production export mixer, no GPU or audio device.

Artifacts are under `target/dq-media/rehearsal/r4/`, including the new `edited.wav`,
complete replies, calls, document snapshots, profile, results, resource counters
and comparison JSON. The historical R3 evidence is unchanged.


| Applied edge moves | R3 | R4 |
|---|---:|---:|
| Absolute median / p95 / max, ms | 13.333 / 3,773.333 / 5,546.667 | 13.333 / 3,813.333 / 5,546.667 |
| Signed min / max, ms | -5,546.667 / 5,536.667 | -5,546.667 / 5,536.667 |
| Cold / warm transcript-profile readiness, ms | 37,035.445 / 28.325 | 37,321.961 / 29.456 |
| Whole-child wall / user / system, s | 55.921 / 138.050 / 9.285 | 54.150 / 134.908 / 7.809 |

Warm-run latency observations (reply bytes remain identical):

| Tool | R4 warm median / worst, ms |
|---|---:|
| `get_timeline_transcript` | 29.061 / 29.533 |
| `cut_dialogue` | 34.413 / 408.254 |
| `check_dialogue_cut` | 55.798 / 55.798 |

### R4 validation and limits

The unchanged ignored test
`server::tests::dialogue_rehearsal_tests::dq_media_real_recording_rehearsal`
passed, including both complete runs. Its **47 ready replies** compare
byte-for-byte equal. The original export is byte-for-byte equal to R3.
The edited export is **429.233333 s**, stereo 48 kHz float PCM. The diagnostics
also preserve all typed refusal examples, precise rational source intervals,
resource counters and the four unavailable acoustic envelopes.

The merge gate passed the complete default-tier workspace suite: **5,604 passed,
0 failed, 277 existing ignores**, across 92 test/doc-test results. A separate
`--test mcp_server` run passed **76 / 0 / 10**. The focused provider tests passed
**9 / 0 / 1**; the existing on-demand rehearsal was the ignore. Workspace build,
workspace test compilation, warnings-denied all-target clippy, Cargo fmt,
explicit included-file rustfmt, slow-test lint, actionlint and all four CI
selftest scripts passed. The pin generator was run against the merged source;
the full generated JSON is unchanged, and both pin tests passed. **No re-pin
commit is needed.**

The three shipping builds passed as separate Cargo invocations:
`cargo build -p kinewright-agent --no-default-features --lib`,
`cargo build -p kinewright-app --no-default-features --bin kinewright-app`, and
`cargo build -p kinewright-agent --no-default-features --bin kinewright`.
Clippy and the rehearsal test compilation passed again immediately before R4.
Only the report and its diagnostic JSON follow the verified merge; the driver,
settings and protected production files receive no further edits.

These are local Linux results. Windows execution and hosted CI are not claimed.
No GPU hardware, audio device, model download, application-data discovery or
probe-directory write was used. M5 remains off. All started builds and tests
finished before the result commit.

One post-pin gate snapshot briefly reached **40,366,032,315 bytes**; it was
immediately pruned to 28.0 GB. Inactive generated test executables were removed,
with `target/dq1/` and all `target/dq-media/` evidence retained. Final target size
is **29.4 GB** (decimal), below the 40 GB limit.


## M5: enable measured dialogue editing on the app branch

M5 starts from `5d0e8b4f`, after the lead accepted R4. The app branch now
advertises `cut_dialogue`, `check_dialogue_cut` and the addressed
`get_timeline_transcript` schema. `SERVE_DIALOGUE_TOOLS` is true. The seven
compact directly served tools remain unchanged; the dialogue descriptors open
through the existing capability dispatcher.

`get_dialogue_pacing` is deleted from discovery, invocation, the inspector
name list, schemas, handler and formatter. The product pacing module and its
tool-only tests are removed. The old unaddressed timeline-transcript handler,
its renderer, public export, frame helper and golden test, and the test-only
opt-in catalog override are also gone. There is no alias,
deprecation route, old-format fallback or compatibility shim.

### Deletion plan and related tools kept

The public deletion list is unchanged from M0: remove `get_dialogue_pacing`.
One private-code detail changed after the caller audit. The sentence-pause
classifier also supplies the independent benchmark's `DialoguePauseBounds`
assertion. Its calculation and oracle tests now live only in
`eval/sentence_pauses.rs`, under the eval feature. The shipping product no
longer compiles or calls that classifier. The benchmark prompt and published
v4 manifest drop the retired inspector lookup and step, along with its obsolete
required-tool-use assertion. The eval runner has no measurement adapter, so it
does not require an always-unready replacement call. Independent scoring and
ground truth are unchanged. No paid benchmark run was made.

These related tools remain because the new pair does not cover their job:

| Tool | Distinct purpose |
|---|---|
| `get_transcript`, `get_transcripts` | Source-frame transcripts of assets, including unplaced takes and caption inputs. |
| `get_timeline_transcript` | Current recording indices and clip bindings required to address the new cuts; optional project or word ranges. |
| `get_silences`, `get_timeline_silences` | Generic source/timeline silence evidence, including non-dialogue audio, used by other planners. |
| `plan_dialogue_assembly` | Select and assemble multiple source takes into a new ordered timeline. The new cut tool removes from existing clips. |
| `plan_dialogue_repair` | Prepare audio repair effects; it does not remove addressed speech or trim pauses. |
| `get_editorial_readiness` | Check broader delivery and editorial completion, beyond dialogue joins. |

The on-demand cut guide remains **1,228 UTF-8 bytes**, below its 1,500-byte
bound, with the same hash. It still points at ranged transcript reads and the
current revision and neighbour addresses returned by cuts. It is returned only
with the cut schema, once per lookup or batch lookup, not in standing tool
instructions. The removed inspector has no remaining current guide entry.

### Check the clip shapes proved by M2

The check no longer rejects a unit-speed played span merely because its enabled
run begins inside its clip. It audits the production timeline segment's exact
Core source span. Coverage includes equal clocks, 25 fps in 24, NTSC in 30,
60 fps in 30, nonzero source in-points, stored-source padding, enabled runs
inside clips, and ranged inspection inside a clip. The existing M2 PCM tests
still compare export and mixer output with Core within one 48 kHz sample.
Retimed clips, overlapping dialogue and unavailable measurements retain their
typed `not_checked` reasons. Cut/retention geometry and safety policy are not
relaxed by this check-only change.

### Model-visible bytes against `5d0e8b4f`

The following values use the existing `ToolSurfaceMetrics` pin method: sums of
compact serialized Tool descriptors, input schemas and UTF-8 descriptions.

| Measurement | At `5d0e8b4f` | M5 | Change |
|---|---:|---:|---:|
| App registry capabilities | 153 | 154 | +1 |
| Registry descriptor bytes | 1,978,588 | 1,981,514 | +2,926 |
| Input-schema bytes | 1,824,687 | 1,826,246 | +1,559 |
| Description bytes | 128,776 | 130,006 | +1,230 |
| Directly served descriptor bytes | 5,660 | 5,660 | 0 |
| Directly served description bytes | 998 | 998 | 0 |
| On-demand guide text bytes | 1,228 | 1,228 | 0 |
| Native task manifests and fixed prefixes | All six roles | Byte-identical | 0 |

The removed inspector cost 1,882 descriptor bytes and 360 description bytes.
The added cut/check descriptors cost 2,769/1,550 bytes, with 817/717 description
bytes. The addressed transcript descriptor changes from 1,125 to 1,614 bytes;
its description changes from 247 to 303 bytes. Those observed changes sum to
the registry delta above. The registry size/count tests and MCP integration
pins are updated from the serialized observations. The measurement pin is
regenerated with its native names, schemas, fixed prefixes and model options
asserted equal to the baseline; only `arm_bridge_sha256.mcp_observation` changes
to the final server source hash.

### Native measurement wiring remains a separate job

Per the lead's M5 ruling, native task roles do **not** advertise
`get_timeline_transcript`, `cut_dialogue` or `check_dialogue_cut`.
`start_task_isolated` and `start_host_isolated` have no installed measurement
provider. Their unconfigured host catalog omits all three; task admission also
refuses them. Director, Editor, Motion, Audio, Colour and Reviewer manifests
retain their existing names and bytes. No permanent `NotMeasured` tool is
advertised to those models.

**Open item for the separately scheduled native job:** install certified,
task-scoped dialogue measurements through owned-input admission, with the
endpoint's Core, retained source authority, content identity and audio stream.
There must be no file-path or legacy-media fallback. Prove the pending-to-ready
flow and cut/check through the production native endpoint before adding these
tools to its role manifests and re-pinning those manifests.

### Verification

The new assertions are:

- `dq_m5_catalog_and_dispatch_expose_measurements_without_pacing`: app catalog
  and dispatch contain the new tools and reject the removed name and lookup.
- `dq_m5_check_accepts_renderer_proven_clocks_and_enabled_origins`: the eight
  proven clock/span shapes have measured check results through the dispatcher.
- `dq_m5_native_manifests_omit_unwired_dialogue_while_app_catalog_serves_it`:
  all six native role schemas omit the tools, the unconfigured host catalog
  hides them, and the app branch catalog includes them.

The first full Agent run exposed two integration misses before publication:
`check_dialogue_cut` was inferred as an Action despite its read-only annotation,
and the destructive-hint count still expected 15. The existing explicit kind
map now classifies the check as an Inspector. The hint pin is 16 for the added
cut tool, with all seven destructive Operation assertions retained. Both
regressions then passed directly. The failed and corrected logs are retained
under `target/dq-media/m5/`; no assertion was relaxed to accept the wrong kind.

The lead reduced the local gate during M5 to the changed crate's all-target
Clippy, its slow tests including `mcp_server`, the pin tests, and formatting.
The full workspace suite and the three shipping builds are deferred to CI.
A subsequent extra workspace build was stopped when that ruling arrived.

Two earlier expanded gate batches ended with SIGTERM before completing;
the cause was not established. Their completed results and interrupted logs
are retained, but neither command is claimed as a complete passing gate.
One redundant standalone MCP retry omitted the software-renderer environment
and opened GPU devices. It was stopped and discarded, then rerun successfully
with lavapipe explicitly selected (86 passed). The final focused gate also
selects lavapipe; the running Agent test had no GPU device file descriptors.

The final Agent package run completed **34 targets: 1,896 passed, zero failed,
105 ignored**, with `slow-tests,te1-eval` enabled. This includes all **86 MCP
integration tests**, all **13 measurement/pin tests**, the 1,493 library tests,
the eval-v4 manifest preflight, native-link/public-API probes and four doc tests.
The ignored tests retain their on-demand or equipped-session gates; no model
was downloaded and no paid benchmark/provider run was made.

Final commands, under the workstation lock with four build jobs and lavapipe:

```text
cargo test --config .cargo/ci-test-linux-slow.toml -p kinewright-agent --features slow-tests,te1-eval -- --test-threads=4
cargo clippy -p kinewright-agent --all-targets --features slow-tests,te1-eval -- -D warnings
cargo fmt -- --check
```

All passed. The explicitly included dialogue test files also pass standalone
`rustfmt --check`. Slow-test lint reports 140 slow tests and 172 well-formed
allowlist entries; `actionlint` passes. The four CI self-test scripts pass
100 tests altogether (slow manifest 52, Linux dependencies 15, shards 24,
NeMo provisioning 9). The earlier default and slow-feature workspace Clippy
checks also passed before the gate was reduced. Windows execution, the full
workspace suite and the three shipping builds remain CI checks, not claimed
local completions.

After pruning inactive test executables, final `target/` is **21,117,625,081 bytes
(21.118 GB)**; the largest recorded M5 snapshot is
**31,347,012,238 bytes**. `target/dq1/` and `target/dq-media/` evidence are
preserved. All 20 tested source-file hashes still match. Every started local
build/test process has finished or been stopped and reaped; no push was made.
