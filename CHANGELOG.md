# Changelog

All notable changes to Kinewright are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[SemVer](https://semver.org/) once releases begin.

## [Unreleased]

The initial development cycle (milestones M0–M7), building the editor end to end:

### Added
- AD0 audio delivery foundation: `kinewright_core::audio_qc` defines typed
  `AudioDeliveryPreset`s (measure-only, streaming −14 LUFS, podcast −16,
  EBU R 128 −23, ATSC A/85 −24) whose numbers live in one `target()`
  function, an `AudioDeliveryMeasurement` (BS.1770 integrated loudness,
  sample peak, 4× oversampled true peak, EBU Tech 3342 loudness range), and a
  pure `measure_audio_qc` engine publishing seven typed codes with
  `observed`/`allowed` strings and the gain that would reach the target.
  `measure_delivery_audio` in the media crate adds the true-peak
  interpolator and the short-term loudness range. Every verified export now
  decodes its own audio and carries an `AudioVerification` on the
  `DeliveryVerification` (defaulting to `not_measured` for older records);
  the export queue attaches each profile's default preset and the export
  dialog offers a `Loudness target` row, an `AUDIO OUT OF SPEC` status, and
  a `DECODED AUDIO` block. Nothing applies gain. See
  docs/AD0-AUDIO-DELIVERY.md.
- `kinewright-eval --write-color-smoke-media DIRECTORY` writes the six CC7
  synthetic sources and the 65³ inverse `.cube` to disk so a person can run
  docs/COLOR-SMOKE-TEST.md, the hands-on procedure for the CC0–CC7 platform
  gates, against known-answer footage.
- docs/ROADMAP-REVIEW-2026-09.md records the post-CC7 review.

### Changed
- The media crate's synthetic-media generators (`test_support`,
  `cc7_sources`) no longer ship in the desktop binary: `kinewright-agent`
  gates them behind an `eval` feature that only the `kinewright-eval` binary
  and the crates' tests enable, and `kinewright-app` opts out of the agent
  crate's default features.
- `kinewright-agent/src/server.rs` is split into `server/` submodules by tool
  family with the tests beside them; no tool, schema, or behaviour changed.
- The six colour fixture files share one `cc_fixture_support` module instead
  of restating their helpers.
- CI caches the cargo registry and build outputs and the pinned FFmpeg
  archive, runs the media crate's fixture suite in its own Linux job, cancels
  superseded runs, and has a timeout on every job.
- `color_qc.rs` uses `RangeInclusive::contains` so `clippy -D warnings`
  passes on Rust 1.94 as well as the newest stable.

### Fixed
- `FfmpegMediaEngine` left its playback, transcript, visual-asset, and
  derived-analysis workers detached, so dropping the engine while the
  playback worker was still inside a proxy render (which `set_document`
  triggers) let process exit run the FFmpeg and lavapipe finalizers under a
  live `libavfilter` call. That was the CC7 F-E6 process-exit SIGSEGV. The
  engine now signals every worker, cancels queued jobs, joins them against a
  10 s deadline, and does so from `Drop`; the previously ignored
  `cc7_every_color_fixture_builds_a_valid_document` runs in the default lane.
- Every H.264 export lost its last frame on playback: video packets were
  muxed without a duration, so the MP4 muxer computed a track duration one
  frame short and wrote an edit list that hid the final coded picture
  (12 packets, 11 presented). Found by CC6's decoded-output verification,
  which reads the file as a player would and therefore refused to sample
  frame `T − 1`. Frames now carry their duration into the muxer, and
  verification cross-checks the presented frame count so a regression fails
  typed with `delivery_verification_frame_count_mismatch`.
- The managed delivery path quantized the 16-bit RGBA intermediate handed to
  the export filter graph on a `65535 = white` scale, while libswscale treats
  16-bit RGB input on the `255 << 8 = 65280` scale (the same `P_8` CC1 §3.1
  already documents for decode). Nominal white therefore encoded to Y′ 236 at
  8 bits (943 at 10 bits) and mid-grey ran about 0.6 code high on every export.
  The intermediate is now `DELIVERY_INTERMEDIATE_WHITE = 65280`; white encodes
  to Y′ 235 exactly, the CC1 decoded-delivery fixture reference uses the same
  scale, and a regression test pushes a white frame through the real filter
  graph.
