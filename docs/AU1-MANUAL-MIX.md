# AU1 manual mix

Status: final for implementation (2026-09-07), critic amendments A1–A23 folded in. The
numbers in this document are the contract; implementation follows them or amends this
file first.

AU1 is the first slice of the audio programme (roadmap "Audio programme"). It gives
every track a typed mix state (gain, pan, mute, solo), a deterministic track stage in
the shared mix processor, per-track/bus/master peak meters, a Mixer panel that works
while the transport is running, agent parity through the generated operation
vocabulary, and one read-only level-measurement tool.

## 0. Changes from implementation (2026-09-07)

Recorded as the code landed; each is a deliberate deviation from the text below, not a
defect.

- **§6.1 stem path.** `mix_audio` and `mix_audio_stems` share one private
  `mix_pass(document, range, settings, collect_stems)`; `mix_audio` passes `false` so
  export does not hold `tracks × samples` extra f32 stems. The export master and the
  measured master remain one function, one chunking, one limiter.
- **§7 item 16.** "A sine that starts at frame 15 reads lower than the full-range figure"
  is unobservable under BS.1770 gating (silent blocks are gated out). The test proves the
  window with `report.range`, exact `sample_frames` per stem, the late sine reading `None`
  in `0..10` and `Some` in `10..20`, the steady sine reading identically in both windows,
  and an empty clamped range returning an error.
- **§7 item 18.** With no audio clips the cursor lands on frame 5 with or without preroll,
  so the test asserts the extracted `needs_seek_preroll(document, from)` predicate
  (`false` for a track-mix-only document, `true` once a bus exists) plus the cursor check.
- **§3.4 on more than two output channels.** Channels ≥ 2 use the steady `gain` without
  a ramp (the contract gives ramp state to channels 0/1 only), so on a surround device
  those channels step while L/R ramp. A latent click source on such devices; a §5.4 limit.
- **§6.2 text summary.** The report text starts with a header line
  `mix_levels range={start}..{end} any_solo={} lufs/peak in hundredths`; bus lines read
  `bus {id} "{name}" lufs={} peak={}` and the master line `master lufs={} peak={}`.
- **§6.2 `render_clip_info`.** The `track_mix=` line is appended on all three return
  paths (title, freeze, media), since every clip has an owning track.
- **§5.1 layout.** Amended in place after the first build measured the stacked strip at
  374 px: meters sit beside the fader, `M`/`S` share one row, the Mixer owns a
  `mixer-dock` panel (320 default / 260 minimum), and a strip fits in 240 px (measured:
  202 px plain, 216 px with `NO AUDIO`, 228 px with `MUTED BY SOLO`). At 72 px the
  caption row cannot also hold `NO AUDIO`, so that label sits on its own line under the
  caption; `MUTED BY SOLO` is suppressed for a track that already says `NO AUDIO` (the
  more specific cause of the same silence). The first app review found the double-click
  reset dead on the rail (drag-only sense) and harmful on the readout; it was replaced by
  the conditional `Reset` button; the second app review measured the 300 px dock at only
  242 px of strip room, so the dock became 320/260 and the budget 240 px. Readouts parse
  the unit they display (`parse_gain_db`, `parse_pan`) and commit typed values once, on
  Enter or blur; the inspector's clip-gain readout gained the same parser.
  `MUTED BY SOLO` (13 characters) broke DESIGN.md's twelve-character micro-caps cap and
  wrapped at 72 px, and `SOLO MUTED` (77 px) still wrapped; the label is `SILENCED`
  (eight characters, one line).
- **§6.1 measurement memory.** `measure_mix_levels` holds one f32 stem per track and bus
  for `0..range.end` on top of the per-track decode buffers: roughly
  `(2 × tracks + buses) × 48 000 × 2 × 4` bytes per second of `range.end`, about 1.8 GB
  for four tracks over ten minutes. Acceptable for an on-demand measurement; a per-chunk
  loudness accumulator is the AU3 remedy when loudness metering becomes continuous.
- **Meters after a clip ends (first media review).** A track absent from a chunk now
  records `[0, 0]` into its slot, so its bar falls when its last clip ends; a ramp keeps
  advancing while the track is absent, so a mute during a gap does not blip at the next
  clip start.
- **Registry figure.** After AU1 the internal capability registry is 126 tools at
  1,303,967 B (input schemas 1,186,449 B, descriptions 96,840 B); the served surface is
  unchanged at 7 tools / 5,660 B.

## 1. In scope and out of scope

In scope:

- `TrackMix` entries in `Document.audio_mix.tracks`, one `SetTrackMix` operation.
- The track stage inside `AudioMixProcessor::mix_chunk` (gate → gain → balance pan),
  identical in playback and export, deterministic summation order.
