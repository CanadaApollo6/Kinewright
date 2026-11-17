# FF9: the project's own FFmpeg build at 9.0.2, joined with dq-native

Branch `ff9`, from `origin/dq-native` (`5138cd5f`) with `origin/ms1` (`66a66cd1`) merged.
Status: **partial**. The merge and the pin are committed and build on Linux. One
merge conflict needs a decision (TE1 still image, below) before the fast tier is
green; the Windows questions wait on the first CI push.

## 1. Merge resolutions

| File | Resolution |
| --- | --- |
| `scripts/setup-ffmpeg.sh` | MS1's source build (libraries + recorder via `build-ffmpeg.py`), dq-native's Linux check, NeMo-Speech sourcing and `KINEWRIGHT_NEMO_DIR` export, ABI check at libavcodec 63. New `--ffmpeg-only` skips NeMo for the CI producer. |
| `scripts/setup-ffmpeg.ps1` | MS1's MSVC/MSYS build and `Export-BuildEnvironment`; dq-native's `-NemoCacheKeyOnly` and `setup-nemo-speech.ps1`; `avcodec-63.dll`; `KINEWRIGHT_NEMO_DIR` added to the exported names. New `-FfmpegOnly` for the producer. |
| `scripts/install-linux-deps.sh` | dq-native's single `apt_packages` manifest (read by `ci-linux-deps.sh`) gains MS1's build packages (nasm, x264/x265/dav1d/zlib/vpx/freetype/harfbuzz/pulse/xcb dev). |
| `scripts/package-linux.sh` | MS1's library list (no avdevice) and dependency copy, plus dq-native's NeMo libraries and `LICENSES/`. |
| `crates/kinewright-core/src/media.rs` | Both variants: `DataDirectory` and `UnsupportedCodec`. |
| `crates/kinewright-media/src/owned_io.rs` | dq-native's owned-descriptor reads (`from_parts`, `ManuallyDrop` format, colour numbers) and MS1's `check_codec`, `path_input` and guarded `find_stream_info`. Every reader, owned or path, opens through `find_stream_info`, which runs `prepare_stream_table` and the codec check before any probe. dq-native's `SINGLE_OBJECT_DEMUXERS` gives way to `codec_policy::FORMATS`. |
| `docs/BUILDING.md` | MS1's source-build text. |
| `.github/workflows/ci.yml` | dq-native's structure (lint, checks, sharded `ci-tests.yml`, Rust 1.99.1, ubuntu-24.04). Added `minimal-ffmpeg` (MS1's `ffmpeg.yml`); `checks`, all four tiers and `recognition-slow` need it and receive the per-OS key; `result` requires it. |

Not conflicted, but changed to join the two:

- `.github/actions/ci-native`: the downloaded-archive cache is gone. It takes an
  `ffmpeg-key` and restores and provisions the profiles (plus NeMo) through MS1's
  `ffmpeg` action. With no key (release) the caller provisions FFmpeg itself.
- `.github/actions/ci-apt` (new): dq-native's cached, deadline-bound apt steps,
  moved out of `ci-native` so MS1's Linux producer installs the same manifest the
  same way. The producer previously ran a bare `apt-get`. The fingerprint includes
  pkg-config versions, so the producer and the shards must resolve the same packages.
- `ci-tests.yml`: new `ffmpeg-key` input passed to `ci-native`; the display-cache
  key hashes `build-ffmpeg.py` and `ffmpeg-profile.json` in place of the archive script.
- `scripts/ffmpeg_archive.py` and its tests are deleted: nothing downloads a
  prebuilt FFmpeg in setup, CI or packaging.
- `docs/reports/ja1-s4b/c5-windows-clippy.sh` (the local cargo-xwin lint gate) used
  that script and the old URL in `setup-ffmpeg.ps1`. It now pins its own lint-only
  header archive (the same System233 9.0.2 zip and SHA-256) and uses `curl` +
  `sha256sum`. It never feeds setup, CI or packaging.
- `.gitignore`: MS1 never ignored `third_party/ffmpeg-recorder/`, `ffmpeg-testgen/`,
  `ffmpeg-deps/` or `nasm/`. They are ignored now.

## 2. The pin and its provenance

- Source: `https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz`, 12,040,788 bytes,
  SHA-256 `8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e`.
- Downloaded with its `.asc` and `https://ffmpeg.org/ffmpeg-devel.asc` into a new
  `target/ff9-source-dl/`. Verified with `gpg --verify` under a throwaway
  `GNUPGHOME=target/ff9-gnupg`:
  `VALIDSIG FCF986EA15E6E293A5644F10B4322F04D67658D8 2026-09-18`, "FFmpeg release
  signing key <ffmpeg-devel@ffmpeg.org>". This is the fingerprint MS1 recorded for
  8.0.3. Trust in the key comes from that match and from the key being served
  by ffmpeg.org. It is not certified by any key in a personal web of trust.