- `track_mask_region` and `track_reframe_subject` wrote tracked composite-space
  centres straight into layer-space parameters (`mask.center_x/y_percent`,
  `reframe.focus_x/y_basis_points`), so a clip with a non-identity `transform`
  got mask and reframe automation offset by exactly its scale and offset. Both
  tools now seed, rescale the template, and convert every observation through
  the clip's layer transform resolved per sample frame (CC5 §5.2 map), write
  fraction-of-extent values, convert the reframe subject bounds into the same
  space before containment planning, and report a `coordinate_space` object;
  identity behaviour is unchanged apart from the ≤1-unit lattice correction.
  All three trackers (including `track_matte_window`) now refuse typed with
  `tracking_seed_outside_composite` when the transform pushes the seed centre
  off the composite, instead of clamping it to the raster edge and tracking
  whatever is there.

### Added
- CC7 workflow evaluation: no colour feature and no MCP tool — the slice
  evaluates the CC0–CC6 surface. `kinewright_core::cc7_scenarios` is the single
  authority for six synthetic scenarios (patch geometry, analytic codes, camera
  transforms in linear light, canonical operations, every budget constant),
  `kinewright_media::cc7_sources` authors each raster in Rust and muxes it FFV1
  lossless, and `cc7_fixtures.rs` gates the canonical document of every
  scenario on both CI operating systems: mixed-camera match (neutral spread ≤ 5
  codes, luma delta ≤ 5.0 codes, reference untouched, saturation left as the
  intentional difference), wrong white balance recovered within authority and
  beyond it (clamped control published, range excursion attributed to the
  primary, `technical_pass` kept), a log-like carrier undone by an imported
  65³ inverse `.cube` within 12 codes, qualifier-only product containment
  (192/192/0/0) and a window-only feather edge (252/140/112), the `warm` look's
  exact 192-pixel out-of-gamut patch, a tracked secondary whose one occluded
  sample is dropped at a floor of 8 500 bp and whose window contains the
  subject at every surviving sample, and decoded delivery at both depths under
  CC6's unchanged budgets. Six scripted `cc7_` MCP tests complete each workflow
  over the live endpoint and commit the canonical document; app tests prove the
  person path through the inspector builders. The eval runner gains a typed
  colour evidence block computed inside the run, `original_document` on the
  outcome, `delivery_bit_depth` on the deliverable spec, a saved-project handle
  for fixtures, the `color-workflow-v6` suite (`c1`–`c6`), `human-review.json`
  schema 2 with per-task questions, and a review package blinded to machine
  provenance (`blind/` artefacts plus a form keyed by a derived id, with the key
  in the run root and `--score-review` unblinding through it). Probing found
  three tool boundaries that the contract now records: `track_matte_window`
  never re-acquires after an occlusion (a range must end at it),
  `analyze_color_shot` percentiles are 16-bit codes, and the tracker's samples
  are evenly distributed rather than stepped. See docs/CC7-WORKFLOW-EVALUATION.md.