- A live track-mix update path so mixer edits do not stop playback.
- `MixMeters` telemetry (per track, per bus, master) through `Playback::mix_peaks`.
- `Analysis::mix_levels` and the agent inspector `get_audio_levels`.
- The Mixer tab in the material strip, track-header M/S toggles, `Ctrl+Shift+M`.
- Compact state rendering of track mix in `get_timeline_state` and `get_clip_info`.
- Documentation: this file; `README.md` audio bullet (line 36); `CHANGELOG.md`
  `### Added` under `[Unreleased]`; `MEDIA-POLICY.md` "Playback audio mixdown" (stage
  order, document-order sum, fill target and latency — every "two seconds" statement);
  `ROADMAP-AND-WORKFLOWS.md` (audio row, near-term sequence entry, and the new "Audio
  programme" section); `DESIGN.md` (mixer tokens, M/S states, track-label width, mixer
  component rules); `M36-AGENT-RUNTIME-EFFICIENCY.md` registry table row.

Out of scope (named deferrals, later AU slices):

- Editing bus effects, bus gain faders, adding/removing buses by hand (AU2).
- Master fader, master gain state (AU2).
- Loudness meters beyond peak, loudness targets, normalization on export, audio QC;
  QA (`qa.rs`) does not consider mute/solo — a fully muted timeline still passes
  `no_audible_media` until AU3. (AU3 delivered all of these; see
  `docs/AU3-LOUDNESS-AND-DELIVERY.md`.)
- Track gain automation, clip gain envelopes (AU4).
- Pan modes other than the balance law (constant-power is an AU2/AU4 mode), surround,
  pan automation.
- Persisting mixer layout across runs; per-track colour or naming.
- Undo/redo of a track-mix edit keeping playback running (falls back to the existing
  stop-and-reseek path, §5.4).

## 2. Core model

### 2.1 `TrackMix`

```rust
/// Per-track mix state (AU1 §2.1). An absent entry is neutral.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TrackMix {
    pub track: TrackId,
    /// Integer tenths of a decibel, inclusive range TRACK_MIX_GAIN_MIN..=TRACK_MIX_GAIN_MAX.
    #[serde(default, skip_serializing_if = "i32_is_zero")]
    #[schemars(default)]
    pub gain_tenth_db: i32,
    /// Integer percent, -100 (hard left) ..= 100 (hard right), 0 centre (AU1 §3.1).
    #[serde(default, skip_serializing_if = "i32_is_zero")]
    #[schemars(default)]
    pub pan_percent: i32,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    #[schemars(default)]
    pub mute: bool,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    #[schemars(default)]
    pub solo: bool,
}

pub const TRACK_MIX_GAIN_MIN: i32 = -600;
pub const TRACK_MIX_GAIN_MAX: i32 = 120;
pub const TRACK_MIX_PAN_MIN: i32 = -100;
pub const TRACK_MIX_PAN_MAX: i32 = 100;
```

- `TrackMix::neutral(track)` = all zero/false. `TrackMix::is_neutral(&self)`.
- Constants live in `model.rs` and are re-exported from `lib.rs`. The clip-audio range at
  `operation.rs:3451` and the `audio_gain` descriptor keep their literal `-600..=120`; a
  core test asserts `TRACK_MIX_GAIN_MIN/MAX` equal the `audio_gain` descriptor min/max.
- `bool_is_false` is a new `const fn` beside `i32_is_zero` (model.rs:362-366) carrying the
  same `#[allow(clippy::trivially_copy_pass_by_ref)]`.

### 2.2 `AudioMix.tracks`

```rust
pub struct AudioMix {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(default)]
    pub buses: Vec<AudioBus>,
    /// AU1: per-track mix entries, sorted by track id, unique, never neutral when
    /// written by Core. Omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(default)]
    pub tracks: Vec<TrackMix>,
}
impl AudioMix {
    pub const fn is_empty(&self) -> bool { self.buses.is_empty() && self.tracks.is_empty() }
    pub fn track(&self, track: TrackId) -> TrackMix   // neutral when absent
    pub fn any_solo(&self) -> bool
}
impl Document {
    pub fn track_mix(&self, track: TrackId) -> TrackMix { self.audio_mix.track(track) }
    /// AU1 §3.1 gate: whether the track's post-stage signal reaches buses/master.
    pub fn track_audible(&self, track: TrackId) -> bool
}
```

Decision: the entries live in `audio_mix` rather than as fields on `Track`. `Track` has
no `Default` and is spelled out in 177 literals across the workspace; `audio_mix` is
already "the branchable, undoable audio routing and processing graph" and already
cascades on `remove_track`. `Document.audio_mix` keeps its
`skip_serializing_if = "AudioMix::is_empty"`, so a project without mix state serializes
exactly as before AU1 (byte-identical, tested).

`AudioMixer::open`'s preroll test (audio.rs:715) changes from
`!document.audio_mix.is_empty()` to `!document.audio_mix.buses.is_empty()`: preroll is a
property of stateful bus effects, not of the stateless track stage. A test opens a mixer
at frame 5 on a document with one track-mix entry and no buses and asserts no preroll
(`cursor_sample == frame_to_samples(5)` before the first chunk).

