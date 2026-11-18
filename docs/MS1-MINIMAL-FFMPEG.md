# MS1 minimal FFmpeg

## Scope and source

Kinewright is GPL-3.0-only; GPL/version3 and x264/x265 stay enabled, with no nonfree components.

All three profiles use `https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz`;
SHA-256: `8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e`.
The pin was checked against the detached signature `ffmpeg-9.0.2.tar.xz.asc` using the
official release key (`https://ffmpeg.org/ffmpeg-devel.asc`), fingerprint
`FCF986EA15E6E293A5644F10B4322F04D67658D8`, the same key that signed the earlier
8.0.3 pin. It provides the FFmpeg 9 ABI (libavcodec 63) that `ffmpeg-next 9.1.0` binds.
Every build, and `build-ffmpeg.py --check-components`, refuses a profile that names a
decoder, demuxer, parser, encoder, muxer, bitstream filter, filter, protocol or device
absent from the pinned source's component lists; configure itself would only warn.
`docs/reports/ff9-own-build.md` records the 8.0.3 to 9.0.2 component and dependency diff.

`scripts/build-ffmpeg.py` downloads into new empty `target/ffmpeg-source/download-*`
directories: three attempts, a 30 s socket timeout, and 2/4 s backoff. SHA-256 is
checked before atomically caching `target/ffmpeg-source/<sha256>/source.tar.xz`.
`actions/cache` uses `ffmpeg-source-<sha256>`; restores and build copies are rehashed.
Extraction only follows verification in `target/ffmpeg-build/<profile>-*`; source
and build trees are removed on success or failure. Verified tarballs and installs remain.
Releases may reuse verified source, never binary caches. No system FFmpeg participates.

## Inventory and chosen profile

The inventory covers media decode/IO/audio/export/verify, recording, every fixture
command and all workflows. Fixtures are generated; only the app icon is committed.
`scripts/ffmpeg-profile.json` is the shared build and runtime policy.

| Category | Required or reached | Decision |
| --- | --- | --- |
| Phone, camera and screen video | H.264, HEVC, ProRes, AV1, VP9, DNxHD/HR, MJPEG | Native software decoders except AV1 through libdav1d |
| Stills | PNG, JPEG | PNG and MJPEG decoders; image2, image2pipe, PNG/JPEG pipe demuxers |
| Compressed audio | AAC, ALAC, MP3, FLAC, Opus, Vorbis | Native decoders; MP3 uses `mp3`, not an additional float implementation |
| PCM | signed 8/16/24/32/64-bit, unsigned 8-bit, float 32/64-bit, both byte orders where meaningful; planar 16/24/32 LE; A-law, mu-law | Explicit names in the profile; no ADPCM family |
| Containers | MOV/MP4/M4A, Matroska/WebM, WAV, MP3, FLAC, Ogg, raw AAC | Enable their demuxers |
| Existing generated fixtures | FFV1, MPEG-4 part 2, MPEG-2, rawvideo; AVI, NUT and elementary MPEG video | Retain; PS/TS only in testgen for refusal fixtures |
| Export | libx264 and AAC delivery; libx265 retained by licence brief; PCM WAV | Four encoders; MOV, MP4, Matroska and WAV muxers |
| Parsers | H.264, HEVC, AV1, VP9, DNxHD, MJPEG, PNG, AAC, MPEG audio, FLAC, Opus, Vorbis, MPEG-4 and MPEG video | Explicit enables plus upstream dependency closure |
| Bitstream filters | AAC ADTS to ASC, extract extradata, VP9 superframe split | Explicit enables plus muxer/demuxer dependency closure |
| Video filters | buffer, buffersink, scale, format | Used by managed decode, export and verification |
| Scaling/resampling | libswscale and libswresample | Used for pixel and audio conversion; retained |
| Hardware acceleration | Media code calls no FFmpeg hardware APIs | No hwaccels or external hardware libraries enabled |
| Protocols | Local file output/input and custom certified IO | Only file protocol; network disabled |

FFmpeg selects the H.263 decoder with MPEG-4 part 2. In 8.0.3 the H.263 decoder
also selected the H.263 parser; 9.0.2's `h263_decoder_select` no longer does, so
that parser is gone. The MOV muxer selects the AC3/EAC3 parser for its packet
inspection. AC3/EAC3 is the complete dependency-only parser set; disabling it
would also disable the required MOV muxer. Its codecs, and H.263, remain
runtime-refused. No Dolby decoder is enabled.
Matroska adds `pgs_frame_merge` and VP9 adds `vp9_superframe` bitstream filters.
The filter library always registers audio and video buffer source/sink pairs.
`avcodec` also supplies required internal conversion helpers.

