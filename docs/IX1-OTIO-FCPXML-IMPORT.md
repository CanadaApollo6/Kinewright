# IX1 — Opening OTIO and FCPXML timelines

Status: design, revision 3, 2026-11-17. Revision 1 is `48f6d0e3`. Revision 2 folds in the lead's
rulings on revision 1's open questions, the lead's two corrections and the critic's fourteen
findings; §0.1 maps each item to its section. Revision 3 adds rulings 6–16 (§14).

This is the first slice of the interchange programme (IX), which Riel approved on 2026-11-16.
Later slices are IX-T1 (native two-sided transitions), IX2 (`.prproj` import with a loss
report) and IX3 (Lightroom presets and `.cube` LUTs as looks). After Effects comes after the
motion work.

IX1 opens an OpenTimelineIO (`.otio`, `.otiod`) or FCPXML (`.fcpxml`, `.fcpxmld`) timeline from
Resolve, Final Cut or Avid as a Kinewright project, with a small typed loss report. Nothing is ever
imported partially.

## 0. Decisions

1. Opening creates a **new, unsaved project** through the File > New path (`app.rs:1849`). Agents
   only inspect. AW1's `open_project` is their only way to open a timeline.
2. A new pure crate, **`kinewright-interchange`**, parses and lowers: bytes in, `Document` and
   report out. A new `kinewright-project::interchange` module reads files, classifies locators and
   probes media.
3. Both formats parse into **one IR with exact rational time** (§2). One lowering builds the
   `Document`. IX2 adds a `.prproj` parser and reuses the lowering and the report unchanged.
4. **Lowering is total.** Every IR within the caps yields a `Document` that passes
   `Document::validate` (`model.rs:1643`). A media clip's end snaps to an achievable cut on its
   source grid (§4.2). `lowering_failed` therefore means a bug.
5. Nests, compounds, multicam and connected clips **flatten onto lane tracks** allocated in nested
   blocks. A same-track dissolve becomes a head clip on the lane above, through one function
   (§5.3). A transition at a flattened container's edge becomes a cut.
6. **`Track.name`** (optional, ≤ 64 bytes) carries OTIO track names and single FCPXML roles.
7. The loss report is **`ImportRecord`**, stored in `Document.imported_from: Vec<ImportRecord>`.
   Item rows obey the accounting identity; timeline and media rows sit outside it. A record is
   ≤ 4 KiB, and codes beyond the row cap are still listed by name.
8. **Missing media never fails an import.** Remote and UNC locators are never touched without the
   person's confirmation. Offline assets get estimated metadata, which relink replaces.
9. Agents get **two registry capabilities**, `inspect_interchange` and `get_import_report`. The
   served surface stays at seven tools.
10. **No new crate downloads.** `serde_json` (pinned) reads OTIO and `roxmltree =0.20.0` (already
    locked) reads FCPXML. Hard caps apply during parsing, DTD internal subsets are refused, and
    `.otioz` is out.
11. Proof: authored fixtures with lavfi media; mixed-rate, flattening and accounting properties;
    fuzzing; a public-timeline corpus that the lead runs; dhat-measured budgets.

Unless shown otherwise, `model.rs`, `time.rs`, `operation.rs`, `transition.rs`, `effect.rs`,
`automation.rs` and `title.rs` are in `crates/kinewright-core/src/`. `app.rs`, `media_bin.rs` and
`timeline_ui.rs` are in `crates/kinewright-app/src/`.

### 0.1 Revision 2 map

| Item | Change | Sections |
|---|---|---|
| Ruling 1 | Agents inspect only; AW1 `open_project` is the only way in | §0, §10, §14 |
| Ruling 2 | Lanes accepted behind `lower_same_track_transition`; IX-T1 lands before IX2 | §5.3, §13 |
| Ruling 3 | `Track.name`, ≤ 64 B; a single FCPXML role names the track; registry bytes measured | §3, §5.1, §6.4, §7.6, §10 |
| Ruling 4 | Marker colour as a label prefix, behind one switch; the value is Riel's call | §3, §7.3, §14 |
| Ruling 5 | `.otioz` stays out | §1, §8.1, §13 |
| Correction 1 | Public-timeline corpus run by the lead; media rows proven by lavfi; unverified readings flagged | §7.4, §12 |
| Correction 2 | Probes use the existing entry point and move to MS2's confined admission | §8.2 |
| Finding 1 | Source end derived from span; lengths clamped; total lowering; mixed-rate property | §4.2, §5.3, §6.2, §12 |
| Finding 2 | Remote locators never touched; person confirms shares; deadlines; no-stat fixtures | §8.1, §8.2, §9, §10, §12 |
| Finding 3 | Item rows versus timeline/media rows; a named Resolve clip is exact | §7.2, §12 |
| Finding 4 | A transition at a container edge becomes a cut (`transition_at_nest_cut`) | §3, §5.2, §12 |
| Finding 5 | Retimed container mapping; variable maps averaged, or the container is gapped | §5.2 |
| Finding 6 | Anchored offsets in parent local time; a `timeMap` clip's in-point is `timeMap(@start)` | §4.3, §5.2, §12 |
| Finding 7 | Head link group separate from the body; timeline-UI test | §5.3, §12 |
| Finding 8 | Relink writes the candidate's metadata; `validate_document` checks the record | §6.3, §6.4, §8.3, §12 |
| Finding 9 | `omitted_codes` | §7.5, §9 |
| Finding 10 | OTIO A/V pairing rule; dual-mono collapse | §3, §5.4 |
| Finding 11 | `nodes_limit` during the parse; `NaN`/`Infinity` are `malformed`; one walk per root | §8.3, §11, §12 |
| Finding 12 | Variable-speed bound in frames; dual-system audio bound about 37 ms | §4.2, §4.3 |
| Finding 13 | `Vec<ImportRecord>` and a list of asset ranges | §6.4, §7.1, §8.3 |
| Finding 14 | Topmost-opaque check; nest order property with lane 1 busy | §5.1, §12 |

## 1. Scope

In scope:
- **OTIO JSON** up to OTIO v0.18.1 (2025-11): `Timeline.1`, `Stack.1`, `Track.1`, `Clip.1`/`.2`,
  `Gap.1`, `Transition.1`, `Marker.1`/`.2`/`.3`, the four media references, `LinearTimeWarp.1`,
  `FreezeFrame.1`, `Effect.1`, `SerializableCollection.1`, and the legacy `Filler`/`Sequence`
  names. Read from `.otio` files and `.otiod` bundles.
- **FCPXML 1.8–1.14** (1.14 is Final Cut Pro 12.0), from `.fcpxml` files and `.fcpxmld` bundles
  (1.10+). Older versions are refused with `unsupported_format_version`.
- **One timeline per import.** The `timeline` argument picks one when a file holds several.

Out of scope: `.otioz` (ruling 5), EDL, AAF, xmeml, export, and effects beyond §3.

## 2. Architecture

`kinewright-project::interchange::prepare(path, options)`:
1. classifies `path` (§8.1) and reads it under the size cap;
2. sniffs the format and calls `interchange::{otio,fcpxml}::parse` to get an `ir::Timeline`;
3. resolves media into a `MediaTable`: classify each locator, stat local ones, search
   `media_roots`, then probe and hash;
