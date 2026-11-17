# IX1 — Opening OTIO and FCPXML timelines

Status: design for review, 2026-11-16. First slice of the interchange programme (IX), approved by
Riel today. Later slices: IX2 `.prproj` import with a loss report; IX3 Lightroom presets and `.cube`
LUTs as looks; After Effects after the motion work. This document designs IX1 only. IX1 opens an
OpenTimelineIO (`.otio`, `.otiod`) or FCPXML (`.fcpxml`, `.fcpxmld`) timeline from Resolve, Final
Cut or Avid as a Kinewright project, with a small typed loss report. Nothing is imported partially.

## 0. Decisions

1. Opening creates a **new, unsaved project** through the File > New path (`app.rs:1849`).
2. A new pure crate, **`kinewright-interchange`**, parses and lowers: bytes in, `Document` and
   report out. A new `kinewright-project::interchange` module reads files and probes media.
3. Both formats parse into **one IR with exact rational time** (§2); one lowering builds the
   `Document`. IX2 adds a `.prproj` parser and reuses the lowering and the report unchanged.
4. Nothing is created until **`Document::validate`** (`model.rs:1643`) and the report check pass.
5. Nests, compounds, multicam and connected clips **flatten onto tracks**. A same-track dissolve
   becomes a head clip on a **transition lane**. Both rate A.
6. The loss report is an **`ImportRecord`** in `Document.imported_from`: ≤ 4 KiB, ~1.5 KiB typical.
7. **Missing media never fails an import.** It becomes an offline asset; M41 relink is relaxed for it.
8. Agents get **two registry capabilities**, `inspect_interchange` and `get_import_report`. The
   served surface stays at seven tools. Agent-driven opening arrives with AW1's `open_project`.
9. **No new crate downloads**: `serde_json` (pinned) for OTIO, `roxmltree =0.20.0` (already locked)
   for FCPXML. Hard caps; DTD internal subsets refused. Proof is measured and fuzzed (§12).

Unless shown otherwise, `model.rs`, `time.rs`, `operation.rs`, `transition.rs`, `effect.rs`,
`automation.rs` and `title.rs` are in `crates/kinewright-core/src/`; `app.rs`, `media_bin.rs` and
`timeline_ui.rs` are in `crates/kinewright-app/src/`.

## 1. Scope

In: OTIO JSON up to OTIO v0.18.1 (2025-11) — `Timeline.1`, `Stack.1`, `Track.1`, `Clip.1`/`.2`,
`Gap.1`, `Transition.1`, `Marker.1`/`.2`/`.3`, the four media references, `LinearTimeWarp.1`,
`FreezeFrame.1`, `Effect.1`, `SerializableCollection.1`, legacy `Filler`/`Sequence` — as `.otio`
and `.otiod`. FCPXML 1.8–1.14 (Final Cut Pro 12.0) as `.fcpxml` and `.fcpxmld` (1.10+); older is
refused (`unsupported_format_version`). One timeline per import (`timeline` picks). Out: `.otioz`,
EDL, AAF, xmeml, export, effects beyond §3.

## 2. Architecture

`kinewright-project::interchange::prepare(path, options)` reads the file under the cap, sniffs the
format and calls `interchange::{otio,fcpxml}::parse` to get an `ir::Timeline`. It then resolves media
(decode locators, stat, search `media_roots`, probe and hash) into a `MediaTable`, and calls
`interchange::lower` to get `(Document, ImportRecord)`. Both are validated, then held in memory as a
`PreparedImport` until the app commits it (§6) or the agent reads it (§10). The IR follows OTIO's
tree, which FCPXML and `.prproj` both fit. Absolute `at` fits FCPXML lanes and Premiere starts; the
OTIO parser sums trimmed durations, skipping transitions, as OTIO's `range_of_child_at_index` does.

```text
Timeline { name, start: Time, rate_hint, resolution_hint, root: Stack }
Stack    { tracks: Vec<Track> (0 = bottom), window: Option<Range>, markers, effects }
Track    { kind: Video | Audio, name, items: Vec<Placed>, markers }
Placed   { at: Time, item: Item, site: SourceSite }        // "V2#14" or an XML line
Item     = Clip { media, source: Range, time: Normal | Speed(Ratio) | Reverse(Ratio) | Freeze(Time)
                  | Variable{in,out}, gain, gain_keys, fades, enabled, markers, effects, name, av_group }
         | Gap { duration } | Transition { kind, before: Time, after: Time }   // placed at the cut
         | Nested { stack, window, time } | Title { text, duration } | Unsupported { what, duration }
Time     = exact rational seconds: i64 / u64, reduced, i128 arithmetic
```

