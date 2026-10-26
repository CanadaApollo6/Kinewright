# Product brief, October 2026

Status: agreed with Riel on 2026-10-20 through a structured interview. This brief
sets who Kinewright is for, what "done" means, and the order of work. Where it
conflicts with older planning text, this brief wins. `ROADMAP-AND-WORKFLOWS.md`
keeps the programme detail.

## What Kinewright is

An agentic video editor for people who don't edit. The person brings the script,
the voice, and the face. The agent does the editing, design, and polish, to a
standard that can stand next to the channels the person admires.

The first user is Riel, who has never edited a video. Kinewright may always have
just that one user, and the plan has to work if it does. Non-editor creators come
later. Professional editors are a long-term bar, not the current priority: anything
the agent makes is an ordinary editable edit (cuts, keyframes, layers, effects), so
a pro can open the timeline and get Premiere-class control over it.

Priorities when time is short:

1. Riel's own videos.
2. The engine thesis: a platform that makes AI agents genuinely capable at video,
   for any model.
3. An open-source product other people use.

## Why it exists

Riel edited a wedding video with a strong model in Codex over two days. The model
was good at the job, but the harness held it back:

- it could not watch the footage or hear the audio properly;
- every change needed a slow re-render before anyone could see it;
- there was no way to point at a moment and say "fix this";
- the edits were ffmpeg commands, so a revision meant redoing work;
- the model kept rebuilding ways to handle clips and audio from scratch, and only
  started to "get it" near the end.

Kinewright removes each of these. The agent has to be fluent from the first
minute: ready-made editing tools, instant previews, a way to be pointed at a
moment, and edits that stay revisable.

## What "done" means

A job is done when Riel records a video they intend to post, makes it in
Kinewright, and is satisfied with how quickly and how well it went. There are no
dedicated test sessions. The bar has three levels:

- **Foundation:** technically clean audio, levels, cuts, and delivery.
- **Pass/fail:** Riel would publish the result without opening the timeline.
- **Target:** it stands next to the reference channels, apart from what only a
  human creative can supply (writing, personality, custom 3D animation).

Budgets:

- **Riel's own time:** 30 to 60 minutes per video, from dropping in the files to
  exporting.
- **Agent time:** may run for hours with subagents, provided it needs no input.
- **Facts:** nothing invented. Every number on screen traces to a data export or a
  source Riel supplied. The agent flags any script claim it cannot support before
  rendering.

Before Riel uses a job, the lead runs a full dry run of it: a serious video made
from a practice take, not a spec demo. An independent model reviews the result
against stills from the reference channels. Riel watches the finished dry run once.
During real videos, the lead reads the session afterwards and turns friction into
fixes, so Riel never has to file a report.

## How it works

- **One window, the Studio layout:** chat, a live player, versions of the edit,
  and comments. The full editor (timeline, mixer, colour) is one click away.
- **Pointing:** pause and type; draw on the frame; leave timestamped comments in a
  batch; send voice notes. Each piece of feedback is bound to the version it was
  made on.
- **Agents:**
  - Opus 5.5 leads through Riel's Claude Code subscription.
  - Specialist subagents use the best model for each role, through any
    subscription Kinewright can drive (built-in connectors or ACP). The roles are
    editor, motion designer, audio engineer, colourist, and an independent reviewer
    that checks renders before Riel sees them.
  - Specialists work on project branches; the lead merges their work.
  - The model for each role is configurable.
- **Run style:**
  - The agent asks only what it cannot work out, shows a short plan (always for
    longer videos), then delivers a complete cut.
  - It almost never interrupts. It records its assumptions instead, and batches
    anything that blocks it.
  - Long runs continue headless with the app closed, and a desktop notification
    says when the cut is ready.
- **Scripts drive the edit:**
  - Riel writes a script or a strong outline, optionally with cues in brackets,
    such as `[chart: draft value by position]`.
  - Riel records takes freely, retakes included.
  - The agent matches takes to the script, picks the best take of each line, and
    builds graphics from the cues.
- **Delivery:** a publish kit containing the video files, 2–3 vertical reels
  (optional), title options, a description, chapters, and thumbnail candidates.
  Riel uploads it; Kinewright never uploads.
- **Platforms:** Riel's Linux (Hyprland/Wayland) desktop first, then Windows, then
  macOS only if people actually use it.

## Creative boundary

- **The agent proposes:** structure, outlines and storyboards from Riel's
  material; code-generated graphics and animation; and slides built from an
  outline or deck.
- **Riel supplies:** the words and the delivery.
- **Voice:** only Riel's own cloned voice, for pickups, patches and draft
  narration. It is always labelled as synthetic, never uses anyone else's voice,
  and is built after the essay job works.
- **Out of scope:**
  - AI-generated video;
  - AI images (deferred, 2026-09-15 decision);
  - generative clutter removal;
  - 3D character animation in the Fern/Hoog style.

## Jobs, in build order

**A. Talking head.** Most of Riel's videos are some form of talking head
(2026-10-26), at the desk or on location.

The epitome references are:

- Todd Kerpelman's "Get to know Cloud Firestore";
- Zohran Mamdani's campaign videos (walk-and-talk across many locations, quick cuts,
  bold on-screen text);
- Good Work's videos.

Other references: Theo Browne, Mo Bitar.

Standard elements:

- a clean, lit look, with background blur or a designed replacement backdrop at the desk;
- tight jump cuts and punch-in zooms;
- explainer-graphic cutaways;
- on-screen cards for tweets, articles and code;
- captions, and bold animated text punches.

On location, the job also needs:

- picking the best take of each line across many shots;
- matching colour and loudness from one location to the next;
- cleaning up outdoor audio (wind, traffic, room changes);
- cutting mid-sentence between locations.

Optional per video: music, chapter cards. Rare: jokes and sound effects.

Recording at the desk uses a C920 at 1080p30 with a QuadCast S, in any recording app
for now. If software can't make the footage look professional, Riel buys a key light,
and may later improve the room. On location, Riel uses a phone or camera with a clip-on
microphone.

**B. Video essay,** the non-talking-head style. Riel's epitome references are Aidin
Robbins, Jacob Geller and exurb1a. Others: AI in Context, Hoog, Fern, LEMMiNO, Run The
Numbers. Elements:

- agent-made animated slides, with PDF or PPTX import when Riel's exact slides are
  needed (Riel makes slides with Claude, ChatGPT and Google Slides);
- atmospheric motion graphics;
- maps, timelines and charts;
- host segments;
- score and sound design;
- archival or game footage when Riel brings it.

**C. MockingBoard football reels.** For Instagram Reels and X. The material is
MockingBoard's own: animated prospect cards, boards, mock results, and stats
graphics from stats.mockingboard.app, plus app screen recordings. Game footage is
used only if licensed.

- Songbird answers (prose, SQL, chart spec, rows) export as JSON. That is a small
  addition on the MockingBoard Stats side.
- Kinewright rebuilds each chart as an animated graphic in the MockingBoard brand.
- Data-bound templates are filled with fresh data each week.

**D. Screen demos,** in the Screen Studio class. This needs:

- a new recorder: Wayland portal capture on Linux and Windows Graphics Capture,
  recording without the cursor, plus a cursor/click/key log;
- automatic zoom that follows clicks and typing;
- a smooth re-drawn cursor;
- styled frames with background, padding, rounded corners and a shadow;
- an optional webcam bubble and keystroke overlays.

**E. Launch and product videos:**

- device mockups;
- camera moves with depth, and 3D layers;
- kinetic typography;
- generated UI animation;
- music-synced cuts, glows and light sweeps;
- mattes and tracking.

**Stitching** separate clips with high-quality transitions shares its building
blocks with C and E, and lands with them.

### Programme mapping

| Job | Work |
|---|---|
| A | Studio layout, pointing and versions; Opus/ACP orchestration v1; person separation (background blur/replace); face-aware look correction; jump-cut and punch-in planning; AW2 Part 1 (SVG code clips) and MO6 titles and templates; styled on-screen cards; PF1-H item H-T (talk-playback holds); on location: take choice across shots (JA-4), cross-location colour and loudness matching, outdoor audio repair |
| B | Slides (agent-made, plus PDF/PPTX import); MO3 compound clips; music and sound design; own-voice clone late |
| C | Data-bound templates; AW2 Part 2 (Remotion) if data visualisation needs it; MO4 speed ramps |
| D | Recorder rebuild, auto-zoom, smooth cursor, frame styling |
| E | Depth/3D layers, device mockups, MO5 mattes and tracking, transitions library |

- **Background lane:** CC8 HDR and the rest of PF1-H, running alongside the jobs
  without blocking them.
- **Parked:** interchange, multicam, and Premiere-breadth work.

## Process

- A design doc and a critic only for big new pieces: the Studio layout with
  orchestration, person separation, and the recorder.
- One review per stage. Two reviews only where data loss, security or sandboxing
  is at stake.
- Small fixes land the same day with one review.
- Lines of code are never a budget, cap or stop rule. Over-engineering is guarded
  by tracing each mechanism to a rule or gate. Performance and memory are guarded
  by measured budgets.

## The first video

A statistical deep dive into how the NFL mis-values tight ends in the draft:
script-driven, with Songbird data on screen and 2–3 reels cut from it. Riel's
practice take for the job A dry run comes from the same script.

Later candidates:

- "AI agents can do more and less than you think"
- "T3 Code is beating Anthropic and OpenAI"
- "The best AI labs no one is talking about"
- "NFL analytics aren't as advanced as you think"
- Same-day news roundups, such as a cybersecurity CVE roundup for X. These need
  turnaround within hours (Riel, 2026-10-21).
- For Smart Data, Riel's employer (2026-10-26):
  - "One-size-fits-all SaaS is dead": a marketing video in the Mamdani style, with Riel
    on camera across many locations. It is a job A on-location candidate, and needs
    Smart Data's brand rules and sign-off.
  - Joining OpenAI's Daybreak Blue as an individual with a hardware security key: an
    explainer with Riel on camera, screen recordings and key close-ups. Its facts
    change quickly, so recheck them against OpenAI's help pages the week it is
    recorded. Blur account details and any QR or recovery codes.

## Measures

- Time from dropping in files to an exported video Riel is satisfied with, and
  Riel's own minutes within that.
- Videos Riel actually publishes, per job.
- Invented or unsupported facts reaching a render (target: zero).
- Questions the agent asks during a run (target: almost none), with recovery
  questions kept at zero, as before.
- Agent tokens, wall time and corrections per finished video.