- `scripts/ffmpeg-profile.json` holds the version and checksum; `build-ffmpeg.py`
  derives the URL and unpacked directory from it.
- ABI: libavcodec 63.1.102 (`setup-ffmpeg.sh` prints it). Windows DLLs:
  avcodec-63, avformat-63, avfilter-12, avutil-61, swscale-10, swresample-7.
  The MSVC import-library move and MSYS make/sh handling are unchanged.

## 3. Allowlist, 8.0.3 to 9.0.2

New guard: `build-ffmpeg.py` reads configure's own component sources
(`allcodecs.c`, `parsers.c`, `bitstream_filters.c`, `allformats.c`, `protocols.c`,
`alldevices.c`, `allfilters.c`) with configure's matching rule. Every build, and
`--check-components`, refuses any profile, for either OS's options, that names a
component missing from them. Configure itself only warns when an enable matches
nothing, and is silent when one name of several is missing. The four buffer
source/sink filters are declared outside configure's list (`extern  const`) and
always built; they count under their registered names. Witnesses are in
`scripts/test_ffmpeg_build.py`: a synthetic omission refused for all three profiles,
the buffer-filter naming, a build refused before configure runs, and the real pinned
tarball passing. With the check removed from `build()`, the build witness fails.

Result: every named component exists in 9.0.2. No component was added.

Upstream component lists, 8.0.3 to 9.0.2. None of the removed entries is ours:
- decoders 596 to 603 (removed libcelt, sonic, v308, v408, v410);
- parsers 64 to 68; bsfs 48 to 51; demuxers 364 to 367; muxers 184 to 185;
- protocol `hls` removed (testgen enables the hls *demuxer*, not the protocol);
- filters 589 to 596 (removed the four `*_npp` filters).

Dependency changes for components we enable (configure `_select`):
- `h263_decoder` no longer selects `h263_parser`. The registered parser set drops
  from 16 to 15; AC3/EAC3 (from MOV) is now the only dependency-only parser.
- `mov_muxer` selects `cbs_apv_lavf cbs_av1_lavf` itself. MS1's recorder enables
  `av1_metadata` only to select CBS_AV1 for MSVC; 9.0.2 makes that unnecessary.
  It is kept here; dropping it is a possible follow-up.
- `mpeg4_decoder` adds `qpeldsp`; `mpeg4video_parser` drops `h263dsp qpeldsp`;
  `mjpeg_decoder` and `avi_demuxer` drop `exif`; `libdav1d_decoder` now selects
  `itut_t35` in place of `atsc_a53 dovi_rpudec`.

Registry (Linux, `inspect-ffmpeg.py --check`): 38 decoders, 14 demuxers,
15 parsers; libraries 12,039,864 bytes (8.0.3: 11,634,104).

## 4. Windows root causes

Both dq-native Windows failures had the same cause, and neither was a defect in 9.0.2.

**Mechanism (source).** For 8-bit output, libswscale's x86 init picks one of two
vertical scalers: the dithered `yuv2yuvX_{sse3,avx2}` (`use_mmx_vfilter = 1`) or
`ff_yuv2planeX_8_*`. In 8.0.3 the only assignment `use_mmx_vfilter = 1` is in
`x86/swscale_template.c` (`sws_init_swscale_mmxext`), compiled only under
`HAVE_MMXEXT_INLINE`, which needs GCC-style inline asm. MSVC has no inline asm, so
every MSVC build of 8.x (the System233 8.0.1 package) used `ff_yuv2planeX_8_*`. GCC
builds (mifi, Linux) used `yuv2yuvX`. 9.0.2's `x86/swscale.c` decides
`use_mmx_vfilter` from `X86_MMXEXT(cpu_flags)` and `HAVE_SSE2_EXTERNAL` (nasm), with
no inline asm. MSVC and GCC builds now take the same path. In both versions,
`accurate_rnd` and `bitexact` force `use_mmx_vfilter = 0`.

**Reproduction (Linux, our 9.0.2 testgen, delivery scale arguments, flat 16x16
rgba64le).** `-cpuflags` without `mmxext` keeps every external SIMD kernel but
forces `use_mmx_vfilter = 0`: the selection MSVC 8.x made.

| Input | Selection | Y | Cb | Cr |
| --- | --- | --- | --- | --- |
| white | default (`yuv2yuvX`) | {235} | {128,129} | {127,128} |
| white | no mmxext (`planeX_8`) | {235} | {128} | {127,128} |
| grey | default | {170,171} | {128,129} | {127,128} |
| grey | no mmxext | {170,171} | {128} | {127,128} |
| either | `accurate_rnd` | as no mmxext | {128} | {127,128} |