The two `AudioMix { buses }` test literals in `audio.rs` (`processor_document`,
`parity_document`) gain `tracks: Vec::new()`.

### 2.3 Operation

```rust
/// Replace the whole mix state of one track (AU1 §2.3). Idempotent full set.
SetTrackMix {
    track: TrackId,
    /// Integer tenths of a decibel in -600..=120.
    gain_tenth_db: i32,
    /// Integer percent in -100..=100; 0 is centre. Balance law, see AU1 §3.1.
    pan_percent: i32,
    mute: bool,
    solo: bool,
},
```

The variant is declared immediately after `SetTrackSyncLock` (operation.rs:88-92), so the
generated tool `set_track_mix` follows `set_track_sync_lock` in every ordered list.

Apply (`set_track_mix`): `MissingTrack` if the track does not exist; validate values; if
the resulting entry is neutral remove any existing entry, otherwise upsert; keep
`audio_mix.tracks` sorted by `track`. Any track kind accepts mix state (video tracks carry
A/V audio). New `OpError` variants:

| variant | message |
| --- | --- |
| `TrackMixGainOutOfRange { track, gain_tenth_db }` | `track mix gain on track {track} is {gain_tenth_db} tenth-dB, outside the inclusive range -600..=120` |
| `TrackMixPanOutOfRange { track, pan_percent }` | `track mix pan on track {track} is {pan_percent} percent, outside the inclusive range -100..=100` |
| `DuplicateTrackMix(TrackId)` | `track {0} has more than one mix entry` |
| `TrackMixUnsorted` | `track mix entries are not sorted by track id` |

`validate_document` gains `validate_track_mix(doc)` called immediately before
`validate_audio_mix` (operation.rs:3810): entries strictly ascending by track id
(`DuplicateTrackMix` when equal, `TrackMixUnsorted` when descending), every referenced
track exists (`MissingTrack(track)` — the same error the operation returns), gain and pan
in range. A stored neutral entry is accepted (Core never writes one; a hand-edited one is
harmless). `remove_track` also drops the track's entry. `apply_unchecked`
(operation.rs:869), `operation_tool_name` (schema.rs:250), and `operation_status`
(app.rs:1623) gain arms — the three exhaustive matches.

### 2.4 Undo, journal, branch

Snapshot undo needs no per-variant code. `DoBatchCoalesced` with key
`track_mix:{track}#{gesture}` makes one fader drag one undo entry (§5.1).

## 3. Mix law and the track stage

### 3.1 Stage order (normative)

```
clip shaping (gain, fades, transition ramps)      — unchanged, before mix_chunk
→ track stage: gate → gain → pan                   — AU1, first thing in mix_chunk
→ routing: unrouted tracks → master; bus tracks → bus; sidechain taps read the
  POST-track-stage signal
→ bus effects in order (unchanged) → master sum
→ master limiter clamp ±1.0 (unchanged) → master meter (unchanged)
```

Gate: `audible(t) = !mute(t) && (!any_solo || solo(t))` where `any_solo` is computed
over `document.audio_mix.tracks` (the whole document, not the tracks present in the
current chunk). A non-audible track contributes exact zeros to master, its bus, and every
sidechain that lists it. Solo on a track with no audio-bearing clips still silences every
other track (the editor asked for it; the panel shows the consequence).

Gain: `g = 10^(gain_tenth_db / 200)` as `f32`, the same expression as `db_gain`
(audio.rs:581-583).

Pan, balance law: `p = pan_percent as f32 / 100.0`; `pan_left = 1.0 - p.max(0.0)`,
`pan_right = 1.0 + p.min(0.0)`. Centre is the exact identity (`1.0`, `1.0`), hard left is
(`1.0`, `0.0`), hard right (`0.0`, `1.0`), `pan_percent = 50` gives (`0.5`, `1.0`). The law
never boosts, so a panned track cannot clip where the unpanned one did not. Balance rather
than constant-power is deliberate: centre must be the identity so a neutral document
renders bit-identically to pre-AU1, and a −3 dB centre would change every existing mix. A
mono source arrives from swresample as identical L/R at unity, so hard-left yields the
left channel at unity and the right silent.

Per-channel gain is folded once per chunk per track: `channel_gain[c] = g * pan[c]` for
`c ∈ {0, 1}` when `channels >= 2`; `channel_gain[c] = g` for `c >= 2` and for every channel
when `channels == 1` (a one-channel device hears no pan). Every sample is multiplied
exactly once: `out = in * channel_gain[c]`. A neutral track has `channel_gain == 1.0`, and
`x * 1.0 == x` in IEEE 754, so neutral output is bit-identical to a pass-through.

### 3.2 Determinism

`mix_chunk` iterates `document.tracks` order for the track stage and for the
unrouted-to-master sum (replacing the `HashMap` iteration at audio.rs:526-530). Tracks
absent from `track_buffers` are silent and skipped. Bus member and sidechain sums keep
`bus.tracks` / `ducking_sidechain_tracks` order. Playback and export therefore add the
same floats in the same order. This also changes export output by at most one ulp per
sample for documents with two or more simultaneously audible unrouted tracks (previously
run-to-run nondeterministic); no committed audio golden exists, and `CHANGELOG.md`
records it.