4. calls `interchange::lower` to get `(Document, ImportRecord)`;
5. validates both and holds them in memory as a `PreparedImport`, until the app commits it (§6)
   or the agent reads it (§10).

All filesystem access in steps 1–3 goes through one `LocatorFs` seam, so tests can record every
path touched. The IR follows OTIO's tree, which FCPXML and `.prproj` both fit:

```text
Timeline { name, start: Time, rate_hint, resolution_hint, root: Stack }
Stack    { tracks: Vec<Track> (0 = bottom), window: Option<Range>, time: ContainerTime, markers, effects }
Track    { kind: Video | Audio, name, items: Vec<Placed>, markers }
Placed   { at: Time, item: Item, site: SourceSite }        // "V2#14" or an XML line
Item     = Clip { media, source_start: Time, span: Time, time: Normal | Speed(Ratio) | Reverse(Ratio)
                  | Freeze(Time) | Variable{points}, gain, gain_keys, fades, enabled, role, markers,
                  effects, name, av_group }
         | Gap { duration } | Transition { kind, before: Time, after: Time }   // placed at the cut
         | Nested { stack, window, time: ContainerTime } | Title { text, duration }
         | Unsupported { what, duration }
ContainerTime { offset: Time, in_point: Time, speed: Ratio }   // §5.2, shared by nests and anchors
Time     = exact rational seconds: i64 / u64, reduced, i128 arithmetic
```

A clip carries its source start and its span, not a source end. The end is derived on the frame
grid (§4.2).

An absolute `at` fits FCPXML lanes and Premiere starts. The OTIO parser sums trimmed durations and
skips transitions, as OTIO's `range_of_child_at_index` does.

**Composition windows (rulings 7 and 11).** One parser-side function trims children,
transitions and markers to the container's window and rebases the retained placements to
its start. OTIO supplies `source_range` for tracks, stacks and nested timelines. FCPXML
supplies `@start` and `@duration` for its containers, including `sync-clip`, `ref-clip`,
compounds and multicam. Items wholly outside the window count as dropped under
`outside_window`. A transition hit by a window edge becomes a hard cut.

Exact rationals hold Premiere ticks (Mirage repository,
`crates/premiere_file/src/schema/timing.rs:7@cdea27e`) without the 0.5 ms rounding in the
Mirage repository's `crates/fx_schema/src/time.rs:5@cdea27e`.

## 3. The mapping

N = faithful, A = approximated (bound stated), Ø = dropped.

| Concept | OTIO | FCPXML | Kinewright target | Rating |
|---|---|---|---|---|
| Timeline | `Timeline.1` | `project/sequence` | one `Document` (`model.rs:1439`) | N; others listed, `timeline` picks |
| Frame rate | inferred (§4.4) | `format@frameDuration` | `Document.fps` (`model.rs:1476`, `time.rs:53`) | N; OTIO floats snapped (§4.1) |
| Resolution | none standard | `format@width/height` | `Document.resolution` (`model.rs:1477`) | N FCPXML; A OTIO: modal probed size, else 1920×1080 (`resolution_inferred`) |
| Start, drop frame | `global_start_time`; DF only in timecode strings | `sequence@tcStart`; `@tcFormat` | rebased to 0; frames exact, NDF display (`timeline_ui.rs:3998`) | A: offset in report; DF labels only |
| Tracks | `Track.1` kind, stack order, `name` | spine, lanes ±n, clip roles | `Track{kind}` in z order (`model.rs:574`, `:1440`), new `Track.name` (§5.1) | N; A name truncated past 64 B; Ø mixed roles |
| Clip range | `source_range` − `available_range.start_time` | `@start` − `asset@start` (or `timeMap(@start)`), `@offset`, `@duration` | `Clip.source_range`, `timeline_start` (`model.rs:429-443`); cuts snap to the outgoing clip (§4.2) | N on grid; A: snapped cuts and source ends bounded by §4.2 |
| A/V clip, split edit | paired only by §5.4 | `asset-clip`, `audioStart`/`audioDuration` | linked pair (`model.rs:446-450`), as `media_bin.rs:617` builds | N; A unpaired OTIO split edit (`av_link_unpaired`) |
| Dual-mono, channel maps, pan | same file, same range on two audio tracks | `audio-channel-source`, `adjust-panner` | one clip kept (§5.4); no clip pan | A `channel_map_dropped`; Ø pan |
| Gap; disabled | `Gap.1`; `enabled` | `gap`; `@enabled="0"` | empty span; `Clip.enabled` (`model.rs:484-489`) | N for retained space and enabled state; A for the timeline tail (§6.2) |
| Constant speed | `LinearTimeWarp.1` (§4.3, unverified reading) | two-point `timeMap`; `conform-rate` | `speed_percent` 10..=1000 (`model.rs:473-483`) | A: speed rounded or clamped, cut snap and source-end drift (§4.2–4.3); Ø audio (muted, `model.rs:477-478`) |
| Reverse, variable | negative scalar; other `TimeEffect` | falling or multi-point `timeMap` | none | Ø reverse (plays forward); A variable: average speed, drift in frames |
| Freeze | `FreezeFrame.1`, scalar 0 | constant-value `timeMap` segment (unverified reading) | `ClipContent::Freeze` (`model.rs:341`, `:393-398`) | N |
| Dissolve; fade from/to empty | `Transition.1` `SMPTE_Dissolve`; beside a `Gap` | `transition` "Cross Dissolve"; at a storyline end | lane head clip with `crossfade` `transition_in` (`model.rs:316-322`, `transition.rs:38`); `opacity` keys out (`effect.rs:1581`) | A: structure, ramp (§5.3) |
| Transition at a container edge | beside a nested `Stack` | beside `ref-clip`, `mc-clip`, `sync-clip` | hard cut at the cut point (§5.2) | A (`transition_at_nest_cut`), window frames as bound |
| Other transitions | `Custom_Transition` | wipe, push, slide… | same-direction descriptor (`transition.rs:37`), else crossfade | A, names reported |
| Audio crossfade | audio-track transition | audio side of `transition` | ramp in + `audio_fade_out_frames` (`model.rs:456-465`) | A: linear, ≤ 3 dB from equal power |
| Clip gain, keys | not standard | `adjust-volume@amount`; `param[@name='amount']` keys | `audio_gain_tenth_db` −600..=120 (`model.rs:451-455`, `:732-734`); `audio_gain_curve` (`model.rs:466-472`, `automation.rs:28-35`) | N ±0.05 dB; A clamped; A keys on frames, inside derived length |
| Audio fades | — | `fadeIn`/`fadeOut` | `audio_fade_in/out_frames` (linear), clamped to derived length | A: shape named |
| Timeline marker | root-stack or track markers | — (all FCPXML markers sit on items) | `Marker{position,label,color_token}` (`model.rs:600-607`) | N; A: duration dropped, colour per switch (§7.3) |
| Item marker | `Clip`/`Stack` markers, item time | `marker`, `chapter-marker`, `@completed`, item time, no colour; `keyword`, `rating` | pinned project marker; chapter, to-do as label prefix | A: stops following; Ø outside trim, keyword, rating |
| Nest, compound, multicam, sync, audition | `Stack.1` in a `Track` | `ref-clip` → `media/sequence`; `mc-clip`, `sync-clip`, `audition` | flattened in nested lane blocks (§5.1–5.2); active angle or pick | A; Ø nest effects, other angles |
| Title | — | `title` | `ClipContent::Title`, ≤ 512 chars (`title.rs:118`, `:262`), default style | A |
| Still | image `ExternalReference` | image asset | `MediaKind::Image` freeze (`model.rs:73`) | N |
| Generators, image sequences, effects, colour | `GeneratorReference`, `ImageSequenceReference`, `Effect.1` | generator `video`, `filter-video`, `adjust-*` except volume, `colorSpace` | none in IX1 | Ø: named, counted; generators leave a gap |
| Clip names, metadata, item colour | `name`, `metadata`, `color` | `@name`, `metadata`, `note` | assets keep names | Ø, counted outside the item identity (§7.2) |

