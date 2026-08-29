# CC6 QC and managed delivery

Status: implementation contract — as implemented 2026-08-25 (errata folded in)
Depends on: [CC0](ROADMAP-AND-WORKFLOWS.md), [CC1 managed SDR primary](CC1-MANAGED-SDR-PRIMARY.md), [CC2 scopes and matching](CC2-SCOPES-AND-MATCHING.md), [CC3 curves and wheels](CC3-CURVES-AND-WHEELS.md), [CC4 look management](CC4-LOOK-MANAGEMENT.md), [CC5 secondaries](CC5-SECONDARIES.md), [M34 creator delivery verification](M34-CREATOR-DELIVERY-VERIFICATION.md), [M36 agent runtime efficiency](M36-AGENT-RUNTIME-EFFICIENCY.md)
Scope: **measuring** a managed grade against range, gamut, skin, and tag expectations at a named high-precision stage; widening managed delivery by exactly one lane (10-bit H.264); pinning the delivery render quality contract against measurement; and making "decode the file you just wrote and compare it" a product surface rather than one fixture.

CC6 does not change CC1's input, working, or monitoring contract, and changes CC1's delivery contract in exactly one direction: `ColorBitDepth::Ten` becomes a second accepted delivery depth. Every CC1 invariant is preserved verbatim. **Invariant 2.2.5 — "No colour stage clamps RGB to 0..1. The only RGB clamp is in the final monitor or delivery encoding step" — is the reason this slice exists**: that single clamp is the only place a managed grade silently loses information, and until CC6 nothing counts the pixels it eats.

CC6 **measures and reports**. It never maps, never clamps differently, never proposes a fix, never mutates a document, and never moves, renames, or deletes a finished encode.

The words **must**, **must not**, and **may** in this document are normative.

---

## 0. Change log

### 0.1 Changes from the draft

For reviewers who read the 2026-08-25 draft. Each line is a design change, not an edit.

- **Delivery white level.** The delivery intermediate quantizes on `DELIVERY_INTERMEDIATE_WHITE = 65_280`, landed as `ad6f6a8` before CC6 starts; nominal white now encodes to Y′ 235 (10-bit 940) instead of 236 (943), and both sides of §6.3's comparison use that one constant.
- **`range=tv` is dropped.** It is not an x264 parameter in x264 core 165 (silently ignored by `-x264-params`, a hard failure under `-x264opts`); `set_color_range(Range::MPEG)` is measured to reach the SPS on every lane, so nothing changes in `x264-params`.
- **Dither.** libswscale 9.1.100 applies a fixed 8×8 ordered dither on 16→8-bit RGB→YUV and none on 16→10-bit; `sws_dither` and `accurate_rnd` are inert on that path. `DELIVERY_DITHER_OPTION` is deleted and the measured behaviour is recorded as a QC rule instead.
- **Scaler.** `bicubic` measured best of bicubic/lanczos/spline; the encode-side `accurate_rnd+full_chroma_int+full_chroma_inp` set is not added. `DELIVERY_SCALER_FLAGS = "bicubic"` names today's value; `DELIVERY_OUT_CHROMA_LOC` is deferred as unmeasured.
- **Edge-dominated maximum.** 4:2:0 chroma decimation at hard saturated edges costs up to 133–134 RGB codes in *both* lanes, so whole-raster RGB max is reported and never gated; the gate is the luma plane plus RGB mean and PSNR.
- **Per-node attribution by removal.** Nodes are attributed by removing the effect on a cloned document, not by setting a `bypass` parameter — `primary_correction` has no `bypass` control, so the draft's method was impossible on it.
- **No proxy QC.** `get_color_qc` has no `resolution` argument; a working-stage measurement is full-resolution or it is refused typed.
- **Two-mode tag check.** `delivery_tag_check` has a pre-export mode (expected tags materialised from `ExportSettings`) and a post-export mode (probed vs expected); the draft's single mode had no observed side before an export existed.
- **QC raster.** `cc6_qc_raster()` is 80 × 40 = 3200 with basis-point-exact rectangles and a stated surround remainder; the draft's 64 × 36 left zero pixels for three fixtures.
- **Delivery source.** 60 frames at 25 fps, so the sample set `0, 14, 29, 44, 59` genuinely spans two GOPs (GOP = 50); the draft's 20-frame two-GOP claim was false.
- **Windows build.** The Windows CI job runs a *different* FFmpeg package from the Linux pin; both are named, the Windows job is the Windows measurement (P11), and budget constants carry a ≥ 2× margin over the Linux measurement rather than being per-OS.
- **Conformance carries the depth.** `delivery_conformance`, `DeliveryConformanceReport`, `get_delivery_conformance`, and the app's `ConformanceKey` all gain the delivery depth, so a cached 8-bit report cannot be served for a 10-bit export.
- **`ScopeError::UnsupportedStage` is kept.** `ScopeRequest::validate` switches to `!stage.measurable_by_scope_engine()`; no second stage-rejection variant is added.
- **Verification never moves a file.** A tag mismatch leaves the job `Completed` with `error: None` and `technical_pass = false`; the quarantine path is not used by verification. `verification` is off `ColorQcReport` entirely and lives on `ExportJobRecord`.

### 0.2 Changes from implementation and the review round (2026-08-25)

Errata E1–E32 and the four crate reviews, folded in. Each line is a contract change, not an edit; the draft's number is given as "was" wherever one was replaced.

- **Non-finite samples are counted, never classified.** A `NaN` compares `false` against every bound and an infinity saturates every extreme, so a visible pixel whose linear or encoded sample is not finite is counted in `non_finite_pixel_count` (on both `ColorQcReport` and `ColorQcRegion`), fed to **no** accumulator, and raised as the Error-severity `color_qc_non_finite_sample` (§3.1, §3.8). The draft had no such state and silently reported such pixels as in range.
- **An unseen `Y′CbCr` plane reports the empty interval**, `UNSEEN_MINIMUM_CODE_HUNDREDTHS = i64::MAX` / `UNSEEN_MAXIMUM_CODE_HUNDREDTHS = i64::MIN`, recoverable through `PlaneLegalExcursion::samples_seen()` — never a fabricated `0` that reads as a legal black (§3.4).
- **`PlaneLegalExcursion`'s extremes are *observed sample codes*, not excursion amounts** (E5). Subtract the plane's bound to get the amount; the `linear = 1.05` anchor pins `24034` at 8 bits and `96137` at 10.
- **The excursion rate is `PlaneLegalExcursion::excursion_basis_points(sample_count)`**, summing both directions, and §6.4's threshold is compared against it and against neither `below_basis_points` nor `above_basis_points` alone (§3.4). The gate and the fixture's prediction call the same method.
- **`DeliveryVerificationRequest::validate()`** returns the typed `DeliveryVerificationError::FrameCountOutOfRange` (`delivery_verification_frame_count_out_of_range`) instead of `sample_frames` clamping a bad `frame_count` silently, and `verify_delivery_output` calls it first (§6.1, §6.2, §3.8).
- **`DeliveryVerificationError::BudgetLaneMismatch`** (`delivery_verification_budget_lane_mismatch`) refuses a request carrying the other lane's budgets, which would otherwise have produced a `within_budgets` verdict published as a pass against a gate nobody chose (§6.1, §6.3).
- **An empty sample set is refused** as `FrameCountMismatch` with `observed: "0 sampled frames"`, rather than reported as a `within_budgets: true` over three planes that never saw a sample (§6.1).
- **The `T` cross-check counts *presented* frames in `O(GOP)`** (E14, E21): the verifier opens the output exactly as a player does, with the edit list honoured, and asks "does frame `T − 1` present, and is there nothing past it?" rather than straight-decoding the whole file. `DeliveryComparison.frames` records the frame identity of the picture the decoder actually returned.
- **The export last-frame defect and its fix** (E22). Every Kinewright MP4 was written with zero-duration packets, so the `mov` muxer's `elst` presented one frame fewer than the track coded and every player dropped the last frame. `VideoPacketDuration::OneFrame` stamps one tick of the encoder time base on each packet; a test-only `Zero` path manufactures the old defect so the verification refusal is asserted against a real file (§0.3, §6.1, §6.2).
- **Three new agent-side typed codes** (E23, E32): `color_qc_frame_selector_conflict`, `working_proof_unavailable`, and `color_qc_frame_out_of_range` — the last because the draft's tool rendered opaque black for an out-of-project frame and reported a clean pass.
- **`MediaError::ColorQc(ColorQcError)`** carries QC refusals structurally through `?`, so the agent recovers the typed code from `recovery_code()` rather than parsing a `Display` string (E32).
- **`ExportJobRecord.verifying: bool`** (skip-if-false) is true while verification runs, so `get_export_jobs` distinguishes encoding from verifying without a new state variant; **`verification_unavailable_reason` is the sole carrier** of an unavailable verification and there is **no `verification_unavailable` exception on the record** (E31). Verification itself stays non-interruptible.
- **The app shares one working proof.** A `(session, revision, frame)`-keyed `WorkingProofCache` is owned by the app and read by both the QC window and the QC mask, so the two states cannot start two concurrent full-resolution renders of the same frame (§8.1, §8.2, §14).
- **The QC mask does not render during playback.** `QcMaskStatus::PausedOnly` is shown instead: one full-resolution working proof per frame is not a playback cost (§8.2).
- **Cancel is live during verification.** A cancellation observed before verification starts yields `ExportVerification::Unavailable("cancelled before verification")` and a `NOT VERIFIED` status, and the dialog replaces the frozen progress bar with a "Verifying export…" line once encoding is done (§8.4).
- **The export dialog's verification block renders per-field probed-tag rows**, reusing the QC window's `tag_field_rows` / `tag_field_color`, with not-representable rows visually distinct (E27, §8.4).
- **`KeyAction::ColorQc` is `Ctrl+Shift+C`** (E28): the only free chord in the map, and deliberately not `Ctrl+Q`.
- **§6.3's budgets are re-baselined against `cc6_delivery_source()`** and are integer `_MILLIONTHS` constants (E8). 8-bit luma max 8 (was 8), P99 **3.0** (was 2.0), mean **0.4** (was 1.0), RGB mean **1.75** (was 1.0), PSNR floor **33.00 dB** (was 40.00); 10-bit luma max **16** (was 32), P99 **4.0** (was 8.0), mean **1.0** (was 4.0), RGB mean **1.0** (was 0.5), PSNR floor **33.00 dB** (was 40.00). §6.3 records every measured value and margin.
- **The `e(−0.02)` anchor takes the power branch** (E1): `0.02 > 0.018`, so the value is `−0.089999733` in f64 and `−0.089999743` in f32, not the draft's `4.5 · 0.02` linear-branch label (§3.2).
- **The `0.0543`-code seam discontinuity is on the 219-code limited luma span** (E2), `2.479e-4 × 219`, not on a full-scale span (§3.2).
- **`delivery_conformance` no longer rejects `bit_depth`** (E4): with the depth argument as the single authority, `export_settings` overwrites it, so the `bit_depth` leg is asserted directly on `delivery_color_mismatch` (§4.1, §11.2.9).
- **`DeliveryProfile`'s serde and `as_str()` spellings diverge** (E3): serde is `rename_all = "snake_case"` → `youtube1080p`, `as_str()` is `youtube_1080p`. A pre-CC6 divergence, deliberately untouched; the manifest pins **both** spellings (§9.2).
- **`delivery_tag_not_representable` is also raised by `ColorQcReport`** in post-export mode (E6), not only by `DeliveryVerification` (§3.8).
- **`ColorQcCheck::Range` and `Gamut` gate nothing** (E7): range and gamut are always measured in the single pass. `checks` gates only `skin`, `tags`, and `per_node` (§3.0, §7).
- **`DELIVERY_REFERENCE_DENOMINATOR` lives in media** (E9, E16) as an alias of `DELIVERY_INTERMEDIATE_WHITE`; core does not carry a second copy (§6.3).
- **`delivery_reference` takes the render scale as an argument** (E15), bound to `FullResolution` by the production caller, so `delivery_verification_not_full_resolution` has a reachable failing case (§5.7, §6.1).
- **`EBU_R103_TOLERANCE_CODES_8BIT = 11`** is a named constant, scaled by `s` (E17, §6.4).
- **Encoder-format refusals are typed and field-complete** (E19, E20): `delivery_encoder_pixel_format_unavailable { observed: the advertised list, allowed: the lane format }` and `delivery_pixel_format_depth_mismatch`; `validate_delivery_description` emits no prose message; `DELIVERY_VIDEO_CODEC`, `DELIVERY_X264_PARAMS`, and `DELIVERY_SCALER_FLAGS` are named constants (§4.2, §4.3).
- **Per-node attribution costs 17 renders for 17 candidates** (E12), not 18: `nodes::measure_color_qc_with_nodes` renders the baseline once and hands it back with the report, so the app's per-node path does not render its own proof as well (§3.7, §8.1).
- **M36 after CC6** (E24): registry **124** tools / 1 280 060 B (schemas 1 163 879, descriptions 95 827); served 7 / 5 660 / 3 510 / 998, unchanged. `get_color_qc`'s description is **936 B** (§7).
- **§12's size is measured, not estimated.** The slice is **22 182 insertions / 420 deletions**, inside the draft's 18 000–25 000 estimate (§12).
- **Assorted UI decisions named** (E25, E26, E29): `QaSeverity` colours are a new `severity_color` mapping over `STATUS_DANGER` / `STATUS_WARNING` / `TEXT_MUTED`, because the branch QA card has no per-severity mapping to reuse; `OVER BUDGET` and `TAG MISMATCH` both use `STATUS_DANGER` and `NOT VERIFIED` uses `STATUS_WARNING`, four distinct labels; the clipping-contribution line appears on the primary and LUT cards' equivalent slot as well as on the node header.
- **Fixture names are the implemented ones** (E30): §11.2.10/11 are `cc6_{eight,ten}_bit_encoded_delivery_passes_tag_luma_and_difference_budgets`, and §11.2 now lists every `cc6_*` test the tree declares rather than a representative subset.

### 0.3 The export last-frame defect

Found while building §6's verification and fixed before the verifier could be trusted, because a verifier that reads the file as a player does would otherwise have reported the defect as a Kinewright verification failure on every export.

`ffmpeg-next` 8.0 exposes no setter for `AVFrame.duration` and `unsafe_code` is forbidden workspace-wide, so libavcodec left `AVPacket.duration` at zero. The `mov` muxer computed the track duration as the last packet's `pts + 0`; libx264's B-frame delay makes the muxer shift the media timeline and write an `elst`, and that edit list ended up one frame shorter than the track. FFmpeg's demuxer then flags the final coded picture `AV_PKT_FLAG_DISCARD`, and **every player dropped the last frame of every Kinewright export**.

The fix is `VideoPacketDuration::OneFrame`: the duration is stamped on the *packet*, in ticks of the encoder time base, and `av_packet_rescale_ts` carries it into the stream time base with the timestamps. The rejected alternative was `ignore_editlist` on the verification decoder — rejected because it would have made verification read a file no player reads, hiding the defect instead of catching it. §6.1 states the reading rule; §6.2 states the `O(GOP)` presented-frame cross-check that catches a recurrence; `every_exported_frame_is_presented_after_the_mp4_edit_list` and `cc6_verification_refuses_an_export_whose_edit_list_drops_the_last_frame` are the two directions, the second exporting through a test-only `VideoPacketDuration::Zero` path that manufactures the defect on a real file.

### 0.4 The `accurate_rnd` chroma erratum (2026-08-29)

Found by CC7's first honest Windows CI run (run `33225964499`, commit `c790fb1`; `docs/CC7-WORKFLOW-EVALUATION.md` §0.3 PM-E12 / PM-E14 carries the CC7 half). Recorded here because two normative sentences in §5 are narrower than they read, and a later contributor reading them would conclude something false.

**§5.3's "`accurate_rnd` is inert on this path" is a luma-only measurement.** The sentence states its own population in the same breath — "0 differing luma samples of 262 144" — and that population is the whole of the evidence. It is **not** inert on chroma. Measured on Linux (`mifi/ffmpeg-builds 8.0-1`, libswscale 9.1.100): adding `accurate_rnd` to `DELIVERY_SCALER_FLAGS` collapses §5.4's neutral-chroma dither straddle from `Cb {128, 129}` / `Cr {127, 128}` to `Cb {128}`, and moves a pinned decode figure — CC7 scenario (a)'s 8-bit luma mean goes **18 677 → 18 688**. This is exactly the number the Windows CI package (`System233/ffmpeg-msvc-prebuilt ffmpeg-8.0.1-r3`) measures without the flag, which is how the root cause was identified: **the MSVC build's swscale rounds chroma as if `SWS_ACCURATE_RND` were set.** The 10-bit lane is bit-identical under the flag, so the 10-bit divergence CC7 also observed is a **second, unexplained build difference** and is not attributed to this one.

**`DELIVERY_SCALER_FLAGS = "bicubic"` is unchanged.** §5.3 measured that value against lanczos and spline and declined to change it, and enabling `accurate_rnd` now would move pinned figures in CC6 and CC7 to buy nothing §5.3 measured a need for. The correct reading of §5.3 is therefore: *`accurate_rnd` was measured inert on the luma plane and was not measured on chroma, where it is not inert.*

**§5.4's normative single-code rule licenses the change the fix made.** §5.4 says: *"a flat 8-bit delivery patch legitimately produces two adjacent Y codes in a fixed 8×8 tiling; no assertion may require a single code from an 8-bit delivery output except where the input lands exactly on a code."* Neutral chroma lands exactly on a code (128.0), so **both** outcomes — the straddle and the single code — are inside the rule, and both occur. `delivery_filter_converts_sixteen_bit_full_range_rgb_to_limited_yuv420p` (`export.rs:1735`, chroma block at `:1766-1806`) had pinned one build's exact dither sets; it now asserts the **window** the claim actually makes — every chroma sample within `127..=129`, `128` itself present, nothing reaching 126 or 130 — with **both builds' measured sets recorded in the test**: Linux `Cb {128, 129}` / `Cr {127, 128}`, Windows `Cb {128}`. The luma assertions in that test are untouched: they are the exactly-on-a-code cases §5.4 exempts (white `65_280` → 235) and the two-code straddle §5.4 predicts (mid-grey → `{170, 171}`).

**Named residual risk.** `delivery_filter_converts_sixteen_bit_full_range_rgb_to_limited_yuv420p10le` (`export.rs:1818-1870`) still asserts **exact** codes taken from swscale output — luma `{940}` and `{682}`, chroma `{512}` on both planes — on the strength of §5.4's "0 of 256 flat grey tiles come out non-flat at 10 bits". That is currently true on **both** CI builds and is the claim the 10-bit lane exists to make, so it is kept rather than pre-emptively widened. It is recorded here so that **if a third FFmpeg build enters CI, that test is the one to review first**, and so that a future widening is a decision against this note rather than a silent tolerance.

---

## 1. In scope and out of scope

CC6 delivers:

- a **named high-precision stage**, `working_linear_post_composite`, and `Analysis::working_proof_for_document` — a linear f32 RGBA full-raster readback of the production `Rgba16Float` composite target, before any monitor or delivery encode, with the same isolated-renderer and `full_resolution` claim rules as `monitor_proof_for_document`;
- a **colour QC engine** in `kinewright-core/src/color_qc.rs`: range (legal), gamut, a forward BT.709 limited-range Y′CbCr reference at 8 and 10 bits, region-scoped skin diagnostics, typed delivery tag checks, optional per-node clipping attribution, and a `ColorQcReport` with typed exceptions and severities — all integer-reported, deterministic, and evidence-only;
- **exactly one new delivery lane**: 10-bit H.264 (`yuv420p10le`, libx264 High 10), BT.709 primaries/transfer/matrix, limited range, D65, `ColorBitDepth::Ten`;
- **typed delivery rejection** (`DeliveryColorError` with `code`/`field`/`observed`/`allowed`/`recovery_action`) replacing `MediaError::Backend(String)`, and the public, all-fields `DeliveryColorMismatch` that both the core QA gate and the media gate report from;
- a **measured delivery render quality contract** (§5): full-resolution decoders, the 65 280 intermediate, the named scaler flag string, the measured dither behaviour, the decode-side rule for verification, and the rule that no proxy render may claim it;
- **`Analysis::verify_delivery_output`**: decode the exported file through the crate's own bindings-based decoder in one seek-based pass at a bounded, deterministic frame sample; re-render the delivery reference; compare with named per-lane budgets; probe tags; and measure the decoded file's **native** Y′CbCr planes for real excursions;
- an evidence-only agent tool **`get_color_qc`**, verification on `ExportJobRecord`/`get_export_jobs`, a `verify` flag and a delivery-depth choice on `queue_export`, and the removal of `get_video_scopes_v2`'s fabricated gamut zero;
- a read-only **Colour QC window**, a **QC clipping mask** in the program viewer, absolute per-channel clipping in the scopes panel, a per-node clipping line in the inspector, and an 8/10-bit choice plus a post-export verification block in the export dialog; and
- a fixture suite whose central gate is **the cross-platform encoded fixture**: a synthetic source exported through the production path at 8 and 10 bits, re-probed, decoded, and gated on tag, Y′CbCr legality, and difference budgets, in the default lane on both CI operating systems.

CC6 does **not** deliver gamut *mapping*, legal-range *clipping policy*, automatic fixes, or any operation that changes a grade; HDR, BT.2020, PQ, or HLG; ProRes / DNxHD / FFV1 / VP9 / AV1 mezzanine or delivery lanes (the pinned build has the encoders — §13 states why that is not a reason); ACES or OCIO; a false-colour or zebra *shader* overlay; loudness normalization; VMAF, SSIM, or any perceptual metric; a skin *detector*; ΔE2000; or a colour eval suite in `eval.rs`.

---

## 2. The named high-precision stage

### 2.1 `working_linear_post_composite`

`ScopeStage` (`crates/kinewright-core/src/scopes.rs:70-92`) gains its second value:

```rust
pub enum ScopeStage {
    #[default]
    #[serde(rename = "monitoring_post_composite", alias = "monitoring/post-composite")]
    MonitoringPostComposite,
    /// The composited scene-linear working surface, before any monitor or
    /// delivery encode. CC6 §2.
    #[serde(rename = "working_linear_post_composite")]
    WorkingLinearPostComposite,
}
```

This is the value the existing gamut stub says it needs (`kinewright-agent/src/color_scopes.rs:1522`: *"source/working gamut requires a named high-precision stage"*), and the value `scopes.rs:64-68`'s doc comment reserves.

**One vocabulary, two consumers, fail-closed on both sides.** `ScopeStage` gains `pub const fn measurable_by_scope_engine(self) -> bool` (`true` only for `MonitoringPostComposite`). `ScopeRequest::validate` (`scopes.rs:482-490`) changes its condition from `self.stage != ScopeStage::MonitoringPostComposite` to `!self.stage.measurable_by_scope_engine()` and **keeps returning the existing `ScopeError::UnsupportedStage { stage }`** (`scopes.rs:752-753`). No second stage-rejection variant is added: one refusal, one code, no dead variant. `get_video_scopes_v2` continues to accept only `monitoring_post_composite` spellings (`color_scopes.rs:51-57`) and keeps its agent-level `unsupported_stage` code (`color_scopes.rs:556-567`), so no scopes caller can reach the new value by accident.

Rejected alternative: a second, separate stage enum owned by `color_qc.rs`. Rejected because two stage vocabularies means two wire spellings for the same pipeline diagram, and `compare_scope_evidence`'s stage equality would silently be comparing values from different alphabets.

**The CC2 scopes engine is not rewritten.** It keeps measuring RGBA8 at `monitoring_post_composite`, keeps its 16-bit full-scale reporting, keeps `SCOPE_LOW_CLIP_CODE = 1` / `SCOPE_HIGH_CLIP_CODE = 254` (`scopes.rs:33-35`). The QC engine consumes the linear raster and never calls it.

### 2.2 `WorkingProof` and `Analysis::working_proof_for_document`

Core gains, next to `MonitorProof` (`media.rs:435`) and `MatteProof` (`media.rs:489`) in `crates/kinewright-core/src/media.rs`:

```rust
/// Row-major, top-left-origin linear RGBA. `pixels.len() == width * height * 4`.
#[derive(Debug, Clone, PartialEq)]
pub struct LinearRgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<f32>,
}

/// Always `"working_linear_post_composite"`; equals `ScopeStage::WorkingLinearPostComposite`'s wire name.
pub const WORKING_PROOF_STAGE: &str = "working_linear_post_composite";
/// Always `"scene_linear_bt709_f32"`: BT.709 primaries, D65, linear light, no clamp.
pub const WORKING_PROOF_ENCODING: &str = "scene_linear_bt709_f32";

pub struct WorkingProofMetadata {
    /// Renderer provenance, reused unchanged from the managed monitor proof.
    pub render: MonitorProofMetadata,   // media.rs:403
    pub stage: String,                  // WORKING_PROOF_STAGE
    pub encoding: String,               // WORKING_PROOF_ENCODING
    pub raster_aspect_millionths: i64,
}

pub struct WorkingProof {
    pub image: LinearRgbaImage,
    pub metadata: WorkingProofMetadata,
}
```

and on the `Analysis` trait, defaulting to `Err(MediaError::NotImplemented)` exactly as its two siblings do (`media.rs:1420`, `media.rs:1454`):

```rust
fn working_proof_for_document(
    &self,
    _document: Arc<Document>,
    _at: TimeCode,
) -> Result<WorkingProof, MediaError> { Err(MediaError::NotImplemented) }
```

**`MonitorProofRenderKind` is not extended**, for CC5 §4.1's reason verbatim: it names the renderer implementation, not an output target.