### 3.3 Processor shape

```rust
struct TrackStageRuntime {
    track: TrackId,
    audible: bool,
    /// Steady-state per-channel gain for channels 0 and 1; channels >= 2 use `gain`.
    target: [f32; 2],
    current: [f32; 2],
    ramp_start: [f32; 2],
    ramp_index: usize,     // == ramp_frames when settled
    gain: f32,
}
pub(crate) struct AudioMixProcessor {
    track_order: Vec<TrackId>,          // document.tracks order
    stages: Vec<TrackStageRuntime>,     // parallel to track_order
    staged: Vec<Vec<f32>>,              // post-stage scratch, one per track, resized per chunk, never freed
    meters: Option<Arc<MixMeters>>,     // None in export and measurement
    buses, routed_tracks, sample_rate, channels, project_fps   // unchanged
}
impl AudioMixProcessor {
    pub(crate) fn new(document, sample_rate, channels, meters: Option<Arc<MixMeters>>) -> Self
    /// AU1 §5.3: replace stage targets from a document whose tracks and buses are
    /// unchanged. Bus effect state and ramp positions are untouched (§3.4).
    pub(crate) fn update_track_mix(&mut self, document: &Document)
    pub(crate) fn mix_chunk(&mut self, track_buffers, start_sample, sample_frames) -> Result<Vec<f32>, MediaError>
    /// AU1 §6: mix_chunk plus post-stage / post-bus copies for measurement.
    pub(crate) fn mix_chunk_with_stems(&mut self, track_buffers, start_sample, sample_frames) -> Result<MixChunkStems, MediaError>
}
```

Stage tables are built from `document.audio_mix` and `document.tracks`; a track without
an entry gets a neutral stage. `update_track_mix` keeps `track_order` (it is only called
when the track and bus sets are unchanged, §5.3). Routing, bus, and sidechain sums read
`staged`, never `track_buffers` directly. The per-chunk scratch is within the existing
discipline (the mix path already allocates `master`, `signal`, and `sidechain` per chunk);
meter recording allocates nothing.

### 3.4 Live change ramp (playback-only transient)

`TRACK_MIX_RAMP_MILLISECONDS = 5`; `ramp_frames = sample_rate * 5 / 1000` (240 at 48 kHz).
Construction sets `current == target` and `ramp_index == ramp_frames` (settled).
`update_track_mix` sets `target` (a non-audible track has target `[0.0, 0.0]`); when the
new target differs from `current`, it stores `ramp_start = current` and `ramp_index = 0`.
While `ramp_index < ramp_frames`, sample frame `i` of the ramp uses
`gain_i = ramp_start + (target − ramp_start) * ((i + 1) as f32 / ramp_frames as f32)` per
channel; at `i == ramp_frames − 1` the stage stores `current = target` exactly. A retarget
mid-ramp restarts the ramp from the current interpolated value. Ramp state is per stage
and per channel. Export and a freshly opened playback mixer never ramp, so steady-state
parity is untouched; the ramp exists only to avoid clicks after a live mixer edit. It is
not document state and is not observable through any facet.

## 4. Meters

### 4.1 `MixMeters`

```rust
/// One lock-free peak slot per mix point (AU1 §4). Telemetry, not state.
pub(crate) struct MixMeters {
    tracks: Vec<(TrackId, MeterState)>,
    buses: Vec<(AudioBusId, MeterState)>,
    master: Arc<MeterState>,           // the engine's existing meter (engine.rs:379)
}
impl MixMeters {
    pub(crate) fn for_document(document: &Document, master: Arc<MeterState>) -> Self
    pub(crate) fn empty(master: Arc<MeterState>) -> Self
    pub(crate) fn peaks(&self) -> MixPeaks
    pub(crate) fn clear(&self)         // every slot including master
}
// core media.rs, beside AudioLoudness
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MixPeaks { pub tracks: Vec<(TrackId, [f32; 2])>, pub buses: Vec<(AudioBusId, [f32; 2])>, pub master: [f32; 2] }
```

Layout: every `document.tracks` entry in order, every bus in order, master. The engine and
the worker share `mix_meters: Arc<RwLock<Arc<MixMeters>>>` (the `export_document`
precedent, engine.rs:301/423). The worker installs `MixMeters::for_document` after
`AudioRuntime::open` succeeds in `start_playback` and installs `MixMeters::empty(master)`
in `pause()`, so `mix_peaks()` is empty whenever the worker is not playing.
`Playback::play/pause` clear whatever table is installed (read lock, atomics). The
processor records track/bus peaks itself through `meters: Option<Arc<MixMeters>>`
(`None` in export and measurement); the master slot keeps being recorded by
`limit_and_meter_audio_mix` exactly as today, so `output_peaks()` is unchanged.