## 4. Time and rates

**4.1 Exact until the grid.** FCPXML times (`"1001/24000s"`, `"3600s"`) parse to exact rationals,
and overflow is refused with `unrepresentable`. OTIO `RationalTime` is two doubles, and real files
carry rates such as 23.976024627685547 (OTIO issue #1822). Rates snap as follows:
- A rate within 1e-6 of an integer becomes that integer.
- Otherwise, with `k = round(r × 1.001)`, a rate within 0.005 of `k × 1000/1001` becomes
  `1000k/1001`. The nearest integer rate is at least 0.024 away.
- Anything else is refused with `unsupported_rate`.

A value within 1e-6 of an integer is exact at its snapped rate; any other value counts
`subframe_rounded`. `NaN` and `Infinity` never reach this step (§11). Both parsers and
offline estimation refuse project or asset rates above 1,000 fps with `unsupported_rate`
(ruling 12). OTIO time magnitudes outside the representable range are `unrepresentable`.

**4.2 Lowering to frames (ruling 6).** Round each IR boundary to its initial project frame
with `frame = ⌊t × fps + ½⌋` in `i128`. Core derives a media clip's project length from its
source range as `map(source.end) − map(source.start)` at the effective rate
(`model.rs:1652-1666`, `time.rs:276-290`). Cuts snap to the outgoing clip's source grid.

Lower each track from left to right:
1. Start a first clip or a clip after a gap at its rounded IR start. Start an abutting
   successor at the previous clip's decided end.
2. At the decided start, evaluate the IR source time by shifting the IR in-point for the
   change from the IR start. Subtract the media origin and map once at the asset rate to
   get `source.start`.
3. Apply the next item's IR end and the media's end as limits. Among the achievable
   ends those limits admit, choose the one nearest the IR end, breaking ties toward
   the shorter end. An achievable end has an exact
   `map_project_duration_to_source(source.start, span, effective, fps)` (`time.rs:365`).
   An abutting successor inherits the decided end. A following gap absorbs the difference.
   Every end targets its own IR end, independently of the previous cut's displacement.
4. If no positive achievable end fits, give the clip one source frame if it fits before the
   next item's IR end. Otherwise drop it with `clip_too_short` (Ø) and start the successor
   at the dropped clip's decided start.
5. Use the decided cuts for transitions, link groups and A/V pairing. Linked video and audio
   cuts can differ by the snap. Clamp transition durations and fade sums to the derived length
   (`transition_fade_clamped`), and keep gain keys inside that length (`gain_keys_mapped`).

`cut_snapped` counts each cut moved off its rounded position and reports the largest move in
project frames. `subframe_rounded` records the input value rounding in §4.1.

**Bounds.** Report each measured maximum. Let `P` be project frames per source frame of the
outgoing clip at its effective rate, and `r = asset fps ÷ project fps`.
- The first start and each start after a gap are within ½ project frame of the IR.
- When the nearest achievable cut is admissible, its error is at most ½ + P/2 project frames.
- A next-item limit that excludes the nearest cut leaves the adjacent earlier cut, when
  it is positive and inside the media. Its error is at most ½ + P project frames.
- Media exhaustion can exclude several source boundaries. The report records the actual
  cut error, and a verified media clamp is reported separately (§8.2).
- At 100% speed with sufficient media, the source end is within 1 + r source frames of the IR.
- For a retimed clip, report the source-end drift from speed quantization separately (§4.3).

**Audio-only assets** probe at 30 fps (`crates/kinewright-media/src/au6_sources.rs:1012`). A
sample-accurate FCPXML or dual-system audio in-point therefore rounds twice: once to the 30 fps
source grid (±16.7 ms) and once to the project grid for its position (±20.8 ms at 24 fps). The
reported sync bound is the sum, about 37 ms at 24 fps, in milliseconds.

**4.3 Speed.** OTIO does not define how a time effect relates to `source_range`; its layout maths
ignores effects. IX1 reads `source_range.duration` as the duration in the parent, and consumes
`duration × time_scalar` of media from `source_range.start_time`. This reading is unverified
against an editor until a corpus sample settles it (§7.4). FCPXML speed is `Δvalue / Δtime` over
the `timeMap` ends, and a retimed clip's media in-point is `timeMap(@start)`.

Kinewright gets `p = round(100 × |s|)`, clamped to 10..=1000 (`speed_clamped`). Cuts snap
through §4.2 at M23's effective rate (`model.rs:531-536`). Report `max |s − p/100|` in tenths
of a percent and, separately, the maximum source-end drift from speed quantization. For each
retimed clip the drift is `|s − p/100|` times the clip's duration expressed in asset-rate
frames. The `1 + r` bound applies to clips at 100% IR speed.

A variable map becomes a constant average speed. Its bound is the largest drift, in source
frames, between the true map and the unquantized average, taken over the map's points.

**4.4 Project rate, start, drop frame.**
- FCPXML takes the sequence format.
- OTIO takes the first that applies: the `project_fps` argument, `global_start_time.rate`, the
  duration-weighted most common video clip rate. Otherwise it refuses with
  `project_rate_required`.

The timeline start is subtracted everywhere, and anything left before frame 0 is trimmed
(`before_start_trimmed`). Drop frame is display-only in both formats. Positions stay exact, but
Kinewright shows nominal non-drop timecode (`timeline_ui.rs:3998`), so labels differ
(`drop_frame_labels`).

## 5. Structure: tracks, lanes, nesting, transitions

**5.1 Tracks, names and lanes.** IR tracks keep their z order. FCPXML video lanes become video
tracks in ascending order (−n … spine … +n). Spine audio comes first on the audio tracks, then each
audio-bearing lane.

*Names.* `Track.name: Option<String>` is at most 64 bytes and skipped when absent.
- An OTIO track keeps its `name`. Past 64 bytes it is cut on a character boundary and counted in
  `track_names_dropped`.
- An FCPXML track whose clips all carry one role (`videoRole`/`audioRole`, as written, e.g.
  `dialogue.dialogue-1`) takes that role as its name.
- Mixed roles leave the track unnamed and count `track_names_dropped`. That row is used only for
  truncation and mixed roles.
- Lane tracks are named `<name> +n` within 64 bytes, or left unnamed.
- IX1 adds no rename operation. `AddTrack` carries the name, and the track header shows it.

*Lanes.* Tracks forbid overlap (`operation.rs:5306-5311`), so lanes are allocated in nested blocks:
- Each IR track has a height `h = max(1 + d, max over its nests of Σ inner heights)`, where `d = 1`
  if the track holds a two-sided dissolve.
- A track at base lane `b` owns lanes `b .. b+h−1`. Lane `b` is the track itself.
- A nest's inner track *i* starts at `b + Σ(j<i) h_j`. Lane `b` is empty while the nest plays.
- Nests and anchored items share this allocator. A block's height also includes the lanes
  needed by concurrent anchors (ruling 15).
- Blocks never interleave, so inner layers keep their order even when lane 1 is busy at the same
  time.
- Overlap the source should not contain, such as two FCPXML clips on one lane in hostile input,
  goes to spill lanes above the block, later items higher, and counts `lane_added`.

More than 128 tracks of one kind is refused with `too_many_tracks`.

**5.2 Containers and anchored items.** The importer flattens OTIO `Stack`s in tracks, FCPXML
`ref-clip` and `sync-clip`, the active `mc-clip` angles, and an `audition` pick when it is a
container.

*Mapping local time to parent time.* One function, `ContainerTime::to_parent`, maps a local
instant through every level:

    t_parent = offset + (t_local − in_point) / s

- `offset` is the container's start in its parent.
- `in_point` is its local in-point: OTIO `source_range.start_time`; FCPXML `timeMap(@start)`, or
  `@start` when there is no `timeMap`.
- `s` is its constant speed.
- Boundaries and markers both go through it.

*Anchored items.* FCPXML anchored (connected) clips use the same function, with their parent as
the container, because an anchored `@offset` is in the parent's local time. A spine clip at 10 s
with `start="3600s"` places a lane clip with `offset="3602s"` at 12 s. On a retimed parent this
rests on a reading that is unverified against an editor (`anchor_retime_reading`).

*Variable and unsupported maps.* A variable map on a container is averaged into `s`, with the
largest drift in parent frames as its bound (`variable_speed_averaged`). If the map falls, is not
monotonic, or its net speed rounds outside 10..=1000 %, the container lowers as a gap of its span
(`nest_retime_unsupported`, Ø). Count the container and every item in its subtree as dropped
under that code (ruling 14).

*Inner items.* Inner speeds are multiplied by `s`, rounded once and clamped (`speed_clamped`).
Container effects are dropped. Container markers become project markers inside the window. Depth
is capped at 8 (`too_deep`), the limit in the Mirage repository's
`crates/premiere_file/src/schema/nested.rs:30-32@cdea27e`.

*Transitions at a container edge.* A per-layer crossfade cannot reproduce a dissolve between two
composites. The outgoing layers stay visible at weight `(1 − a)²` under an opaque entering top
layer, and extending an outgoing nest past its window is undefined. Any transition with a flattened
container on either side therefore lowers as a hard cut at the cut point, audio included, and
counts `transition_at_nest_cut` (A, bound = window frames).

Compositing is associative, so for opaque, normal-blend clips without container effects the
topmost item per frame is unchanged; §12 checks this.

**5.3 Transitions.** A Kinewright transition is a transition-in on the entering clip, over the
layers below (`docs/M20-TRANSITIONS.md`). Every same-track transition lowers through one function,
`lower_same_track_transition`, so IX-T1 can replace it in one place (ruling 2).

Both formats draw on handles around the cut decided in §4.2. OTIO's `in_offset` shows B before
the cut and `out_offset` shows A after it. FCPXML exports centre `transition` on the cut, so
`before = cut − offset` and `after = offset + duration − cut`. A dissolve at cut `T` lowers to:

```text
lane above P:              [B head T−before .. T+after, crossfade transition_in]
track P:   [A ........ T+after][B body from T+after ..........]
```

When the transition consumes the entire entering clip, omit the empty body (ruling 13).

*Audio.* Audio follows on the audio tracks. B's head ramps in, and A gets
`audio_fade_out_frames = before + after`.

*Link groups.* B's video head and B's audio head form their own link group. B's body keeps B's own
A/V group. They cannot share one group:
- `linked_trim_operations` (`timeline_ui.rs:3585-3655`) applies one source delta to every member;
- `linked_transition_operations` (`timeline_ui.rs:3830`) adds a transition to every member.

The `transition_lane` row says that a move or trim can split the head from the body. IX1 changes no
linked-edit builder.

*Clamps.* The head's transition length is the head's derived length (§4.2), and A's fade-out is
clamped the same way.

*Accuracy and edge cases.*
- Kinewright's ramp is `k/(n−1)`, within `1/(n−1)` of the mix per frame of a linear dissolve.
- Gap → B is a `crossfade` on B in place.
- A → gap is `opacity` keys from 100 to 0.
- Short handles shrink the window, counted in `transition_handles_short`.

**5.4 OTIO A/V pairing and dual-mono.** OTIO carries no links. A video clip and an audio clip are
linked only when they share the same media, the same timeline start and the same source start;
they then form a pair, as `media_bin.rs:617` builds one. A same-media pair that differs (a split
edit) stays unlinked and counts `av_link_unpaired` (A).

Resolve writes dual-mono as two clips of one file on two audio tracks. Kinewright plays each clip's
full mix, so the pair would play about 6 dB loud. Audio clips with the same media, the same timeline
range and the same source range collapse to the lowest one, counting `channel_map_dropped` (A).

## 6. Import shape, totality and validation

**6.1 Shape.** `prepare` returns a `PreparedImport` held in memory. Open calls
`create_project_session(name, document, …)` (`app.rs:1301`), as `new_project` does, so the session
exists with the whole document or not at all. The project is untitled and locator-based, and no
media is copied. Save As writes the `.kinewright`, and undo history starts after the import.

**6.2 Lowering is total.** The lowering establishes every invariant that `validate_document`
checks (`operation.rs:5144`), by construction:
- ids are unique and allocated in order (assets, tracks, clips, links, markers, effects);
- starts are ≥ 0 (§4.4), clips are sorted, and no clips overlap (§5.1);
- source ranges lie inside the asset (§8.2), source ends are derived (§4.2), and speeds lie in
  10..=1000;
- video parts sit on video tracks and audio parts on audio tracks, with stills as freezes;
- transitions and fade sums fit the derived length, gain keys lie inside the clip, and gain lies
  in −600..=120;
- titles are ≤ 512 characters and on video tracks;
- markers are ≥ 0 and sorted;
- `Document.duration` is the maximum clip end (`operation.rs:5337`). Report any trailing gap
  removed from the timeline with `trailing_gap_trimmed` and its removed project-frame count
  (ruling 8);
- `audio_mix` is empty and the colour context is the default (`color_space_default`);
- the record and track counts are within their caps.

Any `OpError` left after lowering is `lowering_failed`. It is a bug, which the mixed-rate property
(§12) is designed to catch, and never a partial project.

**6.3 Validation.** Before commit, `Document::validate` must pass. `validate_document` calls
`ImportRecord::validate` for every record (caps, the accounting identity, the 4 KiB size), so a
hand-edited project carrying an oversized or inconsistent record is refused on load like any other
invalid field.

A batch of operations is ruled out: `apply_batch` re-validates the whole document after every
operation (`operation.rs:604-606`), so a 10 000-clip import of about 30 000 operations would be
quadratic.

**6.4 Core changes.**
1. **`Document.imported_from: Vec<ImportRecord>`.**
   - It is skipped when empty and holds at most 16 records. IX1 writes exactly one.
   - Its precedent is `investigator` (`model.rs:1470-1475`).
   - No operation writes it, and no tool input schema embeds `Document`.
   - A list now means importing into an open project later adds a record without changing the
     format.
2. **`Track.name: Option<String>`.**
   - It is ≤ 64 bytes, skipped when `None`, and checked in `validate_document`.
   - `Track` reaches every `Operation`-embedding tool through `AddTrack`, so registry bytes move
     (§10).
   - Every `Track { … }` literal gains `name: None`.
3. **The relink rule for estimated assets** (§8.3).
4. **`ImportRecord::validate`**, called from `validate_document`.

## 7. The loss report

**7.1 Shape.**

```rust
struct ImportRecord {                      // kinewright-core; written once, never by an operation
    format: InterchangeFormat,             // otio | fcpxml
    format_version: String,                // ≤ 16 B: "1.13", "Timeline.1"
    source_name: String, source_sha256: String, // file name only (≤ 128 B); main-file hash
    start_offset_frames: i64,              // the rebased start
    items: u32, exact: u32, approximated: u32, dropped: u32,   // item rows only (§7.2)
    media: MediaCounts,                    // assets, online, offline, remote, timed_out
    estimated_assets: Vec<Range<AssetId>>, // ≤ 8 ranges of offline assets (§8.3)
    rows: Vec<LossRow>,                    // ≤ 8, ordered as §7.5
    omitted_codes: Vec<LossCode>,          // every code with a row past the cap (§7.5)
}
struct LossRow { code: LossCode, fidelity: Fidelity /* approximated | dropped */, count: u32,
    bound: Option<LossBound> /* frames | speed_tenth_percent | tenth_db | milliseconds */,
    unverified: bool /* skipped when false (§7.4) */,
    names: Vec<String> /* ≤ 2, ≤ 24 B */, at: Vec<LossSite> /* ≤ 2 first */ }
struct LossSite { source: String /* "V2#14" */, frame: i64, clip: Option<ClipId> }
```

**7.2 Scope and counting.** Each `LossCode` has a fixed scope.

*Item rows.* An item is one source clip, gap, transition, marker, effect or container. It is exact
with no item rows, approximated with only A item rows, and dropped with any Ø item row. The identity
is `exact + approximated + dropped = items`.

*Timeline and media rows* describe the timeline or its media, not an item, and sit outside the
identity: `start_rebased`, `rate_snapped`, `resolution_inferred`, `drop_frame_labels`,
`color_space_default`, `track_names_dropped`, `clip_names_dropped`, `metadata_dropped`,
`trailing_gap_trimmed`, `media_offline`, `media_timeout`, `remote_url`, `relinked_by_name`, and
the three `*_reading` rows. Treat the removed part of a trailing gap as timeline loss outside
the item totals. Resolve names every clip and attaches metadata, so a named Resolve clip
with nothing else lost is exact; a golden fixture proves it.

**7.3 Marker colour switch (ruling 4).** One constant in the lowering, `MARKER_COLOR_POLICY`,
chooses between two values:
- `LabelPrefix` writes the source colour name as a label prefix (`[red] Fix grade`) and counts
  `marker_color_as_label` (A).
- `Drop` counts `marker_color_dropped` (Ø).

This design assumes `LabelPrefix`; the shipped value is Riel's call (§14). Every marker keeps
`color_token` 0, so `docs/DESIGN.md:82-84` holds either way.

**7.4 Unverified readings (correction 1).** Three mappings rest on readings that no specification
states:
- OTIO time warps (§4.3), row `time_warp_reading`;
- FCPXML hold segments as freezes, row `hold_reading`;
- anchors on a retimed FCPXML parent (§5.2), row `anchor_retime_reading`.

Each row counts the clips that rely on its reading and carries `unverified: true`, which the dialog
shows as "unverified against an editor". When a corpus sample that carries the case confirms a
reading, the slice removes its row. When a sample contradicts it, the slice changes the reading and
records the sample's shape (never the file) as a fixture. Without such a sample the reading stays,
flagged.

**7.5 Order, cap and size.** Rows are ordered in four groups:
1. dropped item rows;
2. the media rows `media_offline`, `media_timeout` and `remote_url`;
3. approximated item rows;
4. the other timeline rows.

Within a group, rows are ordered by count. At most 8 rows are kept, and `omitted_codes` names every
code beyond them. A dropped code is therefore never hidden, and the dialog lists omitted codes under
the rows (finding 9). The media counts are in the header, so offline media is visible even when its
row is cut.

The serialized record is capped at 4 KiB (ruling 10). JSON escaping counts toward the cap.
Eight rows with two 24-byte control-character names and two sites each can serialize to 4,150
bytes. If the record exceeds the cap, trim the name and site samples first. Then remove rows
from the end of the ordered list as needed and add their codes to `omitted_codes`. Preserve
all totals and all omitted codes. `ImportRecord::validate` checks the serialized size.
A typical import (6–8 rows) is about 1.5 KiB:

```json
{"format":"otio","format_version":"Timeline.1","source_name":"reel2.otio","items":1840,"exact":1702,
 "approximated":121,"dropped":17,"media":{"assets":212,"online":209,"offline":3,"remote":0,"timed_out":0},
 "rows":[{"code":"effect_dropped","fidelity":"dropped","count":14,"names":["Blur","Glow"],
   "at":[{"source":"V2#14","frame":1234,"clip":56}]}, …],"omitted_codes":["metadata_dropped"]}
```

**7.6 Codes.** `LossCode` is a closed snake_case enum. A slice that needs a new code adds it.

*Item scope:*
- Timing and structure: `subframe_rounded`, `cut_snapped` (A), `clip_too_short` (Ø),
  `outside_window` (Ø), `before_start_trimmed`, `nest_flattened`, `transition_lane`,
  `lane_added`, `multicam_active_angle`, `audition_active_pick`.
- Speed: `speed_rounded`, `speed_clamped`, `reverse_dropped`, `variable_speed_averaged`,
  `nest_retime_unsupported`, `retimed_audio_muted`.
- Transitions: `transition_type_mapped`, `transition_at_nest_cut`, `audio_crossfade_linear`,
  `transition_handles_short`, `transition_fade_clamped`.
- Audio: `gain_clamped`, `gain_keys_mapped`, `fade_shape_linear`, `pan_dropped`,
  `channel_map_dropped`, `av_link_unpaired`.
- Markers: `marker_duration_dropped`, `marker_color_as_label`, `marker_color_dropped`,
  `item_marker_pinned`, `marker_outside_item`, `keyword_rating_dropped`.
- Content and sources: `effect_dropped`, `title_style_default`, `title_text_truncated`,
  `generator_dropped`, `image_sequence_dropped`, `unknown_item`, `source_clamped`,
  `source_origin_assumed`.

*Timeline and media scope:* the codes listed in §7.2. `trailing_gap_trimmed` is a timeline row
bounded in removed project frames.

## 8. Media

**8.1 Locators, classification and bundles.**

*Where references come from.* OTIO reads the active reference: `media_reference`, or
`media_references[active_media_reference_key]` on `Clip.2`. FCPXML reads
`media-rep[@kind='original-media']@src` (1.9+) or `asset@src` (1.8).

*Classification (finding 2).* Every locator is classified before any filesystem call: media
references, the agent's `path` and each `media_roots` entry.
- **Local:**
  - a `file:` URL with an empty or `localhost` host, percent-decoded;
  - a drive-letter path or `\\?\<drive>:\` on Windows;
  - a `/…` path on Unix, but not `//…`;
  - a relative path, resolved under the document or bundle root (`.otiod` writes
    `media/<name>`; FCPXML may write `./Media/x.mov`).
- **Remote:**
  - a `file:` URL with any other host;
  - `\\host\share`, `\\?\UNC\…`, `\\.\…`, and every other `\\?\` form;
  - `//host/…` on every platform;
  - any scheme other than `file`.

For a `file:` URL, apply the host rule first, percent-decode the path and classify the final
decoded native path before any filesystem call (ruling 9). A URL with encoded separators
(`%2F` or `%5C`, case-insensitive) is remote. A decoded path beginning with `//` or two
backslashes is remote, including `file://localhost//host/share/a.mov`.

*Why remote locators are never statted or probed.* Statting `\\evil.example\s\a.mov`, or a `file:`
URL naming that host, makes Windows authenticate to the host over SMB and send the user's NTLM
hash. A remote locator's asset is offline with `remote_url`.

*Allowing a share.* In the import dialog the person may allow a listed share. Only then is it
statted and probed, under the deadlines of §8.2. An agent cannot allow one: `inspect_interchange`
refuses a remote `path` or root (`remote_path`), and reports remote media without touching it.

*Mapped drives and symlinks.* Mapped drive letters and symlinks are local to the OS and are
treated as local. They are the person's own configuration, and the deadlines bound a hung mount.
Under the open-a-project model, a timeline can name absolute local paths that the importer
probes and hashes without confirmation.

*No network code.* Neither IX crate links network code, and a test asserts it.

*Bundles.* `.otiod` is `content.otio` plus `media/`. `.fcpxmld` holds `Info.fcpxml` and locator
data (tracking shapes, drawings), which is dropped and named. Bundle media is used in place, never
copied.

**8.2 Online media and deadlines.**

*Probe path (correction 2).* Probes use the entry point `import_media` already uses,
`Analysis::probe` (`crates/kinewright-agent/src/server.rs:2476`). IX1 adds no probe path of its
own. When MS2's confined admission lands, import probes go through it like every other admission.
The probe hashes SHA-256 eagerly (`crates/kinewright-media/src/decode.rs:46-47`), so every imported
asset is verified, as M41 requires.

*Workers and deadlines.* Stats and probes run on at most 4 workers, each under a deadline:
- a stat must answer within 2 s;
- a probe must open the file and read its metadata within 10 s;
- hashing must make progress at least every 10 s.

A worker past its deadline is abandoned, and its result is discarded. The thread itself
ends when the call returns. Replace an abandoned stat worker so other locators can still
be checked. A locator can be declared missing after its own stat returns or its own
deadline expires. Cap the number of abandoned workers. Once that cap prevents another
dispatch, report the remaining locators as `media_timeout` (ruling 16).

*Metadata.* Probe metadata wins over interchange metadata. A source range past the probed duration
is clamped (`source_clamped`). A missing media start is taken as 0, and a range that then overruns
counts `source_origin_assumed`.

**8.3 Missing media.**

*Searching `media_roots` (finding 11).* `media_roots` are folders the person or agent names, at most
8.
- Each root is walked once, at most 4 levels deep and 100 000 entries, following no symlinks or
  junctions. The walk builds a name → paths index.
- Each missing file name is looked up once.
- A name with several hits stays missing and is listed.
- Matches are listed before commit and counted (`relinked_by_name`). Nothing is picked silently,
  which M41 forbids (`docs/M41-OFFLINE-RELINK-CACHE-VISIBILITY.md:50`).

*Offline assets.* Media still missing becomes an offline asset:
- its path as written (remote locators included);
- an unknown fingerprint;
- kind, fps, duration and resolution estimated from the interchange: FCPXML `asset` and `format`,
  or OTIO `available_range`, else the furthest frame used.

These assets take the last ids, as contiguous ranges recorded in the record's `estimated_assets`
(finding 13).

*Relink (finding 8).* M41's `relink_asset` checks an exact kind, fps, duration and resolution
(`operation.rs:1923-1956`), then writes only the path and the fingerprint (`:1964-1968`). For an asset
inside any record's `estimated_assets` whose fingerprint is still unknown, IX1 changes both halves:
- **Checks:**
  - the candidate's kind suits every track holding its clips;
  - its fps matches exactly;
  - its duration covers every clip's source end;
  - its resolution is free.
- **Writes:** the path, the fingerprint, and the candidate's kind, duration and resolution, so no
  estimated metadata survives.

`allow_unverified_source` stays required, and the known fingerprint ends the relaxation. The fps is
unchanged, so clip lengths do not move and `survive_clip_edit` has nothing to do.

## 9. App surface

*Opening.* File > Open… (`choose_project`, `app.rs:1836`) gains a "Timeline (OTIO, FCPXML)" filter.
A bundle opens through its `content.otio` or `Info.fcpxml`, or by dropping it.
`import_dropped_files` (`media_bin.rs:69`) routes these files and bundles to the interchange path,
as it already routes `.cube` (`media_bin.rs:79`).

*The import dialog* is modal. It shows:
- the source and a timeline picker;
- fps and resolution (editable for OTIO);
- the totals, with item rows and timeline/media rows in two groups;
- unverified rows labelled, and omitted codes listed by name;
- the media table: online, missing, timed-out, and remote shares with an **Allow** tick;
- **Locate folder…**, which fills `media_roots`;
- hashing progress, **Open** and **Cancel**.

*After opening.* Open adds a tab named after the timeline, with the status "Imported reel2: 1702
exact, 121 approximated, 17 dropped". Project > Import report shows the stored records. Track
headers show `Track.name` when present.

## 10. Agent surface

There are two read-only registry capabilities, reached through `invoke_capability` and listed in
`INSPECTOR_TOOL_NAMES` (`crates/kinewright-agent/src/schema.rs:16`).

- **`inspect_interchange { path, media_roots?: [≤ 8], project_fps?: {numerator, denominator},
  resolution?: [w, h], timeline?: string, code?: string, after?: u32 }`**
  - It classifies, parses, stats local media and lowers.
  - It returns `{summary, record, missing: [≤ 8 {name, reason}], missing_omitted,
    media_probed: false}`.
  - It never hashes, since a tool call must not read terabytes. Rows that need a probe
    (`source_clamped`, `source_origin_assumed`) therefore appear only after opening, and the
    lavfi fixtures prove them (§12).
  - With `code`, it pages up to 64 sites of that code from `after`.
  - Responses are ≤ 6 KiB, about 2 KiB typical.
- **`get_import_report {}`** returns `{revision, records}`. IX1 writes at most one record (≤ 4
  KiB, about 1.5 KiB typical).

*Refusals* carry a typed `code` and one sentence: `not_found`, `remote_path`,
`unsupported_container`, `unsupported_format_version`, `too_large`, `too_deep`, `too_many_items`,
`too_many_tracks`, `malformed` (with a JSON pointer or XML `line:col`), `dtd_not_allowed`,
`ambiguous_timeline` (≤ 16 names), `project_rate_required`, `unsupported_rate`, `unrepresentable`,
`nothing_to_import`, and `outside_roots` once AW1 file roots exist.

*Opening (ruling 1).* Agents only inspect in IX1, and there is no `request_open_interchange`.
AW1's `open_project { path }` (`docs/AW1-HEADLESS-KINEWRIGHT.md:560`) is the only way in. When AW1
lands it accepts interchange paths with these options. Until then the person opens the file and the
agent reads `get_import_report`.

*Bytes.* The served quad stays at 7 tools / 5,660 B (M36), because the served tools embed no
`Operation`. The registry grows by:
- **the two capabilities, about 1.5 KB.** `inspect_interchange` is about 650 B of schema and 350 B
  of description. `code` is a plain string, so it adds no enum bytes. `get_import_report` is about
  60 B and 220 B. The two directory rows are about 220 B.
- **`Track.name` (ruling 3), about 90 B on each `Operation`-embedding tool**, through `AddTrack`'s
  `Track`. This is an estimate.

The slice measures both in the M36 ledger.

## 11. Parsers and untrusted input

**OTIO: `serde_json =1.0.151`** (workspace pin).
- One derived `Node` holds every supported field, all optional, plus `OTIO_SCHEMA`.
- Unknown fields are skipped and duplicate fields fail. `metadata` is counted through `IgnoredAny`.
- The default recursion limit of 128 bounds depth.
- OTIO writers can emit `NaN` and `Infinity`, which are not JSON. serde_json refuses them, and the
  import reports `malformed` with the JSON pointer; a fixture covers this.
- A newer layout schema (`Timeline`, `Stack`, `Track`, `Clip`, `Gap`, `Transition`) is refused,
  since its timing may differ.
- An unknown or newer marker, effect or reference is dropped and counted. An unknown item keeps its
  duration as a gap.

**FCPXML: `roxmltree =0.20.0`**, already locked through `usvg 0.45.1` (egui_extras SVG), so it needs
no download.
- It is pure Rust with no dependencies. It builds a read-only DOM over the input and does no I/O, so
  no external entity is fetched.
- The node cap is passed as `ParsingOptions::nodes_limit`, so the parser stops during parsing, not
  after building the tree. The slice confirms the field exists in 0.20.0; if it does not, the slice
  raises the pin before writing any code.
- A pre-scan refuses any DOCTYPE internal subset or `<!ENTITY` (`dtd_not_allowed`). `allow_dtd` is
  then set for FCPXML's bare `<!DOCTYPE fcpxml>`.
- The Mirage repository parses Premiere XML with `allow_dtd: false` and a 128-level check
  (`crates/premiere_file/src/format/graph/mod.rs:229-245@cdea27e`).
- A DOM beats quick-xml streaming here, because `ref` ids resolve across resources and compounds.
- The walk uses an explicit stack.

**No other crates.** There is no zip support (no `.otioz`). File-URL decoding and locator
classification are about 80 hand-written lines, and they are fuzzed.

| Limit | Value | Breach |
|---|---|---|
| Main file bytes (read with `take`) | 128 MiB | `too_large` |
| JSON depth / XML depth / nest depth | 128 / 128 / 8 | `too_deep` |
| Parsed items / Kinewright clips / media / XML nodes (during parse) | 250 000 / 50 000 / 20 000 / 4 000 000 | `too_many_items` |
| Tracks per kind, lanes included | 128 | `too_many_tracks` |
| Project and asset fps | 1,000 | `unsupported_rate` |
| Names and text; `ref` graph | 4 KiB, then truncated; cycle check | A row; `malformed` |
| `media_roots` | 8 roots, depth 4, 100 000 entries each | the walk stops; names are listed as missing |
| Stat / probe open / hash stall | 2 s / 10 s / 10 s | `media_timeout` |

## 12. Proof

**Fixtures.** About 40 small files are authored for this repository in
`crates/kinewright-interchange/tests/fixtures/`. There is one per §3 row and format, plus one per
named case below. They come in two dialects:
- Resolve-like OTIO: a 01:00:00:00 start, float 23.976 rates, named clips with metadata, `file://`
  URLs, and dual-mono.
- FCP-like FCPXML 1.13: lanes, roles, compounds, centred transitions, anchors on retimed parents.

Media is generated at test time with the existing lavfi fixture pattern
(`crates/kinewright-media/src/in1_fixtures.rs`). No third-party sample file is copied. Each
fixture's `Document` and `ImportRecord` are committed as golden JSON.

Named cases:
- a named Resolve clip that is exact (finding 3);
- a dissolve beside a nest, which becomes a cut (finding 4);
- a retimed nest with markers (finding 5);
- an anchored clip at `offset="3602s"` on a spine clip with `start="3600s"`, and a `timeMap` clip's
  in-point (finding 6);
- OTIO A/V pairing, a split edit and dual-mono (finding 10);
- more than 8 codes, with dropped codes hidden past the cap (finding 9);
- `NaN`/`Infinity` (finding 11);
- cut snap at 23.976 → 60 fps with a seven-frame IR span and a zero asset in-point;
- cut snap at 23.976 → 60 fps with a two-frame IR span, a zero asset in-point and no
  positive shorter representation;
- cut snap at 10% speed with a 30 fps asset, a 60 fps project and a 39-frame IR span;
- constrained cut snap at 10% speed with a 30 fps asset in a 60 fps project. Two clips
  abut at frame 38 and the second ends at 39. The first clip's achievable ends are 20 and
  40, so the limit at 39 decides the cut at 20 and the report records 18 frames;
- an available media range ending at frame 24 with a requested end at frame 240, reporting
  a 216-frame snap instead of a source-grid-only bound;
- retimed source-end drift over 100 seconds of 1.004× playback at 30 fps;
- an OTIO track with red frames 0–24 and blue frames 24–48, windowed to 24–48, importing
  blue alone at frame zero;
- one 24-frame clip followed by a 24-frame gap, reporting a 24-frame trimmed tail;
- a 31.52-second FCPXML sync clip over a 101.88-second inner spine containing a 41.88-second gap
  and a 60-second clip;
- a transition that consumes the whole entering clip, leaving no body;
- the largest admitted rate, 1,000 fps, scaled to 1,000% speed within `Rational`'s
  `u32` terms (`time.rs:138-141`).

**Media-dependent rows.** Probing, `source_clamped`, `source_origin_assumed`, A/V sync and
`media_timeout` are proven by the lavfi fixtures, never by the corpus, whose files come without
media. The timeout cases use a `LocatorFs` double that blocks.

**Corpus (correction 1).** The lead downloads openly licensed public timelines into an untracked
directory and treats them as untrusted input. "Real" excludes test-suite fixtures, such as the OTIO
repository's sample data.
- A test reads the directory from `KINEWRIGHT_IX_CORPUS=<dir>`, prints a counts table and commits
  nothing.
- No `media_roots` are searched, and remote locators stay untouched.
- **Exit:** at least 20 real timelines across both formats open with no panic and no untyped
  refusal, and the lead reads the tables.
- The spec gaps of §7.4 are settled only from a sample that carries them.

**Properties** (proptest is already a dev-dependency):
- **Mixed rate (finding 1).**
  - Assets at 23.976, 25, 30 and 60 fps, plus 30 fps audio-only assets, in 24, 50 and 60 fps
    projects, with random starts, spans and dissolves.
  - The lowered document always validates. Clips on the same track never overlap, and clips
    that abut in the IR never acquire a gap at the cut.
  - Check the first and post-gap starts, snapped cuts, 100% source ends and retimed
    source-end drift against their separate §4.2–4.3 bounds.
  - Check both tiers of the snapped-cut bound, using the tighter tier when the
    nearest achievable cut is admissible.
- **Round trip.** An IR in the N subset, written by a test-only OTIO or FCPXML writer and imported,
  equals the generated document.
- **Determinism and safety.** The same bytes give the same document bytes, and no input panics.
  Every successful import validates, and the accounting identity holds.
- **Report size.** Generate worst-case names and sites, including control characters at the
  maximum name length. Every record the importer writes fits in 4 KiB of serialized JSON
  and passes `ImportRecord::validate`.
- **Topmost opaque item (finding 14).** A pure check over the IR and the `Document`: at each frame,
  the topmost opaque item in the IR is the topmost clip in the `Document`, with the same media and
  the same source frame (±½). It runs on every fixture and on generated nests. Compare topology at
  the decided cuts when a cut snaps. The mixed-rate property measures the snap against the IR.
  Exempt containers that were gapped under `nest_retime_unsupported`.
- **Nest order (finding 14).** Inner nest layers keep their order when lane 1 is busy at the same
  time (nested nests, spill lanes), including when an anchored clip shares the lane block.

**Locator safety (finding 2).** Hostile fixtures cover `file://evil.example/s/a.mov`,
`\\host\share\a.mov`, `\\?\UNC\…`, `\\.\…`, `//host/a.mov`, `file://localhost//host/share/a.mov`,
file URLs with encoded slashes and backslashes, and a remote agent `path` and root.
A recording `LocatorFs` asserts that each one is never statted, opened or probed. Each ends as
an offline asset or a `remote_path` refusal.

**Fuzzing.**
- Proptest mutates fixture bytes and generates structure: 256 cases per parser in the fast tier,
  50 000 in the slow tier (`ci/slow-tests.txt`, `slow-tests` feature as in
  `crates/kinewright-media/Cargo.toml:14-17`).
- `cargo fuzz` targets `fuzz_otio`, `fuzz_fcpxml` and `fuzz_locator` live in
  `crates/kinewright-interchange/fuzz/`, outside the workspace with their own lockfile, so
  `libfuzzer-sys` never ships. Each runs for at least 1 CPU hour before IX1 closes, and every crash
  becomes a fixture.
- Hostile inputs must each end in a typed refusal or an offline asset: billion laughs, an external
  entity, 10⁶-deep nesting, a node count over the cap, NaN/∞/0/negative rates, OTIO time at `1e300`,
  rationals near 2⁶³,
  duplicate ids, `ref` cycles, and `..` locators.

**Edit tests.**
- **Relink (finding 8).** Relinking an estimated asset writes the candidate's kind, duration and
  resolution. Relink refuses a candidate whose kind does not suit the asset's tracks or whose
  duration falls short.
- **Hand-edited record (finding 8).** A project with an oversized or inconsistent record is refused
  on load.
- **Imported dissolve (finding 7).** A timeline-UI test trims and moves an imported dissolve. The
  body trims with its audio, and the head stays put. The move splits head and body as reported. The
  document stays valid throughout.

**Measured limits.** Generated 1k-, 10k- and 50k-clip timelines in both formats, release build,
Riel's desktop, with dhat (`kinewright-project` dev-dependency `=0.3.3`) for peak heap. Budgets for
`prepare` without hashing:
- OTIO at 10k clips (about 30 MB): ≤ 1.0 s and ≤ input + 64 MiB;
- FCPXML at 10k clips (about 25 MB): ≤ 1.5 s and ≤ 4 × input + 64 MiB;
- 50k clips: ≤ 5 s, or the cap falls.

Pins are the measured value plus 25% (`docs/AW1-HEADLESS-KINEWRIGHT.md:649`). A cap that misses its
budget is lowered; the budget is never raised. `Document::asset` is a linear search
(`model.rs:1501`), so `validate` is O(clips × assets). It is measured at 10k clips × 2k assets and
indexed first if it takes over 200 ms.

**Exit gates.**
- Every §3 row and named case has a golden fixture.
- The mixed-rate, topmost-opaque and nest-order properties pass.
- The hostile and locator sets end as typed.
- The corpus gate passes.
- Budgets are pinned, and the M36 ledger records the registry bytes, including `Track.name`.
- The served quad is unchanged.
- Tests, clippy and fmt pass.

## 13. Out of scope and later

- **IX-T1: a native two-sided same-track transition.** It replaces `lower_same_track_transition`
  and the head/body split, and it lands before IX2, because `.prproj` has a dissolve on nearly every
  cut.
- **CORE-T1: a clip's timeline span is authoritative, with a sub-frame source end.** It removes
  the snap and lets a person cut 30 fps footage on any frame of a 60 fps timeline. It lands
  with IX-T1, before IX2.
- **Export**, seeded by the §12 test writer.
- **IX2 `.prproj`:** exact ticks into this IR, the same lowering and report, and a cap on
  decompressed size, missing in the Mirage repository's
  `crates/premiere_file/src/format/reader.rs:70-88@cdea27e`.
- **Containers:** `.otioz` (ruling 5), AAF and EDL.
- **Import into the open project** as one undo step. It needs one new operation that inserts
  tracks and assets and validates once; `imported_from` and `estimated_assets` are lists already.
- **A track rename operation.**
- **Effects, colour and transforms** beyond §3, and a start-timecode field.

## 14. Rulings and open questions

Lead rulings (2026-11-17):
1. **Agent opening:** inspect-only in IX1. There is no `request_open_interchange`; AW1's
   `open_project` is the only way in.
2. **Transition lanes:** accepted for IX1, behind one function. IX-T1 lands before IX2.
3. **Track names:** `Track.name`, optional, ≤ 64 bytes, skipped when absent. A single FCPXML role
   names its track. `track_names_dropped` is only for truncation and mixed roles. Registry bytes are
   measured in the M36 ledger.
4. **Marker colours:** Riel's call, pending. The design writes a label prefix behind one switch.
5. **`.otioz`:** out of IX1.
6. **Cut snap.** Apply the limits first and snap to the nearest admissible cut on the
   outgoing clip's source grid (§4.2).
7. **Composition windows.** The OTIO parser applies every composition's window (§2).
8. **Trailing gaps.** Report the timeline tail removed beyond the last clip (§6.2).
9. **Locators.** Classify file URLs on the final decoded native path (§8.1).
10. **Report size.** Bound the serialized record and trim samples before rows (§7.5).
11. **Windows in both formats.** FCPXML containers use the same window function as OTIO (§2).
12. **Rate cap.** Refuse project and asset rates above 1,000 fps (§4.1).
13. **Whole-clip transitions.** Omit an empty entering body (§5.3).
14. **Gapped subtrees.** Count all their items as dropped (§5.2).
15. **Anchors beside nests.** Size lane blocks for concurrent anchors (§5.1).
16. **Stuck stats.** Replace expired stat workers up to the abandonment cap (§8.2).

Open questions:
1. **Marker colour switch.** Should `MARKER_COLOR_POLICY` ship as `LabelPrefix` or `Drop`? This is
   Riel's call (ruling 4).
2. **Mapped network drives on Windows.** IX1 treats them as local, so a mapped share is statted
   under the deadlines. Classifying them as remote would need a drive-type query, and possibly a
   new Windows binding. Should IX1 do that, or leave mapped drives as the person's own trust
   decision?
