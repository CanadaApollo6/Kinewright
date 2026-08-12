# The Model-First Editor

OpenReel should be a local, transactional video runtime for agents, with a human
director and reviewer. It should not become a traditional NLE with a chat panel
bolted onto it.

The primary editing loop is:

1. **Perceive** the footage and the current cut.
2. **Plan** a bounded change against an exact timeline revision.
3. **Act** through typed, composable editing operations.
4. **Verify** the rendered result with the same senses used to plan it.
5. **Explain** what changed and leave the human a watchable diff, variants, and
   one-click recovery.

The human still owns the goal, taste, source media, approvals, and final call.
The model owns the mechanical editing work.

## What OpenReel already gets right

The current foundation is unusually well aligned with that goal:

- every edit is a validated Rust `Operation`, whether it came from a person or
  an agent;
- integer-frame timing, atomic edit plans, snapshot undo, and the recovery
  journal make changes deterministic and reversible;
- MCP exposes the real editing surface instead of asking a model to click UI or
  invent FFmpeg commands;
- models can inspect timeline state, clips, actual rendered frames, transcripts,
  silence, and scene boundaries;
- destructive changes flow through the confirmation broker;
- model-neutral evals measure whether the edit is correct;
- Claude Code, Codex, and Cursor are harness choices over the same editor.

This is already more than "AI inside an editor." It is the beginning of an
editor runtime designed around machine use.

## Lessons worth taking from the field

