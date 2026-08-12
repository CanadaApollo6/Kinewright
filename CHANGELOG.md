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
- Text-based editing: select words in the transcript panel and Delete to
  ripple-cut the media underneath — atomic across sync-locked tracks, one undo
  entry, retained words protected by an fps-aware safety margin, linked-A/V
  word copies collapsed to one editable sentence
- One-click filler-word removal (um, uh, erm, hmm, mm, mhm — deliberately
  conservative) cutting every filler in a single undoable batch, with fillers
  rendered muted-and-underlined in the transcript
- Captions: SRT/WebVTT sidecar export with exact integer timestamp math, and
  burn-in as lower-third scrimmed title clips on their own sync-locked track —
  one undo removes the caption track, and the shared render path means
  burned-in captions export exactly as previewed
- Transitions completed: fade-from-black and fade-from-white join crossfade,
  defined by a descriptor table shared by validation, the compositor, the
  inspector's transition picker, and agent tool docs; every transition also
  ramps the clip's audio from silence with feeder/export parity at 1e-6, and
  transitioned clips show their fade window on the timeline
- Per-clip audio control: gain (-60 dB..+12 dB in exact tenths) and audio
  fade in/out as clip properties with one idempotent operation, composed with
  transition ramps by a single shared evaluator in both playback and export
  (parity held at 1e-6), inspector controls that route to the linked audio
  member, and a master stereo peak meter in the transport bar
- Crop effect (per-edge percentages through the shared effect descriptor
  table, composing crop-then-transform) and freeze-frame clips: one held
  source frame as first-class timeline content, inserted at the playhead as
  a single undoable split-gap-insert batch, painted with a repeated
  thumbnail, rendered through the same path as everything else
- Constant-rate clip speed (0.1x–10x in exact integer percentages): speed
  scales the clip's effective source rate through one shared helper, so
  duration, splitting, trimming, and decode positioning stay integer-exact
  at every speed; the inspector slider opens ripple gaps when slowing a
  clip would collide, linked A/V members change speed together, and clips
  show a multiplier badge (audio is muted at non-real-time speeds for now)
- Agent panel cleanup: the chat composer is always visible and full width,
  an empty session explains what the agent can see, and the turn cap is
  gone - subscription sessions run until done or stopped
- Conversation-first layout: the session owns the center of the app and the
  program monitor commands the right - the two-surface default of an
  agentic editor. The timeline and transcript live in a compact tabbed
  strip and the media rail beside them, both summoned on demand rather
  than resident, with contextual self-raising: an empty project leads with
  the media rail, and a pending destructive confirmation raises the
  timeline with the affected spans
- The watchable diff: while the agent edits, every applied change becomes
  a card in the session stream with its changed timecode span, a Review
  action that plays the seams with a two-second lead-in, and one-click
  undo - and the monitor auto-cues to the first seam. Changed-range
  computation understands ripples: a shifted-but-identical tail is not a
  change. The composer anchors to the bottom of the session with the
  harness picker, auth state, and session tokens in the composer row
- Slash commands in the composer: type / for filtered suggestions; Enter
  runs the top match, Enter sends plain messages (Shift+Enter for a
  newline). Ships with instant local commands (/remove-fillers, /captions,
  /freeze, /export, /undo, /redo, /help) and agent prompt commands
  (/cut-silences, /tighten)
- Agent threads (T3-style): a left rail lists every session - brand mark,
  auto-title from the first message, a live latest-activity line, RUNNING
  state - and several threads can run concurrently against the same
  timeline, each streaming into its own conversation while you watch
  another. New thread / close thread / click to switch; per-thread
  composer, model, effort, and token counts; applied edits become review
  cards in every running thread. The composer transport went icon-only
  and the harness version moved into the brand mark's tooltip so the row
  survives the narrower center column
- Screen recording gained a Display picker on multi-monitor machines:
  record one display (default: the primary) or all of them. Display
  bounds come from Windows itself and become an exact gdigrab region,
  negative virtual-desktop offsets included
- In-editor recording: capture the screen (with optional microphone), a
  camera, or a voice-only take from the Record button or /record, driven
  by the bundled FFmpeg CLI as a crash-isolated subprocess with a graceful
  stop. Stopping lands the file in Videos\OpenReel and sends it straight
  down the import pipeline - onto the timeline, monitor cued,
  transcription started - so a take is text-editable the moment it stops.
  The agent deliberately has no capture tool. ffmpeg.exe now ships in the
  installer beside the app
- Brand marks in the harness picker: the Claude spark (brand terracotta)
  and the OpenAI blossom (white, per on-dark usage) identify the session's
  harness in the composer row, the picker dropdown, and the install card -
  nominative marks, T3-style
- The session stream collapses machine activity: consecutive tool calls,
  results, and edit events fold into one compact dropdown whose header
  updates live with the latest step ("Edited 00:00:12:00 - 00:00:31:10 ·
  6 steps", "Ran split_clip") - expand it for the full cards, review
  actions included. Conversation stays first-class; machinery is one line
- Reasoning-effort selector beside the model picker: Claude Code offers
  its session levels (low through max), and Codex levels come per model
  from the CLI's catalog - an effort is only ever offered where the
  chosen model supports it, with Default deferring to the CLI. The
  composer now reserves its measured height, so slash suggestions and
  multi-line input shrink the stream instead of pushing the composer out
- Model selector in the composer row: pick the model each harness runs -
  Claude Code offers its Opus/Sonnet/Haiku tiers, and the Codex list comes
  from the installed CLI's own model catalog, so it always matches what
  the CLI can actually run. Default defers to the CLI's configured model,
  choices are remembered per harness, and changing models restarts the
  session just like switching harnesses
- Import is one gesture: an imported file goes straight onto the timeline
  and the monitor cues to its first frame, the media rail stays open once
  media exists instead of vanishing the moment the import lands, and the
  Add button cues the monitor to the added footage. The slash-command
  suggestions now shrink the session stream instead of pushing the
  composer out of view, and the empty media rail's icon sits centered
  instead of on the panel boundary

[Unreleased]: https://github.com/CanadaApollo6/OpenReel/commits/main
