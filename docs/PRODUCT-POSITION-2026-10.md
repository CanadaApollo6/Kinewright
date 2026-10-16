# Product position - October 2026

> Dated snapshot, 2026-10-16, scoring `main` at `9b77ed6`. It re-scores the
> rubric in [Product position after M35](PRODUCT-POSITION-M35-2026-08.md)
> (August 2026) with the same scale, surfaces, and competitors. Work on
> branches (PF1 playback on `pf1/impl`) and accepted designs not yet built
> (AW2) are mentioned but not scored. The living plan is
> [Roadmap and development workflows](ROADMAP-AND-WORKFLOWS.md).

## Method

Two auditors scored independently, without seeing each other's work, from
the repository (docs, code, tests, eval records) and dated public sources for
competitors. No builds, tests, or hands-on sessions were run for the audit.
The two agreed on both overall grades and on eight of the ten Kinewright
scores; the two disagreements and the reconciliation are recorded below.
Competitor score changes required a dated source, which was then checked
independently.

A score of 3 means "useful in real work with limits". Several programmes
still owe their hands-on sessions (colour, audio, investigator), so every
Kinewright score of 3 or more that rests only on automated tests and evals is
marked **unverified by a person**.

## Bottom line

Since August, Kinewright has turned two finishing disciplines (managed SDR
colour and stereo audio) from foundations into complete, tested workflows,
and added the first Premiere-class motion layer (keyframed transform, blend
modes, adjustment clips, solids, transitions), source/program patching, and
relink. Almost none of that new depth has yet been used by a person for real
work, and measured preview playback on `main` is about 1.8 fps for typical
two-source 1080p ([PF1](PF1-PLAYBACK-PERFORMANCE.md), evidence T4). It is a
deeper technical alpha, still not an editor a professional should adopt as
their only tool.

Current product grade: **roughly 4/10 as a general professional editor
replacement** (August: 3/10) and **roughly 7/10 as proof of the agent-native
editor thesis** (August: 7/10).

- The replacement point is earned by breadth a professional would notice:
  CC1-CC7, AU1-AU6, MO1-MO2, M41 relink, M42 patching, and native Linux with
  two-OS CI and releases. It is provisional: if the owed hands-on sessions do
  not happen, or fail, the grade returns to 3/10.
- The thesis mechanism deepened (typed incidents and the investigator,
  colour/audio/motion planners, decoded-delivery verification as agent
  tools), but the proof did not: the colour and audio real-harness runs and
  blind reviews are pending, the investigator's real-harness checklist is
  owed, AW1 has landed only S0-S1, and AW2 is unbuilt. Resolve and After
  Effects gained agent integrations in September, so the thesis is more
  validated and less unique than in August.

## Relative scorecard

Same scale as August: 1 is a narrow foundation, 3 is useful in real work with
limits, and 5 is category-leading production depth. Changes from August are
marked with an arrow.

| Surface | Kinewright | Premiere | AE | Resolve | Nuke | Final Cut | CapCut | Descript |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| General agent-native control | 5 | 2 | 1→2 | 1→2 | 1 | 1 | 1 | 3 |
| Core professional NLE | 2 | 5 | 1 | 5 | 2 | 5 | 3 | 2 |
| Motion / compositing / VFX | 1→2 | 3 | 5 | 5 | 5 | 3 | 3 | 2 |
| Color finishing | 1→3 | 4 | 3 | 5 | 4 | 4 | 3 | 1 |
| Audio post | 2→3 | 4 | 2 | 5 | 1 | 4 | 3 | 4 |
| Text-based editing | 3 | 3 | 1 | 2 | 1 | 3 | 3 | 5 |
| Social output polish | 2 | 4 | 3 | 4 | 1 | 4 | 5 | 4 |
| Verification / reversibility for agents | 5 | 2 | 1 | 1 | 1 | 1 | 1 | 2 |
| Product polish and production trust | 1 | 5 | 5 | 5 | 5 | 5 | 5 | 4 |
| Extensibility ecosystem | 1 | 5 | 5 | 5 | 5 | 5 | 3 | 3 |