- CC6 QC and managed delivery: a named high-precision stage
  (`working_linear_post_composite`, `Analysis::working_proof_for_document`)
  reads the production composite back as linear f32 before any encode; the
  core `color_qc` engine reports range (delivery-encoded clamp events per
  channel, basis points and extremes), gamut (negative linear channels and
  the desaturation fraction), a forward BT.709 limited-range Y′CbCr reference
  at 8 and 10 bits with legality counts, region-scoped skin diagnostics
  (circular mean/spread, chroma, in-band rate against a band derived from the
  CC5 skin patches — a diagnostic of a chosen region, not a detector), a
  two-mode typed delivery tag check, and per-node clipping attribution by
  effect removal on a scratch document, all integer-reported and
  evidence-only. Delivery gains a 10-bit H.264 lane (`DeliveryEncodeDepth`,
  `yuv420p10le`), typed `DeliveryColorError` rejections with
  `code/field/observed/allowed`, serializable `ExportSettings`, and
  `Analysis::verify_delivery_output`, which decodes the written file at
  sampled frames in one seek-based pass, compares the native luma plane and
  RGB against the full-resolution delivery reference under named per-lane
  budgets (RGB max is reported, never gated: 4:2:0 chroma decimation at
  saturated edges dominates it in both lanes), measures decoded Y′CbCr
  legality against the EBU R 103 box, and probes tags; verification never
  moves a finished file. The agent gains `get_color_qc`, `queue_export`
  `verify`/`delivery_bit_depth`, verification on `get_export_jobs`, and a
  typed pointer in place of `get_video_scopes_v2`'s fabricated gamut zero.
  The app gains a Colour QC window, a QC clipping mask view, absolute
  per-channel clipping in the scopes panel, a per-node clipping line in the
  inspector, and an 8/10-bit choice plus a post-export verification block in
  the export dialog. The exit gate is a synthetic 60-frame source exported at
  both depths, re-probed, decoded, and gated on tag, range, and
  visual-difference budgets in the default lane on both CI operating systems.