The default row is Linux's recorded constant (Cb {128,129}, Cr {127,128}). The
no-mmxext row is the old Windows constant (Cb {128}, Cr {127,128}), recorded on
System233 8.0.1. dq-native's red run had System233 9.0.2 producing {128,129}: Windows
had moved onto Linux's path. Luma: dq-native's Windows run measured scenario (a)
8-bit luma mean 18677, which is exactly Linux's recorded value; the manifest's
Windows 18688 was measured on 8.0.1. Same mechanism, consistent with a legitimate
change of path rather than a new rounding defect.

**Own build.** Linux and Windows build the same source with nasm
(`--enable-x86asm`). Every x86-64 CPU has SSE2 and MMXEXT, so both select
`yuv2yuvX`. The prediction is that both Windows failures go away, Windows
matching Linux. **CI has not answered this yet.**

**Change (`4b850ecd`).** The 8-bit test's per-OS `NEUTRAL_CHROMA_CODES` (and its
"unmeasured system" branch) becomes one constant: Cb {128,129}, Cr {127,128}. The
assertion still requires these exact sets on both systems. The manifest's Windows
luma column is **not** touched yet. It is re-measured only once CI shows what our own
build produces on Windows. Before: Windows (a) 8-bit luma mean 18688 (System233
8.0.1). Linux: 18677.

**Why no explicit conversion setting.** The only options that pin the vertical
scaler (`accurate_rnd`, `bitexact`) select `planeX_8`. That changes Linux's
delivered Cb from {128,129} to {128}, and the brief's stop rule forbids changing
Linux's delivered output. The default path is not selected by any filter
argument; it follows from the build (external asm) and the CPU. See the open
decision in §6.

## 5. Measurements

- `scripts/test_ffmpeg_build.py`: 22 tests OK.
- Workspace build and `cargo clippy --workspace --all-targets -- -D warnings`: clean.
  `cargo fmt -- --check`: clean. `slow_tests.py lint`: clean (140 slow tests).
  `slow_tests_selftest`, `ci_shards_selftest`, `ci_linux_deps_selftest`,
  `nemo_speech_selftest`: OK. `check-ffmpeg-recorder.py`: verified.
- Fast tier, `cargo test --workspace --no-fail-fast` at `7c127471` (Linux, our 9.0.2
  build): 92 binaries, 5,614 passed, 2 failed, 277 ignored. The failures were
  `te1_corpus::te1_six_case_rendered_evidence_uses_production_preview` (open, §7) and
  `decode::tests::unlisted_stream_colour_tags_import_as_unknown` (fixed, §6; passes at
  `f84a1031`). Both export neutral-chroma tests pass at `4b850ecd`.
  `cc7_every_scenario_verifies_at_eight_bits`/`_ten_bits` passed on Linux against the
  unchanged Linux column.
- Slow tier (`slow_tests.py slow`, media + agent + app features) at `4b850ecd`:
  "slow tier accounted for exactly the 139 manifest tests: 139 passed".
- Not run: the Windows cross-clippy (`c5-windows-clippy.sh`). Its header-archive
  cache was empty here, and filling it would mean a second download.

## 6. Other merge fallout fixed

- `decode::tests::unlisted_stream_colour_tags_import_as_unknown` (dq-native) wrapped
  its H.264 in MPEG-TS. MS1 removed that demuxer from shipping, so the production
  probe refused the file. Now AVI, which is in the profile and also carries no colour
  of its own; `StreamMetadata` provenance is still asserted (`f84a1031`).
- Not changed, noted: the app's import dialogs still offer `webp`, `bmp` and `tiff`
  stills (`media_bin.rs:31`, `media_workflow.rs:967`). MS1's profile decodes none of
  them. This predates the merge (it came in through MS1 and main).

## 7. Open

- **TE1 still image (blocks the fast tier).**
  `kinewright-agent --test te1_corpus te1_six_case_rendered_evidence_uses_production_preview`
  fails with `certified decoder: incomplete or dynamic stream table unsupported`.
  dq-native's frozen TE1 corpus uses `tests/fixtures/te1/talking-head.ppm`. The MS1
  profile has no PPM decoder, and its pre-probe guard refuses image2's NOHEADER table
  outside the validated PNG/JPEG exception. Keeping the fixture means adding
  components (a PPM decoder and pipe demuxer), which the brief forbids. Converting
  it to PNG changes a frozen corpus: its `hashes.json`, seven `.kinewright` oracles
  that name the path, and `te15-measurement-pin.json`. That needs the lead's decision.
- Windows CI answers for §4.