The `FfmpegMediaEngine` implementation in `engine.rs` mirrors `monitor_proof_for_document` (`engine.rs:814-846`) line for line: a fresh isolated `FrameRenderer`, `set_lut_library(self.document_lut_library(&document)?)`, `let scale = RenderScale::FullResolution` bound once, `document.resolution`, `DecodeStrategy::Seek`, and `self.gpu.monitor_proof_metadata_for(scale, (w, h), resolution)` so `full_resolution = matches!(scale, FullResolution) && rendered == document` (`compositor.rs:277-300`) is derived, never asserted.

**Productionising `render_working`.** `Compositor::render_working` / `render_working_with_luts` (`compositor.rs:1719-1726` and `1729-1760`, currently `#[cfg(test)] pub(crate)`) lose the `#[cfg(test)]` and return `Result<LinearRgbaImage, MediaError>` — width and height alongside the interleaved f32 values — instead of a bare `Vec<f32>`. The body is unchanged: `composite(...)`, then `for_each_linear_pixel(...)` (`compositor.rs:1555`) accumulating `values.extend(linear)`, then `release_layer_textures`. `FrameRenderer` gains `pub(crate) fn render_working(&mut self, document, project_at, resolution, scale, strategy) -> Result<LinearRgbaImage, MediaError>`, built exactly like `render_delivery` (`render.rs:315-333`) — same `decoded_layers`, same `compositor_layers`, same LUT library — differing only in the readback it asks for.

`render_working` has **14 call sites**: seven inside `compositor.rs`'s own tests (3372, 4228, 5172, 5182, 5258, 5268, 5347) and seven in fixture modules (`cc1_fixtures.rs:1343/2887/2924`, `cc3_fixtures.rs:439`, `cc4_fixtures.rs:842`, `cc5_fixtures.rs:1017`). All fixture modules are `#[cfg(test)]` (`media/src/lib.rs:31-41`), so removing the attribute is safe; the return-type change is mechanical and §11.2.6 asserts the pre-CC6 fixtures still produce identical values.

### 2.3 What the working proof is *not*

- It is **not** display-referred. Values may be negative and may exceed 1.0; that is the whole point.
- It is **not** the CPU reference. It is the production GPU compositor's own surface, so it carries the half-float storage quantization CC1 §6.2 bands.
- It **must not** be substituted by a proxy. Every CC6 consumer asserts `metadata.render.full_resolution == true` before measuring and fails typed otherwise (`color_qc_proxy_proof_refused`), the same enforcement `color_scopes.rs:1210-1214` already applies to monitor proofs. There is no proxy working proof: `working_proof_for_document` takes no scale and binds `FullResolution` (§2.2).

---

## 3. The colour QC engine

New module `crates/kinewright-core/src/color_qc.rs`. Deterministic, integer-reported, evidence-only. **`measure_color_qc` and everything it calls perform no I/O, hold no renderer, and never construct an `Operation`.** The one exception is the per-node submodule `color_qc::nodes` (§3.7), which renders and applies operations to a cloned document; its cost and its impurity are stated there and it is never on `measure_color_qc`'s path.

### 3.0 Entry point and input types

```rust
/// Measure one working proof. Pure: no renderer, no I/O, no clock, no RNG.
pub fn measure_color_qc(
    proof: &WorkingProof,
    request: &ColorQcRequest,
) -> Result<ColorQcReport, ColorQcError>;

/// The f32 delivery transfer, owned by core.
///
/// Bit-identical to `kinewright_media::color_pipeline::encode_bt709`
/// (`color_pipeline.rs:355-363`) for every f32 input: same seam (`linear <
/// 0.018`), same rounded constants (4.5, 1.099, 0.099, 0.45), same
/// sign-preserving odd extension, same f32 arithmetic order. §11.2.22 asserts
/// `to_bits()` equality over a stated sample set; core must not gain a
/// dependency on `kinewright-media`, so this is the only permitted second copy
/// and it is gated.
pub fn encode_bt709_delivery(linear: f32) -> f32;

pub struct ColorQcRequest {
    /// CC2's half-open basis-point rect (`scopes.rs:135-200`). `None` = whole raster.
    pub roi: Option<NormalizedRoi>,
    /// CC5's matte scope, with the coverage raster the matte proof produced.
    pub matte_region: Option<MatteRegionScope>,
    pub checks: Vec<ColorQcCheck>,
    pub delivery_bit_depth: DeliveryEncodeDepth,
    /// Pre-export tag mode: the materialised `ExportSettings.delivery_color` (§3.6).
    pub expected_delivery: Option<ColorDescription>,
    /// Post-export tag mode: the probed description of a written file (§3.6).
    pub observed_delivery: Option<ColorDescription>,
    /// `1..=MAX_QC_NODE_CONTRIBUTIONS`; only read by `color_qc::nodes`.
    pub max_nodes: u8,
    /// The project frame identity this proof was rendered at, the same `i64`
    /// identity `ScopeMeasurementMetadata.project_frames` carries
    /// (`scopes.rs:531`). CC6 introduces no new frame-identity type.
    pub project_frame: i64,
}

/// `MatteRegionDescription` (`scopes.rs:111-133`) carries clip/effect/threshold
/// and a covered-pixel count — not pixels. Core needs the coverage image to
/// scope a region, so the request carries it; §7 states that the agent obtains
/// it from `Analysis::matte_proof_for_document` (`media.rs:1454`) and the app
/// from its existing matte proof source.
pub struct MatteRegionScope {
    pub description: MatteRegionDescription,
    /// `MatteProof.coverage` (`media.rs:489-493`): `R = G = B = round(255·m)`, `A = 255`.
    pub coverage: RgbaImage,
}

/// `Range` and `Gamut` **do not gate their sections**: both are measured in the
/// single pass, always, and are always present on the report. `checks` selects
/// only the optional work — `Skin`, `Tags`, and `PerNode` — so naming `Range`
/// or `Gamut` is a no-op that costs nothing and removing them hides nothing.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ColorQcCheck { Range, Gamut, Skin, Tags, PerNode }

pub struct ColorQcRegion {
    pub normalized_roi: NormalizedRoi,
    pub pixel_roi: PixelRoi,
    pub matte_region: Option<MatteRegionDescription>,
    pub region_pixel_count: u64,
    pub visible_pixel_count: u64,
    /// Visible pixels holding a non-finite linear or encoded sample (§3.1).
    pub non_finite_pixel_count: u64,
    pub transparent_pixel_count: u64,
}

pub struct ColorQcNodeContributions {
    pub baseline_range_basis_points: u32,
    pub baseline_gamut_basis_points: u32,
    pub considered_node_count: u32,
    pub truncated: bool,
    /// Always `"node_removed"` (§3.7).
    pub attribution: String,
    pub nodes: Vec<ColorNodeQcContribution>,
}

pub struct DeliveryVerificationRequest {
    /// `1..=DELIVERY_VERIFICATION_MAX_FRAMES`; default `DELIVERY_VERIFICATION_FRAME_COUNT`.
    pub frame_count: u8,
    pub budgets: DeliveryBudgets,
    /// The tags the file is expected to carry: `ExportSettings.delivery_color`.
    pub expected_delivery: ColorDescription,
}

impl DeliveryVerificationRequest {
    /// Refuse a `frame_count` outside `1..=DELIVERY_VERIFICATION_MAX_FRAMES`.
    ///
    /// **Normative: a caller validates before sampling.** `sample_frames` used
    /// to clamp silently, which turned an out-of-contract request into a
    /// quietly narrower measurement; it now documents that validation is the
    /// caller's, and `verify_delivery_output` calls this first.
    pub fn validate(&self) -> Result<(), DeliveryVerificationError>;
}
```

`validate` returns `DeliveryVerificationError::FrameCountOutOfRange` with code
`delivery_verification_frame_count_out_of_range`, `field = "frame_count"`, the
observed count, and `allowed = "1..=16"`.

```rust
/// Every gated number in §6.3. `DeliveryBudgets::for_depth(depth)` returns the
/// lane's named constants; a caller may not invent a looser set, and
/// `verify_delivery_output` refuses `delivery_verification_budget_lane_mismatch`
/// when the request's budgets are not the ones the settings' depth selects
/// (§6.1). The struct's shape is unchanged, so agent and app callers that build
/// it from `for_depth` are untouched.
///
/// **All four fractional terms are integer `_MILLIONTHS`, not floats**: §10.1
/// forbids a float in a reported or gated number, and a budget expressed as
/// `2.0` in one place and `2_000_000` in another is two constants.
pub struct DeliveryBudgets {
    /// Luma-plane maximum absolute difference, delivery code units at the lane depth.
    pub luma_max_code: u32,
    pub luma_p99_code_millionths: i64,
    pub luma_mean_code_millionths: i64,
    /// Whole-raster RGB mean absolute difference, 8-bit-equivalent code units.
    pub rgb_mean_code_millionths: i64,
    pub psnr_floor_db_hundredths: i32,
}

/// The one delivery encoder entry point CC6 adds, so the depth cannot diverge
/// between the codec context and the filter graph (§4.3).
pub fn delivery_color_for_depth(
    document: &Document,
    depth: DeliveryEncodeDepth,
) -> ColorDescription;
```

`Analysis::verify_delivery_output` (§6.1) is the only producer of `DeliveryVerification`.

### 3.1 Population, units, and ordering

**Population.** A pixel is *visible* when `alpha > 0.0`, CC2's rule (`scopes.rs:12-14, 1113-1119`) restated for f32. Alpha is never a weight. The predicate is spelled `if !(alpha > 0.0) { … }` rather than `alpha <= 0.0`, so a `NaN` alpha is **not** visible: `NaN <= 0.0` is false, and the negated spelling is the only one that excludes it.

**Non-finite samples are counted and never classified, normative.** A `NaN` compares `false` against every bound and an infinity saturates every extreme, so a visible pixel whose linear value or whose `e = encode_bt709_delivery(linear)` is not finite cannot be called in range, out of gamut, or on a plane. Such a pixel is guarded **once**, in the region accumulator, before any channel, gamut, `Y′CbCr`-plane, or skin accumulator sees it:

- it is counted in `ColorQcReport.non_finite_pixel_count` and in `ColorQcRegion.non_finite_pixel_count`;
- it contributes to **no** count, extreme, rate, histogram, or circular sum;
- it raises the **Error**-severity `color_qc_non_finite_sample` exception with `field = "non_finite_pixel_count"`, the observed count, and `allowed = "0"`, which clears `technical_pass`;
- it stays inside `visible_pixel_count`, which remains the denominator of every rate, so no basis-point figure is silently rebased onto a smaller population.

A working proof cannot produce a non-finite sample through the managed pipeline — this is a guard against a defect elsewhere, and stating it as an Error rather than a Warning is deliberate: a report that cannot classify some of its own population has not measured the frame. When *every* sample in the region is non-finite the `Y′CbCr` planes saw nothing and report the unseen empty interval of §3.4 rather than a fabricated `0`.

**Normative: the working-stage raster is opaque by construction.** `composite` clears the `Rgba16Float` target with `wgpu::LoadOp::Clear(wgpu::Color::BLACK)` — alpha `1.0` (`compositor.rs:1092`) — and the alpha blend is `src_factor: One, dst_factor: OneMinusSrcAlpha` (`compositor.rs:796-801`), so `a' = a_src + (1 − a_src)·1 = 1` everywhere. Therefore at `working_linear_post_composite`, `visible_pixel_count == region_pixel_count` and `transparent_pixel_count == 0` always. `transparent_pixel_count` is retained for schema symmetry with `ScopeMeasurementMetadata` and **must not** be used as a check: no fixture may assert it is zero as if that could fail. Uncovered background is opaque black, which is in range, and is counted in every denominator; a caller that wants a smaller population uses an ROI or a matte scope.