- CC5 secondaries: node-owned mattes on `primary_correction`, `color_wheels`,
  `color_curves`, and `creative_look` (never on `technical_lut`), expressed as
  47 generated integer parameters per node — up to four rectangular or
  elliptical windows with aspect-corrected rotation, symmetric feather, and
  per-window invert combined by union or intersection, plus an HSL qualifier
  (hue centre/width/softness with a wraparound-safe achromatic rule,
  saturation and luma bands with softness) evaluated on the node input in
  `grade709`. A node applies `out = x + (node(x) − x)·m`, with `m == 0` an
  exact per-pixel identity so nothing outside a matte changes by a single
  bit, and alpha is never modified; the layer `mask` effect is unchanged.
  Tokens and counts are Hold-only keyframes; windows, mix, and qualifier
  scalars animate freely; a `matte_band_inverted_by_automation` QA warning
  covers crossed bands. The GPU node stack gains a 64-word matte block per
  node in the payload region (`GRADE_ABI_VERSION` 3, 32 KiB binding) and a
  matte-debug selector that renders coverage without a transfer; the CPU
  reference evaluates the same contract at pixel centres with an aspect
  argument. New `Analysis::matte_proof_for_document` and
  `matte_coverage_statistics`; matte-scoped scopes feed the unchanged CC2
  engine through `matte_scoped_frame` (`A = 255` where `m > 0`) with a
  `matte_region` recorded on the evidence; comparisons require the same
  clip/effect/threshold and report the covered-pixel delta. The
  agent gains `inspect_grade_matte`, `track_matte_window` (the existing SAD
  tracker with the M40 median filter and step limit, coordinate conversion
  through the layer transform, a prepared plan that never commits, and a
  stated tracking boundary), `plan_secondary_correction`, and matte variants
  of `render_color_proof`; the inspector gains a matte section, keyframe
  badges, a preview overlay with drag editing of windows (converted through
  the clip's layer transform), and a matte view.
  Exit evidence: affected-pixel containment (exactly zero outside pixels
  changed on CPU and GPU), hand-derived window/feather/qualifier anchors, a
  generated moving-box clip for the tracked-shot proof, and the CC1 gates.
  See `docs/CC5-SECONDARIES.md`.
- CC4 look management: LUT looks become project-owned, content-hashed assets.
  `Document.lut_assets` records id/sha256/title/size/domain/provenance; bytes
  live in a project-relative `<stem>.kinewright-assets/luts/<sha256>.cube`
  store derived from the project path and never stored, so copying the project
  file plus that directory reproduces every look bit-identically. Availability
  (`verified`/`missing`/`changed`/`unreadable`) is runtime state injected into
  Core (`export_lut_preflight_with`), with hash-checked restore and explicit
  replace as the recovery paths, symlinked store roots refused, and a 16 MiB
  import cap. Two new managed colour nodes, `technical_lut` (input stage) and
  `creative_look` (look stage), carry an integer asset reference, mix, input
  encoding (`display709`/`linear`/`grade709`, with a new exact
  `decode_display709`), and bypass; Core rejects any vector order that
  contradicts technical → correction → creative. Evaluation is normative
  tetrahedral interpolation with a fixed branch structure, an additive
  out-of-domain rule that keeps over-range values recoverable, and a
  linear-light mix; the CPU reference and the GPU atlas shader implement it
  independently. The four legacy built-in looks are deterministic, hash-pinned
  generated assets baked over [-1, 2]; legacy `look_lut`/`cube_lut` stay
  compatibility stages with an explicit `ConvertLegacyLook`. The compositor
  binds one `Rgba32Float` 3D LUT atlas (up to four managed slots plus the
  legacy slot) with `textureLoad` only and raises the required 3D texture
  dimension to 512. The app gains `Look → Import LUT…`, `.cube` drop, a look
  browser, a mix slider with coalesced undo, press-and-hold A/B, stage
  headings with correct insertion, missing/changed banners with Locate/Replace,
  Convert to managed look, and a dialog-free `write_project` that copies the
  store on Save As; the export dialog and queue run the LUT preflight. The
  agent gains `list_look_assets`, confirmation-gated `import_lut_asset`
  (the only path that can create a LUT record; `AddLutAsset` is blocked from
  plans and generated tools), `plan_technical_lut`/`plan_creative_look` with a
  computed stage-legal `insert_index`, look-aware `render_color_proof`, and
  LUT fields on every `color_nodes` manifest. Exit evidence lives in
  `cc4_fixtures.rs`, `cc4_core.rs`, the app relocatability fixture, and
  `docs/CC4-LOOK-MANAGEMENT.md`.
- Pixel-exact compositor sampling: a layer whose source raster matches the
  output raster with no scale, offset, or reframe is now point-sampled instead
  of bilinear-filtered. The first Mesa lavapipe run of the CC3 parity suite
  showed lavapipe returning bilinear weights one f32 ULP of the texel coordinate
  away from zero at 1:1 (legal under Vulkan's sub-texel precision rules), which
  the non-Lipschitz `sgn(y)·|y|^0.1` wheels power turned into 105-code monitor
  errors; the NVIDIA adapter was exact. Output on exact adapters is unchanged.
  Fixture lanes now print their adapter, and the hardware opt-in is reported as
  ignored when a software adapter exists. The CC1 §6.2 and CC3 §10.3.9
  contracts record the obligation and the measurement.
- CC3 curves and wheels: two new ordered managed colour nodes. `color_wheels`
  applies ASC CDL-style slope/offset/power per channel with master controls;
  `color_curves` applies master/red/green/blue monotone cubic Hermite curves
  (Fritsch-Carlson tangents, linear extrapolation) with 2..=16 integer points.
  Both evaluate inside an exact, invertible `grade709` grading encoding, never
  clamp, execute in `clip.effects` order together with `primary_correction`, are
  skipped bit-identically when neutral or bypassed, and are serialized as plain
  integer parameters with typed validation (`InvalidCurvePoints`,
  `NonHoldKeyframeParameter`, `CurvePointCountAnimatedWithPoints`,
  `TooManyColorNodes`), a `curve_truncated_by_automation` QA warning, and a
  16-node-per-layer limit. The GPU compositor gained a tagged node-stack storage
  buffer (16 KiB binding) with host-solved curve tangents; the CPU reference
  implements the same contract independently. The inspector adds trackball
  wheels and a curve editor with coalesced undo, bypass, per-node reset, and
  keyframe badges; the agent gains evidence-only `plan_color_wheels` and
  `plan_color_curves` planners and an ordered `color_nodes` manifest, with the
  133-parameter curves descriptor summarized compactly in tool documentation.
  Exit evidence lives in `cc3_fixtures.rs` and `docs/CC3-CURVES-AND-WHEELS.md`.
- CC1/CC2 review hardening (2026-08-24): a six-lane audit of the managed SDR
  primary and scopes work found and fixed a sign inversion in the agent
  `plan_shot_match` tint proposal, an unreported display-range clamp when a
  `chroma_key` effect was present (keying is now an alpha-only stage that never
  clamps working RGB), export applying the monitoring encode plus an 8-bit
  quantization before the delivery range conversion (export now hands a 16-bit
  RGBA64 delivery frame to the encoder and quantizes once), a monitoring encode
  that was hardcoded instead of selected from the monitoring description, a
  human Export dialog that never ran `delivery_conformance`, one undo entry and
  revision bump per UI frame while dragging a primary slider (the Core actor
  gained `DoBatchCoalesced`), a scope panel that could strand on "Rendering"
  after a superseded worker, endpoint-anchored waveform/parade row and
  vectorscope bucketing that starved the black row, histogram bin counts above
  256 that could never place code 255 in the final bin, `ColorBitDepth::Integer(8)`
  not comparing equal to `Eight`, `ColorPipelineState` defaulting to
  `managed_sdr_v1` instead of `legacy`, CC0 migration stamping managed state over
  custom monitor/delivery targets, a stale `legacy_display_effect` code on the
  agent status surface, CC2 agent payloads of ~285 KB for a 2×1 frame (analysis and
  matching responses are now ~7-15 KB by default with `include_grids` opt-in;
  `get_video_scopes_v2` keeps grids on by default at ~146 KB, half the old size), uncapped `plan_shot_match` candidate
  renders, silent proposal clamping, proxy evidence reported as full resolution
  inside matching responses, and an export queue that did not re-verify source
  identity after encoding. The CC1 fixture suite was rewritten so every control
  has an analytic expected value, the parity raster exercises highlights/whites,
  manifest tolerances are asserted against code constants, and GPU fixtures can
  run on a hardware adapter (`KINEWRIGHT_CC1_ALLOW_HARDWARE_GPU=1`) with honest
  provenance when no software fallback is installed. `docs/CC3-CURVES-AND-WHEELS.md`
  records the next colour slice's implementation contract.
- Native Linux x86_64 desktop builds: pinned FFmpeg 8.0 shared GPL provisioning,
  Vulkan/ALSA/GTK dependencies, x11grab/v4l2/Pulse recording, a staged tarball
  with bundled libav libraries, and Linux CI/release jobs alongside Windows.
- M40 event/multicam recovery: tracked vertical reframing now stores precise
  basis-point subject evidence, samples the full clip uniformly, preserves
  animated curves through delivery, and solves an offline lookahead camera path
  that contains every tracked box while retaining the 2% per-sample motion
  limit. Encoded loudness is measured independently. The official AMI recovery
  passed 25/25 machine assertions at -16.98 LUFS / -1.72 dBFS, and the project
  owner accepted the exact SHA-bound artifact with "Nailed it." The broader M40
  three-family and repeated-sample gate remains in progress.
- M39 dialogue pacing: dialogue assembly can now cap the total clean acoustic
  pause retained across a removed filler run without shortening an already
  natural bridge or preserving filler audio,
  while a compact read-only pacing inspector reports sentence boundaries and
  short/target/long gaps. Human review exposed that Whisper word endpoints had
  made the original 12-frame metric disagree with rendered pauses of roughly
  47 and 9 frames. Planning and evaluation now use mapped acoustic silence at
  a speech-oriented -35 dBFS threshold, with transcript bounds as an explicit
  fallback, and the calibrated 10-to-40-frame contract preserves later natural
  pauses. The historical Codex baseline passed 3/3 samples and 99/99 assertions
  under the superseded transcript-only scorer. Exact capability names
  in an edit request now bypass redundant discovery, reducing the published M39
  mean from 282,716 to 249,112 tokens without changing that artifact.
  Qualitative review called pacing much better but identified the two opening
  defects; a fresh machine run and formal SHA-bound human review remain
  required for M39's 4.5/5 pacing exit target.
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
- M36 compact agent runtime: Kinewright-owned Claude, Codex, Cursor, and eval
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
  snapping, project save/load (`.kinewright`)
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
- `Kinewright.exe <project>` startup argument
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
- Agent eval harness (`kinewright-eval`): seven scored editing tasks including a
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
- Kinewright icon set by GPT-5.6 Luna (app, taskbar, installer, README)
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
  Videos\Kinewright\<project>, and export naming follows the focused
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
  stop. Stopping lands the file in Videos\Kinewright and sends it straight
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

[Unreleased]: https://github.com/CanadaApollo6/Kinewright/commits/main
