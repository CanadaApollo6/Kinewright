# MS1 minimal FFmpeg

## Scope and source

Kinewright remains GPL-3.0-only. The build enables GPL and version 3, including
libx264 and libx265, and never enables nonfree components.

All three profiles use `https://ffmpeg.org/releases/ffmpeg-8.0.3.tar.xz`.
SHA-256 is `6136812ea6d4e68bdba27e33c2a94382711cdf4f8602ffef056ff792bd6f9818`.
The pin was independently checked with the tarball's detached signature and the
[official release key](https://ffmpeg.org/download.html#release_8.0), fingerprint
`FCF986EA15E6E293A5644F10B4322F04D67658D8`.
8.0.3 contains the current fixes on the 8.0 branch and preserves our FFmpeg 8 ABI.

`scripts/build-ffmpeg.py` downloads each source into a newly created empty
`target/ffmpeg-build/<profile>-*` directory. It verifies SHA-256 before extracting
or executing anything. It removes the source and intermediate build on success
or failure. Only installed outputs remain. No system FFmpeg participates.

## Inventory and chosen profile

The inventory covers `decode.rs`, `owned_io.rs`, `audio.rs`, `export.rs`,
`verify.rs`, `recording.rs`, generated-media helpers, all Rust fixture command
arguments, and all workflows. Media fixtures are generated rather than committed.
The only committed image under `crates/` is the application icon.
`scripts/ffmpeg-profile.json` is the shared build and runtime policy.

| Category | Required or reached | Decision |
| --- | --- | --- |
| Phone, camera and screen video | H.264, HEVC, ProRes, AV1, VP9, DNxHD/HR, MJPEG | Native software decoders except AV1 through libdav1d |
| Stills | PNG, JPEG | PNG and MJPEG decoders; image2, image2pipe, PNG/JPEG pipe demuxers |
| Compressed audio | AAC, ALAC, MP3, FLAC, Opus, Vorbis | Native decoders; MP3 uses `mp3`, not an additional float implementation |
| PCM | signed 8/16/24/32/64-bit, unsigned 8-bit, float 32/64-bit, both byte orders where meaningful; planar 16/24/32 LE; A-law, mu-law | Explicit names in the profile; no ADPCM family |
| Containers | MOV/MP4/M4A, Matroska/WebM, WAV, MP3, FLAC, Ogg, raw AAC | Enable their demuxers |
| Existing generated fixtures | FFV1, MPEG-4 part 2, MPEG-2, rawvideo; AVI, NUT, MPEG PS/TS and elementary MPEG video | Retain for real fixture decoding and useful camera/screen interchange |
| Export | libx264 and AAC delivery; libx265 retained by licence brief; PCM WAV | Four encoders; MOV, MP4, Matroska and WAV muxers |
| Parsers | H.264, HEVC, AV1, VP9, DNxHD, MJPEG, PNG, AAC, MPEG audio, FLAC, Opus, Vorbis, MPEG-4 and MPEG video | Explicit enables plus upstream dependency closure |
| Bitstream filters | AAC ADTS to ASC, extract extradata, VP9 superframe split | Explicit enables plus muxer/demuxer dependency closure |
| Video filters | buffer, buffersink, scale, format | Used by managed decode, export and verification |
| Scaling/resampling | libswscale and libswresample | Used for pixel and audio conversion; retained |
| Hardware acceleration | Media code calls no FFmpeg hardware APIs | No hwaccels or external hardware libraries enabled |
| Protocols | Local file output/input and custom certified IO | Only file protocol; network disabled |

FFmpeg selects H.263 implementation code and its parser with MPEG-4 part 2.
The MOV muxer selects the AC3 parser. These are dependency closure, not
an expansion of the runtime codec policy. H.263 input remains refused.
Matroska adds `pgs_frame_merge` and VP9 adds `vp9_superframe` bitstream filters.
The filter library always registers audio and video buffer source/sink pairs.
`avcodec` also supplies required internal conversion helpers.

The old Linux build's complete explicit enable list was:

```text
gpl, version3, shared, iconv, zlib, libxml2, libsoxr, openssl
libvmaf, fontconfig, libharfbuzz, libfreetype, libfribidi, vulkan, libshaderc, libvorbis
libxcb, xlib, libpulse, opencl, gmp, lzma, amf, libaom
libaribb24, avisynth, chromaprint, libdav1d, libdavs2, libdvdread, libdvdnav, ffnvcodec
cuda-llvm, frei0r, libgme, libkvazaar, libaribcaption, libass, libbluray, libjxl
libmp3lame, libopus, libplacebo, librist, libssh, libtheora, libvpx, libwebp
libzmq, lv2, libvpl, openal, liboapv, libopencore-amrnb, libopencore-amrwb, libopenh264
libopenjpeg, libopenmpt, librav1e, librubberband, sdl2, libsnappy, libsrt, libsvtav1
libtwolame, libuavs3d, libdrm, vaapi, libvidstab, libvvenc, whisper, libx264
libx265, libxavs2, libxvid, libzimg, libzvbi
```

The shared profile opts into only x264, x265, dav1d and zlib. The recorder adds
Linux ALSA, PulseAudio and XCB capture support. Testgen additionally uses libvpx, FreeType and HarfBuzz for the visual-scene
fixtures. These dependencies are not enabled in either shipping profile.

## Exact configuration

The following is the shared-library configure argument vector. The builder adds
only the installation prefix and Windows toolchain arguments described below.

```sh
./configure \
  --disable-everything \
  --disable-autodetect \
  --disable-network \
  --disable-doc \
  --disable-debug \
  --enable-x86asm \
  --x86asmexe=nasm \
  --disable-iconv \
  --enable-gpl \
  --enable-version3 \
  --enable-avcodec \
  --enable-avformat \
  --enable-avfilter \
  --enable-swscale \
  --enable-swresample \
  --enable-protocol=file \
  --enable-libx264 \
  --enable-pic \
  --disable-programs \
  --disable-avdevice \
  --enable-shared \
  --disable-static \
  --enable-libx265 \
  --enable-libdav1d \
  --enable-zlib \
  --enable-decoder=h264,hevc,prores,libdav1d,vp9,dnxhd,mjpeg,png,aac,alac,mp3,flac,opus,vorbis,pcm_s8,pcm_u8,pcm_s16le,pcm_s16be,pcm_s24le,pcm_s24be,pcm_s32le,pcm_s32be,pcm_f32le,pcm_f32be,pcm_f64le,pcm_f64be,pcm_s64le,pcm_s64be,pcm_alaw,pcm_mulaw,pcm_s16le_planar,pcm_s24le_planar,pcm_s32le_planar,rawvideo,ffv1,mpeg4,mpeg2video \
  --enable-demuxer=mov,matroska,wav,mp3,flac,ogg,aac,avi,nut,mpegts,mpegps,mpegvideo,image2,image2pipe,image_png_pipe,image_jpeg_pipe \
  --enable-parser=h264,hevc,av1,vp9,dnxhd,mjpeg,png,aac,mpegaudio,flac,opus,vorbis,mpeg4video,mpegvideo \
  --enable-encoder=libx264,libx265,aac,pcm_s16le \
  --enable-muxer=mp4,mov,matroska,wav \
  --enable-bsf=aac_adtstoasc,extract_extradata,vp9_superframe_split \
  --enable-filter=buffer,buffersink,scale,format
```

Windows adds `--toolchain=msvc --arch=x86_64 --target-os=win64` and
`--pkg-config=<repo>/third_party/pkgconf/pkgconf/.bin/pkgconf.exe` and
`--pkg-config-flags=--static`. Native pkgconf reads native vcpkg paths.
The build retains normal optimized assembly. NASM is a build tool, never a
shipping dependency. When Linux has no NASM, the builder installs NASM 2.16.03
locally from nasm.us, verifies SHA-256
`1412a1c760bbd05db026b6c0d1657affd6631cd0a63cddb6f73cc6d4aa616148`,
and compiles under the same shared lock. Its source tree is removed afterward.
CI supplies NASM through its build-tool packages. No system packages are changed
by the local builder.

## Recording and fixture isolation

The shared profile has no programs or avdevice. The `recorder` profile builds
FFmpeg libraries statically into `third_party/ffmpeg-recorder/bin/ffmpeg`.
It has no file demuxers or general media input. Its file protocol is
needed to write recordings. It has libx264/AAC/PCM encoders and MP4/M4A/WAV muxers,
crop/scale/format, audio conversion and recording-duration trim filters, plus their
internal dependencies.
Upstream selects crop/orientation filters for the executable, plus MOV's AC3 parser and AAC/VP9 bitstream filters; no Dolby or VP9 decoder is enabled.
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
The CLI is not used for import or export.
Feedback voice captures microphone input to 16 kHz mono PCM WAV. The Linux
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
profiles, exports the FFmpeg 8 headers/libraries and recorder PATH, and verifies
the ABI. C/C++, make, Python 3.12+, pkg-config and explicit development libraries
are prerequisites. The dependency installer lists Debian and Arch package names.
No package installation is performed by setup or the source builder.

Windows requires MSVC, Python 3.12+ and MSYS2 make/diffutils/pkgconf/nasm.
Run `scripts/prepare-ffmpeg-windows.ps1` once to build x264, x265, dav1d, zlib and
libvpx, FreeType and HarfBuzz with the static MSVC vcpkg triplet. Its registry is pinned to
`0df93a05b9de080129e653ec4776d7e14a1b2e4f`.
`setup-ffmpeg.ps1` imports vcvars64, provisions pinned pkgconf/libclang helpers,
then invokes the same Python source builder. Cached installations need neither
vcpkg nor MSYS2 to run. FFmpeg DLLs and recorder have separate directories.

`ffmpeg.yml` is a reusable Linux/Windows source-build matrix. CI and release
workflows depend on it and restore the exact outputs with `actions/cache`.
The cache key hashes the pinned source checksum, configure profile and builder,
plus OS and architecture. Linux keys also include the distribution and glibc ABI.
There are no fallback cache keys. Each install has a matching build-key marker
and its actual configure vector. The first push builds
Windows DLLs and the capture CLI with MSVC, audits registrations and runs the
ordinary Windows workspace link/tests against those DLLs. No FFmpeg releases,
new publishing step or outward distribution service is introduced.

Linux make uses `flock -o /tmp/kinewright-cargo-heavy.lock nice -n 10 make -j4 install`
for libraries, or `install-progs` for tools, retaining the lock through installation.
Cargo gates use that lock separately, with four jobs and low priority.
Linux packaging copies the FFmpeg libraries, recorder, and their non-glibc
shared dependencies. Windows static external dependencies require no extra codec
DLLs. Testgen is excluded by explicit source paths in both packaging scripts.

## Runtime refusals

Certified custom IO keeps protocol `none`, deny_open, bounded probing, a stream
cap, and the format whitelist. It now sets `codec_whitelist` before header and
stream-info probing. A Rust policy check refuses unsupported stream codec IDs
before probe decoders open, and checks again at decoder-context creation.
Legacy path-based readers and delivery verification use the same policy through
the existing FFI boundary, retaining the reader interrupt callback.

QTRLE in MOV and H.263 in AVI produce
`OwnedIoError::UnsupportedCodec` or `MediaError::UnsupportedCodec`, with recovery
code `unsupported_codec`. It does not enter certified stream-info decoding.
The H.263 witness exercises runtime refusal of a decoder that is compiled in.
A WAV-only test policy also rejects a valid compiled AVI demuxer before probing,
so format-guard coverage does not depend only on absent playlist components.
The existing generic media incident displays the refusal's codec and message.

## Measurements and exclusions

Registrations are measured with `scripts/inspect-ffmpeg.py`, using FFmpeg's
codec, demuxer and parser iterators rather than executable feature listings.
AVCC H.264/HEVC is decoded directly by their native decoders, so the separate
Annex B conversion filters and the unused AAC-LATM parser are omitted.
Physical sizes count each versioned shared library once, excluding symlinks,
headers and import libraries. External system dependencies are reported separately.
On this host, the recorder is 4,612,472 bytes. Its and the libraries' combined
non-glibc external dependency closure is 39,412,832 bytes across 26 libraries.
The combined payload is 55,708,592 bytes, before application and licence files.
These dependency sizes are host-specific and include PulseAudio dependencies.

| Linux x86_64 | Before | MS1 |
| --- | ---: | ---: |
| Registered decoders | 550 | 38 |
| Registered demuxers | 362 | 16 |
| Registered parsers | 64 | 16 |
| FFmpeg shared-library bytes | 211,707,496 | 11,683,288 |

The old Linux baseline is the checksum-verified mifi archive named by the old
setup. Windows baseline sizes are not measured on this Linux worker. CI prints
the Windows source-build counts and DLL sizes, and refuses unexpected registry
counts. Evidence and gate summaries are in `docs/reports/ms1.md`.

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