The previous build enabled networking, GPU stacks, subtitle rendering and many
codec libraries; its configure vector and registry inventory remain in the original evidence.

The shared profile opts into only x264, x265, dav1d and zlib. The recorder adds
Linux ALSA, PulseAudio and XCB capture support. Testgen additionally uses libvpx, FreeType and HarfBuzz for the visual-scene
fixtures. These dependencies are not enabled in either shipping profile.

## Exact configuration

The following is the shared-library configure argument vector. The builder adds
the install prefix and explicit compiler/linker/archiver selections described below.

```sh
./configure \
  --disable-everything --disable-autodetect --disable-network \
  --disable-doc --disable-debug --enable-x86asm --x86asmexe=nasm \
  --disable-iconv --enable-gpl --enable-version3 \
  --enable-avcodec --enable-avformat --enable-avfilter \
  --enable-swscale --enable-swresample --enable-protocol=file \
  --enable-libx264 --enable-pic --disable-programs --disable-avdevice \
  --enable-shared --disable-static --enable-libx265 --enable-libdav1d --enable-zlib \
  --enable-decoder=h264,hevc,prores,libdav1d,vp9,dnxhd,mjpeg,png,aac,alac,mp3,flac,opus,vorbis,pcm_s8,pcm_u8,pcm_s16le,pcm_s16be,pcm_s24le,pcm_s24be,pcm_s32le,pcm_s32be,pcm_f32le,pcm_f32be,pcm_f64le,pcm_f64be,pcm_s64le,pcm_s64be,pcm_alaw,pcm_mulaw,pcm_s16le_planar,pcm_s24le_planar,pcm_s32le_planar,rawvideo,ffv1,mpeg4,mpeg2video \
  --enable-demuxer=mov,matroska,wav,mp3,flac,ogg,aac,avi,nut,mpegvideo,image2,image2pipe,image_png_pipe,image_jpeg_pipe \
  --enable-parser=h264,hevc,av1,vp9,dnxhd,mjpeg,png,aac,mpegaudio,flac,opus,vorbis,mpeg4video,mpegvideo \
  --enable-encoder=libx264,libx265,aac,pcm_s16le \
  --enable-muxer=mp4,mov,matroska,wav \
  --enable-bsf=aac_adtstoasc,extract_extradata,vp9_superframe_split \
  --enable-filter=buffer,buffersink,scale,format
```

Linux adds `--cc=gcc --cxx=g++ --ld=gcc --ar=ar` (or explicit CC/CXX/LD/AR
overrides). Windows selects `cl.exe`, `cl.exe`, `link.exe`, `lib.exe` respectively.
Windows adds `--toolchain=msvc --arch=x86_64 --target-os=win64` and
`--pkg-config=<repo>/third_party/pkgconf/pkgconf/.bin/pkgconf.exe` and
`--pkg-config-flags=--static`. Native pkgconf reads native vcpkg paths.
The build retains normal optimized assembly. NASM is a build tool, never a
shipping dependency. When Linux has no NASM, the builder installs NASM 2.16.03
locally from nasm.us; SHA-256
`1412a1c760bbd05db026b6c0d1657affd6631cd0a63cddb6f73cc6d4aa616148`.
It compiles under the shared lock and removes its source. CI supplies NASM.
The local builder changes no system packages.

## Recording and fixture isolation

The shared profile has no programs or avdevice. The `recorder` profile builds
FFmpeg libraries statically into `third_party/ffmpeg-recorder/bin/ffmpeg`.
It has no file demuxers or general media input. Its file protocol is
needed to write recordings. It has libx264/AAC/PCM encoders and MP4/M4A/WAV muxers,
crop/scale/format, audio conversion and recording-duration trim filters, plus their
internal dependencies.
Upstream selects crop/orientation filters, MOV's AC3 parser and AAC/VP9 bitstream filters.
MP4/M4A select MOV, which always builds libavformat CBS with only AV1/APV types.
The recorder enables `av1_metadata` BSF to select CBS_AV1 and avoid MSVC's empty
table error; `--enable-cbs-av1` is not a public configure option. No AV1 decoder
or parser is added. The shared profile already selects CBS_AV1 through its AV1 parser.
Windows enables dshow and gdigrab. Linux enables v4l2, xcbgrab, ALSA and PulseAudio.