Exact rationals hold Premiere ticks (`crates/premiere_file/src/schema/timing.rs:7@cdea27e`)
without Mirage's 0.5 ms rounding (`crates/fx_schema/src/time.rs:5@cdea27e`).

## 3. The mapping

N = faithful, A = approximated (bound stated), Ø = dropped.

| Concept | OTIO | FCPXML | Kinewright target | Rating |
|---|---|---|---|---|
| Timeline | `Timeline.1` | `project/sequence` | one `Document` (`model.rs:1439`) | N; others listed, `timeline` picks |
| Frame rate | inferred (§4.4) | `format@frameDuration` | `Document.fps` (`model.rs:1476`, `time.rs:53`) | N; OTIO floats snapped (§4.1) |
| Resolution | none standard | `format@width/height` | `Document.resolution` (`model.rs:1477`) | N FCPXML; A OTIO: modal probed size, else 1920×1080 |
| Start, drop frame | `global_start_time`; DF only in timecode strings | `sequence@tcStart`; `@tcFormat` | rebased to 0; frames exact, NDF display (`timeline_ui.rs:3998`) | A: offset in report; DF labels only |
| Tracks | `Track.1` kind, stack order, `name` | spine, lanes ±n, roles | `Track{kind}` in z order (`model.rs:574`, `:1440`); no name | N; Ø names, roles |
| Clip range | `source_range` − `available_range.start_time` | `@start` − `asset@start`, `@offset`, `@duration` | `Clip.source_range`, `timeline_start` (`model.rs:429-443`) | N on grid; A ≤ ½ frame off grid |
| A/V clip, split edit | separate tracks, unlinked | `asset-clip`, `audioStart`/`audioDuration` | linked pair (`model.rs:446-450`), as `media_bin.rs:617` builds | N |
| Gap; disabled | `Gap.1`; `enabled` | `gap`; `@enabled="0"` | empty span; `Clip.enabled` (`model.rs:484-489`) | N |
| Constant speed | `LinearTimeWarp.1` (§4.3) | two-point `timeMap`; `conform-rate` | `speed_percent` 10..=1000 (`model.rs:473-483`) | A: ≤ 0.5 points, duration exact; Ø audio (muted, `model.rs:477-478`) |
| Reverse, variable | negative scalar; other `TimeEffect` | falling or multi-point `timeMap` | none | Ø reverse (plays forward); A variable (average speed, ends exact) |
| Freeze | `FreezeFrame.1`, scalar 0 | constant-value `timeMap` (undocumented, corpus-checked) | `ClipContent::Freeze` (`model.rs:341`, `:393-398`) | N |
| Dissolve; fade from/to empty | `Transition.1` `SMPTE_Dissolve`; beside a `Gap` | `transition` "Cross Dissolve"; at a storyline end | lane head clip with `crossfade` `transition_in` (`model.rs:316-322`, `transition.rs:38`); `opacity` keys out (`effect.rs:1581`) | A: structure, ramp (§5.3) |
| Other transitions | `Custom_Transition` | wipe, push, slide… | same-direction descriptor (`transition.rs:37`), else crossfade | A, names reported |
| Audio crossfade | audio-track transition | audio side of `transition` | ramp in + `audio_fade_out_frames` (`model.rs:456-465`) | A: linear, ≤ 3 dB from equal power |
| Clip gain, keys | not standard | `adjust-volume@amount`; `param[@name='amount']` keys | `audio_gain_tenth_db` −600..=120 (`model.rs:451-455`, `:732-734`); `audio_gain_curve` (`model.rs:466-472`, `automation.rs:28-35`) | N ±0.05 dB; A clamped; A keys on frames, nearest interpolation |
| Audio fades; pan | — | `fadeIn`/`fadeOut`; `adjust-panner`, `audio-channel-source` | `audio_fade_in/out_frames` (linear); no clip pan | A: shape named; Ø pan, channels |
| Timeline marker | root-stack or track markers | — (all FCPXML markers sit on items) | `Marker{position,label,color_token}` (`model.rs:600-607`) | N; A: duration dropped, colour in label |
| Item marker | `Clip`/`Stack` markers, item time | `marker`, `chapter-marker`, `@completed`, item time, no colour; `keyword`, `rating` | pinned project marker; chapter, to-do as label prefix | A: stops following; Ø outside trim, keyword, rating |
| Nest, compound, multicam, sync, audition | `Stack.1` in a `Track` | `ref-clip` → `media/sequence`; `mc-clip`, `sync-clip`, `audition` | flattened onto lanes (§5.2); active angle or pick | A; Ø nest effects, other angles |
| Title | — | `title` | `ClipContent::Title`, ≤ 512 chars (`title.rs:118`, `:262`), default style | A |
| Still | image `ExternalReference` | image asset | `MediaKind::Image` freeze (`model.rs:73`) | N |
| Generators, image sequences, effects, colour | `GeneratorReference`, `ImageSequenceReference`, `Effect.1` | generator `video`, `filter-video`, `adjust-*` except volume | none in IX1 | Ø: named, counted; generators leave a gap |
| Names, metadata, item colour | `name`, `metadata`, `color` | `@name`, `metadata`, `note` | assets keep names | Ø, counted |

