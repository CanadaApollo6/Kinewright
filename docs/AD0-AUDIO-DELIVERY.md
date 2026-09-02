# AD0 — Audio delivery contract and QC

Status: foundation implemented 2026-09-02, pending platform smoke and the first
real-footage measurement. AD0 is to audio what CC0 and CC6 together are to
colour: an explicit, typed statement of what a delivered file's audio is
expected to measure, a measurement taken from the *decoded* file, and a pure
engine that compares the two and reports. It changes no mix. Normalization,
limiting, meters, and the mixer belong to the slices that follow and are named
in §7.

## 1. The editor job

Deliver a cut whose audio is heard at the level the platform or broadcaster
expects, and know before uploading whether it is. The M40 event/multicam
baseline shipped at −39.9 LUFS (`M40-GENERALIZATION-GAUNTLET.md`, "event");
the eval runner could measure that and the editor could not. After AD0 the
export dialog, the export queue, and `get_export_jobs` all carry the same
decoded measurement against the same named target.

## 2. Ownership

| Responsibility | Product/system | Model | Editor |
| --- | --- | --- | --- |
| Loudness, true-peak, and range semantics | Owns (BS.1770-4, EBU Tech 3342) | Reads them | Can inspect them |
| The delivery target | Supplies presets and defaults | May recommend one | Chooses it per job |
| Measurement of the written file | Owns and performs | May request it | Trusts what is shown |
| Whether a miss is acceptable | Reports it typed | May propose a fix | Decides |

Nothing in AD0 applies gain. `gain_to_target_db_hundredths` is evidence for a
proposal, in the same sense `plan_shot_match` returns operations it does not
apply.

## 3. Units and definitions

All decibel-family quantities are signed integer hundredths on the wire, as
`AudioLoudness` already does.

- **Integrated loudness** (LUFS): ITU-R BS.1770-4 K-weighted, 400 ms blocks at
  100 ms hop, −70 LUFS absolute gate, −10 LU relative gate. Unchanged from
  `kinewright_media::measure_loudness`.
- **Sample peak** (dBFS): the largest absolute decoded sample. Unchanged.
- **True peak** (dBTP): the largest absolute value of the 4× oversampled
  signal. AD0 uses a 48-tap Blackman-windowed sinc polyphase interpolator
  (`TRUE_PEAK_OVERSAMPLING = 4`, 12 input samples of support) normalized to
  unit DC gain per phase, rather than a transcription of BS.1770-4 Annex 2's
  coefficient table. The two agree to well under 0.1 dB on band-limited
  programme; the fixture in §6 pins the canonical inter-sample case.
- **Loudness range** (LU): EBU Tech 3342. Short-term loudness over a 3 s window
  every 100 ms from the same K-weighted signal, −70 LUFS absolute gate, relative
  gate 20 LU below the energy mean of the absolutely gated values, range =
  95th − 10th percentile by nearest rank. `None` when fewer than two windows
  survive (any programme shorter than 3 s reports no range).
- **Analysis path**: the decoded stream is converted to 48 kHz stereo before
  measurement, exactly as `Analysis::asset_loudness` does. The file's own codec,
  rate, and channel count are probed and reported beside the measurement so a
  mono or 44.1 kHz delivery is visible as such.

## 4. Typed core model — `kinewright_core::audio_qc`

```rust
pub enum AudioDeliveryPreset {
    MeasureOnly,        // report everything, gate nothing (source_master default)
    Streaming,          // −14 LUFS ± 1 LU, −1 dBTP
    Podcast,            // −16 LUFS ± 1 LU, −1 dBTP
    BroadcastEbuR128,   // −23 LUFS ± 1 LU, −1 dBTP, range advised ≤ 20 LU
    BroadcastAtscA85,   // −24 LKFS ± 2 LU, −2 dBTP
}
pub struct AudioDeliveryTarget {
    pub preset: AudioDeliveryPreset,
    pub integrated_lufs_hundredths: Option<i32>,
    pub tolerance_lu_hundredths: u16,
    pub maximum_true_peak_dbtp_hundredths: Option<i32>,
    pub maximum_loudness_range_lu_hundredths: Option<i32>,
}
pub struct AudioDeliveryMeasurement {
    pub loudness: AudioLoudness,                 // integrated + sample peak
    pub true_peak_dbtp_hundredths: Option<i32>,
    pub loudness_range_lu_hundredths: Option<i32>,
}
pub fn measure_audio_qc(target, measured) -> AudioQcReport
```

`AudioDeliveryPreset::target` is the single authority for every number above;
no other file restates them. `AudioQcReport` carries the target, the
measurement, `gain_to_target_db_hundredths`, `gain_would_exceed_peak_ceiling`
(gain alone cannot conform: a limiter or a mix change is needed), the
exceptions, and `technical_pass` (no `Error`-severity exception).

### 4.1 Exception codes, severity, and when they fire

| Code | Severity | Condition |
| --- | --- | --- |
| `audio_programme_silent` | Error with a target; Info without | integrated is `None` |
| `audio_integrated_loudness_below_target` | Error | measured < target − tolerance |
| `audio_integrated_loudness_above_target` | Error | measured > target + tolerance |
| `audio_true_peak_over_ceiling` | Error | true peak > ceiling |
| `audio_true_peak_unmeasured` | Warning | ceiling set, true peak `None`, programme not silent |
| `audio_loudness_range_over_limit` | Warning | range > advised limit |
| `audio_analysis_rate_unexpected` | Warning | measurement rate ≠ 48 000 Hz |