The recorder's device-only decoders are rawvideo, MJPEG and PCM s16le, s32le,
f32le and u8. Windows also needs the device-only BMP decoder because gdigrab
emits BMP packets. No BMP file demuxer or app BMP decoder is enabled.
`recording.rs::ffmpeg_record_args_windows` and
`ffmpeg_record_args_linux` explicitly select a capture input for every `-i`.
They never open user-supplied media files. Linux camera input requests 30 fps;
Windows leaves camera input rate/format unforced. Both encode at 30 fps and allow
device format negotiation, including MJPEG. H.264 is never requested and stays absent.
H.264-only capture devices are unsupported. MJPEG stays for ordinary USB webcams. Device listing also selects a capture input.
The CLI is not used for import/export. Feedback voice captures 16 kHz mono PCM WAV. The Linux
file-backed feedback source is compiled only in tests and explicitly uses testgen.
Moving recording onto the libraries is a later slice, likely with MS2.
Recorder lookup uses only the installed bundle or this checkout's recorder prefix; no environment override or PATH fallback can select another FFmpeg.

The `testgen` profile installs only `ffmpeg-testgen` and `ffprobe-testgen` in
`third_party/ffmpeg-testgen/bin`. It has lavfi generators, fixture encoders,
file/playlist/SRT demuxers, pipe IO, filters and test oracle muxers. QTRLE is test-only and
provides the disallowed-codec witness. Static FFmpeg libraries prevent testgen
from loading the app's shared profile. External libraries on Linux remain
system-linked; Windows uses static vcpkg libraries.

Generated fixtures are preferable to committed outputs here because the tests
vary frame rate, colour tags, dimensions, audio layouts and truncation at runtime.
`test_support::ffmpeg_tool` and the two project fixture examples point only at
testgen. Testgen is never put on the app PATH, invoked by shipping setup, or
copied by either packaging script. CI restores it alongside, not inside, the
shipping prefixes. Test generation is explicitly provisioned with
`python3 scripts/build-ffmpeg.py --profile testgen`.

Known exception: the frozen JA S1 predecessor control keeps its historical full build under `target/ja-s1-predecessor/source/third_party/ffmpeg`, test-only; no MS1 setup, app or fixture-tool path resolves to it.

## Local consumption and CI

Linux `source scripts/setup-ffmpeg.sh` builds or reuses the shared and recorder
profiles, exports the FFmpeg 9 headers/libraries and recorder PATH, and verifies
the ABI. C/C++, make, Python 3.12+, pkg-config and explicit development libraries
are prerequisites. The dependency installer lists Debian/Arch packages; setup installs none.

Windows requires MSVC, Python 3.12+ and MSYS2 make/diffutils/pkgconf/nasm.
Run `scripts/prepare-ffmpeg-windows.ps1` once to build x264, x265, dav1d, zlib and
libvpx, FreeType and HarfBuzz with the static MSVC vcpkg triplet. Its registry is pinned to
`0df93a05b9de080129e653ec4776d7e14a1b2e4f`.
`setup-ffmpeg.ps1` imports vcvars64, provisions pinned pkgconf/libclang helpers,
then invokes the same builder. It moves upstream MSVC import `.lib` files from
`bin` to `lib`; DLLs stay in `bin`. The recorder keeps a separate prefix.

`ffmpeg.yml` builds Linux and MSVC profiles before dependent CI jobs. One fingerprint
keys the binary cache and local `.build-key`: source hash, configure vectors,
compiler/linker/assembler binary identities and full versions (including MSVC `/Bv`),
dependency pkg-config versions/flags, build flags, host ABI, builder/profile/setup
scripts and CI action. Producers export exact Linux/Windows keys; a CI consumer
restores by that key, and when the cache step reports an exact hit it reuses the
restored binaries as they are (`KINEWRIGHT_FFMPEG_RESTORED_PRODUCER_CACHE`), without
comparing the marker to its own toolchain: during a runner image rollout a consumer
can run on a newer image than its producer, and it must reuse the producer's
binaries instead of rebuilding inside a test shard. The exported key names the cache
entry and is not compared to the marker: on Windows the two differ within one
producer job (seen on run 44398295450; the key step and the build do not compute
the same fingerprint, cause not yet found, ticket CI-T2). Local builds and release
jobs verify against their own fingerprint. No fallback keys exist.

Binary cache writes require a successful canonical-repository **push**; PR jobs
never write, and fork PRs/fork repositories never restore these caches. Each job
logs SHA-256 for every installed native binary, including dependencies and tools.
Digests record contents, not provenance. Accepted risk: matching cache markers trust
writers who can already push code to this single-owner repository. Local reuse trusts this desktop.

Release jobs bypass binary-cache restore/save and invoke `--no-binary-cache`.
Both packagers require this flag (`-NoBinaryCache` on Windows), force a fresh
verified-source build even with matching markers, and accept only the owned
install prefix. Testgen is excluded from shipping. No new FFmpeg publication or
release service is introduced.

