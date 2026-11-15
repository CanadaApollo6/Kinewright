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

## Part B: the on-demand dialogue guide

The own-words guide lives in `src/dialogue_tools/guide.md` in the Agent crate.
It is **928 UTF-8 bytes**, with SHA-256
`7ba4b847923bdae403c804b174160640724413f6109f1d421270296fd7649282`.
A permanent test asserts both the digest and the 1,500-byte bound. It keeps the
meaning, reaction and hesitation advice separate from the tool's enforced
acoustic rules. The complete Alex/Forward Future MIT notice and component credit
are retained beside the existing Scarlet notice in `LICENSES/`; nothing from the
reference checkout was executed and no model or weights were adopted.

`get_capability("cut_dialogue")` returns a `guide` field with its schema. A batch
lookup also returns it exactly once. The check capability has no guide field.
The same tool-serving gate controls this lookup: production remains off until a
real measurement adapter exists. There is no new standing prefix, role text,
editable library or selection UI. With the tools absent, the existing capability
responses gain no field, including no `guide: null`.

The real single-capability reply measures **2,870 wire bytes**, **948 bytes** more
than the same reply with its guide field removed. That is about 237 tokens at a
rough four bytes per token (the body alone about 232); this is a byte heuristic,
not a tokenizer or provider measurement. One lookup supplies the guide for a ten-removal session
with the schema. Naming a separate lookup would transmit the same guide plus an
extra request and reply wrapper, so the inline choice is cheaper.

Three ordinary dispatch tests pass: bounded/pinned text, single and batch delivery,
and hidden/default delivery. The existing frozen-surface pin checks cover the
unchanged production tool/prompt leaves. Only the private bridge leaf
`arm_bridge_sha256.mcp_observation` changes in this commit:
`1fc289a1db94b52253834d8d0ac13aa42ccb22fa7298fd52b4fe155a09013424`
→ `24b5e68fd892b4528cc1dc8615739df083dc2ba5bbf865b9b3347a73cfbe4094`.
Models and options remain OPEN.

Final-source `cargo build --workspace` passed in 30.31 s. The workspace fast suite
passed in **2,479.42 s wall**, with **5,453 passed / 0 failed / 266 ignored**,
including integration and doc tests:

| Crate | Passed | Failed | Ignored |
| --- | ---: | ---: | ---: |
| Core | 897 | 0 | 2 |
| Agent | 1,741 | 0 | 166 |
| App | 1,082 | 0 | 10 |
| Media | 1,233 | 0 | 73 |
| Project | 483 | 0 | 4 |
| Hash | 6 | 0 | 0 |
| Job-sys | 11 | 0 | 11 |

Workspace all-target clippy with `-D warnings` passed in default and `te1-eval`
modes, as did Cargo fmt and explicit rustfmt of the included test files. Both pin
tests passed (8,856 named fields). Slow-test lint reports 140 slow / 161 ignored /
27 equipped skip sites; parser selftests 52 and shard selftests 23 pass. The
default build guard passed (three artifacts). The Windows gate passed all four
cross-clippy modes and all three isolated MSVC shipping checks; the three Linux
shipping builds passed too. Both successful temporary targets were removed.
The full platform wrapper took **397.76 s**, including shared-lock waits.
Windows execution, the complete slow suite, provider/paid/dry launches and
interactive App launches are NOT RUN. Media sources are unchanged; enabled-run
and seek renderer conformance remains the Media job's work. Evidence is under
`target/dq-levels-guide/`; the eight review pairs are under
`target/dq-tools-review/pairs/` and the five retained pairs under
`target/dq-tools/pairs/`.