Every rate below is basis points **of visible pixels inside the requested region**. The region is the intersection of an optional `NormalizedRoi` and an optional `MatteRegionScope` (CC5's matte identity plus its coverage raster). Region resolution is CC2's floor/ceil rule, unchanged. A coverage raster whose dimensions differ from the proof's raster fails `color_qc_matte_region_raster_mismatch`.

**Units, normative.**

| Quantity | Reported as |
| --- | --- |
| any count | `u64` integer |
| any rate | integer-floor basis points, `floor(value · 10000 / count)`, CC2's `scopes.rs:1317-1326` rule |
| any linear or encoded scalar | signed millionths, `round(v · 1_000_000)`, half away from zero |
| any angle | centidegrees, `0..=35999` |
| any Y′CbCr excursion | signed **delivery code units at the delivery bit depth**, ×100 (hundredths of a code) |
| PSNR | `Option<i32>` hundredths of a dB; `None` means MSE was exactly zero |

**Arithmetic precision, normative.** The QC engine computes `e = encode_bt709_delivery(linear)` in **f32**, matching the delivery clamp it predicts. Accumulators (sums, circular sums, MSE) are `f64`. Fixtures compare against `f64` transcriptions within `SPEC_F64_TOLERANCE = 1e-6` (`cc1_fixtures.rs:120`, which becomes `pub(crate)` so `cc6_fixtures.rs` can see it).

**Ordering.** Every collection in a `ColorQcReport` is emitted in a stated total order: channels are always `[red, green, blue]`; per-node contributions are in the core-owned document order of §3.7; exceptions are sorted by `(severity descending, code ascending, then the code's own tiebreak field ascending)`. Iteration over pixels is row-major from the top-left. **No reduction happens on the GPU** (CC2's non-goal, unchanged): every accumulator is a scalar CPU loop over the readback, with the same overflow checks `scopes.rs:1132-1160` uses.

### 3.2 Range ("legal")

The delivery clamp is the only clamp in the pipeline (CC1 §2.2 rule 5), and it lives in `quantize_delivery16` (`color_pipeline.rs:2445-2457`): `value.clamp(0.0, 1.0)`, then `× DELIVERY_INTERMEDIATE_WHITE` and round. So the *only* correct test for "this pixel will lose information at delivery" is on the value that function receives — the **delivery-encoded** value, with **no clamp applied**:

```text
e_c = encode_bt709_delivery(linear_c)      for c in {r, g, b}      (§3.0)
over_c  ⟺ e_c > 1.0
under_c ⟺ e_c < 0.0
```

Both comparisons are strict, so `e = 1.0` exactly (which is `linear = 1.0` exactly, since `1.099·1^0.45 − 0.099 = 1.000000` in both f64 and f32) is **not** an excursion.

**Decision taken (was draft D1): range and gamut are one pixel set seen from two sides, and the contract says so rather than reporting two independent findings.** `encode_bt709` is odd and strictly increasing, so `e < 0 ⟺ linear < 0`. `range` is a **per-channel, encoded-domain** measurement of *clamp events*; `gamut` (§3.3) is a **per-pixel, linear-domain** measurement of *representability* that adds a metric range cannot produce. **Over-range positive values are range excursions and are not gamut excursions** — a value above 1.0 is inside the Rec.709 chromaticity triangle and merely brighter than diffuse white. §11.2.2 asserts the identity so no consumer double-counts.

**f32 boundary wording.** `e > 1 ⟺ linear > 1` holds **for every f16-representable value**, which is the only kind the working proof carries: the next f16 above 1.0 is 1.0009765625, giving `e ≈ 1.000483`. It is not literally true for arbitrary f32 — at `linear = 1 + 1 ULP` the increment in `e` is ≈ 5.9e-8, under half a ULP, and `e` rounds back to exactly 1.0. The contract claims the f16 statement only.

```rust
pub struct ChannelRangeExcursion {
    pub over_pixel_count: u64,
    pub under_pixel_count: u64,
    pub over_basis_points: u32,
    pub under_basis_points: u32,
    /// max(e) − 1.0 over the region, millionths; 0 when nothing is over.
    pub maximum_over_excursion_millionths: i64,
    /// min(e, 0.0) over the region, millionths; 0 when nothing is under.
    pub minimum_under_excursion_millionths: i64,
}

pub struct ColorRangeReport {
    pub red: ChannelRangeExcursion,
    pub green: ChannelRangeExcursion,
    pub blue: ChannelRangeExcursion,
    /// Pixels with at least one clamped channel, in either direction.
    pub clamped_pixel_count: u64,
    pub clamped_basis_points: u32,
    /// The §3.4 prediction, in delivery code units at `bit_depth`.
    pub predicted_ycbcr: YCbCrLegalReport,
}
```

Hand-derived anchors, pinned in the manifest and asserted by §11.2.1 (f64; the f32 column is the engine's own value):

| linear | `e` (f64) | millionths | verdict |
| ---: | ---: | ---: | --- |
| `-0.02` | `-0.089999733` | −90 000 | under by 90 000 — **power branch** (E1) |
| `-0.01` | `-0.045000` | −45 000 | under by 45 000 (linear branch, `4.5·0.01`) |
| `-0.005` | `-0.022500` | −22 500 | under by 22 500 (linear branch, `4.5·0.005`) |
| `0.0` | `0.000000` | 0 | in range |
| `0.018` | `0.081247944035140462` | 81 248 | in range — **power branch** |
| `0.5` | `0.70551508992212120` | 705 515 | in range (interior anchor) |
| `1.0` | `1.000000` | 1 000 000 | in range — the boundary, strictly |
| `1.05` | `1.0243960098942206` | 1 024 396 | over by 24 396 |
| `1.2` | `1.093969260201581` | 1 093 969 | over by 93 969 |
| `2.0` | `1.4022782421730806` | 1 402 278 | over by 402 278 |

**The `0.018` branch is stated, not assumed.** Rust takes the *power* branch at exactly `0.018f32`, because the f32 literal `0.018` is `0.0179999992251396179` and `linear < 0.018` compares that value to itself → false. The f32 result is `0.0812479332`. BT.709's rounded constants make the function discontinuous there by `2.479e-4`; the fixture asserts the power branch and records the discontinuity so a future edit to the seam is visible.

**The negative anchors take the branch their magnitude selects, not the one their sign suggests** (E1). `encode_bt709_delivery` is odd — it encodes `|linear|` and restores the sign — so the seam test is on the magnitude: `−0.02` has `|linear| = 0.02 > 0.018` and therefore takes the **power** branch, giving `−0.089999733` in f64 and `−0.089999743` in f32, *not* the `4.5 · 0.02 = −0.09` the linear branch would give. The draft's table labelled that row "linear branch"; the two values agree to seven digits, which is exactly why the mislabel survived review and why `cc6_negative_range_anchors_take_the_power_branch` asserts the branch rather than the rounded value. `−0.01` and `−0.005` do take the linear branch.

**The seam discontinuity is 0.0543 codes on the 219-code limited luma span** (E2): `2.479e-4 × 219 = 0.0543`. It is *not* a fraction of a full-scale 255-code span, and no fixture may restate it as one.

`QC_RANGE_EXCEPTION_BASIS_POINTS = 10` (0.1 % of visible pixels). Below it the counts are still reported; at or above it a `delivery_range_excursion` exception is raised. The threshold is a constant rather than a parameter so two reports are comparable, and it exists so one ringing pixel does not raise a warning on every frame of a shot.

### 3.3 Gamut

**Gamut is representability, not brightness.** A linear Rec.709 triple is outside the Rec.709 chromaticity triangle exactly when a channel is negative.

```text
out_of_gamut(pixel) ⟺ min(linear_r, linear_g, linear_b) < 0.0
```

**Normative statement of the set relation:** the out-of-gamut pixel set is exactly the set of pixels with at least one under-range channel (§3.2). `ColorGamutReport` and `ColorRangeReport` therefore describe one set from two sides and **must not** be summed. The report says so in its own `definition` field.

What gamut adds is the *amount of colour* that is unrepresentable:

```text
Y = 0.2126·r + 0.7152·g + 0.0722·b        (linear luma, CC1's coefficients)
m = min(r, g, b)                           ( < 0 for an out-of-gamut pixel )
d = -m / (Y - m)                           desaturation fraction toward this pixel's own luma
```

**Theorem, normative.** `Y` is a convex combination with strictly positive weights, so `Y ≥ m` always, with equality iff `r = g = b`. Given `Y > 0` and `m < 0`, `d ∈ (0, 1]`: `d → 0⁺` as `m → 0⁻`, and `d = 1` exactly when `Y = 0`… which `Y > 0` excludes, so `d < 1` strictly for `Y > 0`, approaching 1 as `Y → 0⁺`. **`d` is only bounded when `Y > 0`**: for `m < Y < 0` it exceeds 1 and diverges as `Y → m⁺` (e.g. `(−0.02, −0.005, −0.005)` gives `Y = −0.008189`, `d = 1.693340`), and no blend toward luma can reach `min = 0` because the luma itself is negative.

Therefore: **`below_black_pixel_count` counts pixels with `Y < 0`**, those pixels are excluded from `maximum_desaturation_millionths`, and they are still counted in `out_of_gamut_pixel_count` (they are out of gamut; only the *metric* is undefined for them).

```rust
pub struct ColorGamutReport {
    pub out_of_gamut_pixel_count: u64,
    pub out_of_gamut_basis_points: u32,
    /// min(min(r, g, b), 0.0) over the region, millionths. Never positive.
    pub minimum_linear_millionths: i64,
    /// max over the region of `d`, millionths, over out-of-gamut pixels with `Y > 0`.
    pub maximum_desaturation_millionths: i64,
    /// Out-of-gamut pixels with `Y < 0`, excluded from the maximum above.
    pub below_black_pixel_count: u64,
    /// Fixed prose stating the §3.2/§3.3 set relation.
    pub definition: String,
}
```

**Reporting `d` is a measurement, not a mapping**: nothing applies it, and §13 defers gamut mapping explicitly. `QC_GAMUT_EXCEPTION_BASIS_POINTS = 10`, same reasoning as §3.2.

### 3.4 Y′CbCr limited-range reference

Core gains the forward BT.709 limited-range encoder Rust has never had (`color_pipeline.rs:39-42` carries only the *inverse* constants, used by `decode_bt709_ycbcr` at `color_pipeline.rs:451-492`; the forward direction has existed only inside swscale). It lives in `color_qc.rs` so core owns it without depending on media.

```rust
pub const BT709_KR: f64 = 0.2126;
pub const BT709_KB: f64 = 0.0722;
pub const BT709_CB_DENOMINATOR: f64 = 1.8556;   // = 2·(1 − Kb)
pub const BT709_CR_DENOMINATOR: f64 = 1.5748;   // = 2·(1 − Kr)
pub const YCBCR_LUMA_OFFSET: i32 = 16;
pub const YCBCR_LUMA_SPAN: i32 = 219;
pub const YCBCR_CHROMA_OFFSET: i32 = 128;
pub const YCBCR_CHROMA_SPAN: i32 = 224;

/// `bits` is 8 or 10; `s = 1 << (bits − 8)`.
pub fn bt709_limited_ycbcr(encoded_rgb: [f64; 3], bits: u8) -> [f64; 3];
```

```text
Y'      = Kr·R' + (1 − Kr − Kb)·G' + Kb·B'
Cb      = (B' − Y') / 1.8556
Cr      = (R' − Y') / 1.5748
s       = 2^(bits − 8)
Y_code  = 16·s + 219·s·Y'
Cb_code = 128·s + 224·s·Cb
Cr_code = 128·s + 224·s·Cr
```

**These are the exact inverses of the constants already in the tree.** `Kb·1.8556/Kg = 0.1873242729306488` and `Kr·1.5748/Kg = 0.46812427293064884`, which are `BT709_GREEN_FROM_CB = -0.187_324` and `BT709_GREEN_FROM_CR = -0.468_124` (`color_pipeline.rs:40-41`) to within 2.73e-7; `BT709_RED_FROM_CR = 1.5748` and `BT709_BLUE_FROM_CB = 1.8556` are the same denominators. Those four constants become `pub(crate)` so `cc6_fixtures.rs` can reference them. §11.2.3 asserts the round trip within `SPEC_F64_TOLERANCE = 1e-6` against an **independent `f64` transcription of the inverse matrix**, not against `decode_bt709_ycbcr` itself (E11): that function is private to `kinewright-media`, and the round trip is asserted in `cc6_core.rs`, which cannot see it. The transcription is permitted — rule 11.0.1 forbids obtaining an *expected value* by calling the code under test, and the inverse is a different function from the forward one under test — and it is the stronger check of the two, because a matched pair of errors in a forward/inverse pair from the same file would cancel. Because the inverse takes **normalized `[0,1]` samples** (it multiplies by `max_code = (1<<bits)−1` internally), the round trip divides the codes above by `2^bits − 1` before feeding them back, and the contract states that conversion rather than leaving it to the implementer. The four constants are separately asserted equal to `color_pipeline.rs`'s within `1e-6`, which is what pins the transcription to the tree.

`YCbCrLegalReport` counts, per plane, samples outside `[16·s, 235·s]` for Y and `[16·s, 240·s]` for Cb and Cr, with the excursion extremes in hundredths of a code:

```rust
pub struct PlaneLegalExcursion {
    pub below_count: u64, pub above_count: u64,
    pub below_basis_points: u32, pub above_basis_points: u32,
    /// The lowest **observed sample code** on this plane, hundredths of a code.
    pub minimum_code_hundredths: i64,
    /// The highest **observed sample code** on this plane, hundredths of a code.
    pub maximum_code_hundredths: i64,
}

impl PlaneLegalExcursion {
    pub const UNSEEN_MINIMUM_CODE_HUNDREDTHS: i64 = i64::MAX;
    pub const UNSEEN_MAXIMUM_CODE_HUNDREDTHS: i64 = i64::MIN;
    /// `minimum <= maximum`: whether any sample reached this plane at all.
    pub const fn samples_seen(&self) -> bool;
}
pub struct YCbCrLegalReport {
    pub bit_depth: u8,        // 8 or 10
    pub luma: PlaneLegalExcursion,
    pub cb: PlaneLegalExcursion,
    pub cr: PlaneLegalExcursion,
    pub source: YCbCrLegalSource,   // Predicted | DecodedNativePlanes
}
```

**The extremes are observed sample codes, not excursion amounts, normative** (E5). `minimum_code_hundredths` and `maximum_code_hundredths` are the lowest and highest sample the plane actually saw, in hundredths of a delivery code; the *excursion* is that extreme minus the plane's bound. The `linear = 1.05` anchor therefore pins `maximum_code_hundredths = 24034` at 8 bits (`240.342726`, an excursion of `5.343` codes over `235`) and `96137` at 10 bits (`961.370905`, `21.371` over `940`). The draft left the sense unstated, and a reader could reasonably have taken `24034` for the excursion.

**A plane that saw no sample reports the empty interval, normative.** `minimum_code_hundredths = UNSEEN_MINIMUM_CODE_HUNDREDTHS = i64::MAX` and `maximum_code_hundredths = UNSEEN_MAXIMUM_CODE_HUNDREDTHS = i64::MIN`, which no real sample set can produce, so `samples_seen()` recovers the fact from the two numbers themselves. `0` is refused as the unseen value because it is indistinguishable from a plane whose worst sample really did land on code `0` — a legible-looking number nothing measured. The flag is **not** a struct field: `PlaneLegalExcursion` is constructed by name outside core (the media verifier's decoded planes, and agent and app fixtures), and a new field would silently invalidate those literals. In a `Predicted` report the unseen case arises only when every visible pixel in the region was non-finite and was excluded (§3.1), which the report states in `non_finite_pixel_count` and refuses to pass.

**The excursion rate is a method, not a field**:

```rust
/// `floor((below_count + above_count) · 10000 / sample_count)`, §10.1's rule.
pub const fn excursion_basis_points(&self, sample_count: u64) -> u32;
```

§6.4's threshold is compared against **this** number and against neither `below_basis_points` nor `above_basis_points`: a plane that wanders 60 bp below the floor and 60 bp above the ceiling is 120 bp out of legal, and a rule that looked at one direction at a time would call it clean. `sample_count` is an argument rather than a field for the same reason `samples_seen` is not one — `PlaneLegalExcursion` is constructed by name outside core, and a new field would silently invalidate those literals. An empty population is `0`. **§6.4's gate and the fixture's prediction call the same method**, so the contract's rule and the test's expectation cannot be two different expressions of the same arithmetic.

It is used in **two different senses, and the difference is the point**:

1. **`Predicted`** — computed from the *unclamped* delivery-encoded R′G′B′ of the working proof. Because `R'G'B' ∈ [0,1]³ ⟹ Y ∈ [16s, 235s] ∧ Cb, Cr ∈ [16s, 240s]` exactly (every extreme is attained), the predicted excursion *set* is the §3.2 range set. What it adds is the **magnitude in delivery code units** and the **attribution to luma versus chroma** — an excursion may be luma-only, chroma-only, or both, and that depends on the matrix, which the RGB test cannot see. Anchor: `linear = 1.05` on all three channels gives `e = 1.0243960098942206`, `Y_8 = 240.342726` (5.343 codes over 235) and `Y_10 = 961.370905` (21.371 over 940), with `Cb = Cr = 128 / 512` exactly.
2. **`DecodedNativePlanes`** — measured from the decoded file's actual Y, Cb, and Cr planes (§6.4). This is a genuinely independent measurement: **codec ringing and rounding can push a plane outside the legal box even after a perfectly legal encode**, and nothing in the prediction can see it.

Pinned anchors for §11.2.3, derived by hand from the equations above:

| R′G′B′ | Y (8) | Cb (8) | Cr (8) | Y (10) | Cb (10) | Cr (10) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `(1,1,1)` | 235.000000 | 128.000000 | 128.000000 | 940.000000 | 512.000000 | 512.000000 |
| `(0,0,0)` | 16.000000 | 128.000000 | 128.000000 | 64.000000 | 512.000000 | 512.000000 |
| `(0.5,0.5,0.5)` | 125.500000 | 128.000000 | 128.000000 | 502.000000 | 512.000000 | 512.000000 |
| `(1,0,0)` | 62.559400 | 102.335848 | **240.000000** | 250.237600 | 409.343393 | **960.000000** |
| `(0,1,0)` | 172.628800 | 41.664152 | 26.269749 | 690.515200 | 166.656607 | 105.078994 |
| `(0,0,1)` | 31.811800 | **240.000000** | 117.730251 | 127.247200 | **960.000000** | 470.921006 |
| `(1,1,0)` | 219.188200 | **16.000000** | 138.269749 | 876.752800 | **64.000000** | 553.078994 |
| `(0,1,1)` | 188.440600 | 153.664152 | **16.000000** | 753.762400 | 614.656607 | **64.000000** |

The bold values sit exactly on a legal chroma bound and are therefore **not** excursions under the strict `>` / `<` tests — maxima from red and blue, minima from yellow and cyan, the sharpest possible check that neither bound is off by one.

### 3.5 Skin diagnostics

**Loud statement, carried verbatim in every response and in the QC window: this is a diagnostic of a region the user chose. It is not a skin detector, it does not find faces, and it makes no claim about whether a skin tone is good.** CC5's qualifier fixture already says CC6 owns skin QC (`cc5_fixtures.rs:4667-4673`); CC6 owns the *measurement*, not the judgement.

Region: an ROI in basis points, or a CC5 matte scope — CC6 invents no stage and no new region type.

Per visible pixel in the region:

```text
e      = ( encode_bt709_delivery(r), encode_bt709_delivery(g), encode_bt709_delivery(b) )   unclamped
Y'     = 0.2126·e.r + 0.7152·e.g + 0.0722·e.b
Cb     = (e.b − Y') / 1.8556
Cr     = (e.r − Y') / 1.5748
chroma = sqrt(Cb² + Cr²)
θ      = atan2(Cr, Cb)   in degrees, wrapped to [0, 360)
```

`θ` is measured counter-clockwise from the **+Cb axis**, which reproduces the conventional vectorscope graticule. The convention is confirmed by derivation, not assumed: the NTSC `+I` direction is `(−sin 33°, cos 33°)` in `(Cb, Cr)`, so `θ(+I) = 123.0000°` exactly, and the Rec.709 red primary lands at `102.906186°`. The real BT.709 matrix is used deliberately — **not** CC2's integer vectorscope axes `U = B − R`, `V = 2G − R − B` (`scopes.rs:1366-1373`), which are a display convenience with a different geometry.

**Near-achromatic exclusion, pinned.** A pixel with `chroma · 1_000_000 < SKIN_MIN_CHROMA_MILLIONTHS` contributes to `excluded_achromatic_pixel_count` and to nothing else, because `atan2` on a near-zero vector is dominated by quantization noise.

```text
SKIN_MIN_CHROMA_MILLIONTHS = 20_000            (chroma 0.02)
```

Derivation, resting only on arithmetic that is checked: `0.02 · 224 = 4.48` eight-bit code units of excursion from 128, so the floor is a few codes rather than a fraction of one; and the least saturated CC5 skin patch, `skin_deep`, measures `chroma = 0.073341`, **3.67×** the floor. No claim is made about how far a chroma plane wanders from rounding and subsampling, because CC6 does not measure that.

Reported (all integers):

```rust
pub struct SkinDiagnostics {
    pub region_pixel_count: u64,
    pub considered_pixel_count: u64,                // chroma at or above the floor
    pub excluded_achromatic_pixel_count: u64,
    pub mean_hue_centidegrees: Option<i32>,         // circular mean, None when considered == 0
    pub hue_concentration_millionths: i64,          // R = |mean resultant vector|, 0..1_000_000
    pub circular_spread_centidegrees: i32,          // see below
    pub median_chroma_millionths: i64,
    pub in_band_basis_points: u32,
    pub band_center_centidegrees: i32,
    pub band_half_width_centidegrees: i32,
    pub boundary: String,                           // the statement above, verbatim
}
```

- circular mean: `θ̄ = atan2(Σ sin θ, Σ cos θ)`, wrapped to `[0, 360)`, rounded to centidegrees.
- `R = sqrt((Σ cos θ)² + (Σ sin θ)²) / n`, **clamped to `[0, 1]` before any logarithm** — the unclamped quotient can exceed 1.0 in f64 for a uniform patch, which would make the spread `NaN`.
- circular spread: `σ = degrees(sqrt(−2·ln R))`, in centidegrees, **capped at 18000**; `R == 0` and `considered == 0` both report exactly `18000`. The cap is normative because `−2·ln R` diverges and a diagnostic must not print an unbounded number.
- median chroma: the lower median (element `⌊(n−1)/2⌋` of the ascending sort over *considered* pixels) so it is deterministic for even `n`, CC2's percentile convention.
- **`in_band_basis_points`' denominator is `considered_pixel_count`**, not the region: an achromatic pixel has no hue and cannot be in or out of a hue band. When `considered_pixel_count == 0` the value is `0`, `mean_hue_centidegrees` is `None`, and **no `skin_region_outside_band` exception is raised** — an all-achromatic region produces no hue evidence at all, and reporting it as "outside the band" would be a fabricated finding.

**Band constants, derived from CC5's four skin patches.** The patches are stored in `grade709` (`cc5_fixtures.rs:4603-4625`, `CHART_PATCHES`). Transforming each through `grade709_decode` (`color_pipeline.rs:977-1000`) to scene-linear and then through `encode_bt709_delivery` and the equations above gives:

| CC5 patch | grade709 | display-encoded R′G′B′ | Cb | Cr | θ | chroma |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| `skin_light` | `(0.85, 0.68, 0.60)` | `0.850041, 0.680086, 0.600108` | −0.059461 | 0.088644 | 123.8532° | 0.106740 |
| `skin_medium` | `(0.72, 0.53, 0.44)` | `0.720076, 0.530127, 0.440151` | −0.066751 | 0.099100 | 123.9631° | 0.119484 |
| `skin_tan` | `(0.55, 0.38, 0.30)` | `0.550121, 0.380167, 0.300189` | −0.059461 | 0.088644 | 123.8532° | 0.106740 |
| `skin_deep` | `(0.32, 0.20, 0.15)` | `0.320184, 0.200216, 0.150229` | −0.038738 | 0.062276 | 121.8835° | 0.073341 |

`skin_light` and `skin_tan` genuinely share an angle and a chroma: their grade709 triples differ by the constant vector `(0.30, 0.30, 0.30)`, so their encoded channel differences — the only inputs to `Cb` and `Cr` — agree to within 1e-6. It is not a transcription error, and the fixture asserts the shared values with a stated ±1-millionth chroma tolerance.

```text
SKIN_PATCH_HUE_CENTIDEGREES: [i32; 4] = [12_385, 12_396, 12_385, 12_188]
SKIN_BAND_CENTER_CENTIDEGREES        = 12_339     // circular mean of the four, R = 0.999885
SKIN_BAND_HALF_WIDTH_CENTIDEGREES    =  1_200     // 12.00°
SKIN_BAND_EXCEPTION_BASIS_POINTS     =  5_000
```

The centre is the circular mean of the four patches, not a borrowed number; that it lands within `0.39°` of the derived NTSC `+I` axis at exactly `123.0000°` is corroboration, recorded as such. The half-width is `12°`, giving the band `[111.39°, 135.39°]`, with these measured margins, every one asserted by §11.2.4:

| item | θ (centidegrees) | position |
| --- | ---: | --- |
| `skin_deep` | 12 188 | 1 049 cd (10.49°) inside the lower edge — the tightest patch |
| `skin_medium` | 12 396 | 1 143 cd (11.43°) inside the upper edge |
| `skin_light`, `skin_tan` | 12 385 | 1 154 cd (11.54°) inside the upper edge |
| NTSC `+I` axis | 12 300 | inside |
| Rec.709 red primary | 10 291 | **848 cd (8.48°) outside** |
| CC5 `product_red` | 10 137 | **1 002 cd (10.02°) outside** |
| CC5 `product_cyan` | 29 201 | far outside |

A `skin_region_outside_band` exception (severity **Info**) is raised when fewer than half the *considered* pixels fall in the band. Info, not Warning, because a chosen region that is not skin is a user choice, not a fault.

### 3.6 Delivery tags

`DeliveryColorMismatch` (`crates/kinewright-core/src/delivery.rs:424-427`) becomes **public and typed**, and `delivery_color_mismatch` (`delivery.rs:430`) gains an all-fields sibling:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeliveryColorMismatch {
    pub field: String,          // "primaries" | "transfer" | ... , stable wire names
    pub observed: String,
    pub allowed: String,
}

/// Every mismatching field, in the fixed check order. Empty means conforming.
pub fn delivery_color_mismatches(color: &ColorDescription) -> Vec<DeliveryColorMismatch>;
/// The first mismatch, unchanged in behaviour.
pub fn delivery_color_mismatch(color: &ColorDescription) -> Option<DeliveryColorMismatch>;
```

The check order is unchanged: primaries → transfer → matrix → range → white_point → bit_depth → provenance → confidence. `delivery_conformance` (`delivery.rs:237-295`) keeps emitting exactly one `unsupported_delivery_color` `QaIssue` formatted from the first mismatch; the vector is the new, additional surface. The stale doc note at `delivery.rs:417-422` ("QaIssue has no structured detail map") is updated to point at the typed accessor.

**The two rejection messages change, and the change is named.** Once `Eight | Ten` are both accepted, `delivery.rs:276`'s *"Current libx264/YUV420P export requires explicit 8-bit SDR Rec.709 delivery colour metadata: …"* and `export.rs:679`'s *"…requires explicit 8-bit SDR Rec.709 (…)"* both become **"8-bit or 10-bit SDR Rec.709"**. No fixture may assert either string is byte-identical to its pre-CC6 form; the affected tests are `delivery.rs:1272` (`unsupported_delivery_color_names_the_mismatching_field_observed_and_allowed`), `delivery.rs:1299` (`…reports_only_the_first_mismatching_field`), and `export.rs:856`'s substring check inside `rejects_non_rec709_delivery_metadata`, and rewriting them is a named step in §12.

The QC side compares an **expected** delivery description against an **observed** one, in one of two modes:

```rust
pub struct DeliveryTagNotRepresentable {
    pub field: String, pub expected: String, pub reason: String,
}
pub struct DeliveryTagCheck {
    /// "materialised_export_settings" (pre-export) | "probed_output_file" (post-export).
    pub tag_source: String,
    pub expected: ColorDescription,
    pub observed: ColorDescription,
    pub mismatches: Vec<DeliveryColorMismatch>,       // every mismatching field
    pub not_representable: Vec<DeliveryTagNotRepresentable>,
    pub conforming: bool,                              // mismatches.is_empty()
}
pub fn delivery_tag_check(expected: &ColorDescription, observed: &ColorDescription,
                          tag_source: DeliveryTagSource) -> DeliveryTagCheck;
```

- **Pre-export mode** (`tag_source = "materialised_export_settings"`). `get_color_qc` and the QC window have no file to probe. `expected` is `ExportSettings.delivery_color` materialised from the document and the selected `DeliveryEncodeDepth` (`delivery_color_for_depth`, §3.0), `observed` is the same value, and `mismatches` is `delivery_color_mismatches(&expected)` — i.e. the check answers "would this document's delivery description be accepted by the gates at this depth?". `not_representable` is empty: nothing has been probed.
- **Post-export mode** (`tag_source = "probed_output_file"`). `observed` is the probed description of the written file, and the white-point rule below applies.

**The white-point rule, normative.** H.264/AVC's VUI carries `colour_primaries`, `transfer_characteristics`, and `matrix_coefficients` and **no white-point field**; `probe_path` correctly reports `ColorWhitePoint::Unknown` for every H.264 stream (`decode.rs:158-161`, asserted by the CC0 fixture at `generated_media.rs:541`). In post-export mode `delivery_tag_check` therefore reports `white_point` as **not representable**, never as a mismatch:

```text
field = "white_point", expected = "d65",
reason = "H.264/AVC carries colour_primaries, transfer_characteristics, and matrix_coefficients
          but no white-point field; bt709 primaries imply D65"
```

Severity **Info**. `provenance` and `confidence_basis_points` are likewise excluded from the post-export mismatch list: a probed description necessarily carries `StreamMetadata` provenance, which is *correct* for a decoded file and would be a false failure. This closes gap #9 of the delivery facts sheet — a re-probed export cannot satisfy `delivery_color_mismatch` itself, so **`delivery_color_mismatch` must never be applied to a probed description**, and `delivery_tag_check` is the only function that may be. §11.2.9 asserts both halves and both modes.

### 3.7 Per-node contribution

The CC4 deferral "this look clips" made measurable. **Optional, bounded, single-frame, and not part of `measure_color_qc`.** It lives in `color_qc::nodes`; the §3 purity claim does not extend to it.

```rust
pub const MAX_QC_NODE_CONTRIBUTIONS: usize = 16;

pub struct ColorNodeQcContribution {
    pub clip: ClipId,
    pub effect: EffectId,
    pub node_kind: String,
    pub active: bool,
    pub inactive_reason: Option<String>,           // core::effect::color_node_inactive_reason
    /// with-all minus with-this-node-removed. Positive = this node adds clipping.
    pub range_basis_points_delta: i32,
    pub gamut_basis_points_delta: i32,
}

pub fn measure_node_contributions(
    analysis: &dyn Analysis,
    document: &Arc<Document>,
    at: TimeCode,
    request: &ColorQcRequest,
) -> Result<ColorQcNodeContributions, MediaError>;

/// The one entry point a per-node consumer calls (E12).
///
/// Renders the baseline working proof **once** and returns both the full
/// `ColorQcReport` measured on it and that proof's metadata, so no caller
/// renders a second baseline of its own. `N` candidates therefore cost
/// `N + 1` renders, not `N + 2`.
pub fn measure_color_qc_with_nodes(
    analysis: &dyn Analysis,
    document: Arc<Document>,
    at: TimeCode,
    request: &ColorQcRequest,
) -> Result<(ColorQcReport, WorkingProofMetadata), MediaError>;
```

**Method: removal, not bypass.** One baseline `working_proof_for_document` on the document as stored. Then, for each candidate node, a **scratch document** — a clone with that node's effect removed by `Operation::RemoveEffect { clip, effect }` (`operation.rs:242`, applied through `Operation::apply` at `operation.rs:1014`, the same operation the inspector's Remove button sends) — is rendered and measured, and the delta of the region's range and gamut basis points is recorded. Removal is used because **`primary_correction` has no `bypass` parameter** (`PRIMARY_CORRECTION_PARAMETERS`, `effect.rs:562-628`; `effect.rs:2315-2317` states it), so a bypass-based method could not attribute the most common colour node in the tree, and adding the parameter is forbidden by §9.1. `attribution` is always the string `"node_removed"` so a consumer cannot mistake the method.

**Candidates** are every colour node kind active at this frame (`color_node_inactive_reason` returns `None`, `effect.rs:2319-2347`). Inactive nodes are listed with `active: false`, their `inactive_reason`, and both deltas exactly `0` — removing something already inactive must produce no delta, and §11.2.14 asserts it.

**Ordering**, normative and **core-owned**: document track order, then clip order within a track, then effect-chain order within a clip. Core cannot depend on `kinewright-media`, so it must not reach for `visual_layers_at`; §11.2.14 asserts that this ordering agrees with production z-order on a multi-track document, so the two cannot drift.

**Cost bound**, normative: at most `MAX_QC_NODE_CONTRIBUTIONS = 16` scratch renders plus one baseline — 17 full-resolution renders for 17 candidates, and `N + 1` for `N` — plus up to 16 `Arc<Document>` deep clones, which is the real memory cost and is stated rather than hidden. Beyond 16 candidates the list is truncated in the stated order, `truncated: true` and `considered_node_count` are reported, and a `qc_per_node_truncated` Info exception is raised, the same "state the omission" discipline CC5 §5.2 applies to dropped tracker samples. The check is **off by default** in `get_color_qc` (§7) and in the app. The live document is never touched; §11.2.14 asserts it is byte-identical afterwards.

### 3.8 `ColorQcReport`, exceptions, severities

```rust
pub struct ColorQcReport {
    pub stage: String,                       // WORKING_PROOF_STAGE
    pub full_resolution: bool,               // always true; a false proof is refused before measuring
    pub raster: (u32, u32),
    pub project_frame: i64,
    pub region: ColorQcRegion,
    pub visible_pixel_count: u64,
    /// Visible pixels whose linear or encoded sample was not finite (§3.1):
    /// counted, fed to no accumulator, and raised as an Error.
    pub non_finite_pixel_count: u64,
    pub transparent_pixel_count: u64,        // always 0 at this stage (§3.1)
    pub delivery_bit_depth: u8,              // 8 or 10; selects the §3.4 scale
    pub range: ColorRangeReport,
    pub gamut: ColorGamutReport,
    pub skin: Option<SkinDiagnostics>,
    pub tags: Option<DeliveryTagCheck>,
    pub nodes: Option<ColorQcNodeContributions>,
    pub exceptions: Vec<ColorQcException>,
    /// No `Error`-severity exception. Warnings do not clear it.
    pub technical_pass: bool,
    pub evidence_only: bool,                 // always true
    pub provenance: ColorQcProvenance,       // engine name, accumulator precision, ordering rules
}

pub struct ColorQcException {
    pub code: String,
    pub severity: QaSeverity,                // core::qa::QaSeverity, reused
    pub message: String,
    pub field: Option<String>,
    pub observed: Option<String>,
    pub allowed: Option<String>,
    pub clip: Option<ClipId>,
    pub effect: Option<EffectId>,
}
```

**`ColorQcReport` carries no `verification` field.** A verification can only be produced by `verify_delivery_output` against a written file, which `measure_color_qc` has no access to; it lives on `ExportJobRecord` (§6.5) and in the export dialog (§8.4), and nowhere else.

**Exception codes, with severity. Every code in this table appears in no other table.**

| Code | Severity | Raised when | Owner |
| --- | --- | --- | --- |
| `delivery_range_excursion` | Warning | any channel's over or under bp ≥ `QC_RANGE_EXCEPTION_BASIS_POINTS` | `ColorQcReport` |
| `delivery_gamut_excursion` | Warning | out-of-gamut bp ≥ `QC_GAMUT_EXCEPTION_BASIS_POINTS` | `ColorQcReport` |
| `delivery_tag_mismatch` | **Error** | `DeliveryTagCheck.mismatches` is non-empty | `ColorQcReport`, `DeliveryVerification` |
| `delivery_tag_not_representable` | Info | `not_representable` is non-empty (post-export mode only) | `ColorQcReport`, `DeliveryVerification` |
| `color_qc_non_finite_sample` | **Error** | `non_finite_pixel_count > 0` (§3.1) | `ColorQcReport` |
| `skin_region_outside_band` | Info | `in_band_basis_points < SKIN_BAND_EXCEPTION_BASIS_POINTS` and `considered_pixel_count > 0` | `ColorQcReport` |
| `qc_per_node_truncated` | Info | more than `MAX_QC_NODE_CONTRIBUTIONS` candidate nodes | `ColorQcReport` |
| `decoded_range_excursion` | Warning | §6.4's EBU R 103 rule trips | `DeliveryVerification` |
| `decoded_difference_over_budget` | **Error** | any gated §6.3 budget is exceeded | `DeliveryVerification` |

**There is no `verification_unavailable` exception** (E31, was a draft row in this table). A job that completed without a verification has no `DeliveryVerification` to hang an exception list on, and inventing one on `ExportJobRecord` would have been a second, parallel exception surface for exactly one code. `ExportJobRecord.verification_unavailable_reason: Option<String>` is the **sole carrier**, and the export dialog renders `NOT VERIFIED` from it (§6.5, §8.4).

**Why range and gamut are Warnings and tags are Errors.** The roadmap's exit evidence requires that "warnings distinguish intentional creative excursions from accidental technical failures". A blown highlight is frequently a deliberate creative choice; a mis-tagged file is never a creative choice, and it will be misinterpreted by every downstream tool. `technical_pass` is therefore "no `Error`", and a report with fifty thousand clipped pixels and correct tags passes — loudly, with the counts on screen. `ColorQcReport` deliberately does **not** reuse the name `export_ready`: `QaReport::export_ready()` (`qa.rs:37-56`) gates an export, and a QC report must never gate one.

**Typed refusals. Every code in this table appears in no other table, and each carries `code`, `field`, `observed`, and `allowed`.**

| Code | Type | Raised when |
| --- | --- | --- |
| `color_qc_proxy_proof_refused` | `ColorQcError::ProxyProofRefused` | `metadata.render.full_resolution == false` |
| `color_qc_raster_length_mismatch` | `ColorQcError::RasterLengthMismatch` | `pixels.len() != width·height·4` |
| `color_qc_region_empty` | `ColorQcError::EmptyPopulation` | the resolved region contains no pixel |
| `color_qc_node_budget_exceeded` | `ColorQcError::NodeBudgetExceeded` | `max_nodes` outside `1..=16` |
| `color_qc_matte_region_raster_mismatch` | `ColorQcError::MatteRegionRasterMismatch` | coverage raster ≠ proof raster, **or** the coverage buffer's length ≠ `w · h · 4` |
| `color_qc_region_required` | agent (`color_qc_tool.rs`) | `checks` contains `skin` with no `roi` and no `matte_region` |
| `color_qc_frame_selector_conflict` | agent (`color_qc_tool.rs`) | more than one of `timecode`, `frame`, `clip_id` is sent; `detail` names the first offending selector |
| `color_qc_frame_out_of_range` | agent (`color_qc_tool.rs`) | the resolved project frame is negative, at or past `document.duration`, or outside the named clip |
| `working_proof_unavailable` | agent (`color_qc_tool.rs`) | this build's renderer cannot produce a working proof at all |
| `unsupported_delivery_codec` | `DeliveryColorError::UnsupportedCodec` | `video_codec != "libx264"` |
| `unsupported_delivery_color` | `DeliveryColorError::UnsupportedField` | any delivery description field is out of contract |
| `delivery_pixel_format_depth_mismatch` | `DeliveryColorError::PixelFormatDepthMismatch` | the negotiated pixel format does not carry the declared depth |
| `delivery_encoder_pixel_format_unavailable` | `DeliveryColorError::EncoderPixelFormatUnavailable` | the build's libx264 does not offer the lane's pixel format |
| `delivery_verification_not_full_resolution` | `DeliveryVerificationError::NotFullResolution` | a sampled reference render is not full-resolution |
| `delivery_verification_plane_out_of_container` | `DeliveryVerificationError::PlaneOutOfContainer` | a 10-bit native sample exceeds 1023 |
| `delivery_verification_frame_count_mismatch` | `DeliveryVerificationError::FrameCountMismatch` | the output's **presented** frame count ≠ the document's implied count; a sampled frame does not decode; the decoded picture is not the requested frame; or the sample set is empty (§6.2) |
| `delivery_verification_frame_count_out_of_range` | `DeliveryVerificationError::FrameCountOutOfRange` | `DeliveryVerificationRequest.frame_count` outside `1..=16` (§3.0, §6.2) |
| `delivery_verification_budget_lane_mismatch` | `DeliveryVerificationError::BudgetLaneMismatch` | `request.budgets != DeliveryBudgets::for_depth(settings' depth)` (§6.3) |
| `unsupported_stage` (agent) / `ScopeError::UnsupportedStage { stage }` (core) | existing | a non-measurable `ScopeStage` reaches the scope engine (§2.1) |
| `stale_revision` (agent, existing) | existing envelope | `expected_revision != timeline_revision` |

`ColorQcError` mirrors `MatteCoverageError` (`media.rs:555-604`): a `const fn code(&self) -> &'static str`, and `{ observed, allowed }` on every variant.

**QC refusals cross the media boundary structurally, not as prose** (E32). `MediaError` gains `#[error(transparent)] ColorQc(#[from] ColorQcError)`, and `MediaError::recovery_code()` returns `error.code()` for it. `color_qc::nodes` — which returns `MediaError` because it renders — therefore propagates a `ColorQcError` with `?` and keeps its typed code, and the agent branches on `recovery_code()` or on the variant to fill `code` / `field` / `observed` / `allowed` / `recovery_action`. **No consumer parses a `Display` string to recover a QC code**; the draft's `MediaError::Backend(String)` round trip is deleted, and `cc6_qc_refusals_keep_their_code_through_media_error` table-drives every `ColorQcError` variant through `?` to prove it.

---

## 4. Managed delivery: tags, transforms, and the 10-bit lane

### 4.1 `DeliveryEncodeDepth` — one orthogonal enum, not eight profiles

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryEncodeDepth {
    #[default] Eight,
    Ten,
}

impl DeliveryEncodeDepth {
    pub const ALL: [Self; 2];
    pub const fn as_str(self) -> &'static str;              // "eight" | "ten"
    pub const fn bits(self) -> u8;                          // 8 | 10
    pub const fn color_bit_depth(self) -> ColorBitDepth;    // Eight | Ten
    pub const fn pixel_format(self) -> &'static str;        // "yuv420p" | "yuv420p10le"
}
```

There is no `x264_profile()`: the pixel format selects High 10, measured byte-identical with and without `-profile:v high10` on the pinned build (§5, P5).

`DeliveryProfile::export_settings` (`delivery.rs:76-105`) gains the depth as an explicit second argument:

```rust
pub fn export_settings(self, document: &Document, depth: DeliveryEncodeDepth,
                       cancellation: ExportCancellation) -> ExportSettings
```

and sets `delivery_color = { ..document.color_context.delivery.clone(), bit_depth: depth.color_bit_depth() }`.

**Decision taken (was draft D2): the depth lives on `ExportSettings`, not in the project document.** `ColorContext::sdr_rec709()` (`color.rs:892-914`) pins the project's delivery description to 8-bit; the **document's `color_context.delivery` stays 8-bit** and `DeliveryEncodeDepth` selects the depth when `ExportSettings` is materialized, so `get_color_context` keeps reporting the project contract while a 10-bit master is still exportable without a `SetColorContext`. The consequence is normative: `get_color_context.color_context.delivery.bit_depth` and a 10-bit job's `settings.delivery_color.bit_depth` legitimately differ, and **the QC tag check compares against `ExportSettings`, never against the document**.

**Why an orthogonal enum.** A `DeliveryProfile` names a *composition* — raster, aspect, bitrate, platform. A bit depth names an *encoding precision*. Making depth a profile variant would take the vocabulary from four names to eight now and to sixteen the moment a second codec arrives. An orthogonal enum keeps every existing `DeliveryProfile` wire string byte-identical, so pre-CC6 `ExportJobRecord`s, `queue_export` arguments, `DeliveryConformanceReport`s, and the app combo box all deserialize unchanged. Rejected alternative: a `bit_depth` field *inside* `ExportSettings` alongside `delivery_color`. Rejected because `delivery_color.bit_depth` would then be a second authority for the same fact; `ExportSettings.delivery_color.bit_depth` is the single authority and the encoder reads it.

**The depth ripples through conformance, and every site is named.** `export_settings` has ten call sites: `delivery.rs:249` (inside `delivery_conformance`), `delivery.rs:805/909/924/940` (tests), `export.rs:803` (test), `cc1_fixtures.rs:3391/3452`, `server.rs:3324` (`get_delivery_profiles`), `export_queue.rs:642`. Therefore:

- `delivery_conformance(document, profile, depth, h, v)` gains the depth, and `DeliveryConformanceReport` gains `pub delivery_bit_depth: DeliveryEncodeDepth`;
- `get_delivery_conformance` (`server.rs:1028`) and the queue's `Conformance` preflight pass it through;
- the app's `ConformanceKey { revision, aspect, focus_x/y, width, height }` (`export_ui.rs:53-62`) gains `delivery_bit_depth`, so a cached 8-bit report can never be served for a 10-bit export. §11.2.20 asserts the cache does not cross lanes.

**Widened acceptance.** `delivery_color_mismatch`'s `bit_depth` leg becomes `Eight | Ten` with `allowed = "8 or 10 (named eight/ten or integer 8/10)"`, keeping `ColorBitDepth`'s canonical equality so `Integer(10)` is accepted exactly as `Integer(8)` is today (`color.rs:402-467`). Full range, non-BT.709 primaries/transfer/matrix, non-D65, and every other depth stay rejected with typed reasons.

**`delivery_conformance` can no longer reject `bit_depth`, and the fixture is moved rather than weakened** (E4). Once the depth is an argument and `export_settings` materialises `delivery_color.bit_depth` from it, every description `delivery_conformance` inspects carries a depth the caller just chose, so the `bit_depth` leg is **unreachable through that path**. That is a consequence of making the depth the single authority, not a hole: §11.2.9 asserts the `bit_depth` leg directly on `delivery_color_mismatch` / `delivery_color_mismatches`, where a twelve-bit description still produces the typed mismatch with all three fields. A fixture that asserted it through `delivery_conformance` would have been asserting something the production path can no longer reach.

### 4.2 Typed delivery rejection

`crates/kinewright-core/src/delivery.rs` gains, modelled on `ColorSourceError` (`color.rs:205-345`):

```rust
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DeliveryColorError {
    UnsupportedCodec { observed: String, allowed: &'static str },
    UnsupportedField(DeliveryColorMismatch),
    PixelFormatDepthMismatch { observed: String, allowed: String },
    EncoderPixelFormatUnavailable { observed: String, allowed: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DeliveryVerificationError {
    NotFullResolution { observed: String, allowed: &'static str },
    PlaneOutOfContainer { observed: String, allowed: &'static str },
    FrameCountMismatch { observed: String, allowed: String },
    FrameCountOutOfRange { observed: String, allowed: &'static str },
    BudgetLaneMismatch { observed: String, allowed: String },
}
```

Both expose `code()`, `field()`, `observed()`, `allowed_values()`, `recovery_action()`, and `actionable_message()`, with the codes of §3.8's second table. `MediaError` gains `#[error(transparent)] DeliveryColor(#[from] DeliveryColorError)`, `#[error(transparent)] DeliveryVerification(#[from] DeliveryVerificationError)`, and `#[error(transparent)] ColorQc(#[from] ColorQcError)` (§3.8). `validate_delivery_color` / `validate_delivery_description` (`media/src/export.rs:650-686`) return them instead of `MediaError::Backend(String)`, so a rejection carries `code`, `field`, `observed`, and `allowed` the way a source rejection has since CC0.

**`validate_delivery_description` emits no prose message of its own** (E19). It returns the typed error and nothing else; the sentence a human reads is `actionable_message()`, composed from the four fields at the surface that displays it. A hand-written second sentence at the refusal site would be a second, drifting copy of the same fact — which is exactly what the pre-CC6 `MediaError::Backend(String)` was.

**Migration note:** the existing message-substring tests in `export.rs`'s mod tests (`rejects_unknown_delivery_metadata`, `rejects_zero_confidence_delivery_metadata`, `rejects_non_rec709_delivery_metadata`, `rejects_non_libx264_for_explicit_color_tagging`, `accepts_only_supported_project_delivery_provenance`, `rejects_unknown_or_non_project_delivery_provenance`, ~709-883) are rewritten to assert `code`/`field`/`observed`/`allowed`. That rewrite is a named implementation step (§12), not incidental churn.

### 4.3 Encoder and filter graph

In `media/src/export.rs`, driven entirely by `settings.delivery_color.bit_depth`:

| Site | 8-bit | 10-bit |
| --- | --- | --- |
| `video_encoder.set_format` (`export.rs:135`) | `Pixel::YUV420P` | `Pixel::YUV420P10LE` |
| filter `format` node (`export.rs:345-346`) | `pix_fmts=yuv420p` | `pix_fmts=yuv420p10le` |
| `x264-params` (`export.rs:158-162`) | `colorprim=bt709:transfer=bt709:colormatrix=bt709` | identical, unchanged |
| `profile` option | not set | not set |
| `set_colorspace` / `set_color_range` (`export.rs:150-151`) | `BT709` / `MPEG` | `BT709` / `MPEG` |
| `scale` node (`export.rs:334-337`) | `flags=bicubic:in_range=jpeg:out_range=mpeg:out_color_matrix=bt709` | identical, unchanged |

**Those two cells are the entire encoder change.** Three things the draft proposed are dropped on measurement:

- **`range=tv` is not added.** It is not an x264 parameter in x264 core 165: under `-x264-params` it prints `Error parsing option 'range = tv'` and is silently ignored; under `-x264opts` the encoder fails to open. `set_color_range(Range::MPEG)` on the codec context is measured to reach the SPS on its own — `ffprobe` reports `color_range=tv` on all nine probe encodes, at both depths. Platform gap #9 is therefore closed by *measurement*, not by a code change, and the contract records that.
- **`profile=high10` is not set.** Encoding `yuv420p10le` with and without `-profile:v high10` produced **byte-identical** files (3771 B each); x264 logs `profile High 10, level 3.0, 4:2:0, 10-bit` and `ffprobe` reports `profile=High 10` either way. Setting `profile` explicitly on the 8-bit lane is likewise not done, because that lane sets no `profile` today and doing so could change the 8-bit SPS for no measured benefit.
- **No `profile` fallback path.** If the build's libx264 does not offer `yuv420p10le`, the export fails typed with `delivery_encoder_pixel_format_unavailable` (§3.8). It never silently falls back to 8 bits.

Before opening the encoder, the export path checks the codec's advertised pixel formats for `settings.delivery_color.bit_depth`'s format and raises `delivery_encoder_pixel_format_unavailable` if absent; the pinned Linux build advertises `yuv420p yuvj420p yuv422p yuvj422p yuv444p yuvj444p nv12 nv16 nv21 yuv420p10le yuv422p10le yuv444p10le nv20le gray gray10le`.

**The two encoder-format refusals, with their fields** (E19):

- `delivery_encoder_pixel_format_unavailable` — `observed` is **the build's advertised format list**, verbatim, so the log says what this FFmpeg actually offers rather than only what it lacks; `allowed` is the lane's single required format (`yuv420p10le` or `yuv420p`).
- `delivery_pixel_format_depth_mismatch` — the declared `delivery_color.bit_depth` and the requested pixel format disagree; `observed` is the format, `allowed` is the format the declared depth requires. This is the leg that catches a caller who sets `Ten` and a `yuv420p` graph, which would otherwise have produced a silently 8-bit file carrying a 10-bit tag.

**Three named constants** (E20) replace the string literals the draft left inline, so a change to any of them is a diff in one place and the manifest can assert them: `DELIVERY_VIDEO_CODEC = "libx264"`, `DELIVERY_X264_PARAMS = "colorprim=bt709:transfer=bt709:colormatrix=bt709"`, and `DELIVERY_SCALER_FLAGS = "bicubic"` (§5.3). None of the three changes value in CC6; naming them is the whole change.

### 4.4 `ExportSettings`

- Derives `Serialize, Deserialize, JsonSchema` (needed by the QC report, the manifest, and `ExportJobRecord.verification`). `ExportCancellation` is `#[serde(skip)]` and reconstructed as `Default` on load — it is a runtime token, not a setting, and §11.2.17 asserts a serialize/deserialize round trip is equal on every other field.
- The stale doc comment (`core/src/media.rs:966-969`, *"This is an output-tag contract only. The current export path does not perform a colour transform"*) is replaced: since CC1 the export path **does** perform the managed delivery transform (`compositor.rs:1680-1712` → `encode_delivery_for_description` → `quantize_delivery16`), and the swscale graph performs the full→limited and RGB→Y′CbCr conversion (`export.rs:334-337`). The new comment says so and names `delivery_color.bit_depth` as the depth authority.

---

## 5. The delivery render quality contract — measured

**This is a quality contract, not a new renderer.** No second compositor, no second code path, no `quality` enum on `ExportSettings`. Every item below is either a fact already true in the tree, now named and asserted, or a measured result that removes a knob the draft proposed to add.

1. **Full-resolution decoders.** `render_delivery` pins `RenderScale::FullResolution` (`render.rs:315-333`, `export.rs:236`). The preview proxy bound (`PREVIEW_MAX_WIDTH = 1280`, `render.rs:27`) is never in the export path.
2. **The 16-bit intermediate and its white level.** `DELIVERY_INTERMEDIATE_PIXEL = RGBA64LE` (`export.rs:419`), fed by the one and only delivery quantization, which scales by `DELIVERY_INTERMEDIATE_WHITE = 65_280` (`color_pipeline.rs:2436`, `quantize_delivery16` at `2445-2457`; landed as **`ad6f6a8`**). libswscale reads 16-bit RGB input with nominal white at `255 << 8`, the same `P_8` CC1 §3.1 documents for the decode direction; scaling by 65 535 made nominal white encode to Y′ 236 (10-bit 943), one code above legal white, deterministically, on every Kinewright export. Measured after the fix: white → Y′ **235** exactly at 8 bits and **940** exactly at 10 bits. The CC1 erratum (`docs/CC1-MANAGED-SDR-PRIMARY.md:186-200`) is the normative statement; §6.3 uses this same constant on both sides of its comparison.
3. **Scaler flags.** `DELIVERY_SCALER_FLAGS = "bicubic"` — the value already at `export.rs:335`, now a named constant so a change is a decision. Measured on the HD chart, flat-field (≥ 16 px from any content edge, 5 006 592 samples): bicubic max 3 / P99 2.0 / MAD 0.2693 / **53.486 dB**; lanczos max 4 / 2.0 / 0.3778 / 51.367 dB; spline max 3 / 2.0 / 0.3038 / 52.674 dB. **bicubic is the best of the three** and needs no change. `accurate_rnd` is inert on this path (0 differing luma samples of 262 144) — **luma only; it is not inert on chroma, and enabling it moves pinned decode figures: §0.4**; `full_chroma_int` / `full_chroma_inp` were not measured on the encode side and are therefore **not** added. `DELIVERY_OUT_CHROMA_LOC` is not introduced: chroma siting was not measured and is deferred (§13).
4. **Dither, measured and unchangeable.** libswscale 9.1.100 applies a **deterministic 8×8 ordered dither with 64 threshold levels** on 16→8-bit RGB→YUV, and **no dither** on 16→10-bit. Every `sws_dither` value (`none`, `bayer`, `ed`, `a_dither`, `x_dither`), as a filter option and as a global output option, is byte-identical to the default; so is `accurate_rnd` — **on the luma plane, which is what was measured; on chroma it is not, and the straddle it removes is a property of the build (§0.4)**. The pattern is spatial, not temporal, and repeated runs are byte-identical. **`DELIVERY_DITHER_OPTION` does not exist.** Normative consequence for QC and for every fixture: *a flat 8-bit delivery patch legitimately produces two adjacent Y codes in a fixed 8×8 tiling; no assertion may require a single code from an 8-bit delivery output except where the input lands exactly on a code (white `65_280` → 235).* 251 of 256 flat grey tiles come out non-flat at 8 bits; 0 of 256 do at 10 bits.
5. **The decode side of verification, normative.** Verification decodes through the crate's own managed decoder (`decode.rs:894-994`, `flags=bicubic` with explicit `in_color_matrix`/`out_color_matrix`/`in_range`/`out_range`), and **must not** add `full_chroma_int`. Measured on the same file and reference: `flags=bicubic` and `flags=bicubic+accurate_rnd` both give max 70 / P99 2.0 / MAD 0.3753 / 43.560 dB, while `flags=bicubic+accurate_rnd+full_chroma_int` gives max 133 / P99 5.0 / MAD 0.4992 / **38.601 dB** — 63 codes worse on max and 5 dB worse — because it interpolates chroma across the 4:2:0 edge instead of replicating it. `flags=bilinear+accurate_rnd+full_chroma_int` is outright broken in this build (max 255, 14.008 dB, skin decoding to magenta) and is recorded as a suspected libswscale defect. The ingest decoder is otherwise untouched: CC1's pixel-exact ingest gate is byte-exact against `flags=bicubic` and CC6 does not move it.
6. **The 10-bit lane** (§4.3).
7. **The claim rule.** No preview, proxy, thumbnail, or cached raster may be labelled a delivery reference. `verify_delivery_output` asserts `reference.full_resolution == true` for every sampled frame and fails `delivery_verification_not_full_resolution` otherwise. This reuses the existing `full_resolution` derivation (`compositor.rs:277-300`); it does not invent a second claim. **`delivery_reference` takes the render scale as an argument** (E15) — bound once to `RenderScale::FullResolution` by the production caller in `measure_samples`, and never internally — so the refusal has a **reachable failing case** (rule 11.0.5): a caller that hands it a proxy scale is refused typed, and the fixture can prove the gate is able to fail. A function that bound `FullResolution` inside itself would have made `delivery_verification_not_full_resolution` dead code that no test could trip.

---

## 6. Verification as a product surface

### 6.1 `Analysis::verify_delivery_output`

```rust
fn verify_delivery_output(
    &self,
    _document: Arc<Document>,
    _path: &Path,
    _settings: &ExportSettings,
    _request: DeliveryVerificationRequest,
) -> Result<DeliveryVerification, MediaError> { Err(MediaError::NotImplemented) }
```

Implemented by `FfmpegMediaEngine` in a new `crates/kinewright-media/src/verify.rs`. It **must** use the crate's own bindings-based decoder; production never shells out (`media/src/lib.rs:46`, `export.rs:8`). The `ffmpeg` CLI stays test-only (`test_support.rs:236-284`).

**Verification opens the output exactly as a player would, normative** (E14). The edit list is honoured; `ignore_editlist` and every other demuxer workaround is **forbidden**. The rule earned itself: the first run of this verifier reported a frame-count mismatch on every export, and the mismatch was real — §0.3's `elst` defect meant every Kinewright MP4 presented one frame fewer than it coded. Suppressing the edit list would have made verification agree with the encoder and disagree with every player on the planet. The export was fixed instead. A verifier that reads the file differently from the audience is not a verifier.

**Three refusals happen before a single frame is decoded**, in this order:

1. `request.validate()?` (§3.0) — an out-of-contract `frame_count` is refused with `delivery_verification_frame_count_out_of_range` rather than silently clamped into a narrower measurement. `sample_frames` stays *total* so that it can never panic; that is exactly why the refusal has to be a separate call the caller makes, and the contract says which one.
2. **The budget lane is checked against the settings' depth.** `request.budgets != DeliveryBudgets::for_depth(depth)` is `delivery_verification_budget_lane_mismatch`, with the request's budgets in `observed` and the lane's in `allowed`. §6.3 says a caller may not invent a looser set; without this check a caller who handed the 10-bit budgets to an 8-bit export would still get a `within_budgets` verdict, and it would be published as a pass against a gate nobody chose. Both directions are asserted.
3. **An empty sample set is refused**, as `FrameCountMismatch` with `observed: "0 sampled frames"`. A document that implies no frames would otherwise produce a report whose every accumulator saw nothing — an empty `frames` list, zero differences, three unseen planes — and `within_budgets: true` because nothing exceeded anything. That is a pass nobody measured. It reuses `FrameCountMismatch` rather than adding a `delivery_verification_no_samples`: the field is `frame_count`, the recovery action is the same one, and a second code would split *"does this file have the frames the document claims?"* across two codes every agent and app surface would have to learn separately.

### 6.2 Sampling rule

```text
DELIVERY_VERIFICATION_FRAME_COUNT = 5        // default n
DELIVERY_VERIFICATION_MAX_FRAMES  = 16       // hard cap
```

`T` is **the frame count implied by the document duration at the export fps**. It is cross-checked against the number of frames the output **presents**, and a mismatch is `delivery_verification_frame_count_mismatch` with both numbers in `observed`/`allowed`. Verification does not silently sample a shorter file.

**The cross-check is against the presented count, and it costs `O(GOP)`, normative** (E14, E21). Three things it is deliberately *not*:

- **not `probe_path`'s frame count**, which is the coded packet count: a container whose edit list trims a coded picture passes that check while a viewer is shown one frame fewer — precisely §0.3's defect;
- **not a straight decode of the whole file**, which the draft implied and the first implementation did: verification must not scale with the export's length;
- **not a seek to frame `T`**, which would only prove a frame exists.

Instead the decoder seeks to `T − 1` — the last frame the document implies — and decodes to end of stream, taking the **highest presented frame identity** `h` and reporting `h + 1`. One seek, one GOP, plus whatever tail follows. `h == T − 1` passes; `h < T − 1` is the `elst` defect (the final coded picture is flagged `AV_PKT_FLAG_DISCARD` and never reaches the decoder's output); `h ≥ T` means the file presents a frame the document does not imply; an empty tail reports `0`. This shares the assumption every `O(GOP)` check must make — that the frames before the seek point are contiguous — and that is the trade the contract takes knowingly. The cost is recorded in P9 (§11.2.24, Appendix A).

**`DeliveryComparison.frames` records the project frame the decoded identity maps to** (E14, review-media H4; the mapping is the identity whenever `settings.fps == document.fps`, as on the CC6 source). `DecodedSample` carries the frame index of the picture the decoder actually returned, and `measure_samples` refuses typed when that index is not the requested one: `delivery_verification_frame_count_mismatch` with `observed: "decoded frame {at} for requested frame {n}"` and `allowed: "decoded frame {n}"`. The seek-and-decode-forward loop stops at `at >= target`, so an overshoot is representable and is refused rather than measured. `delivery_verification_frame_count_mismatch` is one code with three `observed` grammars (a bare presented-frame shortfall, `"0 sampled frames"`, and `"decoded frame {at} for requested frame {n}"`); `observed` is prose for a reader and **must not** be parsed by any consumer. The comparison therefore reports the identity of the picture that was actually measured, never the identity that was asked for — a verification that silently compared frame 30 against the reference for frame 29 would be worse than no verification, because it would report a difference budget as evidence about a frame it never looked at.

For `T ≥ 1` frames and a requested `n ∈ 1..=16`:

```text
if n == 1:   sample frame 0 only
if T <= n:   sample every frame 0..T
else:        f_i = floor(i · (T − 1) / (n − 1))   for i in 0..n
```

Integer arithmetic, deterministic. For `n ≥ 2` it always includes **frame 0 and frame T−1**; for `n == 1` it includes frame 0 only, and the contract says so rather than claiming both. Duplicates (possible only when `T` is small) are removed while preserving order. On the CC6 source (`T = 60`, `n = 5`) the samples are **0, 14, 29, 44, 59**.

### 6.3 Decoded comparison and budgets

```rust
pub struct DeliveryChannelDifference {
    pub maximum_code_diff: u32,
    pub p99_code_diff_millionths: i64,
    pub mean_code_diff_millionths: i64,
}
pub struct DeliveryComparison {
    /// The identity of each frame that was **decoded and measured**, in the
    /// `i64` identity `ScopeMeasurementMetadata.project_frames` carries.
    /// Never the requested identity: a sample whose decoded picture is not the
    /// requested frame is refused typed (§6.2), so these two can only agree.
    pub frames: Vec<i64>,
    /// GATED: the luma plane at full resolution, delivery code units at the lane depth.
    pub luma: DeliveryChannelDifference,
    /// REPORTED, NOT GATED, except for `combined.mean_code_diff_millionths`.
    pub red: DeliveryChannelDifference,
    pub green: DeliveryChannelDifference,
    pub blue: DeliveryChannelDifference,
    pub combined: DeliveryChannelDifference,
    pub psnr_db_hundredths: Option<i32>,       // GATED
    pub decoded_ycbcr: YCbCrLegalReport,       // source = DecodedNativePlanes
    /// Fixed prose: why RGB max and P99 are reported and not gated.
    pub rgb_extremes_note: String,
    pub budgets: DeliveryBudgets,
    pub within_budgets: bool,
}
pub struct DeliveryVerification {
    pub output_path: PathBuf,
    pub delivery_bit_depth: DeliveryEncodeDepth,
    pub probed: ColorDescription,
    pub tags: DeliveryTagCheck,                // tag_source = "probed_output_file"
    pub decoded_pixel_format: String,
    pub comparison: DeliveryComparison,
    pub exceptions: Vec<ColorQcException>,
    /// No `Error`-severity entry in `exceptions`.
    pub technical_pass: bool,
}
```

**Why the gate is the luma plane and not the RGB maximum.** Measured: at the export's own settings, two independent error terms dominate and they have completely different magnitudes. Flat and smooth content round-trips at max 3–4, P99 2, MAD ≤ 0.5 codes, 50–53 dB in 8-bit and max 1, 62.7 dB in 10-bit. **Hard saturated colour edges cost up to 133–134 RGB codes in 8-bit *and in 10-bit alike***, because the loss is 4:2:0 chroma decimation, not quantisation: the worst HD sample is a green/blue triple junction where reference `(0, 0, 255)` decodes to `(0, 17, 122)`. A whole-raster RGB max is therefore not a codec measurement and **must not** gate anything; 10-bit buys ~9 dB on flat fields and nothing at all on edges. The gated set is:

- **(a) the luma plane**, full resolution, exact: the decoded native Y plane against a reference Y′ computed from the reference RGB through §3.4's matrix at the delivery depth — max, P99, and mean, in luma code units at the lane depth. This is codec-only error: no chroma decimation term enters it.
- **(b) RGB mean absolute difference** over the whole raster, 8-bit-equivalent, and **PSNR**.
- **(c) RGB max and P99 are reported and never gated**, carrying `rgb_extremes_note` verbatim: *"4:2:0 chroma decimation at hard saturated edges dominates these two numbers in both lanes; they are evidence, not a gate."*

**Comparison unit, normative.** Both sides are converted to **delivery code units at the delivery bit depth**, `U = 2^bits − 1`, through the *same* denominator:

```text
DELIVERY_REFERENCE_DENOMINATOR = DELIVERY_INTERMEDIATE_WHITE = 65_280

reference: ref_code = round(U · v16      / DELIVERY_REFERENCE_DENOMINATOR)
decoded:   dec_code = round(U · C_rgba64 / DELIVERY_REFERENCE_DENOMINATOR)
```

**`DELIVERY_REFERENCE_DENOMINATOR` lives in `kinewright-media`, as an alias of `DELIVERY_INTERMEDIATE_WHITE`** (E9, E16) — one `pub const` beside the quantizer that already uses the value, exported under the reference name so §6.3's two sides can be seen to share it. Core does **not** carry a copy: core has no delivery intermediate and a second declaration of `65_280` in a second crate is exactly the drift this constant exists to prevent. `cc6_delivery_reference_denominator_is_the_delivery_intermediate_white` asserts the alias.

It is the **same constant**, not a second copy: the encode side quantizes on it (`ad6f6a8`) and CC1 §3.1 states it for the decode side — *"Limited BT.709 YUV-to-RGB conversion uses FFmpeg's 8-bit fixed-point RGB scale even when the source planes are 10 bits (or deeper), so its nominal legal-white denominator is `P_8 = 65280`, not `P_N`."* At 8 bits `round(255 · v16 / 65280) == round(v16 / 256)`, which is exactly `delivery_frame_to_rgba8` (`cc1_fixtures.rs:3822-3835`), so the 8-bit lane is the CC1 measurement with an explicitly configured decode instead of an implicitly configured CLI one.

**Alpha is excluded.** yuv420p carries no alpha, and §3.1 establishes that the working and delivery rasters are opaque by construction, so no fixture may assert a transparent-pixel count as if it could vary.

**PSNR** is defined on the **8-bit-equivalent** MSE so the number is comparable across lanes:

```text
s     = 2^(bits − 8)
d8    = (ref_code − dec_code) / s
MSE8  = mean over compared RGB samples of d8²
PSNR  = 10 · log10(255² / MSE8)            reported in hundredths of a dB
MSE8 == 0  ⇒  psnr_db_hundredths = None
```

`Option` for the degenerate case follows `AudioLoudness`'s precedent (`media.rs:984-992`), not a sentinel.

**Budgets, as named constants — final, re-baselined against `cc6_delivery_source()`.** The draft's numbers were the probe's HD-chart measurements; §12 step 5 re-measured every one of them on the fixture's own source through the production export and the managed decode, and the constants below are that re-baseline. They were set **before** the fixtures landed, never widened afterwards to make a red build green.

**Every fractional term is an integer `_MILLIONTHS` constant** (E8), because §10.1 forbids a float in a gated number and a budget written `2.0` in the contract and `2_000_000` in code is two constants:

```text
DELIVERY_LUMA_MAX_CODE_8BIT                 =         8   (8-bit luma codes)
DELIVERY_LUMA_P99_CODE_8BIT_MILLIONTHS      = 3_000_000   (3.0 codes)      was 2.0
DELIVERY_LUMA_MEAN_CODE_8BIT_MILLIONTHS     =   400_000   (0.4 codes)      was 1.0
DELIVERY_RGB_MEAN_CODE_8BIT_MILLIONTHS      = 1_750_000   (1.75, 8-bit-eq) was 1.0
DELIVERY_PSNR_FLOOR_DB_HUNDREDTHS_8BIT      =     3_300   (33.00 dB)       was 40.00

DELIVERY_LUMA_MAX_CODE_10BIT                =        16   (10-bit luma codes)  was 32
DELIVERY_LUMA_P99_CODE_10BIT_MILLIONTHS     = 4_000_000   (4.0 codes)      was 8.0
DELIVERY_LUMA_MEAN_CODE_10BIT_MILLIONTHS    = 1_000_000   (1.0 codes)      was 4.0
DELIVERY_RGB_MEAN_CODE_10BIT_MILLIONTHS     = 1_000_000   (1.0, 8-bit-eq)  was 0.5
DELIVERY_PSNR_FLOOR_DB_HUNDREDTHS_10BIT     =     3_300   (33.00 dB, on 8-bit-equivalent MSE)

DECODED_RANGE_EXCEPTION_BASIS_POINTS        =       100   (1 %, §6.4)
```

**The measurements these were baselined from** (Linux, lavapipe, `cc6_delivery_source()` at `DeliveryProfile::SourceMaster`, five sampled frames; recorded in the manifest's `budgets.*.measured` and asserted by §11.2.10/11):

| Term | 8-bit budget | 8-bit measured | margin | 10-bit budget | 10-bit measured | margin |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| luma max (lane codes) | 8 | **2** | 4.00× | 16 | **1** | 16.00× |
| luma P99 (lane codes) | 3.0 | **1.0** | 3.00× | 4.0 | **0** | unbounded |
| luma mean (lane codes) | 0.4 | **0.085247** | 4.69× | 1.0 | **0.005545** | 180.34× |
| RGB mean (8-bit-eq) | 1.75 | **0.743535** | 2.35× | 1.0 | **0.414572** | 2.41× |
| PSNR (dB) | ≥ 33.00 | **36.86** | +3.86 dB | ≥ 33.00 | **37.00** | +4.00 dB |

and the failing direction, the same source starved to `-b:v 100k` (§11.2.13):

| Term | 8-bit starved | 10-bit starved |
| --- | ---: | ---: |
| luma max | **35** | **121** |
| luma P99 | **6.0** | **24.0** |
| luma mean | **0.621205** | **2.640545** |
| RGB mean (8-bit-eq) | **1.330318** | **1.180978** |
| PSNR | **35.88 dB** | **35.98 dB** |

Both lanes fail on the codec-only luma terms, which is what a starved codec is supposed to break; the 10-bit lane also crosses its tighter RGB-mean floor, and PSNR stays inside its floor on both, which is why PSNR alone is not the gate.

Four things about these numbers are normative rather than incidental:

1. **The 8-bit luma mean is 0.4, not 0.5, for distinctness.** `0.5` is `MONITOR_CPU_GPU_MEAN`, and CC1's rule — restated below and asserted by `cc6_delivery_budgets_are_distinct_from_the_compositor_gate` — is that no CC6 budget may equal a compositor-parity constant, so that neither can ever be silently substituted for the other. `0.4` is the nearest value that clears the measurement by more than 4× and is not that number.
2. **The 8-bit luma P99 and RGB mean are wider than the 2× bar** — 3.0× and 2.35× rather than the 2.0× and 2.0× the measurement alone would justify. The reason is R5: the Windows CI job runs a *different* FFmpeg package (Appendix A), the Windows measurement is not yet in hand, and a constant sitting exactly on the Linux bar is a constant that goes red on the first Windows run for a reason that is not a regression. The widening is stated here so that a later narrowing is a decision and not a discovery.
3. **The 10-bit luma P99 measures exactly `0`** on the passing source: the codec-only luma error at this bitrate does not reach one 10-bit code at the 99th percentile. A budget nothing approaches proves nothing (rule 11.0.5), so this term's failing direction is carried entirely by the **starved 10-bit fixture**, which measures 24.0 against a budget of 4.0 — six times over. The manifest records `margin_ratio: "infinite (measured exactly zero)"` and names the starved direction as the bound, rather than pretending to a ratio.
4. **The RGB mean and PSNR are whole-raster sanity floors, not codec measurements.** They are dominated by 4:2:0 chroma decimation on this source, which is why their margins are the tightest two in the table and why §6.3(c) forbids gating the RGB *extremes* at all.

**The 10-bit budget is baselined, never derived.** The roadmap is explicit: *"lossy encoded-delivery tolerances are codec-specific and must be baselined rather than reused as compositor tolerances."* Scaling the 8-bit constants by 4 would be exactly that reuse; the 10-bit numbers above are a separate measurement. **CC1's `DELIVERY_CODEC_MAX = 4 / P99 = 2.0 / MEAN = 1.0` (`cc1_fixtures.rs:67-69`) stay untouched and are not reused as CC6's product gate** — they are the CC1 grey-bar fixture's flat-field numbers and would fail instantly on any raster containing a saturated edge. CC6 asserts, as CC1 does at `cc1_fixtures.rs:3930-3940`, that its budgets are **numerically distinct** from `MONITOR_CPU_GPU_{MAX, P99, MEAN}` (`cc1_fixtures.rs:61-63`) so neither can be silently substituted.

### 6.4 Native planes and decoded legality

The Y′CbCr legal measurement of the decoded file **must** read native planes; a value that has been through swscale to RGBA64 has already been clipped and matrixed and cannot show a plane excursion.

**One decode pass, normative.** `verify.rs` opens the output once with `DecodeStrategy::Seek`, seeks to each sampled frame, and for that frame both (a) reads `frame.data(plane)` / `frame.stride(plane)` for planes 0, 1, 2 directly, and (b) converts the same decoded frame through the managed scaler for the RGBA64 comparison. At most `n · GOP` frames are decoded. There is no second decoder and no second traversal of the file.

```rust
pub struct NativePlaneFrame {
    pub width: u32, pub height: u32,
    pub chroma_width: u32, pub chroma_height: u32,
    pub bit_depth: u8,
    pub pixel_format: String,        // "yuv420p" | "yuv420p10le"
    pub luma: Vec<u16>, pub cb: Vec<u16>, pub cr: Vec<u16>,
}
```

8-bit codes are widened to `u16` **without shifting** — an 8-bit sample stays `0..=255` and a 10-bit sample stays `0..=1023` — so the legal bounds are `[16·s, 235·s]` and `[16·s, 240·s]` with `s = 1 << (bits − 8)` and nothing is rescaled. `yuv420p10le` is little-endian 16-bit containers with the top 6 bits zero; the reader asserts `sample <= 1023` and fails `delivery_verification_plane_out_of_container` otherwise, so a byte-order mistake cannot be mistaken for a colossal excursion.

**The `decoded_range_excursion` rule, normative (EBU R 103).** Strict-box excursion counts and extremes are **always reported**. The Warning is raised when either:

```text
EBU_R103_TOLERANCE_CODES_8BIT = 11        // ≈ 5 % of the 219-code luma span, at 8 bits

(a) samples outside the strict legal box exceed DECODED_RANGE_EXCEPTION_BASIS_POINTS = 100 (1 %), or
(b) any sample lies outside the EBU R 103 box: −5 % / +105 % of the nominal range,
    with t = EBU_R103_TOLERANCE_CODES_8BIT · s,
    Y  ∈ [16s − t, 235s + t]      8-bit [5, 246]     10-bit [20, 984]
    C  ∈ [16s − t, 240s + t]      8-bit [5, 251]     10-bit [20, 1004]
```

The tolerance is a **named constant scaled by `s`** (E17), not eleven literal `11`s and two literal `44`s: R 103's allowance is defined as a percentage of the nominal range, so it is one number at 8 bits multiplied by the lane scale, and expressing it any other way invites the 10-bit lane to drift from the 8-bit one. Rate (a) is computed per plane as §3.4's excursion rate, `floor((below_count + above_count) · 10000 / that plane's sample count)`, by the same expression the fixture uses to predict it.

A hard zero-excursion gate is refused because it is not achievable: after `ad6f6a8` the encoder input never exceeds legal, so every remaining decoded excursion is a codec artefact — measured at about ±1–2 codes at 8 Mbps and ±3 codes at 500 kbps — and legitimate content sitting exactly on a bound (75 % blue is `Cb = 240.0` exactly) will cross it by one code under any ringing at all.

Rejected alternative: a second `buffersink` output on the export filter graph. Rejected because the graph belongs to the *encode*, runs on rendered frames, and would measure the input to the codec rather than the codec's output — which is precisely the thing this measurement exists to catch.

### 6.5 Wiring

**Export queue** (`kinewright-agent/src/export_queue.rs`). `WorkItem` gains `verify: bool` and `depth: DeliveryEncodeDepth`. `run_work_item` (`export_queue.rs:614-696`), on `Ok(Ok(()))` (line 670) and after the existing `source_identity_drift` check passes, calls `state.analysis.verify_delivery_output(document, &work.output_path, &settings, request)` — `state.analysis` is already in scope. The result is stored on the record:

```rust
pub struct ExportJobRecord {
    // ... unchanged (export_queue.rs:73-85) ...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<DeliveryVerification>,
    /// The **sole** carrier of "this job completed and was not verified".
    /// There is no `verification_unavailable` exception anywhere (§3.8).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_unavailable_reason: Option<String>,
    /// `true` while verification is running, so `get_export_jobs` and the
    /// dialog can distinguish *encoding* from *verifying* without a new
    /// `ExportState` variant.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub verifying: bool,
}
```

**`verifying` is a flag, not a state** (E31, review-agent M5). A new `ExportState::Verifying` would have been a fourth terminal-adjacent value every existing `match`, every stored record, and every agent client would have to learn, to express a phase that is not a state the job can be resumed or cancelled *into*. The flag is `#[serde(default, skip_serializing_if)]`, so a record that never verified — including every pre-CC6 record — serializes byte-identically to today. **Verification remains non-interruptible** once it has started: it holds no lock on the output and completes in bounded time, and a half-finished measurement is worth nothing. A cancellation observed *before* verification begins is honoured (§8.4).

**Outcome policy, normative: verification is a measurement, and it must never move, rename, or delete a finished encode.**

- A **budget overrun** (`decoded_difference_over_budget`, `decoded_range_excursion`) leaves the job `Completed` with `verification.technical_pass = false` and `error: None`. The encode succeeded and the file is a valid deliverable; failing the job over a measurement would destroy work to report a number.
- A **tag mismatch** (`delivery_tag_mismatch`) likewise leaves the job **`Completed`**, `error: None`, `verification.technical_pass = false`, with an Error-severity exception on the record and a `TAG MISMATCH` status in the dialog. **`quarantine_untrusted_output` is not used by verification**, for any outcome; that path belongs to `SourceIdentityChanged` (`export_queue.rs:672-682`) and stays there. A false positive in a measurement must not be able to move a good file.
- Verification that cannot run at all (missing GPU adapter, unreadable output, an unsupported decoder format, or a **panic** inside the verifier) leaves the job `Completed` with `verification: None` and `verification_unavailable_reason: Some(reason)`. A cancellation observed before verification starts leaves the job **`Cancelled`** (the honest state: the operator asked for it) with `verification: None` and the same `verification_unavailable_reason` recorded whenever `verify` was requested, so the sole carrier is never silent; the app dialog reports the equivalent as `NOT VERIFIED` with the reason. In both surfaces the written file stays exactly where it is. It never invents a pass, and it never attributes the fact to a later, unrelated measurement. A panic is caught, contained, and recorded as a reason containing `"panicked"`, with the output file untouched — a measurement that crashes must not take the export with it. There is **no `verification_unavailable` exception** on the record (E31, was a draft Info exception): the reason string is the whole surface, and the dialog renders `NOT VERIFIED` from it.
- A job **cancelled during** verification stays `Cancelled` with `verification: None`; the cancellation is recorded rather than the measurement (§11.2.25).

**`queue_export`** gains `verify: bool` (`#[serde(default = "default_true")]`, which already exists at `server.rs:8839`) and `delivery_bit_depth: DeliveryEncodeDepth` (`#[serde(default)]` → `eight`). No new confirmation gate: verification reads a file the caller just asked to write. `verify` defaults to **true**; §12 orders the cost measurement (P9) before that default is final, and the measured wall time on both lanes is recorded in the manifest.

**App**: §8.4.

---

## 7. Agent surface

`INSPECTOR_TOOL_NAMES` grows **74 → 75** (`schema.rs:16`), and the count assertion at `server.rs:19031` and the name list at `server.rs:19039-19048` move with it. The M36 table (`M36-AGENT-RUNTIME-EFFICIENCY.md:94-102`) goes from registry 123 to **124** tools (49 operation + 75 inspector). The served surface is unchanged at 7 `COMPACT_TOOL_NAMES`, and `served_surface_is_small_and_keeps_the_internal_registry_discoverable` (`server.rs:20849-20880`) must still pass with `served bytes < registry / 4`. `get_color_qc`'s description **must** be under 1 024 bytes (the assertion at `server.rs:19051-19056`).

**Measured, after CC6** (E24): registry **124** tools / **1 280 060 B** total, of which schemas are **1 163 879 B** and descriptions **95 827 B**; served **7** tools / **5 660 B** total, schemas **3 510 B**, descriptions **998 B** — the served surface is byte-unchanged, and `5 660 < 1 280 060 / 4` holds by a factor of 56. `get_color_qc`'s own description is **936 B**, inside the 1 024 B cap with 88 B to spare; that is a tight budget for a tool with this many arguments, and it is why the description points at this document rather than restating §3.

**No `CAPABILITY_KIND_OVERRIDES` entry is needed**: `get_` already infers `CapabilityKind::Inspector` (`runtime.rs:153-160`). That is stated so the omission is a decision.

### `get_color_qc`

Read-only, evidence-only, mutates nothing. Annotations `read_only(true).destructive(false).idempotent(true).open_world(false)`.

`ColorQcArgs` (`#[serde(deny_unknown_fields)]`, in `kinewright-agent/src/color_qc_tool.rs`):

| Argument | Type | Default | Notes |
| --- | --- | --- | --- |
| `expected_revision` | `Option<TimelineRevision>` | none | **Optional**, like `get_video_scopes_v2` (`color_scopes.rs:111`) — this is an inspector, not a planner. A mismatch is the uniform `stale_revision { expected_revision, actual_revision }` (`color_scopes.rs:600-609`). |
| `timecode` / `frame` / `clip_id` | mutually exclusive | clip midpoint, else frame 0 | More than one is `color_qc_frame_selector_conflict`, whose `detail` names the **first offending selector**. `render_color_proof` (`server.rs:2419-2444`) has no such conflict to reuse, so this is a new typed code rather than a borrowed one (E23). A resolved frame outside the project — negative, at or past `document.duration`, or outside the named clip — is `color_qc_frame_out_of_range` with `field`/`observed`/`allowed`/`recovery_action` mirroring `ColorProofError::ProjectFrameOutOfRange` (E32). |
| `roi` | `Option<NormalizedRoi>` | none | CC2's type, unchanged. |
| `matte_region` | `Option<{clip_id, effect_id}>` | none | CC5's shape, unchanged; composable with `roi` (intersection). The tool obtains the coverage raster from `Analysis::matte_proof_for_document` and passes it in `ColorQcRequest.matte_region` (§3.0). |
| `checks` | `Option<Vec<ColorQcCheck>>` | `[range, gamut, tags]` | `range \| gamut \| skin \| tags \| per_node`. **`range` and `gamut` gate nothing** — both are always measured and always present (E7, §3.0) — so `checks` really selects `skin`, `tags`, and `per_node`. `skin` requires a region and fails `color_qc_region_required` without one; `per_node` is never a default (§3.7's cost). |
| `max_nodes` | `Option<u8>` | 16 | `1..=16`; outside that, `color_qc_node_budget_exceeded`. Validated **unconditionally**, whether or not `per_node` was asked for, so an out-of-contract argument is never silently accepted. |
| `delivery_bit_depth` | `Option<DeliveryEncodeDepth>` | `eight` | Selects the §3.4 code scale and the §3.6 pre-export expected tag set. |

**There is no `resolution` argument.** A working-stage measurement is full-resolution or it is refused: `working_proof_for_document` binds `RenderScale::FullResolution` and takes no scale, so a proxy working proof cannot be produced, and a proof whose `full_resolution` is false is rejected with `color_qc_proxy_proof_refused`. The response's `assumptions` say so in words, and no CC6 surface carries `proxy_sampling`.

`tags` in this tool is always **pre-export mode** (§3.6): the expected description is `ExportSettings.delivery_color` materialised from the document at the requested depth, and `tag_source` says so. A post-export tag check is only available through `verify_delivery_output` and `get_export_jobs`.

Response: `{ timeline_revision, evidence_only: true, applied: false, stage: "working_linear_post_composite", full_resolution: true, report: ColorQcReport, assumptions: [...], exceptions: [...] }` — the `analyze_color_shot` envelope shape (`color_scopes.rs:706-723`), minus the resolution fields it no longer has. The envelope's `full_resolution` **echoes `report.full_resolution`** rather than restating a literal `true`, so the two cannot disagree.

**When the renderer cannot produce a working proof at all**, the tool fails `working_proof_unavailable` — distinct from every `color_qc_*` refusal, because nothing was measured and nothing was wrong with the request. It is CC5's `matte_proof_unavailable` precedent, and it is the code the GPU-skip branch of §11.2.18 asserts: under `KINEWRIGHT_GPU_TESTS_MAY_SKIP=1` the test asserts a **typed** refusal, never both branches.

### `get_export_jobs`

Each `ExportJobRecord` now carries `verification`, `verification_unavailable_reason`, and `verifying` when present (§6.5). No argument change. The record is the only place decoded evidence lives, which closes facts-agent gap #4. `delivery_bit_depth` is **always** emitted (it is not an `Option`); only the two verification keys are absent on a job that did not verify, and §11.2.17 asserts exactly that rather than the draft's looser claim.

### `get_video_scopes_v2`: the gamut stub

**Decision: a typed pointer, not fabricated data.** The hard-coded object at `color_scopes.rs:1522` is replaced by

```json
"gamut": {
  "measured": false,
  "code": "gamut_requires_working_stage",
  "stage_required": "working_linear_post_composite",
  "tool": "get_color_qc",
  "definition": "the RGBA8 monitor proof is display-clamped, so a gamut or legal-range excursion is not observable at monitoring_post_composite; measure it at working_linear_post_composite with get_color_qc"
}
```

Measuring it inline would require a *second* full-resolution render inside every scopes call — doubling the cost of the most frequently called colour tool — and would put a `working_linear_post_composite` measurement inside a result whose `stage_measured` says `monitoring_post_composite`, making CC2's one-stage-per-result contract a lie. The fabricated `"out_of_range_pixels": 0` is the actual defect: it reads as "measured, none found". §11.2.16 asserts the keys `out_of_range_pixels` and `out_of_range_basis_points` are **absent**.

---

## 8. Human UI

Everything below is read-only. The only document-adjacent change CC6 adds anywhere in the app is the export dialog's delivery-depth choice, which is a job parameter, not a document edit. **QC never mutates**, and every panel says so in the same voice `color_scopes_ui.rs:920-923` and `media_workflow.rs:1431-1470` already use.

### 8.1 Colour QC window — `crates/kinewright-app/src/color_qc_ui.rs`

Pattern: the M41 media cache dialog (`media_workflow.rs:1431-1470`) for the non-mutating `egui::Window` over a cloned snapshot with a standing banner, and the scopes panel (`color_scopes_ui.rs:573-704`) for the **single worker with generation-keyed invalidation**. A `pub(crate) trait ColorQcSource` with an `AnalysisColorQcSource` impl, modelled on CC5's `MatteProofSource`, keeps the panel testable without a window.

**One working proof per frame, shared, normative.** The app owns a single `WorkingProofCache` keyed by `WorkingProofKey = (session, revision, frame)`, held behind an `Arc` on `App` and read by **both** the QC window and the QC mask (§8.2). Without it the two states each spawned their own `FrameRenderer` and each rendered a full-resolution working proof of the same frame — two GPU readbacks and two composites for one number. The cache is invalidated by the same `observe_context(session_id, revision, frame)` discipline the scopes panel uses, and it holds one raster at a time because a full-resolution linear f32 RGBA proof is not a thing to accumulate.

`ColorQcSource` carries **two** measurement entry points, and the per-node one is not the other plus a render (E12, review-app H1):

```rust
fn measure(&self, document: Arc<Document>, key: WorkingProofKey, request: &ColorQcRequest)
    -> Result<(ColorQcReport, WorkingProofMetadata), String>;
fn measure_with_nodes(&self, document: Arc<Document>, key: WorkingProofKey, request: &ColorQcRequest)
    -> Result<(ColorQcReport, WorkingProofMetadata), String>;
```

`measure_with_nodes` is backed by `nodes::measure_color_qc_with_nodes`, which renders the baseline once and returns the report measured on it, so 16 candidates cost **17** renders and not 18. The draft's shape — measure, then attribute — rendered the panel's own proof *and* core's baseline. The non-per-node path is taken only when `!per_node`.

- Banner: *"Measuring never changes the project document, the grade, or the exported file."*
- One-shot **Measure current frame** button; no continuous measurement.
- Sections: **Range** (per-channel over/under bp and the extremes in millionths), **Gamut** (bp, minimum linear, maximum desaturation, below-black count, and the §3.3 relation line), **Skin** (only when a region is set; mean hue, spread, median chroma, in-band bp, and the §3.5 boundary statement in full), **Tags** (expected vs observed per field with the `tag_source` shown, not-representable rows visually distinct from mismatches), **Per node** (off by default behind an explicit toggle whose label states "renders up to 17 full-resolution frames"), **Exceptions** (severity · code · message).
- **`QaSeverity` colours are a new `severity_color` mapping** over `STATUS_DANGER` / `STATUS_WARNING` / `TEXT_MUTED` (E25). The branch QA card (`chat_ui.rs:1195-1247`) has no per-severity mapping to reuse — it colours the card, not the row — so this is a new three-arm `const fn` rather than a borrowed one, and `export_ui.rs` reads it too so the two surfaces cannot diverge.
- **In pre-export mode the Tags section renders identical expected and observed columns**, with a note saying why (E27): nothing has been probed, so the "observed" side is the same materialised `ExportSettings.delivery_color`, and the check is answering "would this be accepted?" rather than "does the file match?". Suppressing the observed column instead would have made the pre- and post-export layouts different shapes for the same data; printing it without the note would have implied a probe that did not happen.
- Provenance footer: backend, adapter, `FULL RESOLUTION` badge, raster, stage — the scopes panel's badge row (`color_scopes_ui.rs:1033-1078`), reused.
- `observe_context(session_id, revision, frame)` invalidation, identical to the scopes panel's (`color_scopes_ui.rs:475-497`).
- Opened from a **new View menu** and a `KeyAction::ColorQc` bound to **`Ctrl+Shift+C`** (E28). The chord is free twice over: no other binding uses `C` at all, and no binding in the map is `Ctrl+Shift`. It is deliberately not `Ctrl+Q`, which every desktop environment already spends on Quit. That ripples: `KeyAction` (`keys.rs:12`), `ALL_ACTIONS: [KeyAction; 19] → 20` (`keys.rs:35`), `KEYMAP: [KeyBinding; 19] → 20` (`keys.rs:67`), the `perform_key_action` match (`keys.rs:254`), the completeness test (`keys.rs:412`), and the dialog show block (`app.rs:1366-1375`). All six are named in §12.

### 8.2 QC clipping mask in the program viewer

A whole-picture replacement, exactly like the CC5 matte view — **no shader change**, no new `header.w` encoding (that word is fully consumed by matte-debug, `compositor.wgsl:53-62`, with an early return before the legacy stage). The mask is a CPU-computed `RgbaImage` built from the working proof and uploaded with `load_texture(.., NEAREST)` through the existing `matte_view_texture` machinery (`preview_ui.rs:494-520`).

```rust
pub enum QcMaskView { Off, Clipping }

QC_MASK_UNDER_RANGE_COLOR = [ 32,  64, 255, 255]   // any channel with e < 0
QC_MASK_OVER_RANGE_COLOR  = [255,  32,  32, 255]   // any channel with e > 1
// otherwise: grey = round(255 · clamp(encode_bt709_delivery(Y_linear), 0, 1)) / 2, integer division
```

`e` and the grey are computed with `kinewright_core::color_qc::encode_bt709_delivery` — the single source named by §3.0. The app depends on both crates, so naming which copy it uses is normative, not stylistic.

Precedence: **non-finite first**, then under-range, then over-range, then grey. A `NaN` sample is drawn in the under-range colour rather than falling through the comparisons to black, which is the classification §3.1 gives it — a pixel that cannot be classified must not be painted as one that measured clean. The mask's `is_nan()` branch is explicit for exactly that reason.

Legend line, always visible while the view is on: *"blue = a negative linear channel — out of the Rec.709 gamut and clamped to black; red = an encoded value above 1.0 — clamped to white."*

**The mask does not render during playback, normative** (review-app M5). One full-resolution working proof per frame is not a playback cost, and a mask that lags the picture by a frame or three is worse than no mask. `QcMaskStatus` therefore has a `PausedOnly` value, shown with the line *"Paused only — the mask renders one full-resolution working proof per frame; pause to see it."*, and `request_view_if_needed` is suppressed while the transport is playing. `QcMaskStatus`'s other values are `Off`, `Blocked`, `BehindMatteView` (CC5's matte view wins the viewer), `Pending`, `Unavailable(reason)`, and `Ready`; a toggle whose request has not been issued yet reports `Pending`, not `Unavailable`, so the mask cannot flash a one-frame red "unavailable" on the way to working.

The proof the mask measures comes from the shared `WorkingProofCache` (§8.1), so opening the QC window on the same frame costs nothing extra.

Rejected alternative: three colours, separating "out of gamut" from "under range". Rejected because §3.2/§3.3 establish they are the same pixel set. Rejected alternative: a false-colour or zebra shader overlay — deferred (§13).

### 8.3 Inspector and scopes panel

- **Node header clipping line.** `color_node_header` (`inspector_ui.rs:3702-3751`) gains a second muted line in the same slot the inactive reason occupies (3744-3750), fed from the last `ColorQcReport`'s per-node contributions keyed by `(ClipId, EffectId)`: `Clipping contribution: +{n} bp range · +{m} bp gamut (frame {f})`. Absent when there is no report, when the node is not in it, or when both deltas are `≤ 0`. It is a *report of the last measurement*, never a live computation, and the frame number is shown so a stale reading is visible rather than misleading. **The same line appears in the equivalent slot on the primary-correction and LUT cards** (E29): those two node kinds do not go through `color_node_header`, and a per-node attribution that was invisible on the most common colour node in the tree would have been an attribution nobody saw.
- **Absolute per-channel clipping in the scopes panel.** `ScopeEvidence.clipping` is computed today and never rendered (`scopes.rs:594-607`; only the luma *delta* reaches the UI at `color_scopes_ui.rs:906-911`). CC6 renders it as a four-row R/G/B/luma table of `black {n} bp · white {n} bp`, with the pinned code thresholds named in a tooltip (`SCOPE_LOW_CLIP_CODE = 1`, `SCOPE_HIGH_CLIP_CODE = 254`) so nobody mistakes an 8-bit display-referred clip count for a CC6 range excursion. A one-line note distinguishes them: *"display-code clipping at the monitor stage; delivery range and gamut are measured at working_linear_post_composite in the Colour QC window."*

### 8.4 Export dialog

- **Delivery depth.** A two-value radio (`8-bit H.264` / `10-bit H.264`) writing `DeliveryEncodeDepth`, next to the existing Delivery combo (`export_ui.rs:766-800`). **The dialog keeps its inline `ExportSettings` construction** (`export_ui.rs:505-514`) and sets only `delivery_color.bit_depth = depth.color_bit_depth()`. It must **not** be routed through `DeliveryProfile::export_settings`, which derives resolution from the profile and fps from the document and would silently disable the dialog's own Frame size (`export_ui.rs:838-874`) and FPS (`876-889`) controls. §11.2.19 asserts the dialog and the queue produce the same `delivery_color` for each depth, which is the agreement that actually matters.
- **Post-export verification block.** Replaces the bare `self.status = format!("Exported {}", …)` at `export_ui.rs:643`. Rows: **probed tags per field**, luma max/P99/mean against the lane's budgets with the budget value shown next to the measurement, RGB mean and PSNR against theirs, RGB max/P99 shown with the "not gated" note, decoded Y′CbCr excursions, and a single status line. The `MAX_ADVISORY_LINES = 6` cap (`export_ui.rs:29`) does **not** apply here; it governs preflight advisories, and a truncated verification result would be worse than none.
- **The tag rows are per field, and they are the QC window's** (E27, review-app M3). The block calls `color_qc_ui::tag_field_rows` and `tag_field_color` over `verification.tags`, with not-representable rows visually distinct from mismatches. The draft's summary line — "tags conform" or a count — could not tell a user *which* field was wrong, which is the only thing a tag mismatch is ever about, and a second per-field renderer in `export_ui.rs` would have been a second set of field labels to keep in step with `DeliveryColorMismatch.field`.
- **Four statuses, four distinct labels and colours** (E26): `VERIFIED` (`STATUS_SUCCESS`), `OVER BUDGET` (`STATUS_DANGER`), `TAG MISMATCH` (`STATUS_DANGER`), `NOT VERIFIED` (`STATUS_WARNING`). The first two share a colour because both are Error-severity findings; they are never confusable because the labels differ, and colouring "over budget" as a warning would have contradicted §3.8's severity table. `NOT VERIFIED` is a warning because nothing was measured — there is no finding, only an absence — and it is rendered from `ExportJobRecord.verification_unavailable_reason`, the sole carrier (§6.5).
- **Cancel stays live, and the bar does not freeze** (review-app M6). Cancellation is checked **before** verification starts; if it is set, the result is `ExportVerification::Unavailable("cancelled before verification")` and the dialog shows `NOT VERIFIED` with that reason. Once encoding is done and verification is running, the progress bar — which has nothing left to report — is replaced by the line *"Verifying export… the file is written and is never moved, renamed, or altered by this measurement."* The draft left Cancel inert and the bar at 100 % for the whole verification, which read as a hang.
- **PSNR is printed with its sign preserved** through `(-1, 0)` dB, using the sign/`unsigned_abs` idiom rather than an integer division that rounds a small negative to `0` and prints it as positive.
- **Deep detail is a button**, not a label: "Open the Colour QC window" opens it (E-review L12). A sentence that names a window the user cannot reach from it is a dead end.

---

## 9. Serialization and migration

1. **Pre-CC6 documents load unchanged.** CC6 adds **no `Document` field**. No effect, parameter, node, or asset shape changes. `ColorContext`, `ColorDescription`, and their CC0→CC1 custom `Deserialize` migration (`color.rs:799-871`) are untouched. `ColorPipelineState` stays `managed_sdr_v1` — for CC3 §9 / CC4 §9 / CC5 §8.3's reason verbatim: `pipeline_state` describes the source → working → monitoring → delivery *contract*, and widening the accepted delivery depth does not change the contract a stored project asserts about itself.
2. **`DeliveryProfile`'s four wire strings are byte-identical** — and there are two spellings of one of them, which CC6 does not touch (E3). `DeliveryProfile` derives `#[serde(rename_all = "snake_case")]`, so the **serde** wire form of `Youtube1080p` is `youtube1080p`, while `DeliveryProfile::as_str()` returns `youtube_1080p`. The two have disagreed since the enum was written; every stored record and every tool call uses the serde form, and every human-facing label uses `as_str()`. Changing either now would break one of those two populations for a cosmetic gain, so CC6 leaves the divergence in place, states it here so no reader assumes a single spelling, and **pins both** in the manifest so a future "tidy-up" that unifies them fails a fixture rather than a load. The other three (`source_master`, `vertical_short`, `square_social`) have no underscore ambiguity and are identical in both forms.
3. **`DeliveryEncodeDepth` defaults to `eight`** via `#[derive(Default)]` + `#[serde(default)]` at every use site (`QueueExportArgs.delivery_bit_depth`, `DeliveryConformanceReport.delivery_bit_depth`, `ExportJobRecord`, `ConformanceKey`). A pre-CC6 JSON job record or tool call therefore deserializes to the 8-bit lane, which is what it meant.
4. **`ExportJobRecord.verification` and `.verification_unavailable_reason` are `Option<...>` with `#[serde(skip_serializing_if = "Option::is_none")]`**, and **`.verifying` is a `bool` with `#[serde(default, skip_serializing_if = "std::ops::Not::not")]`**, so a job that predates CC6 or ran with `verify: false` serializes byte-identically to today and deserializes with `None`, `None`, `false`.
5. **`ExportSettings` gains `Serialize`/`Deserialize`/`JsonSchema` with `ExportCancellation` `#[serde(skip)]`.** It has never been serialized, so there is no legacy shape to preserve; the requirement is *determinism*, which `serde_json` gives for a struct with a fixed field order. `ColorDescription`'s existing custom `ColorBitDepth` serde (`color.rs:483-524`) is reused unchanged, so `Integer(10)` and `Ten` serialize to the same canonical form.
6. **`ScopeStage` gains a variant.** No document stores a `ScopeStage` — it is evidence-only. `compare_scope_evidence`'s stage-equality requirement is unchanged and now additionally guarantees the two sides came from the same *engine*, since only one stage is measurable.
7. **`MediaError` gains three variants**, `DeliveryColor(DeliveryColorError)`, `DeliveryVerification(DeliveryVerificationError)`, and `ColorQc(ColorQcError)` (E32; the draft had two). Every `match` on it in the workspace is updated in the same commit, and specifically `MediaError::recovery_code()` (`media.rs:1331-1338`) — an exhaustive `const fn match` — gains `Self::DeliveryColor(error) => Some(error.code())` and the same for the other two, which composes because all three `code()`s return `&'static str`. The `ColorQc` variant is what lets `color_qc::nodes` return `MediaError` (it renders) without losing the typed refusal, and it is why no consumer parses a `Display` string. `cc5_core_proof.rs:591` asserts `recovery_code() == None` for a backend error and is unaffected, but is named here so the change is checked against it.
8. **Both "8-bit SDR Rec.709" rejection messages change** (§3.6). Their three asserting tests are named in §12.
9. Save, reopen, journal replay, branch, undo, redo, and recovery are byte-unaffected: CC6 writes nothing to the document.

---

## 10. Ordering and determinism

1. **Every count is an integer** (`u64` pixels, `u32` basis points, `i32` deltas bounded by ±10 000); **every rate is integer-floor basis points** (`floor(value · 10_000 / count)`, `scopes.rs:1317-1326`'s rule, reused rather than reimplemented, `0` for an empty population and never a division); **every float is reported in millionths** (`round(v · 1_000_000)`, half away from zero) except dB in hundredths and angles in centidegrees. No API in CC6 returns an `f32` or `f64` to an agent or to the UI.
2. **No GPU reductions.** Every accumulator is a scalar CPU loop over a completed readback, with `scopes.rs:1132-1160`'s overflow checks.
3. **Pixel iteration is row-major from the top-left**, matching `for_each_linear_pixel` (`compositor.rs:1555-1629`), so a partial-sum reordering cannot change a floating-point accumulation. `e` is computed in f32 (§3.1); sums are accumulated in `f64`, and both facts are stated in the report's `provenance` so the choice is auditable.
4. **Percentiles and medians use the lower-median / nearest-rank convention** already used by `ChannelStatistics` (`scopes.rs:552-592`): for `n` sorted samples, `p99` is element `min(n − 1, ceil(0.99·n) − 1)` and the median is element `floor((n − 1)/2)`.
5. **Frame sampling is the §6.2 closed-form integer rule**; no clock, no adaptive stride, no "sample until converged".
6. **Per-node ordering** is track → clip → effect-chain order (§3.7). **Exception ordering** is `(severity desc, code asc, tiebreak asc)`. **Channel ordering** is always `[red, green, blue]`.
7. **The skin circular mean is computed from `f64` sums of `cos θ` and `sin θ`**, not from a running angular average, so it has no order dependence and no wrap discontinuity; `R` is clamped to `[0, 1]` before the logarithm.
8. **The QC engine has no clock, no RNG, no filesystem access, and no thread pool.** Two runs on the same raster produce byte-identical JSON.
9. **The delivery dither is deterministic** (§5.4): a fixed 8×8 spatial pattern, identical across runs and independent of frame index. Determinism of the encoded output is therefore not assumed from the codec but stated from measurement — and even so, §13 refuses frame-hash pinning across platforms.

---

## 11. Exit fixtures and numeric gates

`crates/kinewright-media/src/cc6_fixtures.rs` (registered as `mod cc6_fixtures;` in `media/src/lib.rs`), `crates/kinewright-media/tests/fixtures/cc6_manifest.json`, `crates/kinewright-core/tests/cc6_core.rs`, plus agent cases in `crates/kinewright-agent/tests/mcp_server.rs` and inline app cases. Every fixture records git revision, backend, adapter, software-fallback and GPU-claim flags, OS, lane, delivery depth, node stack, thresholds, and output hashes through `cc1_fixtures.rs`'s existing `emit_evidence` (322-374).

### 11.0 Fixture-quality rules (CC1–CC5 reviews — normative, unchanged)

1. Expected values are written out analytically from §3–§6's equations, either as literal constants here or transcribed independently in `f64` in the fixture. **A fixture must not obtain an expected value by calling `measure_color_qc`, `bt709_limited_ycbcr`, `encode_bt709`, `encode_bt709_delivery`, the compositor, or swscale.** (Generating *source content* with an independently transcribed matrix is not an expected value and is permitted; §11.1 says where.)
2. Every reported quantity has a numeric expected value at a minimum, a maximum, and a representative interior case. `is_finite()` is never a sufficient assertion.
3. Manifest thresholds are asserted **equal to the code constants**, never restated as literals (`assert_manifest_f64` / `assert_manifest_f32`, `cc1_fixtures.rs:1595-1614`).
4. Error assertions check `code`, `field`, `observed`, and `allowed`.
5. **A check that cannot fail is a defect.** Every excursion counter has both a case that trips it above its threshold and a case that stays below, and the fixture asserts both directions. Every budget has a case measured strictly inside it, with the measured margin recorded — a budget that no measurement approaches proves nothing.
6. GPU fixtures run on the software fallback in the default lane (`fallback_gpu()`, `cc1_fixtures.rs:1492-1520`) and on hardware in an `--ignored` lane (`hardware_gpu()`, 1543-1556). `KINEWRIGHT_GPU_TESTS_MAY_SKIP` is never consulted by these fixtures; where a skip branch exists in the agent tests it asserts a typed code, per CC5's precedent.
7. The **CC1 §6.2 pixel-exact sampling clause** applies unchanged: the working-proof parity gate compares the GPU pixel at `(x, y)` against the CPU reference on the source texel at `(x, y)`, which holds only because a pixel-exact layer is point-sampled (`compositor.rs:823-833, 1152-1174`).
8. **No assertion may require a single Y code from an 8-bit delivery output** where the input does not land exactly on a code (§5.4).
9. `SPEC_F64_TOLERANCE` (`cc1_fixtures.rs:120`) and the four `BT709_*` matrix constants (`color_pipeline.rs:39-42`) become `pub(crate)` so `cc6_fixtures.rs` can reference them rather than re-declaring drifting copies.

### 11.1 Rasters and the synthetic delivery source

**`cc6_qc_raster()`** — **80 × 40 = 3200** pixels, in-memory, no media. One pixel is exactly **125 basis points horizontally and 250 vertically**, so every region below is a basis-point-exact rectangle and every ROI in §11.2 resolves to the pixel rect it names without relying on rounding.

| Region | Pixels | Content | Purpose |
| --- | ---: | --- | --- |
| in-range ramp | 1 152 (48 × 24) | linear `0.0 … 1.0` in both axes, all channels in `[0, 1]` | the population that must trip nothing |
| over block | 288 (36 × 8) | linear `1.05` on all channels | range over: `e = 1.0243960098942206`, `Y_8 = 240.342726` |
| under block | 288 (36 × 8) | linear `(−0.01, 0.5, 0.5)` | range under **and** gamut, with a bounded `d` |
| skin patches | 4 × 96 (12 × 8 each) | the four CC5 `CHART_PATCHES` skin triples through `grade709_decode` | the §3.5 band anchors |
| product patches | 2 × 96 | CC5 `product_red`, `product_cyan` | must fall **outside** the skin band |
| below-black pixel | 1 | linear `(−0.02, −0.005, −0.005)`, `Y = −0.008189 < 0` | the `d`-undefined case (§3.3) |
| isolated over pixel | 1 | linear `1.2` on all channels, at `(48, 0)` | the sub-threshold direction |
| achromatic surround | 894 | CC5 `CHART_SURROUND` `(0.45, 0.45, 0.45)` | the near-achromatic exclusion |

`1152 + 288 + 288 + 384 + 192 + 1 + 1 + 894 = 3200` exactly, asserted by **`cc6_qc_raster_populations_are_the_contract_table`**. The generator lives in `cc6_fixtures.rs` rather than in `compositor.rs`'s test module because four different places measure it — the two working-proof parity fixtures, the full-resolution refusal, and the manifest — and a second copy of the raster would be a second definition of every population in this table.

Whole-raster expectations, hand-computed on the 3200 denominator and asserted:

| Quantity | Value |
| --- | ---: |
| over count / bp, each channel | 289 / **903** (trips the 10 bp threshold) |
| under count / bp, red | 289 / **903** |
| under count / bp, green and blue | 1 / **3** (below the threshold, in the same measurement) |
| out-of-gamut count / bp | 289 / **903** |
| `clamped_pixel_count` / bp | 578 / **1806** |
| `below_black_pixel_count` | 1 |
| `maximum_over_excursion_millionths` | 93 969 (the isolated `1.2` pixel) |
| `minimum_under_excursion_millionths`, red / green / blue | −90 000 / −22 500 / −22 500 |
| `minimum_linear_millionths` | −20 000 |
| `maximum_desaturation_millionths` | 24 902 (`d = 0.01 / 0.401574`, from the under block; the below-black pixel is excluded) |

**The sub-threshold ROI** is `left = 0, top = 0, right = 6125, bottom = 6000` — 49 × 24 = 1 176 pixels: the whole ramp, the isolated over pixel at `(48, 0)`, the below-black pixel at `(48, 1)`, and 22 surround pixels. In it, over bp = **8**, under bp (each channel) = **8**, and gamut bp = **8**: all below 10, so no `delivery_range_excursion` and no `delivery_gamut_excursion` is raised, while the whole-raster measurement raises both. A single pixel is `floor(10000/3200) = 3` bp of the full raster, which is why no isolated pixel can ever trip either threshold at whole-raster scope.

**`cc6_delivery_source()`** — **320 × 180, 25 fps, 60 frames**, generated at test time. The encoder's GOP is `2 · fps = 50` (`export.rs:142`), so 60 frames span **two** GOPs and §6.2's sample set `0, 14, 29, 44, 59` puts frame 59 in the second GOP.

Generation, concretely: the fixture writes 60 raw `yuv444p` frames to a temp file and invokes the pinned CLI through `run_ffmpeg` (`test_support.rs:262-284`) with `-f rawvideo -pix_fmt yuv444p -s 320x180 -r 25 -i <file>`, `-vf setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709`, `-c:v ffv1 -level 3 -g 1`, and the explicit `-color_primaries bt709 -color_trc bt709 -colorspace bt709 -color_range tv` flag set — the tag recipe of `generate_delivery_source` (`cc1_fixtures.rs:3719-3806`) and `generate_ramp_media` (`cc1_fixtures.rs:545-632`), which CC1 requires because it rejects an untagged source. `run_ffmpeg` cannot pipe stdin, hence the temp file. The planes are computed in the fixture with an **independently transcribed** f64 limited-range BT.709 forward matrix (rule 11.0.1's transcription clause), never by calling `bt709_limited_ycbcr`.

Content, per frame: the twelve-patch CC1 neutral chart; a horizontal and a vertical neutral ramp; the four CC5 skin patches and `product_red` / `product_cyan` at their display-encoded codes; **one hard saturated edge** (a pure-blue block abutting a pure-green block) so the RGB-max term of §6.3(c) is exercised and reported; and **one moving element, pinned**: a 16 × 16 white square whose top-left corner is at `(4 · frame, 20)`, i.e. `x = 0 … 236` over frames 0–59, always fully inside the raster (`236 + 16 = 252 ≤ 320`), so its contribution to every count is derivable from the frame index alone. The document that renders it carries **a deliberately over-range and out-of-gamut grade** — a `color_wheels` node with a strong gain on one region and a strong negative lift on another — so the excursion is a *product of the managed pipeline*, not a hand-written buffer.

### 11.2 Required fixtures

Every entry states its **passing** and **failing** direction (rule 11.0.5). **Every test name below is the name the tree declares**; `cc6_declared_test_names_exist_in_their_source_files` (item 23) fails the build if any of them drifts, and the sub-bullets are the tests that exist and that the draft did not list.

1. **`cc6_range_anchors_match_the_hand_derived_delivery_encode`** — the ten-row §3.2 table asserted in `f64` against an independent transcription within `SPEC_F64_TOLERANCE`; `e(1.0) == 1.0` exactly and is **not** counted (strict `>`); `e(0.018)` takes the **power** branch in both f32 and f64 and the 2.479e-4 seam discontinuity is recorded on the 219-code limited luma span; per-channel counts and basis points on `cc6_qc_raster` equal §11.1's table exactly. *Fails* on the whole raster (903 bp ≥ 10 → `delivery_range_excursion`); *passes* on the 1 176-pixel ROI (8 bp) and on a ramp-only ROI (0 bp).
   - **`cc6_negative_range_anchors_take_the_power_branch`** — pins `e(−0.02) = −0.089_999_733` (f64) and the branch it took, so the E1 mislabel cannot come back; `−0.01` and `−0.005` are asserted to take the *linear* branch, which is the direction that makes the test able to fail.
2. **`cc6_gamut_and_range_under_describe_the_same_pixel_set`** — `out_of_gamut_pixel_count` equals the count of pixels with at least one under-range channel, on a raster containing over-range, under-range, and both; the over block contributes **zero** to gamut; `maximum_desaturation_millionths == 24_902` against the hand-computed `d = −m/(Y − m)`; the below-black pixel lands in `below_black_pixel_count`, is counted as out of gamut, and is excluded from the maximum; a pixel with `m < 0 < Y` small gives `d` approaching but not exceeding 1. *Fails* at 903 bp on the whole raster; *passes* at 8 bp on the §11.1 ROI.
3. **`cc6_bt709_forward_ycbcr_matches_the_spec_at_eight_and_ten_bits`** — the eight-row §3.4 table at both depths, `f64`, `1e-6`; the round trip against an **independently transcribed** `f64` inverse matrix — not against `decode_bt709_ycbcr`, which is private to `kinewright-media` and invisible from `cc6_core.rs` (E11) — recovers the input within `1e-6` **after dividing the codes by `2^bits − 1`**; the derived constants `0.1873242729306488` and `0.46812427293064884` are asserted against `BT709_GREEN_FROM_CB` / `BT709_GREEN_FROM_CR` within `1e-6`; `linear = 1.05` predicts `Y_8 = 240.342726` and `Y_10 = 961.370905`. *Fails* on a synthetic `R'G'B'` outside `[0,1]`; *passes* — and is asserted **not** to be an excursion — on red/blue at `Cr`/`Cb` exactly 240/960 and on yellow/cyan at exactly 16/64.
   - **`cc6_bt709_limited_ycbcr_refuses_a_depth_that_is_not_a_delivery_lane`** — `bit_depth` is 8 or 10 and nothing else; a third value is a `debug_assert`, not a silently scaled answer.
   - **`cc6_ycbcr_legal_bounds_are_strict_through_the_measurement`** — the strictness of §3.4's `>` / `<` proved end to end through `measure_color_qc` on single-pixel proofs rather than on the matrix alone: `(1,0,0)` gives `cr.maximum_code_hundredths == 24_000` with `above_count == 0`, `(1,1,0)` gives `cb.minimum_code_hundredths == 1_600` with `below_count == 0`, and a neighbour **one code away** in each direction does trip the count. It also pins the E5 sense of the extremes — observed sample codes, `24_110` and `1_490` on the neighbours, not excursion amounts.
4. **`cc6_skin_band_constants_are_derived_from_the_cc5_patches`** — the four `CHART_PATCHES` skin triples, transcribed independently, transformed `grade709_decode → encode_bt709_delivery → Cb/Cr → atan2`, produce `[12385, 12396, 12385, 12188]`; their circular mean is `12339` with `R = 999_885` millionths; every patch sits at least **1 049** centidegrees inside the band edge and at most 1 154; the derived NTSC `+I` axis is `12300`. *Fails* (outside) for the Rec.709 red primary at `10291` (848 cd outside), CC5 `product_red` at `10137` (1 002 cd outside), and `product_cyan` at `29201`. `skin_deep`'s chroma `73_341` is 3.67× `SKIN_MIN_CHROMA_MILLIONTHS = 20_000`; the surround's chroma is `0`.
5. **`cc6_skin_diagnostics_report_circular_statistics_on_a_chosen_region`** — on an ROI covering one skin patch: `considered_pixel_count == 96`, `excluded_achromatic_pixel_count == 0`, `mean_hue_centidegrees` equals that patch's pinned angle, `circular_spread_centidegrees == 0` (with `R` clamped, no `NaN`), `median_chroma_millionths` equals the pinned value ±1, `in_band_basis_points == 10000`, no exception. *Failing directions*: an ROI over a product patch gives `in_band_basis_points == 0` and a `skin_region_outside_band` Info exception; an ROI over the surround gives every pixel excluded, `mean_hue_centidegrees == None`, `circular_spread_centidegrees == 18000`, `in_band_basis_points == 0`, and **no** exception (§3.5's 0/0 rule). A two-patch ROI straddling `0°`/`360°` (synthetic hues at `35900` and `100` centidegrees) asserts the circular mean is `0`, not `18000`.
6. **`cc6_working_proof_matches_the_cpu_reference_on_the_software_lane`** — the CC1 §6.2 linear banded gate reused verbatim and asserted equal to the code constants: `|v| ≤ 1` → max `1.5e-3`, P99 `7.5e-4`, mean `2.5e-4`; `1 < |v| ≤ 2` → max `1.5e-3`, P99/mean `9.765625e-4`; `|v| > 2` excluded, counted, recorded. Run on `cc6_qc_raster` with a full CC3/CC4/CC5 node stack. **No new tolerance is invented.** The pre-CC6 `render_working` callers are asserted to produce values identical to their pre-change results. *Failing direction*: the same comparison against a reference perturbed by `2 × LINEAR_CPU_GPU_MAX` is asserted to exceed the gate, so the gate is known to be able to fail. **Measured margin, recorded rather than hidden** (E18): the in-gamut mean is `2.478e-4` against a gate of `2.5e-4` — about **0.8 %** of headroom. That is the f16 storage floor of the `Rgba16Float` working surface, not slack, and it is stated here so that a future reader does not mistake a tight number for a fragile one or "fix" it by widening the CC1 constant.
7. **`cc6_working_proof_matches_the_cpu_reference_on_hardware`** — `#[ignore = "requires a physical supported GPU; run explicitly with --ignored --nocapture"]`, `hardware_gpu()`, same assertions and the same negative control, lane recorded as `hardware`.
8. **`cc6_working_proof_refuses_a_claim_that_is_not_full_resolution`** — a `WorkingProof` whose `metadata.render.full_resolution` is `false`, built by constructing `MonitorProofMetadata` directly (both legs of the `compositor.rs:299` conjunction: wrong scale, and full scale with a raster ≠ `document.resolution`), is refused by `measure_color_qc` with `color_qc_proxy_proof_refused` carrying `field`/`observed`/`allowed`. *Passing direction*: the real full-resolution proof measures normally. No proxy render is requested anywhere, because none can be. The name is declared **twice**, in `cc6_core.rs` and in `compositor.rs`: core proves the refusal against a hand-built metadata claim, media proves it against a proof the production renderer actually produced, and neither can stand in for the other.
9. **`cc6_delivery_tag_check_covers_both_modes_and_marks_white_point_not_representable`** — *pre-export mode*: a managed document at `Eight` and at `Ten` yields `conforming == true`, `tag_source == "materialised_export_settings"`, empty `not_representable`; a document whose delivery description has four wrong fields yields **four** `DeliveryColorMismatch` entries in the fixed check order while `delivery_color_mismatch` still returns exactly the first. *Post-export mode*: a probed H.264 description (`white_point: Unknown`, `provenance: StreamMetadata`) produces **zero** mismatches and exactly one `not_representable` entry for `white_point`, with `tag_source == "probed_output_file"`; and `delivery_color_mismatch` applied to that same probed description **does** reject it, proving the two functions are not interchangeable (facts gap #9). The `unsupported_delivery_color` message is asserted to contain "8-bit or 10-bit", not the pre-CC6 string. The **`bit_depth` leg is asserted directly on `delivery_color_mismatch` / `delivery_color_mismatches`**, not through `delivery_conformance`, which can no longer reach it (E4, §4.1).
10. **`cc6_eight_bit_encoded_delivery_passes_tag_luma_and_difference_budgets`** — **the exit gate.** `cc6_delivery_source()` → managed import → the deliberate over/out-of-gamut grade → production export at `DeliveryEncodeDepth::Eight` → re-probe (`Bt709` × 3, `Limited`, `Eight`, `StreamMetadata`, confidence `10000`; `white_point` not representable) → `decoded_pixel_format == "yuv420p"` → §6.2 sampling picks `0, 14, 29, 44, 59` → per-frame `render_delivery` reference at `FullResolution` with `full_resolution` asserted → §6.3(a)+(b) gated against `DELIVERY_LUMA_*_8BIT`, `DELIVERY_RGB_MEAN_CODE_8BIT`, `DELIVERY_PSNR_FLOOR_DB_HUNDREDTHS_8BIT`, **with the measured margin recorded and asserted ≥ 2×** → RGB max and P99 recorded, explicitly **not** asserted against a bound → decoded native-plane legality under §6.4 → `technical_pass == true`. Budgets asserted numerically distinct from `MONITOR_CPU_GPU_{MAX, P99, MEAN}`. Runs in the **default lane on both Windows and Linux CI**; nothing about it is `--ignored` or environment-gated, and it uses `fallback_gpu()` — which fails loudly — rather than a skipping helper. Per sampled frame it also asserts the moving element's pinned pixels (near-white at `(4·frame + 8, 28)`, grey at `(4·frame − 8, 28)`), so a comparison that silently measured the wrong frame would be caught by the picture and not only by the identity. *Failing direction*: fixture 13.
   - **`cc6_delivery_source_moves_the_pinned_element_across_the_sampled_frames`** (`cc6_fixtures.rs`) — the source generator itself: on each of the five sampled frames the 16 × 16 moving square is asserted near-white at `(4·frame + 8, 28)` and grey at `(4·frame − 8, 28)`. A source whose frames were all identical would make every "we sampled five frames" claim in this section vacuous, and nothing else would notice.
   - **`cc6_eight_bit_export_verifies_end_to_end_through_the_production_surface`** (`verify.rs`) — the same round trip driven through `Analysis::verify_delivery_output` rather than through the fixture's own assembly, so the production entry point is the thing under test.
   - **`cc6_verification_refuses_an_export_whose_edit_list_drops_the_last_frame`** (`verify.rs`) — the §0.3 defect, manufactured on a real file by the test-only `VideoPacketDuration::Zero` export path, refused with `delivery_verification_frame_count_mismatch` naming presented `T − 1` against implied `T`. This is the failing direction of §6.2's `O(GOP)` presented-frame cross-check.
   - **`cc6_delivery_reference_denominator_is_the_delivery_intermediate_white`** (`verify.rs`) — E9/E16's alias asserted, so the two sides of §6.3's comparison cannot acquire separate denominators.
   - **`cc6_delivery_budgets_are_distinct_from_the_compositor_gate`** (`verify.rs`) — every `DeliveryBudgets::for_depth` field asserted numerically distinct from `MONITOR_CPU_GPU_{MAX, P99, MEAN}`, which is what forces the 8-bit luma mean to `0.4` rather than `0.5` (§6.3).
11. **`cc6_ten_bit_encoded_delivery_passes_tag_luma_and_difference_budgets`** — the same at `DeliveryEncodeDepth::Ten`: `decoded_pixel_format == "yuv420p10le"`, probed `ColorBitDepth::Ten` and `profile` reported as High 10 with no `profile` option set, budgets `DELIVERY_*_10BIT`. The lane's justification is asserted as: **8-bit-equivalent RGB mean strictly smaller than the 8-bit lane's AND PSNR strictly larger**, on the same source and the same frames. Non-vacuity clause: if the 8-bit lane's RGB mean is exactly `0`, the fixture **fails** with "the source does not exercise the codec" rather than passing. Both measured values go into the evidence JSON. Before encoding, the fixture asserts the build's libx264 advertises `yuv420p10le` and **fails typed** (`delivery_encoder_pixel_format_unavailable`) if not — it never skips. `ffprobe` is asserted to report profile **`High 10`** here and **`High`** on the 8-bit control, so "10-bit" is read back from the file rather than assumed from the request. Measured: 8-bit-equivalent RGB mean `0.414572` against the 8-bit lane's `0.743535`, and PSNR `37.00` against `36.86` — strictly better on both, with the non-vacuity clause satisfied because the 8-bit mean is not `0`.
12. **`cc6_decoded_native_planes_report_ycbcr_excursions_in_delivery_code_units`** — the native-plane reader on both lanes: plane dimensions are `(w, h)` and `(w/2, h/2)`; every 10-bit sample is `≤ 1023`. *Passing direction*: the production export of the CC6 source reports strict-box excursions **below** `DECODED_RANGE_EXCEPTION_BASIS_POINTS = 100` and nothing outside the EBU R 103 box, with the measured amounts recorded. *Failing direction*: a hand-built FFV1 `yuv420p` and `yuv420p10le` file (test-only CLI) carrying `Y = 250` and `Cb = 5` patches is read by the same reader and reports excursions outside both boxes, raising `decoded_range_excursion`. A `Predicted` report for a legal source shows exactly zero. The passing direction of the *production* export exercises only the **under-threshold** direction; the raising direction is this fixture's hand-built file, and the contract says so rather than letting a comment imply the production export could trip it.
   - **`cc6_delivery_verification_plane_out_of_container_is_typed`** (`verify.rs`) — a 10-bit native sample of `1024` is refused `delivery_verification_plane_out_of_container` with all four fields, and `1023` passes one step away, so a byte-order mistake can never be reported as a colossal legal excursion.
   - **`cc6_plane_excursion_basis_points_count_both_directions`** (`cc6_core.rs`) — `excursion_basis_points(sample_count)` sums `below_count` and `above_count`, so a plane 60 bp below the floor **and** 60 bp above the ceiling rates 120 bp and trips §6.4's 100 bp threshold that either direction alone would not; an empty population is `0`.
   - **`cc6_an_unseen_plane_reports_the_empty_interval_and_an_empty_sample_set_is_refused`** (`verify.rs`) — `PlaneLegalAccumulator::new(16, 235).excursion()` reports the UNSEEN pair with `samples_seen() == false` and `excursion_basis_points(0) == 0`, and a verification whose sample set is empty is refused `delivery_verification_frame_count_mismatch` with `observed: "0 sampled frames"` rather than publishing a `within_budgets: true` nobody measured (§6.1).
13. **`cc6_starved_bitrate_export_trips_the_decoded_difference_budget`** — the same source exported at `-b:v 100k`: `within_budgets == false`, a `decoded_difference_over_budget` Error with `code`/`field`/`observed`/`allowed`, `verification.technical_pass == false`, and the queue leaves the job **`Completed` with `error: None`** and the output file present at its original path (asserted: not renamed, not deleted). Measured over budget on luma max **35**, luma P99 **6.0**, and luma mean **0.621205**, with the RGB mean and PSNR still inside their floors — which is the point: the codec-only terms are the gate.
   - **`cc6_starved_bitrate_ten_bit_export_trips_the_decoded_difference_budget`** — the same failing direction on the 10-bit lane, and the **only** thing that bounds `DELIVERY_LUMA_P99_CODE_10BIT_MILLIONTHS`, which the passing source measures at exactly `0` (§6.3, rule 11.0.5). Measured: luma max **121**, P99 **24.0**, mean **2.640545**, and the RGB mean **1.180978** over its tighter `1.0` floor; PSNR stays inside at **35.98 dB**.
14. **`cc6_per_node_contribution_attributes_clipping_to_the_node_that_causes_it`** — a two-node stack where node A is neutral and node B applies the clipping gain: A's deltas are exactly `0`, B's `range_basis_points_delta` equals the hand-computed difference, `attribution == "node_removed"`, and the sum of the deltas is **not** asserted to equal the total (clipping is not additive, and the fixture states that). A `primary_correction` node — which has no `bypass` parameter — is attributed normally by removal, which is the point of R1. An inactive node reports `active: false`, its `inactive_reason`, and both deltas `0`. Seventeen candidates truncate to sixteen in the stated order with `truncated: true`, `considered_node_count == 17`, and a `qc_per_node_truncated` exception. The source document is byte-identical afterwards.
   - **`cc6_per_node_contribution_order_matches_production_z_order`** (`cc6_fixtures.rs`) — the media half: core's track → clip → effect ordering asserted equal to `visual_layers_at`'s production z-order on a three-track document. It lives in media because core cannot depend on this crate, which is the whole reason the ordering is core-owned and separately asserted.
   - **`cc6_per_node_candidates_find_the_on_screen_clip_whatever_the_clip_order`** — candidate selection scans the whole track for the clip covering the frame instead of stopping at the first clip that starts after it; the sorted-by-`timeline_start` invariant is documented on the function rather than assumed by a `break`.
15. **`cc6_typed_qc_refusals_carry_code_field_observed_and_allowed`** (`tests/cc6_core.rs` plus a media half) — each of `color_qc_raster_length_mismatch` (pixels shortened by 4), `color_qc_region_empty` (an ROI resolving to zero pixels), `color_qc_node_budget_exceeded` (`max_nodes = 0` and `= 17`), `color_qc_matte_region_raster_mismatch` (a coverage raster one pixel wider), `delivery_pixel_format_depth_mismatch`, `delivery_encoder_pixel_format_unavailable`, `delivery_verification_plane_out_of_container` (a native sample of 1024), `delivery_verification_not_full_resolution`, and `delivery_verification_frame_count_mismatch` is tripped once with all four fields asserted, and each has a neighbouring **passing** case one step away (correct length, non-empty region, `max_nodes = 16`, matching raster, matching depth, available format, sample 1023, full-resolution reference, matching frame count). `color_qc_matte_region_raster_mismatch` covers both legs: a coverage raster of the wrong *dimensions*, and one whose buffer length is not `w · h · 4`, each with an accurate `observed`.
   - **`cc6_non_finite_samples_are_counted_and_never_classified`** — §3.1's guard, in five directions: one `NaN` pixel beside a finite neighbour is counted in `non_finite_pixel_count`, classified nowhere, and raises `color_qc_non_finite_sample` as an **Error** that clears `technical_pass`; the finite neighbour measured alone passes, so the Error is the `NaN` and nothing else; a raster of *only* `NaN` leaves every `Y′CbCr` plane reporting the unseen empty interval with `samples_seen() == false` rather than a fabricated `0`; `+inf` and `−inf` are counted rather than saturating an extreme to `i64::MAX` or counting as an over-range pixel; and a `NaN` **alpha** is not visible, which only the `!(alpha > 0.0)` spelling of §3.1's predicate gets right.
   - **`cc6_qc_refusals_keep_their_code_through_media_error`** — every `ColorQcError` variant round-tripped through `MediaError::ColorQc` with `?`, asserting `recovery_code()` still returns the variant's own `code()`. This is what makes E32's "no consumer parses a `Display` string" checkable rather than aspirational.
   - **`cc6_exceptions_sort_by_severity_then_code_then_field`** — §10.6's total order asserted on a list containing all three severities and two exceptions sharing a code, so the tiebreak is exercised and not merely present.
   - **`cc6_delivery_verification_sampling_is_the_closed_form_integer_rule`** — §6.2's formula at `n == 1` (frame 0 only), `T <= n` (every frame, duplicates removed in order), and `T = 60, n = 5` (`0, 14, 29, 44, 59`).
   - **`cc6_delivery_verification_refuses_a_frame_count_outside_the_sampled_range`** — `DeliveryVerificationRequest::validate()` at `frame_count` `0` and `17`, all four fields asserted, and `1` and `16` passing one step away.
   - **`cc6_verification_refuses_a_frame_count_the_sampler_would_have_clamped`** (`verify.rs`) — the media half of the same refusal, proving `verify_delivery_output` really does call `validate()` first rather than inheriting `sample_frames`'s clamp.
   - **`cc6_verification_refuses_budgets_from_the_other_delivery_lane`** (`verify.rs`) — both directions: 10-bit budgets on an 8-bit export and 8-bit budgets on a 10-bit export are each refused `delivery_verification_budget_lane_mismatch` with the request's and the lane's budgets in `observed` and `allowed`, and the matching lane passes.
   - **`cc6_a_decoded_frame_that_is_not_the_requested_frame_is_refused`** (`verify.rs`) — the passing direction reads the export at its own rate and asserts `sample(1).at == 1` and that the comparison records that identity; the failing direction manufactures an overshoot and asserts `observed == "decoded frame 2 for requested frame 1"` against `allowed == "decoded frame 1"` (§6.2).
16. **`cc6_video_scopes_v2_points_at_get_color_qc_instead_of_a_fabricated_zero`** (agent) — the response's `gamut` object has `measured == false`, `code == "gamut_requires_working_stage"`, `tool == "get_color_qc"`, and **no** `out_of_range_pixels` or `out_of_range_basis_points` key. `ScopeStage::WorkingLinearPostComposite` handed to `get_video_scopes_v2` is rejected with the agent's `unsupported_stage`; handed to `ScopeRequest::validate` / `measure_scopes` it returns **`ScopeError::UnsupportedStage { stage }`**, and `MonitoringPostComposite` still validates.
17. **`cc6_export_settings_and_job_records_serialize_deterministically`** (`tests/cc6_core.rs`) — `ExportSettings` round-trips through JSON equal on every field but `cancellation`; two serializations of the same value are byte-identical; a pre-CC6 `ExportJobRecord` JSON (no `verification`, no `verification_unavailable_reason`, no `delivery_bit_depth`) deserializes with `None`, `None`, and `Eight`; a record with `verification: None` re-serializes **without** the key and `verifying: false` **without** its key; the four `DeliveryProfile` wire strings are asserted literally in **both** spellings (serde `youtube1080p`, `as_str()` `youtube_1080p`; §9.2).
18. **`cc6_get_color_qc_is_evidence_only_and_revision_gated`** (`tests/mcp_server.rs`, following the CC5 template at 594) — the tool returns `evidence_only: true`, `applied: false`, `stage: "working_linear_post_composite"`, `full_resolution: true`; the document revision is unchanged after the call; a stale `expected_revision` returns `stale_revision { expected_revision, actual_revision }` and an absent one succeeds; `checks: ["skin"]` with no region returns `color_qc_region_required` and with a region succeeds; the schema is asserted to have **no** `resolution` property. Two selectors together return `color_qc_frame_selector_conflict` with the first offending selector in `detail`; a frame outside the project returns `color_qc_frame_out_of_range` with all four fields. The `assumptions` list is asserted to be the **exact** default set (tags present, skin and per-node absent), not merely non-empty, and the visible-pixel assertion is an exact count rather than a restatement of the region's own number. Under `KINEWRIGHT_GPU_TESTS_MAY_SKIP=1` the skip branch asserts the **typed** `working_proof_unavailable`, never both branches.
19. **`cc6_export_dialog_and_queue_agree_on_delivery_color`** (app + agent) — for each `DeliveryEncodeDepth`, the dialog's inline `ExportSettings` and the queue's `DeliveryProfile::export_settings` produce the same `delivery_color`, while the dialog's Frame size and FPS values are asserted to still reach `ExportSettings` (the regression H7 names).
20. **`cc6_conformance_cache_does_not_cross_delivery_lanes`** (app) — a cached 8-bit `DeliveryConformanceReport` is not returned for a 10-bit `ConformanceKey`, and each report's `delivery_bit_depth` matches its key.
21. **App cases** (inline `#[cfg(test)] mod tests`) —
    - **`cc6_qc_mask_marks_only_the_flagged_pixels`** (`preview_ui.rs`): every pixel of the over block is `QC_MASK_OVER_RANGE_COLOR`, every pixel of the under block and the below-black pixel is `QC_MASK_UNDER_RANGE_COLOR`, every in-range pixel is the hand-computed half-luma grey, a pixel that is both over on one channel and under on another takes the under colour, and a `NaN` sample takes the under colour rather than being drawn black — the same classification core gives it (§3.1).
    - **`cc6_export_dialog_reports_the_verification_result`** (`export_ui.rs`): a headless `egui::Context` (`inspector_ui.rs:7462-7513`'s pattern) renders the block for a passing, an over-budget, a tag-mismatch, and an unavailable verification and asserts four distinct statuses and their four colours (§8.4).
    - **`cc6_verification_block_reports_every_probed_tag_field`** (`export_ui.rs`): the per-field rows come from `color_qc_ui::tag_field_rows`, with a mismatch, a not-representable, and an agreeing field each rendered in its own tone.
    - **`cc6_the_dialog_names_the_verifying_stage_instead_of_freezing_the_bar`** (`export_ui.rs`): once encoding is done and `verifying` is set, the progress bar is gone and the "Verifying export…" line is present.
    - **`cc6_cancelling_before_verification_reports_not_verified_with_the_reason`** (`export_ui.rs`): a cancellation observed before verification starts yields `NOT VERIFIED` carrying `EXPORT_CANCELLED_BEFORE_VERIFICATION`, and the dialog can be closed.
    - **`cc6_scopes_panel_renders_absolute_per_channel_clipping`** (`color_scopes_ui.rs`): all four channels present with the pinned values; the panel with no evidence renders none.
    - The budget expectations in these tests are **formatted from `DeliveryBudgets::for_depth(Eight)`**, never from literals, so a re-baseline of §6.3 does not turn a UI test red for a reason that is not a UI defect.
22. **`cc6_core_delivery_transfer_is_bit_identical_to_the_media_transfer`** (media) — `kinewright_core::color_qc::encode_bt709_delivery` and `kinewright_media::color_pipeline::encode_bt709` agree on `to_bits()` for the ten §3.2 anchors and for a dense sweep of `−2.0 ..= 2.0` in steps of `1/4096`, including both sides of the `0.018` seam and both signs. *Failing direction*: the same comparison against a deliberately mis-seamed transcription (`linear <= 0.018`) differs at `0.018`, proving the sweep can see a one-branch error.
23. **`cc6_manifest_declares_every_required_fixture_and_constant`** and **`cc6_declared_test_names_exist_in_their_source_files`** — the CC5 inventory pattern verbatim (`cc5_fixtures.rs:6144-6366`): `CC6_MEDIA_TESTS` (25, across `cc6_fixtures.rs`, `compositor.rs`, and `verify.rs`), `CC6_EXPORT_TESTS` (10, item 26's un-prefixed gate tests), `CC6_CORE_TESTS` (20), `CC6_AGENT_TESTS` (9), `CC6_APP_TESTS` (8), `CC6_INVENTORY_TESTS` (2), `CC6_EXTERNAL_OWNERS`, and `CC6_TEST_SOURCES: [(&str, &str); 10]` with `include_str!` over every file named — a **compile-time** dependency, so renaming a test in the core, agent, or app crate rebuilds this fixture and fails it. `cc6_test_source` panics on an invented path and `declares_test` requires a real `#[test]` / `#[tokio::test]` attribute. **The comparison is a sorted set equality in both directions**, so a test that exists but is not declared fails exactly as loudly as a declared name that does not exist. The manifest is additionally asserted to contain **no unresolved probe placeholder**: every threshold key must hold a number.
24. **Performance evidence** — **`cc6_performance_evidence_is_recorded_on_software_fallback`** and **`cc6_performance_evidence_is_recorded_on_hardware`** record the wall time of one 1920 × 1080 working proof plus a full `ColorQcReport` (range + gamut + skin + tags), and of a five-frame `verify_delivery_output`, on both lanes with a soft budget of one 24 fps frame (41.7 ms) for the QC measurement. Recorded evidence, not a hard gate — but a regression must be visible, and this measurement (P9) is taken **before** `verify`'s default is final. Measured: the QC measurement itself costs **4.9 ms** on lavapipe and **4.7 ms** on the RTX 3080, an order of magnitude inside the soft budget; the cost is the working proof (**1798.7 ms** / **1727.1 ms**) and, for verification, the five full-resolution delivery re-renders (**12 679.9 ms** / **12 609.5 ms**, about 2.5 s per sampled frame) against a **15.6 s** export. The two lanes agree within 4 %, so the cost is decode, readback, and upload rather than shading — which is why hardware does not rescue it and why `verify`'s default is examined again in §14.

25. **Queue verification outcomes** (`export_queue.rs`) — the §6.5 outcome policy, one test per outcome, each asserting the output file is present at its original path and neither renamed nor deleted:
    - **`cc6_a_verified_export_publishes_its_decoded_comparison_on_the_record`** — the passing direction: `verification: Some(..)`, `technical_pass == true`, `verification_unavailable_reason: None`.
    - **`cc6_a_failing_verification_completes_the_job_and_leaves_the_output_alone`** — over budget: `Completed`, `error: None`, `technical_pass == false`.
    - **`cc6_a_panicking_verification_is_contained_and_leaves_the_output_alone`** — `Completed`, `error: None`, `verification: None`, a reason containing `"panicked"`, file untouched.
    - **`cc6_an_unavailable_verification_records_its_reason_instead_of_a_pass`** — `verification: None` with a reason, and **no** exception on the record (E31).
    - **`cc6_cancelling_during_a_verification_leaves_the_record_cancelled_and_unverified`** — a blocking verification double cancelled mid-measurement leaves the record `Cancelled` with `verification: None`; verification itself is never interrupted.
    - **`cc6_verify_false_skips_the_measurement_and_serializes_byte_identically`** — `verify: false` produces a record whose JSON is byte-identical to a pre-CC6 one.
    - **`cc6_a_pre_cc6_job_record_deserializes_with_the_eight_bit_lane_and_no_verification`** — the §9.3/§9.4 defaults: `Eight`, `None`, `None`, `false`.

26. **The delivery gate's own unit tests** (`export.rs`) — deliberately **not** `cc6_`-prefixed, because they are the gate's tests written where the gate lives, and the inventory names them explicitly the way CC5 named its compositor tests: `accepts_the_ten_bit_sdr_rec709_delivery_contract`, `rejects_a_delivery_depth_outside_the_two_managed_lanes`, `delivery_lane_pixel_format_matches_the_core_lane_names`, `libx264_advertises_both_delivery_lane_pixel_formats`, `rejects_a_pixel_format_that_does_not_carry_the_declared_delivery_depth`, `rejects_a_delivery_pixel_format_this_build_does_not_advertise`, `delivery_filter_converts_sixteen_bit_full_range_rgb_to_limited_yuv420p10le`, `delivery_nominal_white_encodes_to_legal_white_through_the_export_filter`, `ten_bit_export_probes_as_rec709_limited_ten_bit_yuv420p10le`, and `every_exported_frame_is_presented_after_the_mp4_edit_list` — the last being §0.3's passing direction, asserted at both 25 fps and 30000/1001 so a non-integer frame rate cannot reintroduce the defect through rounding.

**No tolerance may be used to excuse a missing or wrong delivery tag, a fabricated measurement, a raster that is not full-resolution claiming to be a delivery reference, a decoded comparison against a non-full-resolution render, a check with no failing case, or a budget no measurement approaches.**

### 11.3 Manifest

`crates/kinewright-media/tests/fixtures/cc6_manifest.json`, `include_str!`'d and asserted key by key. **It is authored after §12 step 5 and must contain no unresolved probe placeholder — every threshold key holds a measured number**; the key-count assertion alone would otherwise be satisfied by a placeholder. Structure follows `cc5_manifest.json`: `contract`, `contract_token`, `manifest_version: 1`, then

- `stages`: `["monitoring_post_composite", "working_linear_post_composite"]`, the second marked `measurable_by_scope_engine: false`;
- `delivery_lanes`: two objects `{ name, bit_depth, pixel_format, codec, x264_params, profile_option: null, primaries, transfer, matrix, range, white_point }`;
- `delivery_intermediate`: `{ white: 65280, source_commit: "ad6f6a8" }`, asserted equal to `DELIVERY_INTERMEDIATE_WHITE`;
- `skin`: `band_center_centidegrees`, `band_half_width_centidegrees`, `patch_hue_centidegrees` (4), `min_chroma_millionths`, `band_exception_basis_points`, plus the derivation note;
- `ycbcr`: the four BT.709 constants, the four offset/span integers, and the eight-row anchor table at both depths;
- `raster`: the §11.1 populations and the whole-raster basis-point table;
- `thresholds`: one key per pinned constant in §3–§6, each asserted equal to the code constant by `assert_manifest_f64` / `assert_manifest_i64`, with the **key count** asserted so a constant cannot be added without declaring it;
- `budgets`: a `measurement` block naming the OS, lane, adapter, source, and FFmpeg build; `eight_bit` and `ten_bit` objects (luma max/p99/mean, rgb mean, psnr floor), each with `measured` and `margin_ratio` recorded from the fixture run, plus `rgb_max_code_reported_not_gated` and `rgb_p99_code_millionths_reported_not_gated` and a `units` string; `ten_bit_justification` with both lanes' RGB mean and PSNR and a `strictly_better` flag; and `starved_bitrate_failing_direction` with both lanes' measured terms, the `over_budget_fields` and `inside_budget_terms` lists, and the `Completed` / `error: None` outcome. The 10-bit luma P99 records `margin_ratio: "infinite (measured exactly zero)"` with a note naming the starved 10-bit direction as its bound, rather than a fabricated ratio. The distinctness assertion against `monitor_cpu_gpu` is asserted here too;
- `performance`: P9's raster, frame count, soft budget, and per-lane working-proof / colour-QC / export / verify timings, with the finding stated in prose;
- `measured_behaviour`: the dither finding, the inert-option list, the scaler comparison, and the decode-flag rule from §5;
- `required_fixtures` and `evidence_fixtures`: every test name in §11.2, cross-checked by fixture 23, plus `manifest_self_test` naming the two inventory tests.

---

## 12. Implementation order

**Size, measured.** CC5 (`fc9d148`) was **28 974 insertions**, of which `cc5_fixtures.rs` alone was 6 778 lines and `cc5_manifest.json` 675. The draft estimated CC6 at **18 000–25 000 insertions**. The slice as landed is **23 254 insertions and 424 deletions**, of which **21 615** are code, fixtures, and the manifest — 7 205 lines into 36 existing files and 14 410 lines in eight new ones — and the remainder is documentation: this contract plus 75 lines across `CHANGELOG.md`, `ROADMAP-AND-WORKFLOWS.md`, `M34`, and `M36`. The eight new code files: `color_qc.rs` 1 693 + `color_qc/nodes.rs` 235, `cc6_core.rs` 2 408, `verify.rs` 2 582, `cc6_fixtures.rs` 3 322, `cc6_manifest.json` 921, `color_qc_tool.rs` 714, `color_qc_ui.rs` 2 535. The estimate held — inside the 18 000–25 000 band, nearer the top of it — and it is recorded as measured so the next slice's estimate has a second data point and not only CC5's.

**Cut order.** If it must shrink, cut in this order and no other: (1) **per-node contribution** (§3.7) — the only part that costs N extra full-resolution renders and the only part whose absence leaves nothing unmeasured, just unattributed; (2) **QC mask view** (§8.2) — the numbers are already in the QC window. Do **not** cut the cross-platform encoded fixture (§11.2.10/11), the working stage (§2), or the typed delivery rejection (§4.2); those are the exit gate.

1. **Core QC engine.** `color_qc.rs` (§3 in full, including `encode_bt709_delivery` and the §3.0 types), `scopes.rs` (`ScopeStage::WorkingLinearPostComposite`, `measurable_by_scope_engine`, `ScopeRequest::validate`'s new condition), `media.rs` (`LinearRgbaImage`, `WorkingProof`, `WorkingProofMetadata`, the two trait methods, `ExportSettings` serde and doc fix, `recovery_code`), `delivery.rs` (`DeliveryEncodeDepth`, public `DeliveryColorMismatch`, `delivery_color_mismatches`, `DeliveryColorError`, `DeliveryVerificationError`, `delivery_tag_check`'s two modes, the depth argument on `delivery_conformance` and `DeliveryConformanceReport`, the §6.3 types and budget constants, the two message changes), `crates/kinewright-core/tests/cc6_core.rs`.
2. **Media working proof.** `compositor.rs` (`render_working*` productionised, returning `LinearRgbaImage`; the four `BT709_*` constants `pub(crate)`), `render.rs` (`FrameRenderer::render_working`), `engine.rs` (`working_proof_for_document`). The 14 existing `render_working` call sites updated mechanically.
3. **Media delivery widening.** `export.rs` (depth-driven `set_format` and `format` node, the encoder pixel-format check, typed rejection, the three named constants, and **`VideoPacketDuration`** with §0.3's packet-duration fix and its test-only `Zero` path), and the rewritten `export.rs` mod tests plus `delivery.rs:1272/1299`.
4. **Media verification.** New `verify.rs` (one seek-based decode pass, `NativePlaneFrame`, the luma-plane and RGB comparison, the EBU R 103 rule), `engine.rs` (`verify_delivery_output`).
5. **Measurement.** Run Appendix A's items on both operating systems and confirm every §6.3 budget against `cc6_delivery_source()`'s own measurement, including P9's cost numbers and P11's Windows run. **Steps 6–9 must not start against unconfirmed budgets, and `verify`'s default is finalised here.** Done: every §6.3 constant was re-baselined against the fixture's own source before the fixtures landed (§6.3's tables), P9 is measured on both lanes (§11.2.24), and `verify` stays defaulted to `true`. P11 remains outstanding until the Windows job runs, which §14 carries as a risk rather than a blocker because §11.2.11 fails typed rather than skipping.
6. **Fixtures.** `cc6_fixtures.rs`, `tests/fixtures/cc6_manifest.json`, reusing `cc1_fixtures.rs`'s provenance, `DiffMetrics`, banded linear gate, lane helpers, and `emit_evidence`; `SPEC_F64_TOLERANCE` made `pub(crate)`.
7. **Agent surface.** New `color_qc_tool.rs` (`get_color_qc`); `color_scopes.rs` (gamut pointer); `export_queue.rs` (verification wiring, `verify`, depth, the two new record fields); `server.rs` (registration, dispatch, `QueueExportArgs`, `get_delivery_conformance`'s depth, the 74 → 75 bookkeeping at 19031 and 19039-19048); `schema.rs` (`[&str; 75]`); `tests/mcp_server.rs`.
8. **Human UI.** New `color_qc_ui.rs`; `preview_ui.rs` (QC mask view); `color_scopes_ui.rs` (absolute clipping); `inspector_ui.rs` (node clipping line); `export_ui.rs` (depth radio, `ConformanceKey`, verification block); `keys.rs` (`KeyAction::ColorQc`, `ALL_ACTIONS` 19 → 20, `KEYMAP` 19 → 20, `perform_key_action`, the completeness test) and `app.rs:1366-1375`.
9. **Docs.** This file promoted to `docs/CC6-QC-AND-MANAGED-DELIVERY.md`; `docs/ROADMAP-AND-WORKFLOWS.md` current-status and CC6 row; `docs/M36-AGENT-RUNTIME-EFFICIENCY.md` table 123 → 124; `docs/M34-CREATOR-DELIVERY-VERIFICATION.md` limits list (10-bit is no longer a limit); `CHANGELOG.md`.

Steps 1 → 2 → 3 → 4 are strictly ordered. Step 5 depends on 3 and 4. Step 6 depends on 5. Steps 7 and 8 depend on 1, 2, and 4 and may proceed in parallel with 6, but neither may assert a budget before step 5 lands.

---

## 13. Explicit deferrals

- **Gamut *mapping*, legal-range clipping policy, and every automatic fix.** CC6 measures and reports. A gamut map is a creative decision with a rendering intent, a compression curve, and its own parity gate; §3.3's desaturation fraction is the measurement such a slice would consume, and deliberately nothing applies it.
- **HDR, BT.2020, PQ (SMPTE 2084), and HLG (ARIB STD-B67).** The `ColorTransfer` and `ColorPrimaries` vocabularies already name them (`color.rs:69-100`) and `probe_path` already reads them on ingest; delivery keeps rejecting them with typed reasons. HDR delivery needs mastering-display and content-light metadata, a tone-mapping contract, and a monitoring path this project does not have.
- **ProRes, DNxHD, FFV1, VP9, and AV1 delivery lanes.** The pinned build has every one of these encoders. **That is not a reason to ship them.** Each needs its own baselined codec budget (§6.3's rule), its own pixel-format and tagging path, its own container rules, and its own cross-platform fixture. CC6 adds one lane and proves it; the second lane is a slice, not a flag.
- **Full-range delivery.** Rejected with a typed reason, unchanged. **ACES, OCIO, and camera vendor transforms** are deferred with CC1 §7's list.
- **Chroma siting (`out_chroma_loc`) and encode-side `full_chroma_int` / `full_chroma_inp`.** All three are reachable on the pinned build — `out_chroma_loc` is a first-class `scale` AVOption — but none was measured on the encode side, and CC6 does not pin an option whose effect it has not measured. `sws_dither` is not deferred: it is measured **inert** (§5.4) and there is nothing to pin. The libswscale `bilinear+full_chroma_int` defect (§5.5) is recorded and worth reporting upstream; CC6 does not work around it, because CC6 does not use those flags.
- **A false-colour or zebra shader overlay.** `header.w` is fully consumed by CC5's matte-debug selector with an early return before the legacy stage (`compositor.wgsl:53-62, 747-755`); a GPU overlay needs a new header word, a new encoding contract, and a new parity gate. §8.2's CPU mask delivers the diagnostic without any of that.
- **Loudness normalization and audio delivery QC.** `AudioLoudness` exists (`media.rs:984-992`); wiring it into a delivery gate is M34's deferred item, not a colour slice.
- **VMAF, SSIM, and every perceptual metric.** `libvmaf` is in the pinned build. A perceptual score is not a *contract*: it has no defensible pass threshold for a synthetic chart, and CC6's exit gate must be a number a reviewer can re-derive by hand. Luma max/P99/mean, RGB mean, and PSNR are.
- **ΔE2000 and neutral-patch colour difference.** The roadmap ties it to a defined working-space conversion; CC6 does not define one.
- **A skin *detector*, face detection, and any semantic region proposal.** §3.5 measures a region the user names. CC5's deferral list said the same about mattes and CC6 does not quietly reverse it.
- **A colour eval suite in `eval.rs`.** Colour verification lives in `ccN_fixtures.rs`; a sixth eval suite is CC7's business.
- **Automatic shot-consistency QC across a timeline.** `plan_shot_match` (CC2) compares two shots on request; a timeline-wide sweep is a different cost model and a different report.
- **Frame-hash pinning of encoded output.** Encoder output is not bit-reproducible across platforms; the cross-platform gate is tags, legality, and difference budgets, which is exactly why the roadmap words it that way.
CC6 is complete only when an editor can ask "what will this deliver as?" and get integers — how many pixels clip, in which direction, by how much, which node caused it, whether the region they care about sits where skin sits, and what tags the file will carry — then export at 8 or 10 bits, have the file decoded and compared against the render automatically, and see the answer in the same dialog they pressed Export in, on both Windows and Linux, with every threshold in this document.

---

## 14. Risks

- **The range/gamut overlap is a communication risk, not a maths risk.** Two named reports over one pixel set will invite double-counting in any summary an agent writes. Mitigation: the relation is normative in §3.2/§3.3, restated in `ColorGamutReport.definition`, asserted by fixture §11.2.2, and the QC window prints it as a line rather than a tooltip. If a reviewer still finds the two confusing, the fallback is one `ColorExcursionReport` with an `over` / `under` / `unrepresentable` split — a rename, not a redesign.
- **The 10-bit lane's justification is an assertion that can fail.** §11.2.11 requires 10-bit to beat 8-bit on 8-bit-equivalent RGB mean *and* PSNR on the CC6 source. The probe measured that it does on the HD chart (MAD 0.2707 vs 0.4992; 38.717 vs 38.601 dB whole-raster; 62.669 vs 53.486 dB flat-field), but the CC6 source is a different raster. If the assertion fails, the honest response is to report the measurement and cut the lane, not to weaken the assertion.
- **The budgets are Linux measurements until Windows runs.** Every §6.3 constant is now re-baselined against `cc6_delivery_source()` on Linux with a margin of at least 2×, and the two tightest — the 8-bit luma P99 at 3.0× and the RGB mean at 2.35× — were deliberately widened *past* the measurement precisely because Windows is unmeasured (§6.3). The failure mode is a constant that is comfortable on Linux and tight on Windows. Mitigation: R5's rule — one constant, re-baselined with a per-OS note if Windows exceeds it, never a per-OS constant, never a widening after a red build.
- **The 10-bit luma P99 measures exactly zero on the passing source.** A budget nothing approaches proves nothing, and this one is approached by nothing at all in the healthy direction. Mitigation: the starved 10-bit fixture measures 24.0 against a budget of 4.0, so the constant is bounded from above by a real failing measurement rather than by a passing one; the manifest records the zero and names the starved direction as its bound instead of printing a margin ratio it does not have.
- **The Windows FFmpeg is a different package.** `setup-ffmpeg.ps1` pins `System233/ffmpeg-msvc-prebuilt` `ffmpeg-8.0.1-r3` (SHA-256 `3399afab…e433`), not the Linux `mifi/ffmpeg-builds` `8.0-1` (`c201d31f…5cb1`). Its GPL variant is documented to include x264, and vcpkg's x264 port builds `--bit-depth=all`, so `yuv420p10le` is *expected* — but it is **unverified until the Windows job runs**. Mitigation: §11.2.11 fails typed rather than skipping, so the first Windows run answers the question in a red or green build, not in silence.
- **17 full-resolution renders.** The per-node check is by far the most expensive thing in this slice, and removal-based attribution also deep-clones the document up to 16 times. Mitigation: off by default in the tool and behind an explicit, cost-labelled toggle in the app; hard-bounded with truncation reported; §11.2.24 records the measured cost on both lanes; §12 names it as the first cut.
- **Verification on by default, and it costs about 80 % of the export.** Measured (§11.2.24): a five-frame verification is **12.6 s** against a **15.6 s** export of the same 320 × 180 / 60-frame source, on both lanes, and it does not get better on hardware. The cost is the five full-resolution delivery re-renders, not the decode — R6's single-pass rule and §6.2's `O(GOP)` cross-check already bound the decode to `n · GOP` frames and to one seek. The default stays `true` because a delivery nobody checked is the failure this slice exists to prevent, and the caller can turn it off per job. Mitigation and the honest escape hatch: `verify: false` on `queue_export`, and the number is recorded in the manifest so a future slice that wants to lower it (fewer sampled frames, or a reference cache shared with the export's own renders) has the baseline to argue against.
- **The shared working proof is one raster deep.** The QC window and the QC mask read one `(session, revision, frame)`-keyed cache, which is what stops them rendering the same frame twice — but it holds one full-resolution linear f32 RGBA raster, so a caller that alternates between two frames gets no reuse at all. Mitigation: both consumers are one-shot or paused-only (§8.2), so alternation is a user action rather than a loop; the cache is dropped on revision change; and a second entry is a one-line change if a measurement ever asks for it.
- **`ScopeStage` gains a variant the scopes engine must refuse.** A future contributor will reasonably assume any `ScopeStage` is measurable. Mitigation: `measurable_by_scope_engine` is a `const fn` on the enum rather than a rule in a doc comment, `ScopeRequest::validate` fails closed through the *existing* `UnsupportedStage`, and §11.2.16 asserts the refusal on both the core and agent sides.
- **The working proof doubles GPU readback traffic for any caller that wants both monitor and working data.** Mitigation: the app's single-worker generation discipline coalesces requests; the QC window is one-shot; and if the cost bites, the fix is one render with two readbacks from the same composite target — a mechanical change `for_each_linear_pixel` already permits, deliberately not taken speculatively.
- **Both CI operating systems must run the encoded fixture in the default lane.** It needs FFmpeg (present in both jobs) and a GPU adapter (lavapipe on Linux, WARP on Windows). A Windows WARP path that cannot render the working proof would take the whole exit gate offline. Mitigation: `fallback_gpu()` fails loudly rather than skipping (`cc1_fixtures.rs:1492-1520`), so this surfaces as a red build on the first run rather than as a quietly absent gate — and the CC1 delivery fixture already runs on both, so the path is known good.

---

## Appendix A — Measurement provenance

Every number in §5 and §6.3 was measured, not inferred. The Linux measurements are the probe's, taken on **`mifi/ffmpeg-builds` 8.0-1, `ffmpeg-n8.0-latest-linux64-gpl-shared-8.0.tar.xz`, SHA-256 `c201d31f…5cb1`** (`scripts/setup-ffmpeg.sh:27-28`), reporting `n8.0-23-gd1f31a829d-20251022`, libavcodec 62.11.100, libswscale 9.1.100, x264 core 165. The Windows CI job uses a **different package**: **`System233/ffmpeg-msvc-prebuilt` release `ffmpeg-8.0.1-r3`, `ffmpeg-8.0.1-r3_x64-windows-shared-gpl.zip`, SHA-256 `3399afab…e433`** (`scripts/setup-ffmpeg.ps1:12-13`), an MSVC/vcpkg build whose GPL variant is documented to include x264 and x265. **No codec tolerance in this document may be invented, scaled, or inherited from another lane.**

| ID | Question | Answer | Consumed by |
| --- | --- | --- | --- |
| **P1** | 8-bit vs 10-bit decoded error at the export's own settings | Flat-field 8-bit max 3 / P99 2.0 / MAD 0.2693 / 53.486 dB; flat-field 10-bit max 1 / 62.669 dB; whole-raster max 133–134 in **both** lanes from 4:2:0 edge decimation | §6.3 gate shape and constants |
| **P2** | Dither on 16→8 and 16→10 RGB→YUV; is `sws_dither` reachable? | 8-bit: deterministic 8×8 ordered dither, 64 levels, 251/256 flat tiles non-flat. 10-bit: none. `sws_dither` (all values, filter and global) and `accurate_rnd` **inert**. Also found the `+1/256` white-level defect fixed by `ad6f6a8` | §5.4, §5.2, rule 11.0.8 |
| **P3** | Decoded native-plane Y′CbCr legality | 0.1 %–25 % of Y samples outside `[16, 235]` depending on content **before** `ad6f6a8`; codec ringing alone is ±1–2 codes at 8 Mbps, ±3 at 500 kbps | §6.4's EBU R 103 rule and the 100 bp threshold |
| **P4** | Skin-line angle and convention | `θ(+I) = 123.0000°` derived, not assumed; Rec.709 red 102.906186°; the four CC5 patches at `[12385, 12396, 12385, 12188]`, circular mean 12339, `R = 0.999885` | §3.5 band constants |
| **P5** | Range/profile/depth tags | `range=tv` is **not** an x264 param (ignored under `-x264-params`, fatal under `-x264opts`); `-color_range tv` alone yields `color_range=tv` in the SPS; `-profile:v high10` produces **byte-identical** output and is unnecessary; the 10-bit file re-probes as High 10 / 10-bit | §4.3, §11.2.11 |
| **P6** | Encode-side scaler choice | bicubic best of bicubic/lanczos/spline (flat-field 53.486 / 51.367 / 52.674 dB; max 3 / 4 / 3); differences ≤ 2 codes | §5.3 |
| **P7** | Decode-side flags | `flags=bicubic` and `bicubic+accurate_rnd` identical (max 70, 43.560 dB); adding `full_chroma_int` costs +63 max and −5 dB; `bilinear+full_chroma_int` is broken (max 255, 14.008 dB) | §5.5, normative |
| **P8** | 8-bit PSNR baseline | 43.560 dB whole-raster HD, 53.486 dB flat-field, with the recommended decode | `DELIVERY_PSNR_FLOOR_DB_HUNDREDTHS_8BIT = 4000` (a ≥ 2× margin in linear MSE terms) |
| **P9** | Cost of a 1080p working proof plus a full `ColorQcReport`, and of a five-frame `verify_delivery_output`, on both lanes | **Measured.** Colour QC itself 4.9 ms (lavapipe) / 4.7 ms (RTX 3080), against a 41.7 ms soft budget; the working proof 1798.7 / 1727.1 ms; a five-frame verification 12 679.9 / 12 609.5 ms (≈ 2.5 s per sampled frame) against a 15.6 s export. The lanes agree within 4 %, so the cost is decode, readback, and upload rather than shading. `verify` stays defaulted to `true` | §11.2.24, §6.5, §14 |
| **P10** | Does the 10-bit filter graph negotiate with the existing node order? | Yes — `buffer → scale → format(yuv420p10le) → buffersink` produced correct 10-bit output on the pinned Linux build with no node reordering | §4.3 |
| **P11** | Windows: does libx264 exist, does `yuv420p10le` open, and what are the decoded luma/RGB/PSNR numbers at both lanes? | **The Windows CI job is this measurement.** Its `CC6_EVIDENCE` output is recorded in the manifest's `budgets.*.measured`; if it exceeds a constant, the constant is re-baselined once, with a per-OS note — never split into a per-OS constant | §11.2.10/11, §6.3 |
