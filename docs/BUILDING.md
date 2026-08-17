# Building OpenReel

OpenReel is a native desktop app for **64-bit Windows** (MSVC) and **64-bit Linux**
(glibc). The media crate links FFmpeg at build time and loads its shared
libraries at test and run time. Each platform provisions a pinned GPL FFmpeg 8
build into `third_party/ffmpeg` — no system FFmpeg install is required.

## Linux (x86_64)

### Prerequisites

- A 64-bit GNU/Linux distribution with glibc 2.28 or newer (Ubuntu 22.04+,
  Debian 12+, Fedora 39+, and current Arch all qualify).
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
cargo run -p openreel-app
```

`source` the setup script in the same shell as Cargo. It downloads the pinned
FFmpeg 8.0 x86_64 shared GPL build (the same 8.0 ABI Windows links), verifies
its SHA-256, extracts it to `third_party/ffmpeg`, and exports `FFMPEG_DIR`,
`PKG_CONFIG_PATH`, `PATH`, and `LD_LIBRARY_PATH` for `ffmpeg-sys-next` and the
FFmpeg CLI.

The script downloads this exact shared GPL build:

- Provider: mifi FFmpeg-Builds (BtbN linux64 gpl-shared 8.0)
- FFmpeg: `n8.0-latest`, linux64, shared, GPL
- Archive: `ffmpeg-n8.0-latest-linux64-gpl-shared-8.0.tar.xz`
- SHA-256: `c201d31f5c8a3b169345101c63ca70f71442848a271bec4a16ca29a1876e5cb1`

### Environment variables

`setup-ffmpeg.sh` exports these for the current shell:

```text
PKG_CONFIG_PATH=<repo>/third_party/ffmpeg/lib/pkgconfig
FFMPEG_DIR=<repo>/third_party/ffmpeg
PATH=<repo>/third_party/ffmpeg/bin:<existing PATH>
LD_LIBRARY_PATH=<repo>/third_party/ffmpeg/lib:<existing LD_LIBRARY_PATH>
```

If the script is executed rather than sourced, it still provisions the archive
and prints the `source` command to run next.

### Packaging

After a release build:

```bash
source ./scripts/setup-ffmpeg.sh
cargo build --package openreel-app --release --locked
./scripts/package-linux.sh --version 0.1.0
./scripts/test-linux-bundle.sh --bundle-dir ./dist/linux-x64
```

The staged bundle is `dist/linux-x64` (`OpenReel`, `ffmpeg`, `lib/libav*.so*`,
licenses, a `.desktop` file). `package-linux.sh` then writes
`dist/tarball/OpenReel-<version>-linux-x64.tar.gz`. `patchelf` sets
`$ORIGIN/lib` so the binary finds bundled FFmpeg without a machine-level
install.

## Windows (x86_64 MSVC)

M0 targets 64-bit Windows with the MSVC Rust toolchain. The media crate links
FFmpeg at build time and loads its DLLs at test and run time.

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
cargo run -p openreel-app
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
the FFmpeg DLLs when tests or OpenReel run. `FFMPEG_DIR` is an OpenReel
convenience variable for later media milestones; `ffmpeg-sys-next` itself uses
pkg-config. The script imports the complete environment emitted by
`vcvars64.bat`, including `INCLUDE`, `LIB`, `LIBPATH`, `WindowsSdkDir`,
`VCINSTALLDIR`, and `VCToolsInstallDir`; `BINDGEN_EXTRA_CLANG_ARGS` passes the
`INCLUDE` directories to bindgen's provisioned libclang. It then restores any
caller `PATH` entries so the Rust toolchain remains available.

Environment changes are not permanent. Run the setup script again in each new
PowerShell session before building, testing, or launching OpenReel.

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