Windows uses the [MSYS2 action output](https://github.com/msys2/setup-msys2#msys2-location)
for make/sh/awk; `SHELL=/usr/bin/sh`, with MSVC then MSYS before inherited tools. Configure receives an argv
vector directly through MSYS bash, without `shell=True` or a `bash -c` layer.
A pre-build witness exercises upstream's exact backslash-replacing awk depfile
expression. This removes the native-Windows make/shell selection implicated in
run 44045226331. `libavfilter` stays: decode/export/verify use buffer, buffersink,
scale and format; only those plus upstream audio source/sink pairs are registered.

Linux make uses `flock -o /tmp/kinewright-cargo-heavy.lock nice -n 10 make -j4 install`
for libraries, or `install-progs` for tools, retaining the lock through installation.
Cargo gates use that lock separately, with four jobs and low priority.
Linux packaging copies the FFmpeg libraries, recorder, and their non-glibc
shared dependencies. Windows static external dependencies require no extra codec
DLLs. Testgen is excluded by explicit source paths in both packaging scripts.

## Runtime refusals

All production readers use one admission/probe boundary. After header opening,
before any packet read or `avformat_find_stream_info`, zero streams and
`AVFMTCTX_NOHEADER` are refused. MPEG-PS/TS demuxers are absent from shipping.
The named **validated single-image exception** permits exactly `png_pipe` or
`jpeg_pipe` only after bounded raw-header validation, with exactly one stream,
the demuxer's fixed PNG/MJPEG codec, and that codec allowlisted. The header
signature must match the demuxer. Certified and path readers perform the same
check; raw AVIO header reads do not invoke packet readers or parsers.

`codec_whitelist` is set before opening and again on every probe decoder. Rust
checks codec IDs before probing and decoder construction. The fixed stream count
is installed as `max_streams` before probing, preventing later table growth.
Certified IO retains protocol `none`, deny_open, format whitelist and limits.
Path readers retain their interrupt callback and share the policy.

Witnesses count the sole parser-capable probe entry: allowed controls reach it;
AC3-in-PS, an injected dynamic AVI table, and PNG tables with a second/wrong
stream reach it zero times. Rejected contexts also have zero native parsers;
the valid PNG control reaches the native probe.

QTRLE in MOV and H.263 in AVI produce
`OwnedIoError::UnsupportedCodec` or `MediaError::UnsupportedCodec`, with recovery
code `unsupported_codec`. It does not enter certified stream-info decoding.
The H.263 witness exercises runtime refusal of a decoder that is compiled in.
A WAV-only test policy also rejects a valid compiled AVI demuxer before probing,
so format-guard coverage does not depend only on absent playlist components.
The existing generic media incident displays the refusal's codec and message.

## Measurements and exclusions

Registrations use `scripts/inspect-ffmpeg.py` and native codec/demuxer/parser iterators.
AVCC H.264/HEVC is decoded directly by their native decoders, so the separate
Annex B conversion filters and the unused AAC-LATM parser are omitted.
Physical sizes count each versioned shared library once; symlinks, headers and
import libraries are excluded. System dependencies are reported separately.
On this host, the recorder is 4,784,504 bytes. Its and the libraries' combined
non-glibc external dependency closure is 39,412,832 bytes across 26 libraries.
The combined payload is 55,831,440 bytes, before application and licence files.
These dependency sizes are host-specific and include PulseAudio dependencies.

| Linux x86_64 | Before | MS1 (8.0.3) | FF9 (9.0.2) |
| --- | ---: | ---: | ---: |
| Registered decoders | 550 | 38 | 38 |
| Registered demuxers | 362 | 14 | 14 |
| Registered parsers | 64 | 16 | 15 |
| FFmpeg shared-library bytes | 211,707,496 | 11,634,104 | 12,039,864 |

The old Linux baseline is the checksum-verified mifi archive named by the old
setup. Windows baseline sizes are not measured on this Linux worker. CI prints
the Windows source-build counts and DLL sizes, and refuses unexpected registry
counts. Original evidence is in `docs/reports/ms1.md`; rework evidence is in
`docs/reports/ms1-review-fixes.md`; CBS build evidence is in `docs/reports/ms1-cbs.md`.

Excluded common interchange includes WebP, GIF, BMP, TIFF, JPEG XL, JPEG 2000,
AVIF/HEIF image workflows, MPEG-1 video, WMV/WMA, Theora, AC3/EAC3, MP2, DTS and ADPCM. They are not
in the required format set or current fixtures. AVCHD with Dolby audio therefore
needs external conversion; its dynamic MPEG-TS admission was already refused
by the certified-input path before MS1. Remux or transcode these with
an external tool before import. HLS, DASH and concat remain absent; ordinary
import accepts complete local media objects. Certified input already refuses non-AV tracks by stream kind;
unsupported AV codecs now receive a typed refusal. Native AV1 is replaced by software
libdav1d, rather than enabling GPU decoding. PCM variants not named in the
profile, including unsigned high-bit-depth PCM, are outside MS1.
