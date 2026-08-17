# AGENTS.md

## Cursor Cloud specific instructions

### Platform reality: this is a Windows/MSVC desktop app on a Linux VM

OpenReel targets **64-bit Windows with the MSVC toolchain** (see `docs/BUILDING.md`).
The Cloud Agent VM is Linux, so only part of the workspace is buildable/testable here:

- `openreel-core` — pure logic (document model, `Operation` set, the `Core` actor,
  undo/redo, `.openreel` JSON serde). **Builds, tests, lints cleanly on Linux.**
  This is the product's architectural keystone (every human/agent edit is an
  `Operation` through one actor), so it is the meaningful Linux dev surface.
- `openreel-media`, `openreel-agent`, `openreel-app` — **cannot build on this Linux VM.**
  They require FFmpeg **8.x** MSVC import libs (Ubuntu 24.04 only ships FFmpeg 6.1),
  a working GPU for `wgpu`, ALSA (`cpal`), Whisper (`whisper.cpp`), and the Windows
  toolchain. `openreel-agent` and `openreel-app` transitively depend on
  `openreel-media`, so they inherit these blockers. `cargo build --workspace` fails
  at `alsa-sys` / `ffmpeg-sys-next`; this is expected — scope work to `-p openreel-core`.

Do **not** run `scripts/setup-ffmpeg.ps1` (PowerShell, downloads a Windows MSVC
FFmpeg build). It does nothing useful on Linux. FFmpeg/GPU-dependent work must be
validated on Windows (that is what CI, `.github/workflows/ci.yml`, uses).

### Toolchain

The project needs Rust **>= 1.92** (edition 2024, `rust-version = "1.92"`). The VM's
default rustup toolchain can be older (1.83); the environment update script installs
and defaults to `stable` (currently 1.97), which satisfies this. There is no
`rust-toolchain.toml`, so `stable` is used.

### Commands (run from repo root, scoped to core)

```bash
cargo build -p openreel-core
cargo test  -p openreel-core                        # ~51 contract/proptest tests
cargo fmt   -p openreel-core -- --check
cargo clippy -p openreel-core --all-targets -- -D warnings
```

There are no long-running services, databases, or dev servers — OpenReel is a desktop
GUI binary. The only "server" is an in-process, ephemeral, localhost MCP endpoint the
app starts for agent sessions (Windows-only path). Nothing to start on Linux.
