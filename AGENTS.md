# AGENTS.md

## Cursor Cloud specific instructions

### Platform reality: Windows/MSVC desktop app, Linux-native on this VM

OpenReel targets **64-bit Windows (MSVC)** and **64-bit Linux (glibc)** — see
`docs/BUILDING.md`. This Cloud Agent VM is Linux, so the full workspace is
buildable here after provisioning FFmpeg and the native desktop libraries:

- `openreel-core` — pure logic (document model, `Operation` set, the `Core` actor,
  undo/redo, `.openreel` JSON serde). No extra system deps.
- `openreel-media`, `openreel-agent`, `openreel-app` — FFmpeg 8.x shared libs
  (not Ubuntu's FFmpeg 6.1), Vulkan (`wgpu`, including Mesa lavapipe), ALSA
  (`cpal`), Whisper (`whisper.cpp` via CMake), GTK 3 (`rfd`), and X11/Wayland
  (`eframe`/`winit`). `scripts/install-linux-deps.sh` plus
  `source scripts/setup-ffmpeg.sh` provide these. Do **not** run
  `scripts/setup-ffmpeg.ps1` on Linux (it downloads the Windows MSVC build).

Windows FFmpeg/GPU-dependent CI remains in `.github/workflows/ci.yml` on
`windows-latest`. Linux CI runs the same workspace commands on `ubuntu-latest`.

### Toolchain

The project needs Rust **>= 1.92** (edition 2024, `rust-version = "1.92"`). The VM's
default rustup toolchain can be older (1.83); the environment update script installs
and defaults to `stable` (currently 1.97), which satisfies this. There is no
`rust-toolchain.toml`, so `stable` is used.

### Commands (run from repo root)

```bash
./scripts/install-linux-deps.sh          # once per machine
source ./scripts/setup-ffmpeg.sh         # once per shell
cargo build --workspace
cargo test  --workspace
cargo fmt   -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p openreel-app
```

Core-only (no FFmpeg) still works:

```bash
cargo build -p openreel-core
cargo test  -p openreel-core
```

There are no long-running services, databases, or dev servers — OpenReel is a
desktop GUI binary. The only "server" is an in-process, ephemeral, localhost MCP
endpoint the app starts for agent sessions. Launching the GUI needs a display
(`DISPLAY` is typically set on this VM).
