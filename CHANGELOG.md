# Changelog

All notable changes to OpenReel are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[SemVer](https://semver.org/) once releases begin.

## [Unreleased]

The initial development cycle (milestones M0–M7), building the editor end to end:

### Added
- M38 editorial truth benchmark: a new locally generated five-take garden
  story replaces the incoherent color-bar fixture with accepted and rejected
  facts, exact authored captions, and semantically distinct vertical scenes.
  Machine scoring now removes recognized fillers, compares captions exactly,
  retranscribes the rendered MP4 with a 15% word-error-rate ceiling, and keeps
  SHA-bound human acceptance separate. Agents can inspect and atomically
  correct caption cues, while dialogue assembly exposes natural-pause
  retention and filler-boundary padding while coalescing overlapping cleanup
  cuts instead of emitting silent micro-clips. Batched transcript reads and one
  compact editorial-readiness proof reduce repeated model context; silence
  thresholds apply after transcript protection and cut margins. English eval
  transcription is hinted explicitly, and a broken `whisper-rs` safe abort
  callback was removed after it intermittently aborted healthy Windows
  inference. The published Codex machine baseline passes 3/3 samples and 93/93
  assertions with 16-17 tool calls and 236,336-251,345 tokens per sample. Human
  review accepts the deterministic output at 4.08/5 with no dimension below
  3.5, no audible filler, and no material caption error, completing M38's exit
  contract. Sentence-boundary pause refinement remains the next pacing target.
- M37 finished-cut preparation: titles and captions now use one exact Inter-
  measured layout across preview, export, delivery conformance, and QA. Type
  scales from the short edge, wraps inside an 8% safe area, adapts only when it
  cannot fit, and reserves the largest built-in caption motion. Unsafe animated
  captions block delivery and the finished-cut benchmark now asserts vertical
  containment plus timeline/export audio presence. Generated-caption state and
  successful bulk-plan responses are compacted to reduce repeated model
  context. Capability queries and schema opens can be batched, and the new
  transcript/silence-backed dialogue assembly planner removes model-side frame
  arithmetic. The resulting Codex sample passes 32/32 machine assertions with
  234,924 tokens, 67.9% below the prior passing baseline. SHA-bound human review
  then rejected the artifact at a 2.25/5 mean, exposing retained audible
  fillers, inaccurate captions, awkward cuts, unclear story assembly, and a
  reproducible-fixture visual surface that cannot measure real shot selection
- M36 compact agent runtime: OpenReel-owned Claude, Codex, Cursor, and eval
  sessions expose seven stable tools instead of the complete catalog,
  discover exact capability schemas on demand, validate revision-bound edit
  plans before one atomic commit, and reject direct calls to the internal
  85-capability registry. Eval and chat telemetry now distinguish cached input,
  cache-creation input, and reasoning output when providers report them and
  record exact tool-schema bytes; the initial serialized surface falls from
  585,247 bytes to 5,305 bytes (99.1%)
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
- Settings window (T3-style), from the top-bar gear or /settings: a
  Providers page shows each harness with its brand mark, detected CLI
  version, authentication state, executable path (and Codex's sandbox
  notice), plus an enable toggle per provider. Disabled providers vanish
  from the harness picker for new turns without interrupting running
  sessions; if every provider is off, the session panel says so and
  offers Settings instead of the install card
- Ultracode joins the effort picker on xhigh-capable Claude models: it
  enables the CLI's session mode of xhigh reasoning plus standing
  multi-agent workflow orchestration (passed as a session setting, since
  it is not an --effort value). The ultrathink and ultracode prompt
  keywords also pass straight through the composer - typing them in a
  message triggers the CLI's per-turn deeper-reasoning and workflow
  opt-ins, same as in a terminal
- Effort levels are now correct per model: each Claude model carries the
  levels its generation actually supports (the 4.7+/5 models take the
  full low-max ladder, Sonnet 4.6 has max but not xhigh, Haiku 4.5 stops
  at high), matching the CLI's own capability rules. And the picker's
  "Default" model now resolves to what the Codex CLI's config really
  runs, so Default offers that model's true efforts and speed tiers -
  previously the Speed picker could hide entirely because two catalog
  models offer no fast tier
- The hard UI pass (M28): the app's material grew up, measured against
  Zed's actual theme source and Radix dark scales. Real Inter
  Medium/SemiBold weights (the variable font had rendered every "bold"
  as Regular), letter-spaced caps labels, a shadow/elevation scale, a
  re-anchored surface ladder out of the near-black dead zone with
  borders lifted strictly above fills, desaturated neutrals, content
  wells darkest with chrome lighter, accent starved to its four earned
  places, one type step larger everywhere, a one-card composer (input
  and controls on one continuous surface, wrapping on narrow columns),
  a flat rail list with Settings at its foot, slimmed settings cards
  with pill toggles, summoned panels that slide instead of pop, an
  art-directed empty session state, and review builds switched to
  release (the debug build's diagnostic overlays read as flashing red
  boxes). Two-designer collaboration: Claude specified and reviewed;
  Cursor's Grok 4.6 implemented the token rounds and contributed
  reviewable design proposals of its own
- Three columns by default: thread rail, session, monitor. The media
  column no longer self-raises on empty projects or pins itself open on
  import - it is summoned from the top bar only. Media enters through
  the project hub instead, T3-style: drop a file anywhere on the window,
  type /import, or use the focused project's quiet "Import media" row in
  the rail - every path lands footage on the timeline with the monitor
  cued
- Service tier picker (Speed): where a provider offers faster-than-standard
  tiers - Codex's Fast mode (1.5x speed at increased usage) - a Speed
  picker joins the composer row. Tiers come per model from the CLI's own
  catalog, Standard is the default, and the choice is remembered per
  harness
- The Claude model list is now versioned - Fable 5, Opus 5, Opus 4.8,
  Sonnet 5, Sonnet 4.6, Haiku 4.5 - so a model can be pinned exactly
  instead of riding a tier alias
- Recordings are now constant-frame-rate (30fps): webcams deliver
  variable wall-clock timestamps, which made recorded footage resist
  exact frame edits; every capture now lands on a clean frame grid the
  editor and agents can cut anywhere
- Chat sessions genuinely run without a turn ceiling: the drivers
  enforced a hidden 8-turn cap even though the app asked for none.
  Stop is the only limit, as intended
- /remove-fillers now says the transcript is still being generated when
  it is, instead of claiming there are no filler words
- The hard UI pass, rounds 1-2 (border discipline): hierarchy comes from
  surface fills and type, not outlines - widget strokes dropped for a
  stepped surface ladder, the chat stream de-boxed so agent prose reads
  as conversation, ghost buttons in a stroke-free top bar, thin
  scrollbars, one transport slot (Send idle / Stop running), and
  hairlines rationed to true containers like the slash popup
- Projects (M27): work on several videos at once, T3-style. Each open
  project is its own timeline, media pool, undo history, agent server,
  and crash journal; switching is instant and tears nothing down, and
  agents in background projects keep editing their own timelines while
  you watch another - an agent in one project can never touch a
  different project's cut. The rail is two-level: project headers with
  running/confirmation/dirty markers and close, the focused project
  expanded to its threads, background projects collapsed to one quiet
  activity line. Open and New create projects instead of replacing;
  closing and exit guard every project's unsaved changes individually;
  crash recovery offers every crashed project's journal by name and
  restores each into its own session. Recordings save per project under
  Videos\OpenReel\<project>, and export naming follows the focused
  project. The derived-data layer (transcripts, silence, scenes,
  thumbnails, waveforms) now keys by media content, so identical asset
  ids in different projects can never cross-contaminate - and projects
  sharing footage share cached analysis
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