Recording points, per chunk (overwrite semantics, like today):

- track slot: peak of the post-track-stage buffer (what the track contributes);
- bus slot: peak of the bus signal after its last effect;
- master: post-limiter, unchanged.

Not recorded during seek preroll (the meter is attached after preroll, as today).

### 4.2 Facet

```rust
pub trait Playback {
    ...
    fn output_peaks(&self) -> [f32; 2];            // unchanged
    /// AU1 §4.2: per-track, per-bus, and master post-stage peaks. Empty when
    /// nothing is playing. Default: empty so test doubles need no change.
    fn mix_peaks(&self) -> MixPeaks { MixPeaks::default() }
    /// AU1 §5.3: apply a document that differs only in `audio_mix.tracks`
    /// without stopping playback. Default: `self.set_document(doc)`.
    fn update_audio_mix(&self, doc: Arc<Document>) { self.set_document(doc); }
}
```

## 5. Human workflow

### 5.1 Mixer tab

`MaterialTab::Mixer` joins Timeline and Transcript in the bottom material strip (third
`selectable_value`, label "Mixer"); `KINEWRIGHT_SCREENSHOT_SHOW=mixer` selects it and
opens the strip (app.rs:317-323). `KeyAction::Mixer` on `Ctrl+Shift+M` opens the strip and
selects the tab (`KEYMAP` and `ALL_ACTIONS` go 20 → 21; the guard-test comment moves with
them). The top-bar "Timeline" toggle keeps its name.

Layout, left to right in a `ScrollArea::both()`: one strip per `document.tracks` entry in
order, a separator, one strip per bus in order, a separator, the master strip. New tokens
in `theme.rs` and `DESIGN.md`: `size::MIXER_STRIP_WIDTH = 72`,
`size::MIXER_FADER_HEIGHT = 120`, `size::MIXER_METER_WIDTH = 4`. When the Mixer tab is
active the bottom dock uses its own panel id `mixer-dock` with default height 320 and
minimum 260 (the Timeline/Transcript dock keeps `timeline-dock` at 240/160), so each tab
remembers its own height; the default dock leaves about 262 px for strips after margins,
tab row, and separator, and a track strip must fit in 240 px in its tallest state
(measured by a headless test; the tallest state measures about 236 px).

Track strip, top to bottom (amended after the first build measured the stacked layout at
374 px):

1. Caption `V{n}`/`A{n}` with the kind icon (as the timeline header); tracks with no
   audio-bearing clip add the micro caps label `NO AUDIO` in `TEXT_MUTED` on the next
   line; their controls stay enabled (mix state may be set ahead of media).
2. One horizontal group, `MIXER_FADER_HEIGHT` tall: on the left two vertical meter bars
   (L/R), `MIXER_METER_WIDTH` wide, same thresholds and colours as `draw_meter_bar`
   (0.8 / 0.95 → success / warning / danger), same `peak_to_meter_level`, same 0.9/s decay,
   level source `mix_peaks().tracks` while playing, else 0; on the right the vertical
   `egui::Slider` over `-600..=120`, `.integer()`, formatter `{:+.1} dB`. Live drag →
   `push_live(SetTrackMix, "track_mix:{id}")`; release and typed values → `push`. There is
   no double-click reset: egui 0.35 slider rails sense drag only, so a double click never
   reports as such and would write an arbitrary value. Tracks silenced by another track's
   solo show the micro caps label `SILENCED` in `TEXT_MUTED` directly under the meters
   (omitted when the strip already says `NO AUDIO`).
3. Pan: horizontal `egui::Slider` over `-100..=100`, formatter `L{n}` / `C` / `R{n}`, same
   live/discrete rule, same coalesce key (one gesture per strip at a time).
4. `M` and `S` toggles side by side, each `ICON_BUTTON` square, micro caps text. Inactive:
   `TEXT_MUTED` on no fill. Active M: `STATUS_WARNING` text on `SURFACE_ACTIVE` (a
   functional warning, the `FREE` precedent). Active S: `TEXT_PRIMARY` text on
   `SURFACE_ACTIVE`. No accent anywhere (DESIGN.md track-header rule).
5. A `Reset` small button, shown only while the track's mix is non-neutral (the
   inspector's audio Reset pattern), pushing one discrete `SetTrackMix` equal to
   `TrackMix::neutral(track)`.

Bus strip (read-only in AU1): name, `tracks=` list as `A2 A3`, effect chain names in order
(`gain → eq → compressor`), sidechain list, meter pair fed by `mix_peaks().buses`. A
`TEXT_MUTED` sentence at the bottom: "Bus controls arrive with AU2; the agent can edit
buses today." Master strip: label `MASTER`, meter pair fed by `mix_peaks().master` (the same post-limiter
slot `output_peaks()` reads).

The mixer reuses `InspectorEdits` (no new accumulator); `push` and `push_live` become
`pub(crate)`. Strips are free functions
`fn mixer_strips(ui, document, peaks: &MixPeaks, playing: bool, edits: &mut InspectorEdits)`
so the headless `ctx.run_ui` test pattern applies (§7 item 23). Submission goes through
`submit_inspector_edits`'s send rules (coalesced when keyed).

