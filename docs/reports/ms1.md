# MS1 worker report

Branch `ms1` starts at `origin/main` commit
`266f8890e259fb20d4c8f10d778b5b5fed6b3257`.
The lead reviews and pushes this work. No push or publishing action was performed.

## Inventory and implementation

[The design](../MS1-MINIMAL-FFMPEG.md) records the inventory, exact configure
arguments, source checksum, build/cache paths, exclusions and runtime boundary.
[The profile](../../scripts/ffmpeg-profile.json) defines the native decoder,
parser, format, encoder, filter and bitstream-filter enables. The builder and runtime decoder policy use that one profile.

- Retain H.264, HEVC, ProRes, AV1 through dav1d, VP9, DNxHD/HR, MJPEG, PNG,
  AAC, ALAC, MP3, FLAC, Opus, Vorbis and the explicit PCM variants.
- Retain FFV1, MPEG-4 part 2, MPEG-2 and rawvideo for existing fixtures.
- Keep libx264 and libx265. No nonfree component or FFmpeg networking is enabled.
- Shared libraries have no programs, devices or hardware acceleration.
- The separate recorder has only capture inputs, rawvideo/MJPEG/PCM decoders, Windows device-only BMP for gdigrab,
  libx264/AAC/PCM encoders, recording muxers and conversion filters. Recording never
  supplies media files to the CLI. H.264 capture is not explicitly requested.
- Testgen has a separate prefix and explicit executable names. App setup and
  packagers never consume it. Tests keep their generated parameterized fixtures.
- The lead-approved frozen JA S1 predecessor retains its historical full build
  only under `target/ja-s1-predecessor/source/third_party/ffmpeg`; no MS1 setup, app or fixture-tool path uses it.
- Recording lookup accepts only the packaged or checkout recorder; the old
  environment override, ancestor search and PATH fallback are removed.
- Codec policy runs before probe decoding and at certified decoder creation.
  Both path-based and certified input return structured unsupported-codec errors.
  The existing AVIO protocol, nested-open, stream and probe guards remain active.

Source is FFmpeg 8.0.3 from ffmpeg.org, SHA-256
`6136812ea6d4e68bdba27e33c2a94382711cdf4f8602ffef056ff792bd6f9818`.
Its detached signature verified against the official release key fingerprint
`FCF986EA15E6E293A5644F10B4322F04D67658D8`.
Linux assembly uses locally built, checksum-pinned NASM 2.16.03.

## Measured Linux registry and sizes

The baseline is the old setup's checksum-verified mifi GPL shared archive.
The result is this worker's FFmpeg 8.0.3 build using GCC 16.2.1, glibc 2.44 and NASM 2.16.03.
Counts come from native registry iterators. Sizes are actual file lengths,
counting each versioned ELF once and excluding symlinks and headers.

| Measure | Before | MS1 |
| --- | ---: | ---: |
| Decoders | 550 | 38 |
| Demuxers | 362 | 16 |
| Parsers | 64 | 16 |
| FFmpeg shared-library bytes | 211,707,496 | 11,683,288 |

| Library | Before bytes | MS1 bytes |
| --- | ---: | ---: |
| libavcodec | 105,613,584 | 7,518,440 |
| libavdevice | 5,185,072 | 0 |
| libavfilter | 65,961,080 | 183,416 |
| libavformat | 24,359,608 | 1,061,584 |
| libavutil | 8,309,848 | 1,079,984 |
| libswresample | 498,632 | 186,640 |
| libswscale | 1,779,672 | 1,653,224 |

The recorder is 4,612,472 bytes. The non-glibc external dependency closure of
both shipping profiles is 39,412,832 bytes on this host, across 26 libraries.
The combined FFmpeg/recorder/dependency payload is 55,708,592 bytes.
The 11.7 MB figure counts FFmpeg shared libraries; the complete shipping footprint
is reported separately above. External codec libraries
are dynamically linked on Linux and statically linked on Windows.

