# Dialogue level measurement and guide

## Part A: streaming levels

`dialogue::measure::ProfileBuilder` accepts a nonzero rate and channel count,
then arbitrary interleaved f32 slices, including slices that end inside a frame.
It retains one f64 energy sum per channel, one hop's counters/peak and the two
output level arrays. It never retains PCM. `finish(Some(intervals))` supplies
measured voice activity. `Some(&[])` means measured no voice; `None` refuses
with `VoiceUnavailable`. A failed feed remains failed, so ignoring a non-finite
sample error cannot turn a partial prefix into a profile.

Hop assignment and the ceiling duration use integer u128 arithmetic. Levels
use maximum absolute sample and maximum per-channel RMS, then nearest centidBFS
with floor/full-scale saturation. Only this module uses floating arithmetic;
its docs state the possible one-centidB rounding-tie difference across machines
and that profiles are local cache values. Incomplete frames, empty input and
invalid/overlapping/out-of-recording voice intervals are typed refusals.

The first four hop starts and ceiling durations have literal numeric oracles:

| Rate | Frame starts | Frames fed | Duration, us |
| ---: | --- | ---: | ---: |
| 44,100 | 0, 441, 882, 1,323 | 1,324 | 30,023 |
| 48,000 | 0, 480, 960, 1,440 | 1,441 | 30,021 |
| 22,050 | 0, 221, 441, 662 | 663 | 30,069 |
| 16,000 | 0, 160, 320, 480 | 481 | 30,063 |

Ten measurement tests passed. They cover exact silence, full scale, half scale,
saturation, sine RMS, a loud stereo channel, bit-identical chunking, fractional
rate boundaries, duration, partial frames, sticky non-finite errors and voice
interval endpoints. The large-index clock assertion checks the integer quotient
at frame 9,007,199,254,741,147 without allocating a recording of that length.

Four deliberate breaks, each reverted byte-for-byte, each 0 passed / 1 failed
at its named assertion, followed by 1 passed / 0 failed restored:

| Break | Assertion |
| --- | --- |
| Mean of channel RMS values | `MEASURE_CHANNEL_MAX` |
| Float hop index | `MEASURE_INTEGER_HOP_CLOCK` |
| Floored duration | `MEASURE_DURATION_CEIL` |
| Closed voice interval end | `MEASURE_VOICE_HALF_OPEN_END` |

Full Core tests passed: 894 passed / 0 failed / 2 ignored, including doctests.
Workspace all-target clippy with `-D warnings` and fmt passed. The focused tests
ran in under 0.01 s; the full Core command took 30.84 s wall, including compilation.
The four pairs took 148.76 s wall. Logs/proofs are under
`target/dq-levels-guide/`. No Media source, profile persisted shape, or tool
serving switch changed. No provider or app launch ran.

Part B is deferred until the higher-priority dialogue-tools review fix is done.
The final stack still owes the workspace fast/build and platform/shipping gates.
