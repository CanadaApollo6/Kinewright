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
