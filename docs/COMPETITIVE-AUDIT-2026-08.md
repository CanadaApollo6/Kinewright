# Competitive Audit — August 2026

> Historical pre-M32 snapshot. The current post-M35 assessment is
> [Product position after M35](PRODUCT-POSITION-M35-2026-08.md); it reflects
> M32-M35 and current official product material, including DaVinci Resolve 21.

Where OpenReel stands against DaVinci Resolve 20, Premiere Pro 2026, and
CapCut Desktop 2026; why the app still feels janky; and a proposed refocus.

---

## 1. The field, as of August 2026

The single most important fact: **every major editor now ships AI editing,
and Adobe ships an agent.** The agentic thesis is no longer contrarian —
it is the industry direction. That is validation, and it is also a clock.

### DaVinci Resolve 20 (Blackmagic)
100+ new features, heavily AI:
- **AI IntelliScript** — builds a timeline from a text script
- **AI IntelliCut** — removes silences, checkerboards dialogue by speaker
- **AI Multicam SmartSwitch** — speaker-detection camera switching
- **AI Audio Assistant** — full-mix balance, noise removal, voice clarity
- **AI Animated Subtitles**, **AI Music Editor** (length-fit),
  **Dialogue Matcher**, **SuperScale 3x/4x**, **Voice Convert**
- Everything else Resolve already was: the best free professional NLE,
  world-class color, Fairlight audio, Fusion motion graphics.

### Premiere Pro 2026 (Adobe)
- **AI Assistant (public beta)** — an in-app agent panel: understands the
  goal, breaks it into steps, picks Premiere tools, executes in the real
  project. Beta focus: organization and assembly (renaming, syncing
  multicam, reorganizing media).
- AI Object Masking (click-to-track people/objects), Generative Extend,
  media-intelligence search across audio/visuals, new Color Mode.
- Still the deepest pro-NLE feature set and ecosystem, still subscription,
  AI increasingly metered by credits.

### CapCut Desktop 2026 (ByteDance)
- **AI Auto-Edit** — upload footage, type a description, receive a
  finished platform-optimized edit (selection, transitions, music, pacing).
- Auto captions (styled, ~15–20s for 10 min), background removal, smart
  reframe to vertical, avatars, effects engine, platform presets.
- Free tier covers most of it; claims one billion users — more active
  creators than Adobe, Apple, and Blackmagic combined.

