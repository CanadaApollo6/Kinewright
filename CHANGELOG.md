# Changelog

All notable changes to OpenReel are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[SemVer](https://semver.org/) once releases begin.

## [Unreleased]

The initial development cycle (milestones M0–M7), building the editor end to end:

### Added
- Operation-based core: every mutation is a validated, pure `Operation` through
  a single Core actor; snapshot undo/redo; append-only operation log
- A/V playback with the audio device callback as master clock; exact
  keyframe-then-step seeking; frame cache with prefetch
- Timeline editing: tracks, cut/trim/move/split/delete, drag interactions,
  snapping, project save/load (`.openreel`)
- Multi-track wgpu compositing with effects (brightness, contrast, saturation,
  opacity, transform) and crossfade transitions; H.264/AAC export sharing the
  preview's render path, with progress and cancellation
- Agent harness: in-process MCP server; mutator tools generated from the
  operation set; timeline/clip/frame inspectors; Claude Code driver
  (stream-json) and locked-down Codex driver (read-only sandbox, direct tool
  mode); chat panel with streaming, cost tracking, spending cap, turn cap, and
  destructive-op confirmations
- Edit by transcript: local Whisper transcription at import (pinned model,
  one-time download), word-level timestamps, transcript agent tools, transcript
  panel with click-to-seek
- Crash recovery from a journaled operation log with restore/discard flow
- Keyboard-first editing (J/K/L, I/O, frame stepping, help overlay)
- "Cut Room" design system: dark token-based theme, embedded Inter/JetBrains
  Mono, SVG iconography, filmstrip thumbnails and audio waveforms on clips,
  adaptive timecode ruler, animated zoom
- Windows installer with bundled GPL FFmpeg; tag-driven release pipeline
- `OpenReel.exe <project>` startup argument
- Real-world media hardening: VFR sources mapped onto the CFR timeline with
  bounded drift, rotation metadata applied everywhere consistently, odd audio
  rates, 4K long-GOP and HEVC coverage, actionable errors instead of silent
  failures (see `docs/MEDIA-POLICY.md`)
- Proxy-resolution preview decode with measured budgets: 4K long-GOP scrubs at
  p95 ≈ 120 ms and plays sequentially well above 60 fps on software decode
- Agent senses and planning: silence detection (windowed RMS), scene-boundary
  detection, and `apply_edit_plan` — atomic multi-operation plans with a single
  undo entry, one summarized confirmation for destructive plans, and
  self-reporting results (remaining cuttable silence count)
- Agent eval harness (`openreel-eval`): seven scored editing tasks including a
  five-take rough-cut flagship, per-eval budgets, pass-rate sampling
  (`--only`, `--samples`), committed baselines in `docs/EVALS.md`
- Transcript-aware silence cutting: spans reported for cutting never overlap
  transcribed words and split around embedded words
- Editing ergonomics: ripple delete/insert with cross-track sync-lock
  (default-locked tracks, per-track opt-out), A/V clip linking with atomic
  linked edits, project markers with ripple-following semantics, marker and
  cross-track snapping with Alt-bypass
- Titles as first-class timeline clips (declarative style tokens, position
  presets, scrim, frame-based fades) rendered through the same wgpu path as
  export, with WYSIWYG parity tested; context-sensitive inspector panel driven
  by the effect descriptor table
- Multi-track audio playback mixing matching the export mixdown to sample-level
  parity — what you hear is what you export
- Codebase audit and remediation: −7.9% source lines, shared derived-data
  framework, `MediaEngine` split into Playback/Analysis/Export facets, fmt and
  clippy (`-D warnings`) enforced in CI (see `docs/AUDIT-2026-08.md`)
- OpenReel icon set by GPT-5.6 Luna (app, taskbar, installer, README)

[Unreleased]: https://github.com/CanadaApollo6/OpenReel/commits/main