The native registry includes one H.263 decoder as a required MPEG-4 dependency.
Runtime policy refuses it. MOV adds an AC3 parser, MPEG-4 adds the H.263 parser,
and muxers add PGS/VP9 bitstream filters. These upstream dependency selections
are recorded explicitly rather than hidden behind the requested configure list.
There are no registered hardware-device types. Six filters remain, including
FFmpeg's mandatory audio/video buffer source and sink pairs.

## Verification

All required local gates passed on the final source. Every final Cargo invocation
sourced setup in the same shell, took the shared heavy lock, used four build jobs
and `nice -n 10`. Final test children had a synthetic home, four CPU cores and lavapipe;
CUDA was hidden. The standalone media slow run used one libtest thread.

Counts below aggregate every result line, including integration tests and doctests.
Logs are local worker evidence under `target/ms1/`, not shipping artifacts.

| Gate | Result | Evidence |
| --- | --- | --- |
| Touched-file rustfmt check | exit 0 | 13 Rust files; `skip_children=true` |
| Clippy: core, media, project, app; all targets; warnings denied | exit 0 | `clippy-final.log.gz` |
| Core tests | 814 passed, 0 failed, 1 ignored; 20 summaries | `core-final.log.gz` |
| Project tests with `test-util` | 420 passed, 0 failed, 2 ignored; 18 summaries | `project-final.log.gz` |
| App tests with `slow-tests` | 966 passed, 0 failed, 4 ignored | `app-final.log.gz` |
| Media tests with `slow-tests` | 1,271 passed, 0 failed, 32 ignored; 6 summaries | `media-final.log.gz` |
| `cargo build -p kinewright-app` | exit 0; rebuilt again after mutation restoration | `app-build-restored.log.gz` |
| `python3 scripts/slow_tests.py fast` | 4,528 passed, 0 failed, 144 expected ignores; 71 summaries | `workspace-final.log.gz` |
| `python3 scripts/slow_tests.py lint` | 118 markers and 61 allowlist entries agree | final static gate |
| Source-admission Python witnesses | `Ran 2 tests ... OK` | checksum refusal and profile separation |
| actionlint on all three changed workflows; Bash syntax | exit 0 | final static gate |
| PowerShell parsing | all three scripts parse | `powershell-syntax.log.gz`; syntax only |

Clippy enabled `kinewright-project/test-util`, `kinewright-media/slow-tests` and
`kinewright-app/slow-tests`. The production app build retains the pre-existing
unused `namespace/linux.rs::publish` warning; no lint suppression was added.

The fast-tier verifier's raw result was:

```text
fast tier on linux skipped exactly the 83 manifest tests and 61 allowlisted ignores (ci/ignored-tests.txt), each observed, nothing else; 34 other-OS manifest tests ran and passed
```

<details><summary>Raw touched-crate result lines</summary>

```text
core-final:
test result: ok. 327 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 1.22s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 59 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.39s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 39 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 65 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 113 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s

project-final:
test result: ok. 161 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 17.54s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.64s
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 52.67s
test result: ok. 86 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 110.09s
test result: ok. 35 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 22.86s
test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 173.78s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 60.17s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 20.04s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.60s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.39s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.81s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.63s
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.05s
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.70s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 10.16s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.63s
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

app-final:
test result: ok. 966 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 267.42s

media-final:
test result: ok. 1237 passed; 0 failed; 29 ignored; 0 measured; 0 filtered out; finished in 1623.51s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 57.82s
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.51s
test result: ok. 14 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 2.88s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
```

</details>

Focused MS1 tests passed: six codec/format-policy tests; actual video and audio
frames are received, including H.264 and HEVC in MOV. H.263 is present in the
library but runtime-refused; QTRLE is absent. Both return typed refusals before
certified stream-info decoding. Removing the Rust policy failed the exact
unsupported-codec assertion (`0 passed; 1 failed`); FFmpeg independently logged
that H.263 was not on its codec whitelist. Restoring the policy passed the tests.
A positive/negative AVI control exercises the format whitelist with a present
demuxer. Evidence: `policy-final.log.gz`, `mutation-red.log.gz` and
`mutation-green.log.gz`.