| Kinewright surface | Aug | Oct | Person-verified? | If person evidence never arrives |
|---|---:|---:|---|---:|
| General agent-native control | 5 | 5 | Narrowly (M38 human-accepted eval) | 5 |
| Core professional NLE | 2 | 2 | No | 2 |
| Motion / compositing / VFX | 1 | 2 | No | 2 |
| Color finishing | 1 | 3 | **No** | 2 |
| Audio post | 2 | 3 | **No** | 2 |
| Text-based editing | 3 | 3 | Narrowly (M38 4.08/5 human score) | 3 |
| Social output polish | 2 | 2 | Narrowly (M38 vertical artifact) | 2 |
| Verification / reversibility for agents | 5 | 5 | Narrowly (eval review gates) | 5 |
| Product polish and production trust | 1 | 1 | No | 1 |
| Extensibility ecosystem | 1 | 1 | No | 1 |

### What moved

- **Colour, 1→3 (unverified by a person).** CC1-CC7 deliver a managed
  Rec.709 pipeline with ten primary controls, scopes and matching, wheels and
  curves, hashed `.cube` look management with a browser, window and HSL
  secondaries with tracking, and QC plus 8/10-bit delivery verified by
  decoding the written file. Human surfaces exist at every stage, so this is
  workflow breadth, not kernel depth. Not 4: SDR only. CC8 on `main` is the
  S1 numeric kernel; there is no integrated HDR, RAW or log input transform
  beyond LUTs, ACES/OCIO, or calibrated monitoring.
- **Audio, 2→3 (unverified by a person).** AU1-AU6 deliver a track, bus,
  and master mixer; parametric EQ, compressor, gate, and true-peak limiter;
  loudness-targeted delivery with decoded verification; clip envelopes and
  automation; noise, hum, and click repair with room tone. One mix processor
  serves playback, proof, and export. Not 4: stereo only, no plug-in hosting
  or spectral editing, and repair is proven only on synthetic corruptions.
- **Motion, 1→2.** MO1-MO2 add full transform with keyframe editing, stills,
  seven blend modes with CPU reference twins, adjustment clips, solids, and
  push/slide/wipe transitions. Not 3: keyframe tangents are not evaluated,
  and there are no compound clips, time remap, track mattes, or animated
  titles yet.

### What did not move, and why

- **Core NLE stays 2.** M41 relink and M42 source/program patching add real
  editing breadth, but 1.8 fps preview, constant-speed-only retiming, no
  compound clips, no generated proxies, no multicam viewer, a scrub-only
  source monitor without source audio, and H.264/AAC-only export keep it
  below "useful in real work".
- **Text-based editing stays 3; social polish stays 2.** The evidence is
  largely the August evals (M37-M39). There is still no 4:5 portrait-feed
  delivery profile (four profiles), no animated titles, and no template
  library.
- **Polish and trust stay 1; ecosystem stays 1.** Native Linux, two-OS
  releases, incident fallbacks, and honest relink are progress, but the
  version is 0.1.0, the installer is unsigned, fixtures are overwhelmingly
  synthetic, and no production session has happened. There is no plugin SDK,
  scripting surface, interchange (OTIO/FCPXML/AAF), or collaboration.

### Reconciled disagreements

- **Agent-native control and verification: 5, not 4.** One auditor lowered
  both to 4 because category-leading production depth has not been shown in
  real use. Nothing regressed and both surfaces deepened, and the August 5s
  rested on the same kind of evidence, so lowering them now would change the
  standard mid-series. The scores stay 5 with that caveat recorded: they are
  firm only once a production session exercises them. The lead over
  competitors also narrowed in September.
- **After Effects and Resolve agent control: 1→2.** Resolve 21.1
  (Blackmagic, 2026-09-08) integrates Claude, Claude Code, and Codex for
  media organisation, highlight edits, and batch rendering; After Effects'
  AI Assistant entered formal public beta (reported 2026-09-28) with
  whole-project scope and project cleanup. Neither shows reversibility or
  verification evidence, so their verification scores stay 1. Premiere's
  September 26.5 release (Paper Edit, generated media, wider RAW support)
  widens existing leads without a whole-number change.