## 4. Time and rates

**4.1 Exact until the grid.** FCPXML times (`"1001/24000s"`, `"3600s"`) parse to exact rationals;
overflow is refused with `unrepresentable`. OTIO `RationalTime` is two doubles, and real files carry
rates such as 23.976024627685547 (OTIO issue #1822). A rate within 1e-6 of an integer becomes it.
Otherwise, with `k = round(r × 1.001)`, a rate within 0.005 of `k × 1000/1001` becomes `1000k/1001`
(the nearest integer rate is at least 0.024 away). Anything else is `unsupported_rate`. A value within
1e-6 of an integer is exact at its snapped rate; any other counts `subframe_rounded`.

**4.2 Lowering.** Each absolute boundary maps once, `frame = ⌊t × fps + ½⌋` in `i128`; durations are
differences of mapped boundaries, so neighbours share a boundary and drift cannot accumulate (the M0
rule, `docs/BUILDING.md:209-214`, `time.rs:276-290`). Source ranges map at the asset's rate after
the media start is subtracted. A boundary more than 1e-9 frame off the grid counts `subframe_rounded`
(bound ½ frame). Audio-only assets probe at 30 fps (`crates/kinewright-media/src/au6_sources.rs:1012`),
so sample-accurate FCPXML audio in-points round to 1/30 s; that 16.7 ms bound is reported.

**4.3 Speed.** OTIO does not define how a time effect relates to `source_range`; its layout maths
ignores effects. IX1 reads `source_range.duration` as the duration in the parent and consumes
`duration × time_scalar` of media from `source_range.start_time`; the corpus (§12) checks this
against Resolve. FCPXML speed is `Δvalue / Δtime` over the `timeMap` ends. Kinewright gets
`p = round(100 × |s|)` clamped to 10..=1000 (`speed_clamped`). The clip keeps its exact project
duration: `map_project_duration_to_source` (`time.rs:365`) picks the source end through M23's
effective rate (`model.rs:531-536`). When no exact end exists, the nearest shorter one leaves a
counted gap of at most one frame. The reported bound is `max |s − p/100|` in tenths of a percent.

**4.4 Project rate, start, drop frame.** FCPXML takes the sequence format. OTIO takes the
`project_fps` argument, else `global_start_time.rate`, else the duration-weighted most common video
clip rate, else refuses with `project_rate_required`. The timeline start is subtracted everywhere;
anything left before frame 0 is trimmed (`before_start_trimmed`). Drop frame is display-only in both
formats. Positions stay exact, but Kinewright shows nominal non-drop timecode
(`timeline_ui.rs:3998`), so labels differ (`drop_frame_labels`).

## 5. Structure: tracks, lanes, nesting, transitions

**5.1 Tracks and lanes.** IR tracks keep their z order. FCPXML video lanes become video tracks in
ascending order (−n … spine … +n); spine audio, then each audio-bearing lane, become audio tracks.
Tracks forbid overlap (`operation.rs:5306-5311`), so each source track owns *lane tracks* directly
above it. A clip that would overlap takes the lowest lane free over its whole span, and is never
trimmed to dodge the overlap. More than 128 tracks of one kind is refused (`too_many_tracks`).

**5.2 Nesting.** OTIO `Stack`s in tracks, FCPXML `ref-clip`/`sync-clip`, and the active `mc-clip`
angles and `audition` pick are flattened. The nest window trims inner items; inner positions shift
by parent position − window start (FCPXML: `t_parent = offset + (t_local − start)` per level);
speeds multiply and round once; nest effects drop; nest markers become project markers. Inner track
*i* uses lane *i* of the parent (lane 0 is the parent, empty while the nest plays), so tracks grow
with depth, not count. Depth ≤ 8 (`too_deep`), Mirage's limit
(`crates/premiere_file/src/schema/nested.rs:30-32@cdea27e`). Over is associative, so opaque,
normal-blend clips without nest effects look identical.

**5.3 Transitions.** A Kinewright transition is a transition-in on the entering clip over the
layers below (`docs/M20-TRANSITIONS.md`). Both formats keep the cut fixed and draw on handles. OTIO's
`in_offset` shows B before the cut and `out_offset` shows A after it; FCPXML exports centre
`transition` on the cut, so `before = cut − offset` and `after = offset + duration − cut`. A
dissolve at cut `T` lowers to:

```text
lane above P:              [B head T−before .. T+after, crossfade transition_in]
track P:   [A ........ T+after][B body from T+after ..........]
```

Audio follows on the audio tracks: B's head ramps in, and A gets
`audio_fade_out_frames = before + after`. B's head, body and audio share B's link group, so moves
treat them as one. Kinewright's ramp is `k/(n−1)`, within `1/(n−1)` of the mix per frame of a
linear dissolve. Gap → B is a `crossfade` on B in place; A → gap is `opacity` keys 100 → 0. Short
handles shrink the window, and the shortfall is counted (`transition_handles_short`).

## 6. Import shape and validation

`prepare` returns a `PreparedImport` held in memory. Open calls `create_project_session(name,
document, …)` (`app.rs:1301`) as `new_project` does, so the session exists with the whole document
or not at all. The project is untitled and locator-based; no media is copied. Save As writes the
`.kinewright`, and undo starts after the import. Before commit, `Document::validate` must pass (the
validator that load and every batch run, `operation.rs:5144`), and so must `ImportRecord::validate`
(caps, and `exact + approximated + dropped = items`). A failure is `lowering_failed` with the
`OpError` text: a bug and a test failure, never a partial project. A batch of operations is ruled
out, because `apply_batch` re-validates the whole document after every operation
(`operation.rs:604-606`); a 10 000-clip import of ~30 000 operations would be quadratic.

There are two core changes. The first is `Document.imported_from: Option<ImportRecord>`, skipped
when `None`, as `investigator` is (`model.rs:1470-1475`); no operation writes it, and no tool input
schema embeds `Document`. The second is the relink relaxation of §8.3.

## 7. The loss report

```rust
struct ImportRecord {                    // kinewright-core; written once, never by an operation
    format: InterchangeFormat,           // otio | fcpxml
    format_version: String,              // ≤ 16 B: "1.13", "Timeline.1"
    source_name: String, source_sha256: String, // file name only (≤ 128 B); main-file hash
    start_offset_frames: i64,            // the rebased start
    items: u32, exact: u32, approximated: u32, dropped: u32,
    media: MediaCounts,                  // assets, online, offline, remote
    estimated_assets: Option<Range<AssetId>>, // offline assets, contiguous (§8.3)
    rows: Vec<LossRow>,                  // ≤ 16, dropped first, then by count; rows_omitted: u32
}
struct LossRow { code: LossCode, fidelity: Fidelity /* approximated | dropped */, count: u32,
    bound: Option<LossBound> /* frames | speed_tenth_percent | tenth_db */,
    names: Vec<String> /* ≤ 2, ≤ 32 B */, at: Vec<LossSite> /* ≤ 2 first */ }
struct LossSite { source: String /* "V2#14" */, frame: i64, clip: Option<ClipId> }
```

An item is one source clip, gap, transition, marker, effect or nest. It is exact with no rows,
approximated with only A rows, and dropped with any Ø row. The worst case is 260 + 16 × 220 ≈ 3.8 KB,
under a 4 KiB cap that `validate` enforces. A typical import (6–8 rows) is about 1.5 KiB:

```json
{"format":"otio","format_version":"Timeline.1","source_name":"reel2.otio","items":1840,"exact":1702,
 "approximated":121,"dropped":17,"media":{"assets":212,"online":209,"offline":3,"remote":0},
 "rows":[{"code":"effect_dropped","fidelity":"dropped","count":14,"names":["Blur","Glow"],
   "at":[{"source":"V2#14","frame":1234,"clip":56}]}, …]}
```

`LossCode` is a closed snake_case enum; a slice that needs a new code adds it. IX1's codes:
start_rebased, rate_snapped, subframe_rounded, drop_frame_labels, before_start_trimmed,
nest_flattened, transition_lane, multicam_active_angle, audition_active_pick, track_names_dropped,
clip_names_dropped, speed_rounded, speed_clamped, reverse_dropped, variable_speed_averaged,
retimed_audio_muted, transition_type_mapped, audio_crossfade_linear, transition_handles_short,
gain_clamped, gain_keys_mapped, fade_shape_linear, pan_dropped, channel_map_dropped,
marker_duration_dropped, marker_color_as_label, item_marker_pinned, marker_outside_item,
keyword_rating_dropped, effect_dropped, title_style_default, title_text_truncated, generator_dropped,
image_sequence_dropped, unknown_item, metadata_dropped, media_offline, remote_url, source_clamped,
source_origin_assumed, relinked_by_name.

## 8. Media

**8.1 Locators and bundles.** OTIO reads the active reference (`media_reference`, or
`media_references[active_media_reference_key]` on `Clip.2`). FCPXML reads
`media-rep[@kind='original-media']@src` (1.9+) or `asset@src` (1.8). Accepted: percent-decoded
`file:` URLs (`localhost`, `/C:/` drives; UNC hosts become `\\host\share` on Windows, missing
elsewhere), absolute paths, and paths relative to the document or bundle root (`.otiod` writes
`media/<name>`, FCPXML may write `./Media/x.mov`). Other schemes (`http`, `https`, `s3`) are never
fetched: the asset is offline with `remote_url`. Neither IX crate links network code; a test asserts
it. `.otiod` is `content.otio` plus `media/`; `.fcpxmld` holds `Info.fcpxml` and locator data
(tracking shapes, drawings), dropped and named. Bundle media is used in place, never copied.

**8.2 Online media.** The existing media layer probes each regular file and hashes its SHA-256
eagerly (`crates/kinewright-media/src/decode.rs:46-47`), so every imported asset is verified as M41
requires. Probes are deduplicated by canonical path and run on at most 4 workers, with progress and
Cancel. Probe metadata wins over interchange metadata. A source range past the probed duration is
clamped (`source_clamped`). A missing media start is taken as 0; a range that then overruns counts
`source_origin_assumed`.

**8.3 Missing media.** `media_roots` are folders the person or agent names. They are searched by
exact file name, ≤ 4 levels deep, ≤ 100 000 entries; a name with several hits stays missing. Matches
are listed before commit and counted (`relinked_by_name`), never picked silently, which M41 forbids
(`docs/M41-OFFLINE-RELINK-CACHE-VISIBILITY.md:50`). Media still missing becomes an offline asset
with the path as written, an unknown fingerprint, and kind, fps, duration and resolution from the
interchange (FCPXML `asset`/`format`; OTIO `available_range`, else the furthest frame used). These
take the last ids, one range recorded as `estimated_assets`; the project stays valid, saveable and
M41-offline. M41 relink needs exact kind, fps, duration and resolution (`operation.rs:1923-1956`).
For an `estimated_assets` asset with an unknown fingerprint, IX1 relaxes this: kind must suit its
tracks, fps must still match (or source frames change meaning), duration need only cover every
clip's source end, and resolution comes from the candidate. `allow_unverified_source` stays
required, and the known fingerprint after relink ends the relaxation.

## 9. App surface

File > Open… (`choose_project`, `app.rs:1836`) gains a "Timeline (OTIO, FCPXML)" filter; a bundle
opens via its `content.otio` or `Info.fcpxml`, or by drop. `import_dropped_files` (`media_bin.rs:69`)
routes these files and bundles to the interchange path, as it routes `.cube` (`media_bin.rs:79`).
A modal dialog shows the source, a timeline picker, fps and resolution (editable for OTIO), N/A/Ø
counts and top rows, the media table with **Locate folder…** (fills `media_roots`), hashing progress,
**Open** and **Cancel**. Open adds a tab named after the timeline ("Imported reel2: 1702 exact, 121
approximated, 17 dropped"). Project > Import report shows the stored record.

## 10. Agent surface

There are two read-only registry capabilities, reached through `invoke_capability` and listed in
`INSPECTOR_TOOL_NAMES` (`crates/kinewright-agent/src/schema.rs:16`):

- **`inspect_interchange { path, media_roots?: [≤ 8], project_fps?: {numerator, denominator},
  resolution?: [w, h], timeline?: string, code?: string, after?: u32 }`** parses, stats media and
  lowers. It returns `{summary, record, missing: [≤ 8 {name, reason}], missing_omitted,
  media_probed: false}`. It never hashes, since a tool call must not read terabytes, so rows that
  need a probe (`source_clamped`, `source_origin_assumed`) appear only after opening. With `code`
  it pages up to 64 sites of that code from `after`. Responses are ≤ 6 KiB, ~2 KiB typical.
- **`get_import_report {}`** returns the project's `imported_from` and revision, or
  `{imported: false}`: ≤ 4 KiB, ~1.5 KiB typical.

Refusals carry a typed `code` and one sentence: `not_found`, `unsupported_container`,
`unsupported_format_version`, `too_large`, `too_deep`, `too_many_items`, `too_many_tracks`,
`malformed` (with a JSON pointer or XML `line:col`), `dtd_not_allowed`, `ambiguous_timeline` (≤ 16
names), `project_rate_required`, `unsupported_rate`, `unrepresentable`, `nothing_to_import`, and
`outside_roots` once AW1 file roots exist. Opening is project lifecycle, which AW1 designs
(`docs/AW1-HEADLESS-KINEWRIGHT.md:560`): once AW1 lands, its `open_project { path }` accepts
interchange paths and these options, with no new capability. Until then the person opens the file
and the agent reads `get_import_report`. The served quad stays at 7 tools / 5,660 B (M36). The
registry grows ~1.5 KB: `inspect_interchange` ~650 B schema + ~350 B description (`code` is a plain
string, so no enum bytes), `get_import_report` ~60 B + ~220 B, two directory rows ~220 B; the slice
measures them in the M36 ledger.

## 11. Parsers and untrusted input

**OTIO: `serde_json =1.0.151`** (workspace pin). One derived `Node` holds every supported field,
all optional, plus `OTIO_SCHEMA`; unknown fields are skipped, duplicates fail, `metadata` is counted
through `IgnoredAny`, and the default recursion limit (128) bounds depth. A newer layout schema
(`Timeline`, `Stack`, `Track`, `Clip`, `Gap`, `Transition`) is refused, since its timing may differ;
an unknown or newer marker, effect or reference is dropped and counted; an unknown item keeps its
duration as a gap.

**FCPXML: `roxmltree =0.20.0`**, already locked through `usvg 0.45.1` (egui_extras SVG), so no
download. It is pure Rust without dependencies, builds a read-only DOM over the input and does no
I/O, so no external entity is fetched. A pre-scan refuses any DOCTYPE internal subset or `<!ENTITY`
(`dtd_not_allowed`); `allow_dtd` is then set for FCPXML's bare `<!DOCTYPE fcpxml>`. Mirage parses
Premiere XML with `allow_dtd: false` and a 128-level check
(`crates/premiere_file/src/format/graph/mod.rs:229-245@cdea27e`). A DOM beats quick-xml streaming
because `ref` ids resolve across resources and compounds; caps bound it, and the walk uses an
explicit stack. No zip crate (no `.otioz`); file-URL decoding is ~40 hand-written, fuzzed lines.

| Limit | Value | Breach |
|---|---|---|
| Main file bytes (read with `take`) | 128 MiB | `too_large` |
| JSON depth / XML depth / nest depth | 128 / 128 / 8 | `too_deep` |
| Parsed items / Kinewright clips / media / XML nodes | 250 000 / 50 000 / 20 000 / 4 000 000 | `too_many_items` |
| Tracks per kind, lanes included | 128 | `too_many_tracks` |
| Names and text; `ref` graph | 4 KiB then truncated; cycle check | A row; `malformed` |

## 12. Proof

**Fixtures.** About 24 small files are authored for this repository in
`crates/kinewright-interchange/tests/fixtures/`, one per §3 row and format, in two dialects:
Resolve-like OTIO (01:00:00:00 start, float 23.976, `file://` URLs) and FCP-like FCPXML 1.13 (lanes,
compounds, centred transitions). Media is generated at test time with the existing lavfi fixture
pattern (`crates/kinewright-media/src/in1_fixtures.rs`). No third-party sample file is copied. Each
fixture's `Document` and `ImportRecord` are committed as golden JSON.

**Corpus.** Riel's own Resolve, Final Cut and Avid exports stay on his machine; a test reads
`KINEWRIGHT_IX_CORPUS=<dir>`, prints a counts table and commits nothing. IX1 closes when ≥ 20 real
timelines across both formats open without panic or refusal and Riel has read each N/A/Ø table. The
corpus also settles the two spec gaps: OTIO time-effect ranges (§4.3) and FCPXML freeze encoding.

**Properties** (proptest is already a dev-dependency). An IR in the N subset, written by a
test-only OTIO or FCPXML writer and imported, equals the generated document; that writer later seeds
export. The same bytes give the same document bytes, and no input panics. Every successful import
validates. The accounting identity holds, and each track's clips and gaps sum to its end.

**Fuzzing.** Proptest mutates fixture bytes and generates structure: 256 cases per parser in the
fast tier, 50 000 in the slow tier (`ci/slow-tests.txt`, `slow-tests` feature as in
`crates/kinewright-media/Cargo.toml:14-17`). `cargo fuzz` targets `fuzz_otio`, `fuzz_fcpxml` and
`fuzz_file_url` live in `crates/kinewright-interchange/fuzz/`, outside the workspace with their own
lockfile, so `libfuzzer-sys` never ships; each runs ≥ 1 CPU hour before IX1 closes, and crashes
become fixtures. Hostile cases (billion laughs, external entity, 10⁶-deep nesting, NaN/∞/0/negative
rates, rationals near 2⁶³, duplicate ids, `ref` cycles, `..`/UNC/`http` locators) must each end in a
typed refusal or an offline asset.

**Measured limits.** Generated 1k/10k/50k-clip timelines in both formats, release build, Riel's
desktop, dhat (`kinewright-project` dev-dependency `=0.3.3`) for peak heap. Budgets for `prepare`
without hashing, at 10k clips: OTIO (~30 MB) ≤ 1.0 s and ≤ input + 64 MiB; FCPXML (~25 MB) ≤ 1.5 s
and ≤ 4 × input + 64 MiB; at 50k clips ≤ 5 s, or the cap falls. Pins are measured + 25%
(`docs/AW1-HEADLESS-KINEWRIGHT.md:649`); a cap that misses is lowered, never the budget raised.
`Document::asset` is a linear search (`model.rs:1501`), so `validate` is O(clips × assets); it is
measured at 10k × 2k, and indexed first if over 200 ms.

**Exit gates.** Every §3 row has a golden fixture showing its rating; the hostile set refuses as
typed; the corpus opens; budgets are pinned; the served quad is unchanged; tests, clippy, fmt pass.

## 13. Out of scope and later

Export, seeded by the §12 test writer. IX2 `.prproj`: exact ticks into this IR, same lowering and
report, plus a decompressed-size cap Mirage lacks (`crates/premiere_file/src/format/reader.rs:70-88@cdea27e`).
`.otioz`, AAF, EDL. Import into the open project as one undo step (one new operation that inserts
tracks and assets and validates once). Effects, colour, transforms, track names, start timecode.

## 14. Open questions

1. **Agent opening.** Is inspect-only enough for agents in IX1, or should a GUI-confirmed `request_open_interchange` ship before AW1?
2. **Transition lanes.** Lanes split one source clip into a head and a body. Accept them, or design a native two-sided transition first?
3. **Track names.** Should IX1 add `Track.name` for OTIO names, FCPXML roles and later `.prproj`? It costs registry bytes on every track-embedding tool.
4. **Marker colours.** Is writing the colour into the label acceptable, given that `docs/DESIGN.md:82-84` forbids decorative per-marker colours?
5. **`.otioz`.** Resolve also writes `.otioz`. Is it needed in IX1, at the cost of a zip reader and an owned extraction folder?