The recorder-lookup witness passed in the full app suite. Restoring the old
external-CLI override failed its exact path assertion (`0 passed; 1 failed`).
Restoring final code passed (`1 passed; 0 failed`), followed by a successful app
build. Evidence: `locator-mutation-red.log.gz` and `locator-mutation-green.log.gz`.
The source file was compared byte-for-byte with the pre-mutation backup.

The recorder registry audit passed. A private software ALSA null input produced
16 kHz mono PCM WAV without opening capture hardware. Linux stage-only packaging
with the debug app copied no testgen files; the staged recorder exposed only the
six capture decoders and had no dynamic libav dependency. `patchelf` is absent
locally, so RPATH rewriting and relocated bundle execution were not tested.
The CI dependency installer includes patchelf. Evidence: `recorder-final.log.gz`
and `packaging-smoke.log.gz`. No release package or installer was published.

Initial failures were retained and resolved before the final gates:

- Fixture generation needed pipe, M4A/WebM/SRT support, and visual scenes needed
  test-only FreeType/HarfBuzz plus the workspace-relative bundled font.
- The trial without standalone assembly failed existing color witnesses. Normal
  NASM assembly passes both direct 8/10-bit conversion witnesses and the encoded
  8-bit legality/difference witness without changing their assertions.
- The new path-input bridge initially lost the corrupt filename from an error;
  the media matrix exposed it, and the filename context was restored.
- App tests required the evaluation host rebuilt after the shipping-host build.
  File-backed feedback fixtures needed explicit testgen selection; production
  feedback needed PCM/WAV encoding and its duration-trim support.

The earlier full media summaries were `1183 passed; 53 failed; 29 ignored` and
`1235 passed; 1 failed; 29 ignored`; final media has no failures. Earlier logs
remain as `media-tests-initial.log.gz`, `media-tests-second.log.gz`,
`app-tests-initial.log.gz`, `app-tests-second.log.gz` and
`workspace-fast-initial.log.gz`.

Physical GPU, real capture hardware, real speech models, the opt-in long 4K/HEVC
matrix and performance lanes, Windows/MSVC execution, hosted CI and release
installer execution are not claimed locally. Existing ignored tests were kept;
the fast-tier verifier checked their exact allowlist membership.

## CI and exclusions

`ffmpeg.yml` builds source profiles on Linux and Windows. Both normal CI and
release jobs depend on that matrix and restore its exact `actions/cache` key.
The key includes the source/checksum profile, configure builder, dependency pin,
OS, architecture and Linux distribution/glibc ABI. There are no fallback keys or new publishing steps.
The first push must build and inspect the MSVC DLLs, verify capture-only CLI
registries and testgen pipes, and link/test the ordinary Windows workspace.
The lead's push supplies that Windows evidence; it is not available locally.

Common exclusions are WebP/GIF/BMP/TIFF/JPEG XL/JPEG 2000 and HEIF/AVIF still-image
workflows, WMV/WMA, Theora, Dolby AC3/EAC3, MP2, DTS, ADPCM and MPEG-1 video.
They are outside the required format set and current fixtures. AVCHD/Dolby
material needs external conversion. H.264-only capture devices are unsupported:
the recording code never requests H.264 input. HLS/DASH/concat are absent. The existing
certified stream-kind guard still refuses metadata and subtitle tracks. The design records these
limits for the lead's review instead of retaining the old general build.

Recording migration to the libraries is deferred to a later slice, likely MS2.
Provider-sensitive test children use a synthetic home. No credentials were
read, no system package installation ran on this desktop,
and no commit was pushed. After cleanup, `target/` occupies 36,425,060,352 allocated
bytes (33.92 GiB), below 40 GB. Rehearsals r3/r4 are removed; r5/r6 remain.
FFmpeg intermediate/source trees are removed, and completed logs are compressed.
The commit SHA is supplied with the handoff.