## Premiere Pro and After Effects

What Premiere and After Effects still do that Kinewright does not, ranked by
how often a working editor hits it:

1. Play the timeline at speed on ordinary hardware and codecs.
2. Ingest what clients send: HEVC, ProRes/DNx, camera RAW and log with input
   transforms, HDR end to end, and generated proxies for 4K and long-form.
3. Multicam: sync, an angle viewer, and switching.
4. Nesting and compound clips; time remapping with ramps, reverse, and frame
   blending.
5. Titles and motion graphics: animated type and templates, and for After
   Effects the layer, shape, text, 3D, expression, tracking, and roto depth
   that Kinewright defers to a later programme.
6. Masking and tracking depth: AI object masks, planar and camera tracking,
   rotoscoping.
7. Interchange and collaboration: XML/AAF/EDL/OTIO, shared projects, review
   links.
8. Ecosystem: third-party effects, panels, hardware surfaces, and template
   libraries.

What Kinewright does that they do not: one typed, reversible `Operation`
document shared by person, agent, preview, proof, and export;
provider-neutral agent sessions on the user's own subscriptions with footage
local; revision-gated plans, isolated agent branches, and destructive-operation
confirmation; typed incidents with a budgeted investigator; decoded-output
verification exposed as agent tools; published evals with hash-bound human
review; open source on Windows and Linux.

## Repository health

| Measure | August (M35, `8b80d6b`) | October (`9b77ed6`) |
|---|---:|---:|
| Tracked Rust files | 85 | 190 |
| Rust lines (all tracked `.rs`, including tests) | 59,104 | 399,657 |
| Crates | 4 | 5 (`kinewright-project`) |
| `#[test]` attributes | 388 | 3,262 |
| CI | Windows only | Windows and Linux; fast and slow tiers; Kani lane; release workflow |

Counts are newline and attribute counts over tracked files at each commit,
so they reconcile with August's published figures. The test estate is now
limited less by coverage than by the absence of person-run sessions on top
of it. Concentration risk grew: the agent server file is 35,788 lines
including tests, against August's 4,499-line warning.

## Gaps that block the Premiere-competitor goal

Both auditors ranked the same two first:

1. **Playback.** About 1.8 fps on `main` caps the core NLE at 2 and trust at 1,
   and blocks the production session itself. Covered first: PF1 close.
2. **No person-verified production use.** The colour and audio 3s and the
   investigator rest on automation. The production session sits after AW2,
   MO6, MO3, and MO5, each of which adds more surface whose acceptance is
   again automated.
3. **Ingest and delivery reality: formats, HDR, proxies, multicam.** Partly
   covered: CC8 S2+ follows PF1 in the roadmap text, but proxies, broader
   export codecs, and a multicam viewer have no slot.
4. **Editorial structure: compound clips and time remap.** Covered: MO3, and
   MO4 last.
5. **Interchange, collaboration, ecosystem.** Not covered by the current
   order.

**Recommendation from both auditors, independently:** bring person
verification forward to immediately after PF1 closes, ahead of AW2. Either
run the owed hands-on pieces (colour and audio smokes, the `color-workflow-v6`
and `audio-workflow-v7` real-harness runs with blind review, the
investigator's real-harness checklist), or add a small unfamiliar dialogue
edit taken through manual correction, repair, mix, save and reopen, and
delivery. Keep the planned graphics production session after MO5. This
converts the unverified 3s into evidence, or into cheaper fixes, before more
surface is built on them.

Of the items not on the roadmap, the auditors split on what matters most:
**interchange** (the bridge that lets Kinewright sit beside Premiere in an
existing pipeline) or **multicam** (a daily tool for the interview, podcast,
and event footage the near-term wedge targets, with sync groups and a
speaker planner already in core).
