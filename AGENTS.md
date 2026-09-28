# AGENTS.md

## Cursor Cloud specific instructions

### Platform reality: Windows/MSVC desktop app, Linux-native on this VM

Kinewright targets **64-bit Windows (MSVC)** and **64-bit Linux (glibc)** — see
`docs/BUILDING.md`. This Cloud Agent VM is Linux, so the full workspace is
buildable here after provisioning FFmpeg and the native desktop libraries:

- `kinewright-core` — pure logic (document model, `Operation` set, the `Core` actor,
  undo/redo, `.kinewright` JSON serde). No extra system deps.
- `kinewright-media`, `kinewright-agent`, `kinewright-app` — FFmpeg 8.x shared libs
  (not Ubuntu's FFmpeg 6.1), Vulkan (`wgpu`, including Mesa lavapipe), ALSA
  (`cpal`), Whisper (`whisper.cpp` via CMake), GTK 3 (`rfd`), and X11/Wayland
  (`eframe`/`winit`). `scripts/install-linux-deps.sh` plus
  `source scripts/setup-ffmpeg.sh` provide these. Do **not** run
  `scripts/setup-ffmpeg.ps1` on Linux (it downloads the Windows MSVC build).

Windows FFmpeg/GPU-dependent CI remains in `.github/workflows/ci.yml` on
`windows-latest`. Linux CI runs the same workspace commands on `ubuntu-latest`.
CI has a fast tier (every non-docs push) and a slow tier (push to `main`, manual
run, or `[slow-tier]` in the head commit message); the slow tests are listed in
`ci/slow-tests.txt` and gated by each crate's `slow-tests` feature. Kani runs in
`.github/workflows/kani.yml`, only when its source files change.

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
cargo test  --workspace                  # fast tier: slow tests are skipped
cargo test  --workspace --features kinewright-media/slow-tests,kinewright-agent/slow-tests,kinewright-app/slow-tests   # full suite
python3 scripts/slow_tests.py lint       # slow-test manifest and markers agree
cargo fmt   -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p kinewright-app
```

Core-only (no FFmpeg) still works:

```bash
cargo build -p kinewright-core
cargo test  -p kinewright-core
```

There are no long-running services, databases, or dev servers — Kinewright is a
desktop GUI binary. The only "server" is an in-process, ephemeral, localhost MCP
endpoint the app starts for agent sessions. Launching the GUI needs a display
(`DISPLAY` is typically set on this VM).