`AUDIO_QC_CODES` lists exactly these seven; a core test proves every one is
reachable and that nothing else is ever published. Each exception carries
`field`, `observed`, and `allowed` as strings with units (`"-39.90 LUFS"`,
`"-15.00..=-13.00 LUFS"`, `"<= -1.00 dBTP"`).

## 5. Where the target comes from

A job parameter, never a document edit — the same rule CC6 §4.1 gives the
delivery bit depth.

- `DeliveryProfile::default_audio_preset()`: `source_master` → `MeasureOnly`;
  `youtube_1080p`, `vertical_short`, `square_social` → `Streaming`. The export
  queue attaches this to every job it verifies.
- The export dialog has a `Loudness target` radio row beside `Delivery depth`,
  listing every preset by label. It defaults to `Measure only` until the smoke
  test in §7 has run on real footage; the deliberate question is whether the
  dialog should follow the aspect's profile default.
- `DeliveryVerificationRequest.audio_target` (`#[serde(default)]`, measure-only)
  carries it to the media crate; `with_audio_target` attaches it.

## 6. Measurement of the written file — `verify_delivery_output`

The picture leg (CC6 §6) is unchanged. After it, the audio leg:

1. If the probed file's kind is not `Audio` or `AudioVideo` →
   `AudioVerification::NoAudioStream`. Not a failure.
2. Probe the best audio stream's codec name, native rate, and channel count.
3. Decode the whole stream to 48 kHz stereo through the same
   `decode_audio_range` the analysis facet uses, honouring the job's
   cancellation token.
4. `measure_delivery_audio` → `measure_audio_qc(request.audio_target, ..)`.
5. Any failure in 2–4 → `AudioVerification::Unavailable { reason }`. Never a
   pass.

`DeliveryVerification.audio: AudioVerification` (`#[serde(default)]` →
`NotMeasured`, so a record written before AD0 reads back honestly).
`DeliveryVerification.technical_pass` stays colour-only: CC6 §3.8 pins it to
exactly two codes, and the export dialog's `OVER BUDGET` label reads it. The
audio leg's own `technical_pass` produces a fifth status label,
`AUDIO OUT OF SPEC`, after `TAG MISMATCH` and `OVER BUDGET` and before
`VERIFIED`. The queue's rule is unchanged: verification never fails a job.

### 6.1 Fixtures

- `audio_qc` core tests: every preset round-trips; measure-only gates nothing;
  the M40 event cut at −39.90 LUFS against `Streaming` fails by
  `below_target`, reports `+25.90 dB` to target, and flags that this gain
  would exceed the ceiling; both band edges pass and one hundredth outside
  fails; a true-peak overrun is an error when loudness conforms; silence is an
  error with a target and information without; range and rate only warn.
- `loudness` media tests: a full-scale `fs/4` tone sampled 45° off its peaks
  reads a sample peak of −3.01 dBFS and a true peak within ±0.20 dB of
  0 dBTP; a 1 kHz tone's true peak is never below and at most 0.10 dB above
  its sample peak; silence carries no peak and no range; a 2 s tone has no
  range and an 8 s + 8 s 12 dB step measures a range of 11.00–12.10 LU; every
  kernel phase sums to unit gain.
- The CC6 exit fixture is picture-only, so it takes the `NoAudioStream`
  branch and its budgets are untouched.

## 7. Deferrals — each a slice, not a flag

- **The smoke test on real footage** (Windows and Omarchy): export a real cut
  at `Streaming`, read the `DECODED AUDIO` block, and compare with a
  third-party meter. This decides the dialog's default preset.
- **Normalization as a typed operation.** `gain_to_target_db_hundredths` is
  reported, not applied. The existing `plan_audio_normalization` planner
  targets tracks with its own `target_lufs_hundredths` argument; moving it
  onto `AudioDeliveryPreset` and adding a bus-level gain proposal is the next
  audio slice.
- **A true-peak limiter.** `gain_would_exceed_peak_ceiling` names the case a
  limiter exists for. `audio_limiter` is an effect name today; a measured,
  oversampled limiter with a parity gate is its own slice.
- **Meters and the mixer.** Per-track and bus meters, a loudness meter that
  reads the live mix, and a mixer panel: the human audio surface the
  competitive audit rates one star, unchanged since August.
- **`get_audio_qc` and `queue_export.audio_preset` on the agent surface.**
  Both are additive; they wait for the `server.rs` split to land so they can
  be added to the delivery family in one place. Until then the agent reads the
  audio leg through `get_export_jobs`, which serializes the whole
  `DeliveryVerification`.
- **Surround and >2-channel delivery.** The analysis path folds to stereo;
  `measure_loudness` refuses more than two channels. Channel-weighted BS.1770
  for 5.1 is a contract of its own.
- **The BS.1770-4 Annex 2 coefficient table.** Stated in §3; a transcription
  with a fixture that pins the difference against the windowed sinc is cheap
  if a broadcaster's meter ever disagrees by more than the 0.1 dB claimed.
- **Dialogue-gated loudness, momentary loudness, and a loudness history
  graph** for the QC window.

## 8. Definition of done for AD0

- [x] Typed target, measurement, presets, and QC engine in core with tests.
- [x] True peak and loudness range measured in the media crate with tests.
- [x] Decoded-file audio verification on every verified export, from both the
      dialog and the queue, never failing a job.
- [x] Export dialog: preset choice and the `DECODED AUDIO` block.
- [ ] Platform smoke on Windows and Omarchy with a third-party meter
      cross-check (§7).
- [ ] Agent tools `get_audio_qc` and `queue_export.audio_preset` (§7).
