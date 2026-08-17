# Contributing to OpenReel

Thanks for your interest! OpenReel is early and moving fast, which means contributions land easily — and also means the ground rules below matter more than usual.

## Getting set up

Follow [docs/BUILDING.md](docs/BUILDING.md). Short version:

- **Windows:** MSVC Rust toolchain + Python, then `.\scripts\setup-ffmpeg.ps1` once per shell.
- **Linux:** `./scripts/install-linux-deps.sh` once per machine, then `source ./scripts/setup-ffmpeg.sh` once per shell.

No system FFmpeg, vcpkg, or LLVM install is required — everything is provisioned locally and pinned.

Run the test suite with `cargo test --workspace`. Some tests are gated behind environment variables because they need real hardware or your own agent subscription:

| Variable | Enables |
|---|---|
| `OPENREEL_AUDIO_TEST=1` | Real audio-device playback tests |
| `OPENREEL_AGENT_TEST=1` | Live agent E2E via your installed Claude Code / Codex CLI (uses your subscription; costs cents) |
| `OPENREEL_TRANSCRIPT_TEST=1` | Real Whisper transcription E2E (downloads the model once) |

CI runs the ungated suite on clean Windows and Linux runners; your PR must keep both green.

## The ground rules (short but firm)

These come from [OpenReel-Architecture.md](OpenReel-Architecture.md) and are enforced in review:

1. **Every document mutation is an `Operation`** applied through the Core actor. UI code, agent code, anything — nothing mutates `Document` directly. A PR where the chat panel or a widget edits the document by hand is wrong by construction.
2. **Operations are pure.** No file, network, or process I/O inside `apply()`. Do the I/O first, then apply an operation carrying the results.
3. **Time is integer frames.** No floats in any time-carrying type. Mixed frame rates go through the exact rational mapping in `openreel-core::time`.
4. **Undo is snapshots**, not inverse operations. Don't implement inverses.
5. **The UI thread never blocks** on decode, disk, or network. Heavy work happens on workers; results arrive over channels. The audio callback is realtime code: no locks, no allocation.
6. **One render path.** Preview and export use the same wgpu compositor. Don't add a second effects implementation.
7. **Dependency versions are pinned once, in the workspace root `Cargo.toml`.** Use eframe's re-exported egui/wgpu; never declare those separately.
8. **Adding an `Operation` variant?** The agent inherits it automatically as a tool via the schema pipeline — the exhaustiveness guard in `openreel-agent` will make you acknowledge the new tool surface. Destructive variants must be added to the confirmation broker's match.

## Visual changes

The design system is specified in [docs/DESIGN.md](docs/DESIGN.md) ("Cut Room"). Every color, spacing, radius, and motion value traces to a token in `openreel-app/src/theme.rs` — use tokens, don't invent values inline. The accent color has exactly three jobs: playhead, selection, live agent state. Include a screenshot in any UI-affecting PR (`OPENREEL_SCREENSHOT_TO=out.png OPENREEL_SCREENSHOT_AFTER_MS=3000 cargo run -p openreel-app -- project.openreel`).

## Pull requests

- Branch from `main`; keep PRs focused on one thing.
- `cargo build --workspace` and `cargo test --workspace` green locally before opening.
- Describe what you verified by hand, not just what you changed.
- Commit messages: imperative summary line, body explaining *why*.

## Releases

Maintainers cut releases by pushing a version tag (`v*`), which builds, tests, packages the Windows installer and Linux tarball, and publishes a GitHub Release. See [docs/RELEASING.md](docs/RELEASING.md).

## Licensing

OpenReel is GPL-3.0-only. By submitting a contribution you agree it is licensed under GPLv3 (inbound = outbound). There is no CLA and no copyright assignment — you keep your copyright.

## Questions

Open a [discussion or issue](https://github.com/CanadaApollo6/OpenReel/issues). For security matters, see [SECURITY.md](SECURITY.md).