- [T3 Code](https://t3.codes/) treats the application as a control plane over
  interchangeable coding agents and gives work its own thread and branch.
  OpenReel should do the same for model providers and timeline variants.
- [Premiere's AI Assistant](https://helpx.adobe.com/premiere/desktop/premiere-ai-assistant/overview.html)
  works through project-aware tools to organize media and assemble an initial
  edit. The important idea is structured project context, not a branded chatbot.
- [DaVinci Resolve](https://www.blackmagicdesign.com/products/davinciresolve)
  separates editing, compositing, color, audio, media, and delivery into deep
  domains. Those domains are a useful capability map even when OpenReel does
  not copy their panels.
- [Final Cut Pro's multicam workflow](https://support.apple.com/guide/final-cut-pro/intro-to-multicam-editing-ver23c76439/mac)
  and Magnetic Timeline show how sync and story relationships can be first-class
  data instead of incidental clip coordinates.
- [Nuke's node toolset](https://www.foundry.com/products/nuke-family/nuke/features)
  and [After Effects' expressions and tracking](https://helpx.adobe.com/after-effects/desktop.html)
  show the compositing and automation primitives a serious motion system needs.
- [Descript](https://help.descript.com/hc/en-us/articles/37585546799757-The-editor-interface)
  proves that transcript, scenes, and timeline can be different views of one
  edit; its [Underlord](https://help.descript.com/hc/en-us/articles/36803785502221-Underlord-beta-Your-AI-co-editor-in-Descript)
  makes common finishing actions available to an agent.
- [CapCut's transcript editing](https://www.capcut.com/tools/video-transcript-editing)
  and creator-oriented automation set the convenience bar for captions,
  reframing, pacing, and delivery variants.

The synthesis is not "copy every feature." It is to encode professional depth
and creator convenience as a coherent machine-facing contract.

## The largest remaining gaps

### 1. Perception is precise but not yet scalable

`get_frame_at` gives an agent real eyes, but only after it knows which frame to
ask for. A model needs cheap ways to understand hours of footage and inspect a
whole result:

- contact sheets and storyboards over an asset, scene, or timeline range;
- waveform, loudness, clipping, speech, speaker, music, and beat summaries;
- searchable media facets that combine visual scenes, transcript, speakers,
  faces or subjects, camera quality, and technical quality;
- low-resolution review renders with exact frame mappings back to the timeline;
- comparison inspectors for before/after and alternate cuts.

The output should stay compact and structured. Images are evidence, not a
replacement for stable IDs and frame coordinates.

### 2. Concurrent agents currently share one live timeline

That makes parallel threads vulnerable to stale plans and surprising collateral
changes. Every project needs a monotonically increasing timeline revision.
Inspection results should report it, and mutations should carry an expected
revision or explicit preconditions.

Each agent thread should eventually edit a cheap branch or snapshot. The human
can then compare A/B cuts, merge a complete plan, or cherry-pick one useful
change. This is the video equivalent of the branch-per-thread model that makes
agentic coding safe.

### 3. Verification needs to become a product surface

"The operation succeeded" is not the same as "the video is good." OpenReel
should automatically check the result for:

- unintended gaps, black frames, freezes, and offline media;
- A/V sync drift, clipped audio, bad loudness, abrupt speech cuts, and music
  collisions;
- captions outside safe areas, unreadable durations, overlaps, and truncation;
- aspect ratio, frame rate, codec, duration, and delivery-profile conformance;
- requested content that disappeared or forbidden content that remains.

An agent turn should end with claims tied to inspector evidence. The human sees
the affected range, a preview, the checks performed, and the exact undo or
branch action.

### 4. The operation vocabulary needs professional depth

OpenReel does not need every panel in Premiere or Resolve. It does need the
underlying primitives agents would use from those panels:

- source ranges, three-point editing, roll, slip, slide, replace, and fit-to-fill;
- bins, selects, string-outs, sync groups, multicam angles, and takes;
- keyframes and interpolation curves for every animatable parameter;
- a compositing graph with transforms, masks, tracking, keys, and effect nodes;
- a color pipeline with scopes, exposure and balance controls, curves, and LUTs;
- an audio mix graph with buses, meters, EQ, dynamics, automation, and ducking;
- styled captions, motion templates, beat-aware timing, reframing, and platform
  delivery variants;
- background jobs with progress, cancellation, resource budgets, and resumable
  results.

The agent API should expose these as typed capabilities, never UI coordinates or
shell recipes.

## Target architecture

Keep the Rust core as the source of truth and grow it into six related graphs:

1. **Media graph** - assets, proxies, transcripts, scenes, speakers, beats,
   technical metadata, and searchable facets.
2. **Story graph** - timelines, tracks, clips, storylines, sync groups, multicam,
   markers, versions, and branches.
3. **Compositing graph** - layers, effects, masks, trackers, keys, and color.
4. **Automation graph** - keyframes, curves, expressions, and time remapping.
5. **Mix graph** - routing, buses, gain, EQ, dynamics, loudness, and automation.
6. **Delivery graph** - render jobs, profiles, aspect variants, captions, and
   conformance reports.

Every graph should be addressable through stable IDs, queryable without dumping
the whole project, versioned, serializable, and mutated through atomic plans.
FFmpeg, GPU kernels, Whisper, and future analyzers remain replaceable execution
backends behind those semantics.

## Roadmap after M29

M29 establishes the multi-harness control plane: Claude Code, Codex, and Cursor
can drive the same local editor through the same tool contract. More providers
are now lower value than making every provider a better editor.

Pillar B, **agent lethality**, remains a parallel release track rather than a
late polish bucket. Its four public proofs are:

1. **Beat sense** - the model can understand musical structure and cut or retime
   a sequence intentionally against it.
2. **Styled captions** - the model can produce platform-ready captions with a
   declarative, preview/export-identical style system.
3. **Auto-reframe** - the model can make reviewable 16:9, 9:16, and 1:1 variants
   while preserving the subject and caption-safe composition.
4. **A published auto-edit benchmark** - generated, redistributable fixtures;
   versioned tasks and scorers; per-model pass rates, cost, latency, and human
   acceptance; and reproducible traces when a run fails.

The underlying safety and perception work below makes those features dependable.
The Pillar B features make that infrastructure visible and valuable to users.

### M30 - Perception and revision safety

- storyboard/contact-sheet inspectors;
- beat and musical-structure analysis as a cached media facet and agent tool;
- timeline revision IDs and optimistic preconditions on edit plans;
- structured analysis-job status, progress, cancellation, and errors;
- low-resolution proof renders for an affected range.
- publish the first reproducible auto-edit benchmark baseline.

### M31 - Branches, variants, and verification

- one timeline branch per agent thread;
- compare, merge, discard, and cherry-pick plan results;
- automatic technical and editorial QA;
- provenance linking prompt, evidence, plan, rendered result, and approval.
- declarative styled-caption presets with preview/export parity;
- auto-reframe and 16:9, 9:16, and 1:1 delivery variants.

### M32 - Editorial credibility

- source monitor semantics and three-point editing;
- roll, slip, slide, replace, and fit-to-fill;
- bins, string-outs, sync groups, and multicam foundations;
- media search across transcript, scenes, speakers, and quality facets.

### M33 - Parametric depth

- keyframes and curves;
- compositing nodes, masks, tracking, and basic keying;
- color controls, scopes, and LUTs;
- audio buses, EQ, dynamics, automation, and ducking.

### M34 - Creator leverage and delivery

- animated caption behaviors built on the automation graph;
- music fitting and beat-aware pacing built on the M30 analysis facet;
- speaker-aware multicam and subject-aware reframe;
- platform variants, export queue, and delivery conformance.

The published "footage in, finished cut out" benchmark should grow continuously
through every milestone instead of waiting for a final automation pass. Its
history is the scoreboard for both foundational work and Pillar B.

## Scoreboard

The metrics should reflect whether agents are becoming better editors:

- time to first watchable cut;
- percentage of first passes accepted by a human;
- corrections and undos per accepted minute;
- unintended collateral changes per plan;
- percentage of material claims backed by verification;
- deterministic replay rate;
- output conformance and QA escape rate;
- model cost and wall-clock time per accepted minute.

Provider-specific benchmarks matter, but the product metric is how well the
same OpenReel contract lifts every capable model.

## Product boundaries

- No generated video in the core workflow.
- No cloud account or hosted project requirement.
- No feature parity for its own sake.
- No privileged filesystem, shell, or web access as a substitute for editing
  capabilities.
- No invisible autonomy: important changes remain inspectable, attributable,
  reversible, and reviewable.
- No requirement that the human learn an expert NLE before the agent can help.

The manual timeline remains important, but its primary role becomes direction,
review, correction, and escape-hatch precision. The model-facing contract is the
editor.
