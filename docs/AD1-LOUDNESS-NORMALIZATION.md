# AD1 — Loudness normalization onto the delivery contract

Status: implemented 2026-09-05, pending platform smoke. AD0 measured a
delivered file against a named target and reported the gain that would reach
it. AD1 closes that loop: the same target drives a normalization plan, the plan
is one typed operation that the person and the agent both apply, and the next
export's decoded-file verification is the proof. Nothing here is a limiter,
a meter, or a mixer; those remain AD2 and AD3 (`AD0-AUDIO-DELIVERY.md` §7).

## 1. The editor job

A cut measures −39.9 LUFS on export. Make it −14, hear it that way, and know
from the file that it is. Before AD1 the agent could do the first step with its
own numbers; the person could not, and neither could prove the result from the
decoded file.

## 2. One plan, two callers — `kinewright_core::audio_normalization`

```rust
pub fn plan_audio_normalization<E: Display>(
    document: &Document,
    tracks: &[TrackId],
    target: AudioDeliveryTarget,
    measure: impl FnMut(&Document) -> Result<AudioDeliveryMeasurement, E>,
) -> Result<AudioNormalizationPlan, AudioNormalizationError>
```

The recipe, the validation, and the predict-and-correct loop moved from the
agent crate into core, so the export dialog's button and the agent's planner
cannot drift. Core cannot render audio, so the measurement of a candidate
document is injected; both callers pass `Analysis::timeline_delivery_audio`.

### 2.1 The recipe (`normalization_bus`)

One `UpsertAudioBus` named `Delivery normalization` over the chosen tracks:

| Requested gain | Effects, in order |
| --- | --- |
| ≥ 0 | `audio_compressor` (threshold 0 / ratio 1:1 when the peak leaves room; otherwise a 4:1 threshold derived from the ceiling, gain, and peak; up to 24 dB makeup), then `audio_gain` for any gain past 24 dB, then `audio_limiter` |
| < 0 | `audio_gain`, then `audio_limiter` |

The limiter clamps at the target's true-peak ceiling minus
`LOSSY_CODEC_PEAK_HEADROOM_HUNDREDTHS` (2 dB): AAC overshoots the samples it is
given, and the decoded file is what AD0 judges. Gain is bounded to
`−60.00..=+36.00 dB`. The bus id and the first effect id are the document's
next free ids, so the operation is valid against the document it was planned
on and stale against any other.

### 2.2 The loop

Request `target − current`, apply to a scratch copy, measure, correct by the
residual, stop inside the tolerance or after `NORMALIZATION_MAX_ROUNDS = 4`.
The tolerance is the target's, floored at `0.25 LU`. The plan then fails typed
if the prediction is outside the tolerance or its peak (true peak when
measured, else sample peak) is over the ceiling.

### 2.3 Refusals — `AudioNormalizationError`

`target_gates_nothing`, `no_tracks`, `duplicate_tracks`, `missing_track`,
`not_an_audio_track`, `empty_track`, `track_already_mixed` (names the bus),
`silent`, `no_peak`, `gain_out_of_range`, `measurement`, `not_applicable`,
`out_of_tolerance`. Serialized with `code` as the tag; `code()` returns the
same string.

### 2.4 The plan

`AudioNormalizationPlan { operation, bus_id, tracks, target, current, predicted,
gain_hundredths_db, processing_ceiling_dbfs_hundredths, rounds, predicted_qc }`.
`predicted_qc` is `measure_audio_qc(target, predicted)`, so the caller reads
the same seven codes the export verification will.

## 3. The measurement facet

`Analysis::timeline_delivery_audio(&Document) -> AudioDeliveryMeasurement`
renders the mix in memory (48 kHz stereo, the same path as
`timeline_loudness`) and takes the AD0 measurement: integrated loudness,
sample peak, 4× oversampled true peak, loudness range. Default implementation
`NotImplemented`; `FfmpegMediaEngine` implements it.

## 4. The agent surface

- **`get_audio_qc` (new, read-only).** `audio_preset` optional
  (`measure_only` default). Publishes `stage: "timeline_mix_pre_encode"`, the
  analysis parameters, the target, and the full `AudioQcReport`; typed
  `audio_measurement_unavailable` when the mix cannot be rendered. The
  decoded-file stage stays on `get_export_jobs`.
- **`plan_audio_normalization`.** Gains `audio_preset`. With a preset the
  three explicit numbers are ignored; without one they become a `Custom`
  target with their original bounds, so the M40 prompts still run unchanged.
  The response keeps every pre-AD1 key and adds `audio_preset`, `target`,
  `current_measurement`, `predicted_measurement`, `predicted_qc`,
  `gain_hundredths_db`, `rounds`, `bus_id`, `tracks`. Refusals are typed with
  the core error's code.
- **`queue_export`.** Gains `audio_preset`; omitted, the profile default
  applies (`streaming` for the platform profiles, `measure_only` for
  `source_master`).
- Registry: 125 tools, 76 inspector names, registry metadata 1 285 751 B;
  the served surface is byte-identical to CC6/CC7 (7 tools, 5 660 B).

`AudioDeliveryPreset::Custom` exists for the legacy arguments only: it is not
in `ALL`, not offered in the dialog, and its own `target()` gates nothing.

## 5. The person path

When an export's `DECODED AUDIO` block shows a measured miss against a target
(`normalization_offer`: measured leg, integrated target set, `technical_pass`
false, gain reported), the dialog shows one button, `Normalize the mix to
<preset>`. It plans over every audio track that carries clips and is not
already on a bus (`audio_tracks_for_normalization`), sends the one operation
through the ordinary edit path (one undo entry), and writes the predicted
result to the status line with "Export again to verify from the decoded file."
A refusal is recorded as an error with the core message.

The mix is rendered on the UI thread once per round; a long timeline pays for
that in the dialog. Recorded in §7.

## 6. Fixtures

- Core: the M40 event cut (−39.90 LUFS, −25 dBFS peak) normalizes to Streaming
  in one round with +25.90 dB, compressor 4:1 engaged, limiter at −3.0 dBFS,
  and a passing `predicted_qc`; a loud mix gets plain −9 dB and the limiter; a
  model that only half-responds converges in two or more rounds; every refusal
  code is reachable; a track already on a bus is refused by that bus's name;
  the person-path default excludes mixed and empty tracks; the plan and the
  error round-trip as JSON.
- Agent: `get_audio_qc` is registered read-only under the description budget;
  `get_audio_qc`, `plan_audio_normalization`, and `queue_export` all expose
  `audio_preset` with the preset variants in their schemas; the registry and
  served byte counts are re-pinned.
- App: `normalization_offer` yields the offer (Streaming, +25.90 dB) for the
  M40 miss and nothing for an in-band result, a measure-only target, silence,
  a picture-only file, an unavailable verification, or no verification.

## 7. Deferrals

- **Platform smoke.** `COLOR-SMOKE-TEST.md` test 14: normalize a real clip
  and re-export to `VERIFIED`.
- **The render off the UI thread.** The dialog's plan renders the mix
  synchronously. A worker with a progress line belongs with the mixer slice.
- **A true-peak limiter (AD2).** `audio_limiter` is still a sample clamp under
  a 2 dB headroom; `gain_would_exceed_peak_ceiling` still names the case a
  real limiter exists for.
- **Meters and the mixer (AD3).**
- **Re-planning an existing normalization bus.** A second plan on the same
  tracks is refused (`track_already_mixed`) rather than revised; revising the
  bus in place is a small follow-up once the mixer can show it.
- **Per-track targets and stems.** One bus over the selected tracks; dialogue,
  music, and effects stems are the mixer's problem.