### 5.2 Track-header M/S toggles

`TRACK_LABEL_WIDTH` grows 76 → 96 (DESIGN.md timeline section records the new width). The
M/S column is 14 px wide at `lane.right() − space::TWO − ICON_BUTTON − space::TWO − 14`
(x = 40..54), leaving 8..36 for the caption and icon column; the two toggles are stacked
with `space::ONE` between them, centred on the lane, same visual rules as §5.1 item 5,
each returning `Some(SetTrackMix { .. })` with the flipped flag and the other three values
carried from `document.track_mix(track)`. One click is one discrete undo entry (the
existing `send_operations` path).

### 5.3 Live mixing

App side (`poll_background`, `Event::DocumentChanged` for the focused project):

```rust
fn is_live_track_mix_change(journal_command: Option<&JournalCommand>) -> bool
// true iff Some(Do(SetTrackMix)) | Some(DoBatch(ops)) | Some(DoBatchCoalesced{ops,..})
// where every op is SetTrackMix; false for None, Undo, Redo, and every other operation.
```

When true: call `self.playback.update_audio_mix(doc)` and skip `playing = false`,
`set_document`, `seek`, `request_frame` (also when paused: it keeps the paused frame).
Otherwise the existing path runs unchanged. Isolated in-app agent batches arrive as
`JournalCommand::DoBatch` and take the same live path; nothing there may add a
`set_document`.

Engine side: `Playback::update_audio_mix` stores `doc` into `export_document` (as
`set_document` does; it does not touch `clock` or `next_asset_id`) and sends
`Control::UpdateAudioMix(doc)`. The worker replaces `self.document`, calls
`audio.mixer.update_track_mix(&doc)` when a runtime exists, and does nothing else: no
pause, no clock change, no renderer clear, no LUT rebind (the document differs only in
`audio_mix.tracks`, which no video path reads). The worker does not verify that claim; the
app's predicate is the contract.

Latency: the output ring keeps `BUFFER_SECONDS = 2` of capacity, but the fill loop now
stops when the ring already holds `LIVE_FILL_MILLISECONDS = 1000` of audio
(`target_samples = sample_rate * channels * 1000 / 1000`). Justification: the worker serves
`Control::Thumbnail` synchronously with a seek decode budgeted at p95 250 ms
(`media_matrix_tests.rs` `COLD_SEEK_P95_BUDGET`), and `Worker::run` drains all queued
controls before it refills; 1 000 ms covers three budgeted seeks plus a present. In
addition `Worker::run` calls `audio.fill()` after each handled control while
`self.playing`, so a burst of thumbnails cannot starve the ring for longer than one decode.
A mixer edit is heard within ≈1 s plus device latency instead of up to 2 s. The fill loop
is a device-free function
`fn fill_ring(producer: &mut rtrb::Producer<f32>, pending: &mut Vec<f32>, pending_index: &mut usize, mixer: &mut AudioMixer, target_samples: usize) -> Result<bool, MediaError>`
(returns `false` when the mix is exhausted); `AudioRuntime::fill` calls it. The hands-on
smoke must be run with the timeline filmstrip visible and auto-scrolling.

### 5.4 Limits stated to the user

- Undo/redo of a mix edit during playback stops and re-cues (existing path).
- A device with one output channel hears no pan.
- Bus and master controls are not editable in AU1.
- A mixer edit is heard about one second after the gesture.
- On an output device with more than two channels, channels beyond the first two step
  instantly on a live change instead of ramping.

## 6. Measurement

### 6.1 Media

```rust
// core media.rs
pub struct MixLevelRequest { pub range: Option<Range<TimeCode>> }  // None = whole document
pub struct MixLevelReport {
    pub range: Range<TimeCode>,
    pub any_solo: bool,
    pub tracks: Vec<TrackLevels>,   // every document track in order
    pub buses: Vec<BusLevels>,      // every bus in order
    pub master: AudioLoudness,
}
pub struct TrackLevels { pub track: TrackId, pub kind: TrackKind, pub mix: TrackMix, pub audible: bool, pub bus: Option<AudioBusId>, pub levels: AudioLoudness }
pub struct BusLevels { pub bus: AudioBusId, pub name: String, pub levels: AudioLoudness }
```

All derive `Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema`.

`Analysis::mix_levels(&self, document, request) -> Result<MixLevelReport, MediaError>`
(default `Err(NotImplemented)`; `BaselineProofAnalysis` in `color_qc_ui.rs` delegates
explicitly). The engine implementation mixes at 48 kHz stereo from frame 0 through
`range.end` (the seek-preroll rule, so stateful bus effects match export exactly), keeps
only `[range.start, range.end)`, and measures each stem with `measure_loudness`. Track
stems are post-track-stage (a muted track reads silent, `integrated_lufs_hundredths:
None`); bus stems post-effects; master post-limiter. `range` is clamped to `0..duration`;
an empty clamped range is an error. Cost is one full mix pass over `0..range.end`, the
same class as `timeline_loudness`. `export::mix_audio` is refactored to share the stem
path (`mix_audio_stems(document, range, settings) -> MixStems`; `mix_audio` = master of the
whole range) so the export mix and the measured master are one code path. Master over
the whole document equals `timeline_loudness` bit-for-bit (tested).

