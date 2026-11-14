# Building Kinewright

Kinewright is a native desktop app for **64-bit Windows** (MSVC) and **64-bit Linux**
(glibc). The media crate links FFmpeg at build time and loads its shared
libraries at test and run time. Each platform provisions a pinned GPL FFmpeg 8
build into `third_party/ffmpeg` — no system FFmpeg install is required.

## Linux (x86_64)

### Prerequisites

- A 64-bit GNU/Linux distribution with glibc 2.28 or newer. The dependency
  installer supports Ubuntu 22.04+, Debian 12+, current Arch, CachyOS, and
  Omarchy.
- Rust installed by `rustup`, using `stable` (`rust-version = "1.92"`).
- Python 3, `pkg-config`, a C/C++ toolchain, CMake, and the packages listed in
  `scripts/install-linux-deps.sh` (ALSA, Vulkan/Mesa, X11/Wayland, GTK 3,
  libclang).

A GPU is optional. Headless tests fall back to Mesa's lavapipe software Vulkan
implementation. A display (or Xvfb) is required only to *launch* the GUI.

### Clean-clone setup

```bash
./scripts/install-linux-deps.sh
source ./scripts/setup-ffmpeg.sh
cargo build --workspace
cargo test --workspace
cargo run -p kinewright-app
```

`cargo test --workspace` runs the fast test tier. The slowest tests (the list is
`ci/slow-tests.txt`) run only when each crate's `slow-tests` feature is on:

```
cargo test --workspace --features kinewright-media/slow-tests,kinewright-agent/slow-tests,kinewright-app/slow-tests
```

Programme documents whose exit gate says `cargo test --workspace` mean the fast
tier unless they say otherwise; CI runs the slow tier on every push to `main`.

`source` the setup script in the same shell as Cargo. It downloads the pinned
FFmpeg 8.0 x86_64 shared GPL build (the same 8.0 ABI Windows links), verifies
its SHA-256, extracts it to `third_party/ffmpeg`, and exports `FFMPEG_DIR`,
`PKG_CONFIG_PATH`, `PATH`, `RUSTFLAGS`, and `LD_LIBRARY_PATH` for
`ffmpeg-sys-next` and the FFmpeg CLI. The native linker flag keeps a newer
system FFmpeg from taking precedence over the pinned ABI when another native
crate publishes `/usr/lib` as a global Cargo search path.

The script downloads this exact shared GPL build:

- Provider: mifi FFmpeg-Builds (BtbN linux64 gpl-shared 8.0)
- FFmpeg: `n8.0-latest`, linux64, shared, GPL
- Archive: `ffmpeg-n8.0-latest-linux64-gpl-shared-8.0.tar.xz`
- SHA-256: `c201d31f5c8a3b169345101c63ca70f71442848a271bec4a16ca29a1876e5cb1`

#### Why FFmpeg 8 is pinned

`ffmpeg-next` tracks FFmpeg's major/minor release line, and this workspace pins
`ffmpeg-next = 8.0.0`. Windows and Linux therefore provision the same FFmpeg 8
ABI instead of compiling against whichever incompatible version a host happens
to ship. This keeps media behavior and release bundles reproducible and avoids
mixing generated FFmpeg 8 bindings with a newer shared library.

The host may still install and use another FFmpeg version. In particular,
rolling Arch-family distributions such as CachyOS can move to a newer ABI
without affecting Kinewright: the setup script prepends the bundled headers,
libraries, and CLI, then verifies that `libavcodec` ABI 62 was selected.

### Environment variables

`setup-ffmpeg.sh` exports these for the current shell:

```text
PKG_CONFIG_PATH=<repo>/third_party/ffmpeg/lib/pkgconfig
FFMPEG_DIR=<repo>/third_party/ffmpeg
PATH=<repo>/third_party/ffmpeg/bin:<existing PATH>
LD_LIBRARY_PATH=<repo>/third_party/ffmpeg/lib:<existing LD_LIBRARY_PATH>
RUSTFLAGS=-Lnative=<repo>/third_party/ffmpeg/lib <existing RUSTFLAGS>
```

