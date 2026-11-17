# Building Kinewright

Kinewright is a native desktop app for **64-bit Windows** (MSVC) and **64-bit Linux**
(glibc). The media crate links FFmpeg at build time and loads its shared
libraries at test and run time. Each platform provisions a pinned GPL FFmpeg 8
build into `third_party/ffmpeg` — no system FFmpeg install is required.

## Linux (x86_64)

### Prerequisites

- A 64-bit GNU/Linux distribution with glibc matching the source-build host or newer. The dependency
  installer supports Ubuntu 22.04+, Debian 12+, current Arch, CachyOS, and
  Omarchy.
- Rust installed by `rustup`, using `stable` (`rust-version = "1.92"`).
- Python 3.12+, `pkg-config`, a C/C++ toolchain, CMake, and the packages listed in
  `scripts/install-linux-deps.sh` (ALSA, Vulkan/Mesa, X11/Wayland, GTK 3,
  libclang).

A GPU is optional. Headless tests fall back to Mesa's lavapipe software Vulkan
implementation. A display (or Xvfb) is required only to *launch* the GUI.

### Clean-clone setup

```bash
./scripts/install-linux-deps.sh
source ./scripts/setup-ffmpeg.sh
python3 scripts/build-ffmpeg.py --profile testgen
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

`source` the setup script in the same shell as Cargo. It builds the pinned minimal
FFmpeg 8.0.3 libraries and a separate recording-only CLI from verified upstream
source. See [MS1 minimal FFmpeg](MS1-MINIMAL-FFMPEG.md) for the exact profile,
checksum, prerequisites and measurements. The build uses four make jobs and the
shared heavy-build lock. An unchanged install is reused.

For tests, provision the isolated fixture generator separately:

```bash
python3 scripts/build-ffmpeg.py --profile testgen
```

The test tools never appear on the app PATH or in shipping bundles. The source
build requires Python 3.12+ and explicitly listed codec development libraries.
The glibc floor follows the Linux build host; use the CI Ubuntu output for release
packaging, rather than a rolling-distribution development build.

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
PATH=<repo>/third_party/ffmpeg-recorder/bin:<existing PATH>
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
./scripts/package-linux.sh --version 0.1.0 --no-binary-cache
./scripts/test-linux-bundle.sh --bundle-dir ./dist/linux-x64
```

The staged bundle is `dist/linux-x64` (`Kinewright`, `ffmpeg`, `lib/libav*.so*`,
licenses, a `.desktop` file). `package-linux.sh` then writes
`dist/tarball/Kinewright-<version>-linux-x64.tar.gz`. `patchelf` sets
`$ORIGIN/lib` so the binary finds bundled FFmpeg without a machine-level
install.

## Windows (x86_64 MSVC)

M0 targets 64-bit Windows with the MSVC Rust toolchain. The media crate links
FFmpeg at build time and loads its DLLs at test and run time.

### Prerequisites

- Windows 10 or newer.
- Visual Studio 2019 Build Tools or newer with **Desktop development with C++**.
- Rust installed by `rustup`, using `stable-x86_64-pc-windows-msvc`.
- Python 3.12 or newer available as `python`.
- MSYS2 with make, diffutils, pkgconf, and nasm.

No system FFmpeg is used. The source-build dependencies are listed below.

### Clean-clone setup

Open PowerShell in the repository root and run:

```powershell
rustup default stable-x86_64-pc-windows-msvc
& .\scripts\prepare-ffmpeg-windows.ps1
& .\scripts\setup-ffmpeg.ps1
python scripts/build-ffmpeg.py --profile testgen
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

Before the first source build, install MSYS2 make, diffutils, pkgconf and nasm, then run:

```powershell
& .\scripts\prepare-ffmpeg-windows.ps1
& .\scripts\setup-ffmpeg.ps1
python scripts/build-ffmpeg.py --profile testgen
```

This builds the explicit static MSVC dependencies using pinned vcpkg, then the
same verified FFmpeg source and profile as Linux. Python 3.12+ is required.
`setup-ffmpeg.ps1` provisions the pinned pkgconf and libclang helpers locally.
See [MS1 minimal FFmpeg](MS1-MINIMAL-FFMPEG.md) for the complete build contract.

### Environment variables

`setup-ffmpeg.ps1` sets every variable for the current PowerShell process:

```text
PKG_CONFIG=<repo>\third_party\pkgconf\pkgconf\.bin\pkgconf.exe
PKG_CONFIG_PATH=<repo>\third_party\ffmpeg\lib\pkgconfig
LIBCLANG_PATH=<directory containing provisioned libclang.dll>
FFMPEG_DIR=<repo>\third_party\ffmpeg
PATH=<repo>\third_party\ffmpeg\bin;<repo>\third_party\ffmpeg-recorder\bin;<repo>\third_party\pkgconf\pkgconf\.bin;<MSVC PATH>;<existing PATH>
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