### 6.2 Agent

`get_audio_levels` (read-only inspector):

```rust
struct AudioLevelsArgs {
    /// Optional project-frame range; defaults to the whole timeline.
    #[serde(default)] start_frame: Option<TimeCode>,
    #[serde(default)] end_frame: Option<TimeCode>,
}
```

Returns `success_structured` with the `MixLevelReport` plus `timeline_revision`, and a
text summary: one line per track
`track {id} {kind} gain={} pan={} mute={} solo={} audible={} bus={} lufs={} peak={}`,
one per bus, one for master (`none` for `None` measurements). No audio-bearing media in
range → a normal report with `None` measurements; invalid range → `error_text`. No
`CAPABILITY_KIND_OVERRIDES` entry (`get_` infers Inspector, runtime.rs:178).

`set_track_mix` is generated automatically. `schema.rs`: `operation_tool_name` arm;
description augmentation: " gain_tenth_db is an integer number of tenths of a decibel in
-600..=120; pan_percent is an integer in -100..=100 using a balance law (0 is an exact
identity, -100 silences the right channel, 100 silences the left); mute silences the track
everywhere including ducking sidechains; any solo silences every non-solo track. The
operation replaces all four values; sending neutral values removes the track's entry.";
`.idempotent(matches!(name.as_str(), "set_clip_audio" | "set_track_mix"))`.

Moved assertions: `tests/mcp_server.rs:2488-2494` → 126 / 76 with the message rewritten to
"AU1 adds set_track_mix and get_audio_levels"; `server.rs:19094` → 76; `server.rs:20960-20966`
registry bytes regenerated (record the new figure here and in `M36`; served stays 5,660 B
because the seven served tools do not embed the `Operation` schema); `schema.rs:731` gains
`"set_track_mix"` after `"set_track_sync_lock"`; `schema.rs:940` gains
`("set_track_mix", false)`; `schema.rs:896` guard gains a `SetTrackMix` spot check;
`contracts.rs:283` gains the round-trip entry after `SetTrackSyncLock`;
`INSPECTOR_TOOL_NAMES` becomes `[&str; 76]`.

Rendering: the track line becomes `track {id} {kind} sync_lock={} clips={}{mix}` where
`{mix}` is empty when neutral and otherwise ` mix=gain:{},pan:{},mute:{},solo:{}`.
`render_clip_info` appends a final line `track_mix=gain:{},pan:{},mute:{},solo:{}` only
when non-neutral. Existing goldens are unchanged because their fixtures are neutral.

## 7. Evidence

Core (`tests/contracts.rs` additions + new `tests/au1_core.rs`):

1. `SetTrackMix` joins the every-variant JSON round trip (after `SetTrackSyncLock`).
2. Bounds: gain −600/120 accepted, −601/121 rejected with `TrackMixGainOutOfRange`; pan
   ±100 accepted, ±101 rejected with `TrackMixPanOutOfRange`; missing track →
   `MissingTrack`; every rejection leaves the document equal to `before`.
3. Neutral set removes the entry; `audio_mix` is absent from the serialized document when
   only neutral entries were ever set (byte-identical to the serialization of the same
   document before any `SetTrackMix`).
4. Hand-edited: duplicate entry, descending entries, missing track, out-of-range values
   are rejected by `Document::validate` with `DuplicateTrackMix`, `TrackMixUnsorted`,
   `MissingTrack`, `TrackMixGainOutOfRange`/`TrackMixPanOutOfRange`; a stored neutral
   entry loads and validates.
5. `remove_track` drops the entry.
6. Ten `Command::DoBatchCoalesced` with key `track_mix:1#1` and increasing gains; one
   `Command::Undo` yields `initial`; a second `Command::Undo` yields `initial` again with
   the same revision; one `Command::Redo` yields the tenth document.
7. `TRACK_MIX_GAIN_MIN/MAX` equal the `audio_gain` descriptor min/max.

Media (`audio.rs` unit tests; DC-buffer processor tests need no ffmpeg):

8. Gain: −60 tenth-dB on a 1.0 buffer yields `10^(-0.3)` within 1e-6; +120 yields `10^0.6`.
9. Pan endpoints and midpoint as in §3.1 on a stereo buffer; `channels == 1` ignores pan;
   `channels == 4` pans channels 0/1 only.
10. Mute: contribution is exactly 0.0 to master and to a ducking sidechain (the ducked bus
    returns to unity within the release).
11. Solo: soloing track 1 zeroes track 2 even when track 2 is the only key in
    `track_buffers`; two solos both pass; solo on a track absent from the chunk still
    silences others.
