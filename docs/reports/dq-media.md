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

After rebuilding the fixture host with `eval-harness`, 78 targeted agent tests
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