Sources:
- [CineD — Resolve 20 beta](https://www.cined.com/davinci-resolve-20-released-in-public-beta-with-ai-powered-features/),
  [rAVe — Resolve 20 features](https://ravepubs.com/blackmagic-design-announces-davinci-resolve-20-with-over-100-new-features-and-ai-powered-tools/)
- [Adobe Community — AI Assistant beta](https://community.adobe.com/announcements-727/meet-your-new-assistant-editor-ai-assistant-in-premiere-pro-is-now-in-public-beta-1629317),
  [Adobe blog — 2026 video AI](https://blog.adobe.com/en/publish/2026/01/20/new-ai-powered-video-editing-tools-premiere-major-motion-design-upgrades-after-effects),
  [ProVideo Coalition](https://www.provideocoalition.com/premiere-ai-assistant/)
- [Flowith — CapCut Desktop Pro 2026](https://flowith.io/blog/capcut-desktop-pro-2026-ai-auto-edit-define-short-form-video-2026/),
  [Marc Andrews — CapCut AI review](https://marcandrews.com/capcut-ai-features-review-2026-which-ones-are-worth-it/)

## 2. Where OpenReel actually is (honest inventory)

**Editing core.** Operation-centric validated core, integer-frame math,
atomic batches, snapshot undo, journaled crash recovery (per-project),
multi-project sessions, tracks, cut/trim/move/split/delete, ripple,
linked A/V, markers, snapping, sync-lock, transitions (crossfade, fades),
titles-as-clips, freeze frames, crop, constant-rate speed.

**Audio.** Playback mixing with device-clock master, per-clip gain and
fades, transition ramps, stereo peak meter, 1e-6 export parity.

**Text-based editing (the Descript pillar).** Local Whisper transcription
at import, word-level timeline mapping, click-word ripple cuts, one-click
filler removal, captions (SRT/VTT export + burn-in), silence and scene
detection with transcript-aware clamping (words are never clipped).

**Agents (the differentiator).** Conversation-first layout; local Claude
Code, Codex CLI, and Cursor Agent subscriptions as subprocesses (no
credentials, no OpenReel cloud, no OpenReel per-credit fees); full
mutator/inspector MCP toolset; destructive-op confirmation broker;
provider-specific isolation; concurrent threads over one timeline;
watchable diffs (EditCards with review/undo); live model, effort, speed,
and subscription metadata where the harness exposes them; slash commands;
scored eval harness with committed baselines.

**Capture.** In-editor screen/camera/voice recording (CFR 30), monitor
picker, straight into the import→transcribe→edit loop. Agent deliberately
has no capture tool.

**Delivery.** H.264/AAC export sharing the preview render path, progress
and cancellation, per-project default naming.

**Platform.** Windows installer, bundled FFmpeg, GPLv3, free forever.

## 3. Gap matrix

| Discipline | Resolve 20 | Premiere 2026 | CapCut 2026 | OpenReel today |
|---|---|---|---|---|
| Core cutting | ★★★ | ★★★ | ★★ | ★★ solid but thin tools |
| Trim suite (roll/slip/slide, 3-point, source monitor) | ★★★ | ★★★ | ★ | — |
| Media mgmt (bins, search, proxies UI) | ★★★ | ★★★ | ★★ | ★ pool only |
| Color | ★★★ world-class | ★★★ new Color Mode | ★★ filters/LUTs | — |
| Audio depth (EQ, ducking, keyframes, mix) | ★★★ Fairlight+AI | ★★★ | ★★ | ★ gain/fades/meter |
| Keyframes / motion | ★★★ Fusion | ★★★ + AE | ★★ | — |
| Speed ramps | ★★★ | ★★★ | ★★★ | ★ constant only |
| Masking / tracking | ★★★ | ★★★ AI one-click | ★★ auto cutout | — |
| Multicam | ★★★ AI switch | ★★★ AI sync | — | — |
| Captions | ★★★ animated AI | ★★★ | ★★★ styled, fast | ★★ plain burn-in |
| Text-based editing | ★★ (IntelliCut) | ★★ | ★★ | **★★★ native** |
| Silence/filler removal | ★★★ IntelliCut | ★★ | ★★ | ★★★ transcript-aware |
| Auto-edit from a prompt | ★★ IntelliScript | ★ assistant (assembly) | ★★★ Auto-Edit | ★★ agent rough-cut (eval-proven) |
| **Agent as primary interface** | — | ★ bolted-on panel | — | **★★★ the whole app** |
| Multi-agent / multi-project concurrency | — | — | — | **★★★ unique** |
| Local/private AI (footage never leaves) | ★★ local AI models | ★ cloud+credits | ★ cloud | **★★★ by design** |
| AI pricing | included | credits | freemium | **BYO flat-fee subscription** |
| Auto-reframe / platform presets | ★★ | ★★ | ★★★ | — |
| Background removal | ★★ | ★★ | ★★★ | — |
| Beat-sync / music tools | ★★★ AI Music | ★★ | ★★★ | — |
| Export breadth (HEVC/ProRes/presets/queue) | ★★★ | ★★★ | ★★ | ★ one H.264 path |
| Open source / extensible | — | — | — | **★★★ GPLv3** |
| UI feel | ★★★ dense pro | ★★★ dense pro | ★★★ consumer-slick | ★ functional, janky |

## 4. Positioning: what we win, what we concede

**Concede, permanently:** legacy pro depth as a goal in itself. Nobody
out-Premieres Premiere on breadth or out-Resolves Resolve on color. Chasing
parity feature-by-feature is a losing race against 30-year head starts.

**Win, structurally — nobody else can copy these without becoming us:**
1. **The agent IS the editor.** Premiere's assistant is a panel bolted
   onto a 30-year-old UI, currently doing assembly chores, metered by
   credits. Our whole product is conversation-first: the session is the
   center surface, edits are watchable diffs, threads run concurrently
   across projects. Their beta legitimizes the category; our architecture
   owns it.
2. **Local and private, flat-fee.** Footage never leaves the machine.
   Whisper runs locally; agents are the user's own Claude/Codex
   subscriptions. No upload, no credits, no metering anxiety.
3. **Trust as a feature.** Validated operations, atomic batches,
   confirmation broker on destructive edits, sandboxed harnesses,
   replayable journal. "Let an AI edit my footage" requires exactly this,
   and the incumbents' assistants have none of it visible.
4. **Open source, free forever.** The only editor in this field where
   both the editor and its agent harness are inspectable.

**Fight for (agent-leveraged parity):** the CapCut/Resolve AI vein where
our transcript+scene+agent stack gives us leverage — auto-edit quality,
styled captions, reframe, beat-sync, multicam-by-speaker. Each of these
is dramatically cheaper for us because the agent composes existing tools.

**Table stakes (credibility floor):** enough manual depth that a real
editor doesn't bounce in the first ten minutes: trim suite, bins/source
monitor, basic color, keyframes, speed ramps, export presets + queue.
Not to win reviews — to not lose the demo.

## 5. Why it feels janky (the feel autopsy)

Riel's refined read: *the layout is good* — the roughness is in the
material, not the geometry. "Font, lighting and colors, shapes, shadows
or eye-depth… it feels like a rough draft." That is the precise
difference between a wireframe that shipped and a finished surface, and
it decomposes into buildable properties:

**The material layer (the "rough draft" gap):**
- **Lighting.** Finished dark UIs are *lit*: surfaces carry a whisper of
  vertical gradient, raised elements get a 1px lighter top edge (the
  specular line) and a soft shadow below, inset wells darken at the top.
  Light comes from above. Our flat hex fills have no light source, and
  flat fills are exactly what "rough draft" looks like.
- **Depth.** No shadow/elevation scale: popups, dialogs, and toasts do
  not float, they paste. One consistent three-step shadow scale changes
  perceived quality more than any single feature.
- **Typography.** Hierarchy exists but contrast is timid: weights too
  close, caps labels without letter-spacing, pure-value grays for muted
  text, line-height rhythm uneven between surfaces. Finished UIs get
  their "expensive" look mostly from type tuning.
- **Color temperature.** Pure neutral grays read dead. Polished dark
  themes tint the surface ladder a few degrees (cool or warm) and pull
  text off pure white; the accent then reads intentional instead of
  decorative.
- **Shape discipline.** Corner radii should scale with element size and
  role; uniform radii on everything reads default-settings.

**The behavior layer (the "janky" gap):**

1. **Native window chrome.** The OS titlebar over a dark custom UI is the
   loudest "janky desktop app" signal there is. Zed, Discord, T3 Code all
   draw their own chrome. (`decorations(false)` + drag regions + caption
   buttons; snap-layout care on Windows.)
2. **Nothing moves.** Panels summon/dismiss instantly, popups pop,
   hover states flip binary, nothing eases. Web apps animate presence
   (slide/fade over 120–200ms), hover (~80ms fades), and press states.
   egui has `animate_bool`/`animate_value_with_time`; we barely use them.
   This is the single highest-leverage smoothness fix after the titlebar.
3. **Stock egui widgetry.** Default ComboBox, Checkbox, scrollbar, and
   window frames read as debug tooling. They need house styling: custom
   dropdown surfaces with shadow elevation, chip-styled selects, styled
   focus rings.
4. **No elevation language.** Everything is flat fills; overlays don't
   cast shadows, so popups feel pasted rather than floating. One shadow
   scale (popup/dialog/toast) changes the perceived quality instantly.
5. **Layout instability.** Content pops in without fades (thumbnails,
   transcript words), reserved heights are measured-after-the-fact,
   loading states are voids instead of skeletons/spinners.
6. **Scroll feel.** egui scrolling is stepped and shadowless; web apps
   have inertial, eased scrolling with scroll-edge affordances.
7. **Empty states.** The center-column void reads unfinished rather than
   calm. Empty states need intentional art direction (quiet glyph, one
   line, one action).
8. **Frame pacing.** Repaint-on-demand plus long UI passes can hitch
   during scrub/playback; the feel bar requires a consistently smooth
   paint loop while media plays.

None of this requires leaving egui. It requires treating motion, chrome,
elevation, and loading as first-class design tokens the way M25 treated
color and border.

## 6. Proposed refocus: three pillars

### Pillar A — THE FEEL (M28, next)
Kill the rough-draft look as a systematic pass, bar = "could pass for a
polished web app in a screen recording". Ordered by leverage:
1. **The material pass** (Riel's diagnosis): lighting model for the
   surface ladder (gradients + specular top edges on raised elements),
   shadow/elevation scale (popup/dialog/toast), typography tuning
   (weight contrast, caps tracking, off-white text, line-height rhythm),
   color-temperature pass on the ladder, radius scale by element role.
2. Motion system: presence/hover/press animation tokens (duration +
   easing scale in theme.rs), applied to panels, popups, buttons, rows.
3. Widget re-skin: dropdowns, checkboxes, scrollbars, dialogs on the
   house surface ladder with the new elevation language.
4. Custom titlebar and window chrome (riskiest single item; the loudest
   "desktop app" tell).
5. Loading/empty-state language: fades for async content, skeletons for
   thumbnails, art-directed empty states.
6. Frame-pacing audit during playback/scrub.

### Pillar B — AGENT LETHALITY (the moat)
Make "tell OpenReel what you want" beat CapCut Auto-Edit on quality and
Premiere Assistant on scope, using senses + tools the agent composes:
1. **Music/beat sense** — onset/beat detection as an analysis facet +
   agent tool (unlocks beat-sync cuts, montage pacing).
2. **Styled caption templates** — CapCut-class animated captions as
   title presets the agent can apply per platform.
3. **Auto-reframe + platform presets** — 9:16/1:1 reframe (subject-aware
   later; center+crop first) and TikTok/Shorts/Reels export presets.
4. **Media intelligence** — agent-searchable transcript+scene index
   across the pool ("find every take where I mention pricing").
5. **Speaker sense** — diarization on transcripts (unlocks
   multicam-by-speaker later, checkerboard dialogue now).
6. **Auto-edit eval flagship** — grow the e7-style eval into a full
   "footage in, finished cut out" benchmark we publish. Our quality
   claim should be measured, not vibes.

### Pillar C — CREDIBILITY DEPTH (steady drumbeat)
The old items 7–10, unchanged in content, demoted in urgency: trim suite
+ source monitor/3-point + bins; color basics + LUTs; export presets +
HEVC + render queue; keyframes; speed ramps; audio keyframes/ducking.
Interleave one C item between A/B milestones rather than blocking on them.

### Sequencing after M29
M29 is now the multi-harness control plane: Cursor joins Claude Code and
Codex through ACP, with live model discovery, effort/speed controls, tier
reporting, cancellation, and a real timeline-edit acceptance test.

The next constraint is not provider count. It is the quality of the editing
contract every provider receives:

M30 perception + timeline revisions + beat sense → M31 branches, verification,
styled captions + auto-reframe → M32 editorial depth + media intelligence →
M33 keyframes, compositing, color, and audio graphs → M34 deeper creator
workflows + delivery → re-audit.

The published auto-edit benchmark runs continuously as the scoreboard. Beat
sense, styled captions, auto-reframe, and that benchmark remain the explicit
Pillar B agent-lethality proofs, not deferred polish. The full rationale and
acceptance metrics live in [The Model-First Editor](MODEL-FIRST-EDITOR.md).