12. Determinism: on a document with two unrouted tracks and no mix entries, `mix_chunk`
    over `{1: a, 2: b}` equals, with `==`, the test-local sum `a[i] + b[i]` computed in
    document order; the same processor over a map built in the other insertion order gives
    the same bytes; a processor built from the same document with an explicit neutral
    entry for track 1 gives the same bytes.
13. Ramp: after `update_track_mix` from unity to mute at 48 kHz, frames 0..239 are strictly
    decreasing, frame 239 onward is exactly `0.0`, and frame 119 equals `0.5` within 1e-6; a
    fresh processor with the same document produces exactly the post-ramp values.
14. Parity: `parity_document` is unchanged. A new `parity_document_with_track_mix` applies
    `audio_mix.tracks = [{1: gain −60, pan −30}, {2: pan 25}, {3: gain 30}]`. A new test
    `playback_feeder_mix_matches_export_through_the_track_stage` runs the nine-window
    1e-6 comparison and the frame-5 seek comparison on that document, and a second variant
    with `{3: mute}` and `{1: solo}` (which also silences bus track 2 and its sidechain). It
    additionally asserts `peak_in(&exported, 22..28)` is within 2% of
    `peak_in(&exported_neutral, 22..28) * 10^(0.15)` where `exported_neutral` is the
    unmodified `parity_document` mix.
15. Meters: a chunk through a two-track document with one bus records per-track
    post-stage peaks, the bus peak, and master; `MixPeaks` order equals document order;
    `MixMeters::empty` yields empty vectors.
16. `mix_levels` on generated sines: track LUFS shifts by the applied gain within 5
    LU-hundredths; a muted track reads `None`; master equals `timeline_loudness` exactly; a
    range `10..20` frames measures only that window (a sine that starts at frame 15 reads
    lower than the full-range figure).
17. With `rtrb::RingBuffer::new(48_000 * 2 * 2)`, a video-only two-second document mixer,
    and no device, `fill_ring` with a 1 000 ms target leaves `capacity − slots()` in
    `[target, target + 1024·2)` and returns `true`; popping 48 000 samples from the consumer
    and calling again tops it back up; repeated calls until `false` deliver exactly
    `2 s × 48 000 × 2` samples in total.
18. No preroll for a track-mix-only document (§2.2).

Agent:

19. Ordered tool-name list includes `set_track_mix`; description contains the ranges and
    "balance"; `idempotentHint` true; registry counts 50/76/126; served metrics unchanged;
    registry byte figure regenerated.
20. `render_timeline_state` golden unchanged; a fixture with `gain −60, pan 25, solo`
    renders the exact suffix; `render_clip_info` shows `track_mix=` only when non-neutral.
21. `tests/mcp_server.rs`: `prepare_edit_plan` with `{"op":"set_track_mix",...}` → commit →
    `get_timeline_state` shows the suffix; `get_audio_levels` on the generated sine media
    returns a report whose track LUFS moves by a −6 dB `set_track_mix` within 5 hundredths
    and whose muted track reads `None`.

App:

22. Builders: fader/pan/M/S → exact `SetTrackMix`; coalesce key `track_mix:{id}`;
    `is_live_track_mix_change` truth table (Do, DoBatch, DoBatchCoalesced all-SetTrackMix →
    true; mixed batch, Undo, Redo, None → false).
23. Keymap guard at 21 with `Mixer` reachable on `Ctrl+Shift+M`.
24. Headless paint of the mixer strips writes no operation and paints `MASTER`, each track
    caption, `M`, `S`, and `NO AUDIO` for an empty track; the track-header toggles emit the
    flipped flag with the other values carried.

Exit gate: 1–24 green in `cargo test --workspace` locally on Linux, `cargo fmt --check`
and `cargo clippy --workspace --all-targets -- -D warnings` clean; both CI operating systems
green after push; a hands-on Omarchy/Windows smoke of live mixing on a real device, with the
timeline filmstrip visible and auto-scrolling, is recorded by Riel.

## 8. Files

Core: `model.rs`, `operation.rs`, `lib.rs`, `media.rs` (facet + `MixPeaks`,
`MixLevelRequest`, `MixLevelReport`), `tests/contracts.rs`, new `tests/au1_core.rs`.
Media: `audio.rs`, `export.rs`, `engine.rs`, `lib.rs`, tests. Agent: `schema.rs`,
`render.rs`, `server.rs`, `tests/mcp_server.rs`. App: `app.rs`, `keys.rs`, `theme.rs`,
`timeline_ui.rs`, `inspector_ui.rs` (visibility), new `mixer_ui.rs`, `color_qc_ui.rs`
(delegate). Docs: this file, `README.md`, `CHANGELOG.md`, `ROADMAP-AND-WORKFLOWS.md`,
`MEDIA-POLICY.md`, `DESIGN.md`, `M36-AGENT-RUNTIME-EFFICIENCY.md`.
