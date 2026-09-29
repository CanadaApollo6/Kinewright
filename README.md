<p align="center">
  <img src="docs/assets/kinewright-logo-cyan-play-wordmark.png" width="240" alt="Kinewright — a circular film reel around a play button">
</p>

# Kinewright

**An open-source agentic video editor.** Native on Windows and Linux. Your footage, your subscriptions, your timeline.

[![CI](https://github.com/CanadaApollo6/Kinewright/actions/workflows/ci.yml/badge.svg)](https://github.com/CanadaApollo6/Kinewright/actions/workflows/ci.yml)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)
[![Windows](https://img.shields.io/badge/Windows-native-0078D4?logo=windows&logoColor=white)](docs/BUILDING.md)
[![Linux](https://img.shields.io/badge/Linux-native-FCC624?logo=linux&logoColor=black)](docs/BUILDING.md)

Kinewright is a **native desktop** video editor written in Rust — the same fast binary on **Windows and Linux**, not a web wrapper, Electron shell, or VM. At its core it is an **agentic harness for video editing**. Type "cut the first three seconds and tighten the pauses" into the chat panel, and the agent CLI you already pay for — Claude Code, Codex, Cursor, Muse, OpenCode, Qwen, Kimi, Kiro, Devin, or Copilot — makes the edits on your timeline, using the exact same operations you'd use by hand. Every agent edit lands on the same undo stack as yours: **Ctrl+Z reverses the robot.**

> Early development. The editor works end to end — import, cut, grade, mix, composite, animate, export, agent editing, transcript editing — but expect rough edges. The product goal is an open-source, agent-native Premiere-class editor, built slice by slice. Current work is preview playback performance (full-rate 1080p preview is not there yet — see [PF1](docs/PF1-PLAYBACK-PERFORMANCE.md)), followed by more of the motion and compositing programme.

<p align="center">
  <img src="docs/assets/kinewright-editor.png" width="960" alt="Kinewright with an Iceland landscape cut on the timeline, program monitor, and agent chat">
</p>

<p align="center"><sub>Preview footage: <a href="https://commons.wikimedia.org/wiki/File:Brei%C3%B0armerkurj%C3%B6kull_glacier_lagoon.webm">Breiðarmerkurjökull glacier lagoon</a> by Jason Eppink, <a href="https://creativecommons.org/licenses/by/2.0/">CC BY 2.0</a>; <a href="https://commons.wikimedia.org/wiki/File:Ocean_waves_at_L%C3%A6kjavik_beach,_Iceland.webm">ocean waves at Lækjavik beach</a> by Alexander Grebenkov, <a href="https://creativecommons.org/licenses/by/3.0/">CC BY 3.0</a>. Music: <a href="https://www.scottbuckley.com.au/library/vanguard/">Vanguard</a> by Scott Buckley, <a href="https://creativecommons.org/licenses/by/4.0/">CC BY 4.0</a>.</sub></p>

## What makes it different

- **Native on Windows and Linux.** One Rust desktop app on both platforms — `eframe` / `wgpu` / FFmpeg, not a browser tab. Same editor, same agent harness, same undo stack.
- **Not a video generator.** Models never create footage here. They edit footage *you shot*. The source of truth is your media plus an inspectable edit log — never model output.
- **Bring your own subscription.** Kinewright drives the agent CLIs already installed on your machine (Claude Code, Codex CLI, Cursor Agent, Muse, OpenCode, Qwen Code, Kimi, Kiro, Devin, or Copilot). No API keys to paste, no account, no server, no markup. The CLI handles auth; Kinewright never sees a credential.
- **Human/agent parity by construction.** Every Kinewright mutation — human or agent — is an `Operation` flowing through one core actor. The agent's editing tools are *generated from the operation set*, so it receives the same validated, undoable editing vocabulary as the GUI.
- **Edit by transcript.** Local Whisper transcription (on-device, one-time model download) gives the agent word-level timestamps. "Remove the filler words" becomes a set of precise, frame-accurate cuts.
- **Free forever.** GPLv3. No paid tier, no telemetry, no plans to monetize.

## Features

- Timeline editing: cut, trim, move, split, multi-track, ripple delete/insert with cross-track sync-lock, A/V clip linking, project markers, snapping (markers, cross-track edges, Alt to bypass), filmstrip thumbnails, audio waveforms
- Playback with sample-accurate A/V sync (the audio clock is the master; video never leads) and **multi-track audio mixing that matches the export mixdown to sample-level parity** — per-track gain, pan, mute and solo in a Mixer with per-track, bus and master meters that keeps playing while you mix; parametric EQ, soft-knee compression, gating and a true-peak limiter on editable bus and master chains with latency-compensated lookahead; a constant-power pan law; BS.1770-4 loudness metering (momentary, short-term, integrated, loudness range, true peak) in the Mixer, published by what has actually been heard; keyframed clip gain envelopes drawn as a rubber band on the clip, track gain and pan automation, and bus and master fader automation edited in the Mixer, all evaluated integer-exactly and identically in playback and export, surviving every trim, split, slip and speed change under a stated policy; broadband noise reduction with a learned floor, hum removal and de-click as latency-declaring chain nodes, with room-tone capture and gap fills that butt-join seamlessly; and levels, spectra, repair measurements, and an evidence-only audio QC report the agent can measure through the same mix path
- Source and Program viewers with marked In/Out, visible track patching and targeting, and revision-safe insert/overwrite edits
- Offline media and relink by content identity (SHA-256), with Kinewright-owned caches shown and cleared separately from your source media
- Smooth 4K scrubbing via proxy-resolution preview decode, with performance budgets asserted in tests — and hostile real-world media (VFR phone footage, rotation metadata, HEVC, odd audio) handled by written policy
- Multi-track GPU compositing (wgpu) with effects, crossfades, and **titles as first-class clips** — preview and export share one render path, so what you see is what you export
- **Colour-managed grading** (SDR Rec.709): primary correction, ASC CDL-style wheels and monotone curves, project-owned content-hashed LUTs (technical and creative), secondaries with up to four feathered windows and an HSL qualifier that can be keyframed and tracked, and waveform, RGB parade and vectorscope scopes with reference-shot matching. Every grade is an ordered, inspectable node stack; the agent can measure shots and propose matches, but a proposal stays unapplied until you accept it
- Grade QC and managed delivery: range, gamut, skin-tone and colour-tag checks measured at a high-precision stage of the pipeline, and an optional 10-bit H.264 delivery lane
- **Motion**: keyframes on transform (per-axis scale, rotation, anchor), opacity and effect enables, with timeline key lanes and auto-keying in the inspector; still images as clips; six scene-linear blend modes; adjustment clips; solid-colour clips; push, slide and wipe transitions; and solo preview of any clip
- Export to H.264/AAC mp4 with progress and cancellation, optional loudness normalization to the delivery profile's target (an explicit job setting, off by default, with a true-peak limiter under the ceiling), and decoded audio verification of the written file's loudness and true peak alongside the video check
- Agent editing with senses and plans: transcript, silence, and scene-change inspectors; atomic multi-operation edit plans with a **single undo entry** and one summarized confirmation for anything destructive; plan results self-report remaining dead air so the agent finishes the job
- The agent has *eyes*: it can fetch actual frames from your timeline, not just metadata
- **Measured agent competence**: a scored eval suite (`kinewright-eval`) with an [exact-operation baseline](benchmarks/auto-edit/v1/README.md) and a [finished-cut benchmark](benchmarks/auto-edit/v2/README.md) that renders, probes, hashes, and packages a real MP4 for separate human review — see `docs/EVALS.md`
- The [editorial-truth benchmark](benchmarks/auto-edit/v3/README.md) independently checks rendered speech and exact authored captions; [dialogue-pacing v4](benchmarks/auto-edit/v4/README.md) adds acoustically measured sentence rhythm; the in-progress [generalization v5 gauntlet](benchmarks/auto-edit/v5/README.md) starts testing pinned, licensed real footage; [colour-workflow v6](benchmarks/auto-edit/v6/README.md) scores the six CC7 colour workflows; [audio-workflow v7](benchmarks/auto-edit/v7/README.md) scores the five AU6 audio workflows
- Context-sensitive inspector panel (clips, titles, markers) driven by the same effect table that validates operations
- Transcript panel with click-word-to-seek
- **Typed incidents instead of dead ends**: when something fails (an unsupported colour description, a media or export error), the editor shows a card that explains the problem and applies the safe recovery as a visible, undoable operation. If no deterministic fix exists, you can start an investigator session: the agent proposes a typed fix, and nothing changes until you approve it. Open incidents persist with the project
- Crash recovery from an operation journal
- Project save/load (`.kinewright` JSON), keyboard-first editing (J/K/L, I/O, frame stepping — press `?` in-app)

## Getting started

**Requirements**

- **Windows** 10/11, 64-bit, or **Linux** x86_64 (glibc 2.28+) — native desktop apps on both, with bundled FFmpeg
- At least one agent CLI installed and authenticated, if you want agent editing:
  - [Claude Code](https://docs.anthropic.com/en/docs/claude-code/getting-started) (any Claude subscription)
  - [Codex CLI](https://developers.openai.com/codex/cli) 0.147.0+ (ChatGPT subscription)
  - [Cursor Agent CLI](https://docs.cursor.com/en/cli/installation) (Cursor subscription)
  - [Muse](https://dev.meta.ai) (Meta account)
  - [OpenCode](https://opencode.ai) (your configured providers, e.g. DeepSeek, GLM, Kimi, MiniMax)
  - [Qwen Code](https://github.com/QwenLM/qwen-code) (Qwen OAuth or DashScope key)
  - [Kimi](https://github.com/MoonshotAI/kimi-cli) (Moonshot account)
  - [Kiro CLI](https://kiro.dev/docs/cli) (Kiro subscription)
  - [Devin](https://docs.devin.ai) (Devin subscription)
  - [Copilot CLI](https://docs.github.com/copilot/how-tos/copilot-cli) (Copilot subscription)
- Everything else (FFmpeg, Whisper) is bundled or downloaded automatically

**Install**

There is no published release yet, so build from source (below). When releases are cut, each one publishes a native Windows installer and a native Linux x64 tarball on [Releases](https://github.com/CanadaApollo6/Kinewright/releases) — see [docs/RELEASING.md](docs/RELEASING.md). Manual editing works with no setup; the chat panel lights up automatically when it detects an agent CLI.

**Build from source**

See [docs/BUILDING.md](docs/BUILDING.md) for the full walkthrough.

Windows:

```powershell
git clone https://github.com/CanadaApollo6/Kinewright.git
cd Kinewright
.\scripts\setup-ffmpeg.ps1     # provisions a pinned FFmpeg + build tools locally
cargo build --workspace
cargo run -p kinewright-app
```

Linux:

```bash
git clone https://github.com/CanadaApollo6/Kinewright.git
cd Kinewright
./scripts/install-linux-deps.sh
source ./scripts/setup-ffmpeg.sh   # provisions a pinned FFmpeg 8.x shared GPL build
cargo build --workspace
cargo run -p kinewright-app
```

## How the agent works

Kinewright runs a local MCP (Model Context Protocol) server inside the app and spawns your agent CLI as a subprocess pointed at it. Every session gets the same seven runtime tools, then discovers and loads exact capabilities on demand. The complete generated capability registry stays internal. It has two families:

1. **Edit operations** — plan schemas such as `split_clip`, `trim_clip`, and `add_effect`, auto-generated from the same operation definitions the GUI uses.
2. **Capabilities** — inspectors, planners, proofs, and actions such as timeline summaries, rendered frames, transcripts, analysis, and delivery.

Claude Code and Codex sessions run with their built-in shell/file/web tools disabled or sandboxed away. Copilot gets Kinewright as its only MCP server, but its own shell and file tools stay available and run auto-approved, because Copilot cannot ask headlessly - the weakest boundary of the ten, and the settings panel says so. Cursor, Muse, OpenCode, Qwen, Kimi, Kiro, and Devin receive only the per-project Kinewright MCP endpoint and start in an empty scratch directory, but their agent servers may still expose vendor-owned tools; the settings panel states this weaker boundary explicitly. Destructive Kinewright operations pause for your approval in the chat panel. Costs are surfaced when the harness reports them, and turns are capped per session.

Details: [roadmap and development workflows](docs/ROADMAP-AND-WORKFLOWS.md) · [agent harnesses](docs/agent-harnesses.md) · [M36 agent runtime efficiency](docs/M36-AGENT-RUNTIME-EFFICIENCY.md) · [M37 human-acceptable first cut](docs/M37-HUMAN-ACCEPTABLE-FIRST-CUT.md) · [model-first editor](docs/MODEL-FIRST-EDITOR.md) · [M35 product-position snapshot](docs/PRODUCT-POSITION-M35-2026-08.md) · [transcription](docs/TRANSCRIPTION.md)

Programmes: colour ([CC1](docs/CC1-MANAGED-SDR-PRIMARY.md) managed SDR · [CC2](docs/CC2-SCOPES-AND-MATCHING.md) scopes and matching · [CC3](docs/CC3-CURVES-AND-WHEELS.md) curves and wheels · [CC4](docs/CC4-LOOK-MANAGEMENT.md) looks · [CC5](docs/CC5-SECONDARIES.md) secondaries · [CC6](docs/CC6-QC-AND-MANAGED-DELIVERY.md) QC and delivery · [CC7](docs/CC7-WORKFLOW-EVALUATION.md) evaluation · [CC8](docs/CC8-FLEXIBLE-COLOUR-AND-YOUTUBE-HDR.md) HDR, in progress) · audio ([AU1](docs/AU1-MANUAL-MIX.md)–[AU6](docs/AU6-WORKFLOW-EVALUATION.md)) · motion ([MO0](docs/MO0-MOTION-PROGRAMME.md) programme · [MO1](docs/MO1-KEYFRAMES-TRANSFORM-STILLS.md) keyframes and transform · [MO2](docs/MO2-BLEND-ADJUSTMENT-TRANSITIONS-SOLIDS.md) blend, adjustment, transitions, solids) · investigator ([IN1](docs/IN1-INCIDENTS-AND-THE-COLOUR-CASE.md) incidents · [IN2](docs/IN2-INVESTIGATOR-SESSIONS.md) investigator sessions · [IN2 Part B](docs/IN2B-INCIDENT-PERSISTENCE.md) persistence) · agent workbench ([AW0](docs/AW0-AGENT-WORKBENCH.md) programme · [AW1](docs/AW1-HEADLESS-KINEWRIGHT.md) headless Kinewright) · performance ([PF1](docs/PF1-PLAYBACK-PERFORMANCE.md) playback, in progress).

M38: [editorial truth, independent output scoring, and caption correction](docs/M38-EDITORIAL-TRUTH.md). M39: [normalized filler bridges and independently scored dialogue pacing](docs/M39-DIALOGUE-PACING.md). M40: [licensed real-footage generalization across interview, event/multicam, and montage tasks](docs/M40-GENERALIZATION-GAUNTLET.md).

## Architecture

Five crates, two trait boundaries, one message bus:

```
kinewright-core    document model, operations, undo, the Core actor  (pure logic)
kinewright-media   FFmpeg decode/encode, wgpu compositor, audio, Whisper
kinewright-agent   MCP server, tool generation, agent CLI drivers
kinewright-project project IO shared by the app and the coming headless mode: save/load, locking, sidecars, recovery
kinewright-app     the native egui desktop app (Windows and Linux)
```

The full design doc — including why every mutation is an operation, why undo is snapshots, and why the audio callback owns the clock — is [Kinewright-Architecture.md](Kinewright-Architecture.md). The visual language is specified in [docs/DESIGN.md](docs/DESIGN.md).

Repeatable startup, memory, preview-seek, and edit-plan measurements are described in [docs/PERFORMANCE.md](docs/PERFORMANCE.md).

## Contributing

Contributions are welcome — see [CONTRIBUTING.md](CONTRIBUTING.md) for the build setup, the architectural ground rules (they're short but firm), and how releases work. By contributing you agree your work is licensed under GPLv3; there is no CLA.

## License

[GPL-3.0-only](LICENSE). Kinewright bundles a GPL build of [FFmpeg](https://ffmpeg.org/) (which is why the GPL is a feature here, not a constraint), the [Inter](https://rsms.me/inter/) and [JetBrains Mono](https://www.jetbrains.com/lp/mono/) typefaces (SIL OFL 1.1), and downloads [Whisper](https://github.com/ggerganov/whisper.cpp) models (MIT) on first use.

---

*Kinewright is developed through agentic orchestration — a Claude orchestrator directing Claude implementation agents, with designs and changes critiqued and reviewed by independent models (currently OpenAI's, through Codex), verified, and CI-gated. The tool is built the way it works.*