If the script is executed rather than sourced, it still provisions the archive
and prints the `source` command to run next.

### Packaging

After a release build:

```bash
source ./scripts/setup-ffmpeg.sh
cargo build --package kinewright-app --release --locked
./scripts/package-linux.sh --version 0.1.0
./scripts/test-linux-bundle.sh --bundle-dir ./dist/linux-x64
```

The staged bundle is `dist/linux-x64` (`Kinewright`, `ffmpeg`, `lib/libav*.so*`,
licenses, a `.desktop` file). `package-linux.sh` then writes
`dist/tarball/Kinewright-<version>-linux-x64.tar.gz`. `patchelf` sets
`$ORIGIN/lib` so the binary finds bundled FFmpeg without a machine-level
install.

## Windows (x86_64 MSVC)

M0 targets 64-bit Windows with the MSVC Rust toolchain. The media crate links
FFmpeg and NeMo-Speech at build time and loads their DLLs at test and run time.
The Windows installer must also ship the MSVC OpenMP runtime (`vcomp140.dll`) required by the NeMo-Speech DLLs.

### Prerequisites

- Windows 10 or newer.
- Visual Studio 2019 Build Tools or newer with **Desktop development with C++**.
- Rust installed by `rustup`, using `stable-x86_64-pc-windows-msvc`.
- Python 3.10 or newer available as `python`.

No system FFmpeg, vcpkg, pkg-config, or LLVM installation is required.

### Clean-clone setup

Open PowerShell in the repository root and run:

```powershell
rustup default stable-x86_64-pc-windows-msvc
& .\scripts\setup-ffmpeg.ps1
cargo build --workspace
cargo test --workspace
cargo run -p kinewright-app
```

Keep the setup and Cargo commands in the same PowerShell process. The setup
script sets process-local environment variables needed by `ffmpeg-sys-next`.

### Build-cache storage

The workspace profiles disable Cargo incremental state and full dependency
debug symbols. This keeps repeated Windows test builds from accumulating tens
of gigabytes of PDB, rlib, and incremental artifacts under `target`.

To reclaim build outputs without deleting benchmark runs or downloaded eval
fixtures:

```powershell
.\scripts\clean-build-cache.ps1
```

Use `-WhatIf` to inspect the exact directories first. The script removes only
known regenerable build-cache directories and preserves `target/evals` and
`target/eval-fixtures`.

This cleanup is also automatic. Every `setup-ffmpeg.ps1` invocation measures
the known Cargo build directories and prunes them once they reach 6 GiB. Eval
runs, fixture media, and other benchmark artifacts do not count toward that
limit and are never removed. Change the threshold for one invocation with
`-BuildCacheLimitGiB`, or use `-SkipBuildCachePrune` when deliberately retaining
a large diagnostic build:

```powershell
& .\scripts\setup-ffmpeg.ps1 -BuildCacheLimitGiB 6
& .\scripts\setup-ffmpeg.ps1 -SkipBuildCachePrune
```

The standalone cleaner supports the same size gate:

```powershell
.\scripts\clean-build-cache.ps1 -MaximumGiB 6
```

The script downloads this exact shared GPL build:

- Provider: System233 FFmpeg MSVC Prebuilt
- FFmpeg: `8.0.1-r3`, x64, shared, GPL
- Archive: `ffmpeg-8.0.1-r3_x64-windows-shared-gpl.zip`
- SHA-256: `3399afab045f6bc64301001d4f5ca1aba3d6df96948cc1799028cf2f24ede433`

It extracts the archive to `third_party/ffmpeg`, then installs these pinned
build helpers locally:

- `pkgconf==3.0.1.post0` to `third_party/pkgconf`
- `libclang==18.1.1` to `third_party/libclang`

All three directories are ignored by Git.

### Environment variables

`setup-ffmpeg.ps1` sets every variable for the current PowerShell process:

```text
PKG_CONFIG=<repo>\third_party\pkgconf\pkgconf\.bin\pkgconf.exe
PKG_CONFIG_PATH=<repo>\third_party\ffmpeg\lib\pkgconfig
LIBCLANG_PATH=<directory containing provisioned libclang.dll>
FFMPEG_DIR=<repo>\third_party\ffmpeg
PATH=<repo>\third_party\ffmpeg\bin;<repo>\third_party\pkgconf\pkgconf\.bin;<MSVC PATH>;<existing PATH>
BINDGEN_EXTRA_CLANG_ARGS=<MSVC and Windows SDK include directories from vcvars64.bat>
```

`PKG_CONFIG` and `PKG_CONFIG_PATH` let `ffmpeg-sys-next` find the MSVC import
libraries and headers. `LIBCLANG_PATH` lets its bindgen step load libclang.
`PATH` is required both for the crate's build-time feature probe and for loading
the FFmpeg DLLs when tests or Kinewright run. `FFMPEG_DIR` is an Kinewright
convenience variable for later media milestones; `ffmpeg-sys-next` itself uses
pkg-config. The script imports the complete environment emitted by
`vcvars64.bat`, including `INCLUDE`, `LIB`, `LIBPATH`, `WindowsSdkDir`,
`VCINSTALLDIR`, and `VCToolsInstallDir`; `BINDGEN_EXTRA_CLANG_ARGS` passes the
`INCLUDE` directories to bindgen's provisioned libclang. It then restores any
caller `PATH` entries so the Rust toolchain remains available.

Environment changes are not permanent. Run the setup script again in each new
PowerShell session before building, testing, or launching Kinewright.

## M0 contract notes

- All time-bearing values are integer frames. Mixed-rate conversion maps
  absolute source-frame boundaries with integer round-to-nearest arithmetic.
  Mapping both ends of a range preserves shared clip boundaries and prevents
  cumulative drift.
- `SplitClip.at` is a project-frame position. If that position does not map to
  an integer source-frame boundary, the operation is rejected with
  `UnrepresentableSplit`. The architecture leaves this ambiguity open; M0
  chooses rejection instead of storing hidden fractional time.
- CLI detection in M0 only checks `PATH`. Authentication and protocol-version
  checks require starting the CLI and are intentionally deferred to M3.

## Recognition runtime

The existing setup command also provisions NeMo-Speech.cpp v0.3.0 from commit
`20999cc921d2b14aa58708f61756a8b2e787ba20`, llama.cpp
`bd4f514db14d87fded667787a7a963bfbaa98e89` and SentencePiece
`31646a467d2051eb904e0b45de3a73e91fe1c1e3`. Each source is fetched by its full
commit hash from its official GitHub repository, and detached `HEAD` is checked
against that pin. CMake builds against the
prepared dependency prefix without fetching anything. The SDK cache key includes
pins, patches, compiler and build settings; only `lib`, `bin`, `include`, `share`
and `.build-key` remain after installation. Cargo's media `build.rs` only links.

The CPU build uses `GGML_NATIVE=OFF`; it requires AVX2, FMA, F16C, BMI2 and SSE4.2.
Model loading checks these features before calling the native library. Set up each
shell before cargo so `KINEWRIGHT_NEMO_DIR`, the native search path and the runtime
loader path are present. Direct rustc probes linking workspace crates also need
`-L native=$KINEWRIGHT_NEMO_DIR/lib`.

Packaging copies both NeMo shared libraries and the notices under `LICENSES/`.
Linux additionally requires the system OpenMP runtime (`libgomp.so.1`). Windows
OpenMP installer work remains the follow-up noted above. Recognition uses CPU
inference on both platforms. Model files and licences, cache identity, session
clocks and the real-model slow-tier witness are documented in
[TRANSCRIPTION.md](TRANSCRIPTION.md).

Application data (models, caches and feedback captures) is persistent:
`$XDG_DATA_HOME/kinewright` on Linux when that variable is absolute, otherwise
`$HOME/.local/share/kinewright`; `%LOCALAPPDATA%/Kinewright` on Windows. Linux
creates a private user-owned 0700 root. Linked or foreign-owned roots are refused,
and missing home configuration reports an error instead of using temporary storage.

Links above the chosen data or explicit model root are resolved through its parent;
the root itself and entries below it must be real entries, never links or reparse points.
