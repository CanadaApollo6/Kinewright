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
`derived-analysis/v1/sound-profiles/v1/`; its inventory and scoped clear include
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

M3 will request background work when either cache half is missing, returning a
typed not-measured status rather than running inference inline. Its resident
bound-value cap is proposed at 64 MiB, counting words and profiles. The current
unit `NotMeasured` refusal needs a status payload, including its protected callers.

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
