# MS1 recorder CBS build fix

Base `df6e3570`; no pull or reset. Windows run `44085793006`, job `134047749375`,
passed the awk stage, then failed recorder compilation at `cbs.c:33` with C7757.
The full job log confirms this was its first compiler error.

The verified FFmpeg 8.0.3 source has SHA-256
`6136812ea6d4e68bdba27e33c2a94382711cdf4f8602ffef056ff792bd6f9818`.
Its configure maps `mp4`/`ipod` to `mov_muxer`. `libavformat/Makefile:382` always
builds `cbs.o`, `cbs_av1.o` and `cbs_apv.o` for MOV. `libavformat/cbs.h` disables
every type except AV1/APV. The old recorder enables neither, leaving
`cbs_type_table` empty. The main profile already has CBS_AV1 through `av1_parser`;
CBS_VP9 alone would not suffice because the libavformat wrapper masks it.
Testgen also has CBS_AV1.

MP4/M4A are required recording outputs, so MOV stays. The recorder now enables
`av1_metadata` BSF, whose configure selector enables CBS_AV1. The direct internal
option `--enable-cbs-av1` is unsupported. No FFmpeg source is patched, and no
AV1 decoder/parser or file demuxer is added to recording.

The cheap check covers all three profiles on Linux and Windows, MOV selectors
(including HLS), AV1/APV types, a WAV-only control and insufficient H.264/VP9 CBS cases.
It failed twice on the old recorder and passes after the profile fix. CI runs
these checks before installing/building the native profiles. The recorder audit
also checks its closed BSF set alongside its existing decoder/device checks.

Verification evidence is under `target/ms1-cbs`:

- Strict Clang compilation of the original `libavformat/cbs.c` translation unit
  with `-Werror=zero-length-array`: old recorder exit 1 at `libavcodec/cbs.c:33`;
  corrected recorder, original main and original testgen each exit 0. These four
  checks run under the shared lock. Generated recorder configuration
  has only CBS_AV1 enabled; AV1/libdav1d decoders and the AV1 parser remain off.
- Build-script tests: `Ran 12 tests`, `FAILED (failures=2)` before the fix
  (recorder on each platform); `Ran 12 tests`, `OK` afterward.
- Linux source builds: libraries, recorder and testgen each built fingerprint
  `8b319a789891473cafead0ac03c9136c8961799349c71a4c6f36ca74193d07ab`.
  All installed binary digests are logged. Registry and recorder audits pass,
  including software ALSA capture to 16 kHz mono PCM WAV.
- Shared libraries: unchanged 38 decoders, 14 demuxers, 16 parsers, 11,634,104 bytes.
  Recorder: 4,612,472 → 4,784,504 bytes (+172,032); decoder/device sets unchanged.
  External closure: unchanged 39,412,832 bytes across 26 libraries.
- MS1 media witnesses: `9 passed; 0 failed`; app build passed with the same
  pre-existing `namespace/linux.rs:797` dead-code warning as the earlier gate.
- Workspace fast gate: exit 0; 4,531 passed, 0 failed, 144 ignored across 71
  suites. Raw runner summary: `fast tier on linux skipped exactly the 83 manifest
  tests and 61 allowlisted ignores (ci/ignored-tests.txt), each observed, nothing
  else; 34 other-OS manifest tests ran and passed`.
  Slow-test manifest lint and actionlint pass.

No Rust source changed; Rustfmt, clippy and the local slow tier were not rerun
in this pass. The commit requests the CI slow tier with `[slow-tier]`.

Native makes and Cargo use the shared lock, nice 10 and four build jobs; Rust
tests use a synthetic home and software Vulkan. Temporary FFmpeg source/build
trees have been removed, keeping installed outputs and logs.
After log compression, `target/` is 36,796,821,504 bytes; r3/r4 are absent and r5/r6 remain.
Windows rerun is NOT RUN after this fix and awaits the lead's push.
