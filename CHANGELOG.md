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

[Unreleased]: https://github.com/CanadaApollo6/OpenReel/commits/main
