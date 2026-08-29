# CC8 — HDR interpretation and delivery

**DRAFT v1 — not yet accepted.** Nothing in this document is normative until the
owner accepts it and `docs/ROADMAP-AND-WORKFLOWS.md` is amended per §0.5. No
implementation should begin against a draft; §0.2's six open questions change the
shape of the slice, not merely its wording.

Status: draft contract, 2026-08-29
Depends on: CC0–CC7, principally [CC1](CC1-MANAGED-SDR-PRIMARY.md) (managed
input/working/output boundaries), [CC3](CC3-CURVES-AND-WHEELS.md) (the `grade709`
grading encoding), and [CC6](CC6-QC-AND-MANAGED-DELIVERY.md) (QC engine, the
10-bit lane, typed delivery rejection)
Scope: one honest HDR *interpretation* path and one HDR *delivery* contract —
not an HDR grading programme, not a monitoring claim, and not ACES.

The words **must**, **must not**, and **may** in this document are normative once
accepted.

---

## 0. Scope memo

### 0.1 Why this shape

CC7 closed the colour table by evaluating the SDR workflow end to end. The
roadmap has said since CC0 that "HDR, camera RAW controls, ACES/OCIO integration,
calibrated-monitor output, and advanced temporal noise reduction are deliberate
later programmes." CC8 takes the first of those, and the honest first slice is
narrower than "HDR support."

The pipeline is already further along than it looks. The working space is
scene-linear `Rgba16Float` with **no intermediate display-range clamp** (CC1
§2.2 invariant 5), so HDR luminance is *already representable* — a linear 10.0
survives every node today. `ColorTransfer` already names `Smpte2084` and
`AribStdB67`, `ColorPrimaries` already names `Bt2020`, and `decode.rs:275-276`
already maps both transfers on probe. What does not exist is any *interpretation*
of those values, any delivery target that accepts them, and any QC that means
anything about them.

So CC8 is not "make the pipeline HDR-capable." It is: **decide what an HDR source
means, decide what happens to it, deliver it once, and be honest about everything
that is still SDR-shaped.** The three things CC8 must not do are invent a
tolerance, claim a monitoring path it does not have, and back-door ACES.

### 0.2 Open questions — owner decisions

These six are not the implementer's to settle. Each carries a recommendation and
the reason; the recommendation is what the body of this draft is written against,
so a different answer means a redraft of the named sections.

---

**Q1. HLG or PQ first?**

*Recommendation: HLG (ARIB STD-B67), Rec.2020, limited range, 10-bit.*

HLG is scene-referred and relative. It is complete without static metadata: an
HLG file is correct on its own terms with nothing but its three tags. PQ is
display-referred and absolute, and an HDR10 deliverable is only *conformant* when
it carries mastering-display primaries and MaxCLL/MaxFALL. Kinewright cannot
honestly supply the first (it has no mastering display — that is the
calibrated-monitor programme) and would have to *measure* the second from the
content, which is new QC work.

Writing a plausible-looking MaxCLL would be inventing a number, which the house
rule forbids as squarely as inventing a tolerance. HLG lets CC8 ship a complete,
defensible HDR deliverable without that. It also matches the working space: HLG's
curve is scene-referred relative, so no absolute-nits anchor has to be chosen for
the *delivery* side (it is still needed on the source side — see Q3).

The cost is honest and should be weighed: HLG is a broadcast format. Streaming
platforms and consumer devices predominantly ask for HDR10 (PQ). If the intended
deliverable is "a file YouTube or a TV ingests as HDR," PQ is the answer and CC8
grows by MaxCLL/MaxFALL measurement (§6) plus a mastering-display provenance
decision. That is a real slice, not an afterthought — perhaps 30–40% more work —
but it is coherent, and §6 already scopes the measurement as evidence.

**If the answer is PQ:** §5 changes lane, §6's MaxCLL/MaxFALL rows move from
*reported* to *required*, and §2.2's anchor becomes normative on both sides.

---

**Q2. HEVC, or reuse the existing H.264 lane?**

*Recommendation: reuse the existing 10-bit libx264 lane. Do not add HEVC in CC8.*

This is the finding that most changes CC8's size, and it was measured rather than
assumed. **The pinned Linux build's libx264 writes BT.2020 primaries and either
HLG or PQ transfer into H.264 High 10 VUI, correctly, through exactly the
mechanism CC6 already uses** — the `x264-params` string at `export.rs:529`
(`DELIVERY_X264_PARAMS`). Verbatim, on `ffmpeg n8.0-23-gd1f31a829d`:

```text
$ ffmpeg -f lavfi -i testsrc2=size=320x180:rate=25:duration=1 \
    -pix_fmt yuv420p10le -c:v libx264 -color_range tv \
    -x264-params "colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc" out.mp4
$ ffprobe -show_entries stream=profile,pix_fmt,color_range,color_space,color_transfer,color_primaries
profile=High 10
pix_fmt=yuv420p10le
color_range=tv
color_space=bt2020nc
color_transfer=arib-std-b67
color_primaries=bt2020
```

`transfer=smpte2084` substitutes cleanly in the same string. So an HDR delivery
lane costs **one changed parameter string and one changed set of allowed
`ColorDescription` values** — not a new encoder, not a new container path, not a
new codec budget baselined from scratch, and not a second Windows encoder to
verify. CC6 §11.2.11 already proves `yuv420p10le` libx264 on both CI operating
systems; CC8 inherits that evidence instead of rebuilding it.

It also sidesteps a question that is genuinely the owner's: **HEVC patent
posture.** Kinewright is GPL and already ships libx264, so the *licence* answer
for libx265 is the same (both GPL-2.0+, both in the pinned `--enable-gpl` build).
The *patent* answer is not the same. H.264's pools are mature and broadly
tolerated for open-source distribution; HEVC's (Access Advance, Via LA, and
others) are more actively enforced and cover encoder distribution more
aggressively. That is a posture call, not a technical one, and CC8 should not
force it.

For completeness — HEVC *is* available and *does* work. Measured on the same
build: `libx265` supports `yuv420p10le`, encodes Main 10, and carries HDR10 static
metadata that probes back exactly (§0.3). Nothing here says HEVC is hard; it says
CC8 does not need it, and adopting it drags in a licensing decision and a second
unverified Windows encoder for no measured gain in this slice.

**The honest counter-argument:** HLG-in-AVC is unusual in the wild. The VUI values
are well-defined and a tag-honouring player will do the right thing, but broadcast
HLG is carried in HEVC, and some consumer players will not treat an AVC HLG file
as HDR. If the deliverable must be recognised as HDR by a specific target device
or platform, that target's requirement decides Q2 and probably Q1 with it. **This
is the single most important thing to check before accepting this draft.**

---

**Q3. Do Rec.2020 primaries enter the working space?**

*Recommendation: no. Keep BT.709 primaries and D65 in the working space; treat
Rec.2020 as a named matrix conversion on the source and delivery sides.*

The conversion is exactly invertible, and because the working space is unclamped
float, out-of-Rec.709-gamut colours survive as **negative BT.709 values** through
every node and convert back to Rec.2020 at delivery without loss. CC1 §3 already
reserves the stage: "For CC1, source and working primaries are the same for the
accepted profiles, so the primaries conversion is an identity matrix. It is still
a named stage so that CC2+ can add real conversion without changing the order."
CC8 is the slice that makes that stage non-identity for the first time. This is
the design working as intended.

Changing the working primaries instead would invalidate the Rec.709 luma
coefficients baked into `saturation_percent` (CC1 §3.2), the skin-line angles CC6
derived at `θ(+I) = 123.0000°`, `grade709`'s relationship to the monitor encoding,
and effectively every pinned constant in CC3–CC7. That is a re-baselining of the
whole colour programme, and it is not a first HDR slice.

**The cost, which §3.2 and §6 must state plainly:** wide-gamut content will read
as out-of-gamut to the CC6 gamut report, because that report measures the Rec.709
triangle and says so (`color_qc.rs:338`, `:1209`). That is a *correct Rec.709
statement* and a *misleading HDR statement*. §6 makes fixing that a CC8
obligation rather than letting the report quietly lie.

---

**Q4. What is the monitoring story on a non-HDR display?**

*Recommendation: a named, explicitly-labelled tone-mapped preview that is not a
monitoring reference and carries no exit gate.*

Every developer and CI machine here has an SDR display, so the alternatives are:
show nothing, show clipped garbage, or show a tone-mapped approximation that is
honest about what it is. The third is the only useful one, provided it never
claims to be a reference. It must be a named stage, inspectable in the colour
status, and labelled in the UI as not calibrated. No CC8 exit gate may be a
judgment about how the preview looks.

Calibrated HDR monitoring stays where the roadmap put it: a separate later
programme. CC8 must not imply otherwise anywhere in the UI.

---

**Q5. Where does CC8 sit relative to ACES/OCIO?**

*Recommendation: strictly before it, with an explicit prohibition.*

CC8 introduces the first non-identity primaries conversion and the first
scene-referred anchor — the two pieces that make it tempting to say "this is
basically a colour-management system, let us just adopt ACES." It must not. CC8
adds no ACES transform ID, no OCIO config, no RRT, no ODT, and no view transform
abstraction. §1 states this as a non-deliverable rather than leaving it to
judgment, because a back-doored half-ACES is worse than either endpoint: it
inherits ACES's vocabulary without its guarantees.

If ACES/OCIO is ever adopted, CC8's named stages are what it would replace, and
they are easier to replace for being explicit.

---

**Q6. Does SDR-from-HDR conversion ship?**

*Recommendation: as a preview only (Q4), never as a deliverable in CC8. HDR-from-SDR:
never at all.*

A delivery-grade tone map is a creative decision with a rendering intent and a
compression curve — CC6 §13 refused gamut *mapping* on exactly this reasoning, and
tone mapping is the same argument with a different axis. It has no defensible
objective pass threshold, so it cannot have a CC8-shaped exit gate.

HDR-from-SDR (inverse tone mapping) is worse: it invents information. There is no
measurement that makes it correct. It is out permanently, not deferred.

### 0.3 Measured encoder feasibility

Every claim below was measured on the pinned Linux build, not inferred. Linux:
`mifi/ffmpeg-builds 8.0-1`, reporting `n8.0-23-gd1f31a829d-20251022`, libavcodec
62.11.100, `--enable-gpl --enable-libx264 --enable-libx265 --enable-libsvtav1
--enable-libaom --enable-librav1e`.

**(a) HDR10 through libx265 round-trips completely,** including static metadata:

```text
$ ffmpeg ... -pix_fmt yuv420p10le -c:v libx265 \
    -x265-params "hdr-opt=1:repeat-headers=1:colorprim=bt2020:transfer=smpte2084:\
colormatrix=bt2020nc:master-display=G(13250,34500)B(7500,3000)R(34000,16000)\
WP(15635,16450)L(10000000,1):max-cll=1000,400" hdr10.mp4
$ ffprobe ...
codec_name=hevc / profile=Main 10 / pix_fmt=yuv420p10le
color_range=tv / color_space=bt2020nc / color_transfer=smpte2084 / color_primaries=bt2020
side_data_type=Mastering display metadata
red_x=34000/50000  green_x=13250/50000  blue_x=7500/50000  white_point_x=15635/50000
min_luminance=1/10000  max_luminance=10000000/10000
side_data_type=Content light level metadata
max_content=1000  max_average=400
```

`libx265` supports `yuv420p10le yuv422p10le yuv444p10le` (and 12-bit). HLG
substitutes cleanly (`transfer=arib-std-b67`, verified). So HEVC is fully
available if Q2 answers that way.

**(b) The generic FFmpeg `-color_*` flags are not sufficient on any encoder
tested.** With generic flags only and no encoder-param string, both libx265 and
libx264 dropped primaries and transfer:

```text
$ ffmpeg ... -c:v libx265 -color_primaries bt2020 -color_trc smpte2084 \
    -colorspace bt2020nc -color_range tv     (no -x265-params)
color_range=tv / color_space=bt2020nc / color_transfer=unknown / color_primaries=unknown
```

This independently reproduces what `export.rs:221-223` already documents for the
SDR lane: "FFmpeg's generic codec-context colour fields do not reliably carry
primaries and transfer through libx264's SPS." **CC8 inherits that rule rather
than rediscovering it:** any HDR lane must write its tags through the encoder's
own parameter string and must re-probe to prove it.

**(c) AV1 is available but tags worse.** `libsvtav1` supports `yuv420p yuv420p10le`,
but the generic flags reach neither MP4 nor Matroska:

```text
$ ffmpeg ... -c:v libsvtav1 -color_primaries bt2020 -color_trc smpte2084 ...
color_space=bt2020nc / color_transfer=unknown / color_primaries=unknown     (.mp4 and .mkv alike)
```

Tags survive only via bitstream-level `-svtav1-params
"color-primaries=9:transfer-characteristics=16:matrix-coefficients=9:color-range=0"`.
AV1 is royalty-free and therefore the answer if Q2 rules out HEVC *and* AVC, but it
is a third tagging mechanism to build and verify, so it is not recommended for CC8.

**(d) The recommended lane needs no new encoder** — see Q2's transcript.

**(e) Windows is a precondition, not an assumption.** Windows CI pins a different
package: `System233/ffmpeg-msvc-prebuilt ffmpeg-8.0.1-r3` (SHA-256
`3399afab…e433`), an MSVC/vcpkg build. CC6 §14 already flags that its encoder set
is documented rather than verified. **CC8 implementation must not begin until a
Windows job has confirmed that its libx264 accepts `colorprim=bt2020`,
`transfer=arib-std-b67` (and/or `smpte2084`), and `colormatrix=bt2020nc` and that
the tags re-probe.** The x264 build there is a different vcpkg port at a
potentially different core version, and `arib-std-b67` is a later addition to
x264's `colorprim`/`transfer` tables than the CC6 lane's `bt709` values. Per CC6
§11.2.11's rule, this fails typed rather than skipping, so the first Windows run
answers it in a red or green build rather than in silence.

**(f) Probe already reads both HDR transfers.** `decode.rs:275-276` maps
`TransferCharacteristic::SMPTE2084 → ColorTransfer::Smpte2084` and
`ARIB_STD_B67 → ColorTransfer::AribStdB67`; `ColorPrimaries::Bt2020` exists
(`color.rs:73`). **No mastering-display or content-light-level metadata is read
anywhere in the repository** — a repo-wide search for `mastering`, `max_cll`,
`content_light`, and `MaxFALL` returns nothing. That is genuinely new work if Q1
answers PQ.

### 0.4 What in the current code actively blocks HDR

These are the hard stops, with locations, found by reading rather than assumed.
None is an accident; each is a correct SDR statement that becomes a wrong HDR one.

| # | Location | What it does | CC8 |
| --- | --- | --- | --- |
| 1 | `kinewright-core/src/color.rs:697-722` (`classify_source_with_assumption`) | Closed set: only `Rec709Video` / `SrgbFull`; every other tuple is `UnsupportedCombination`. PQ and HLG are rejected here today. | §2.1 adds arms. Clean extension point. |
| 2 | `kinewright-core/src/delivery.rs:537-548` (`delivery_color_mismatches`) | Hard-requires `Bt709` primaries, `Bt709` transfer, `Bt709` matrix, `Limited` range. | §5.3 widens by lane, not globally. |
| 3 | `kinewright-media/src/export.rs:216-217` | `set_colorspace(BT709)` / `set_color_range(MPEG)` unconditionally on the encoder. | §5.2 selects from the delivery description. |
| 4 | `kinewright-media/src/export.rs:529` (`DELIVERY_X264_PARAMS`) | Literal `"colorprim=bt709:transfer=bt709:colormatrix=bt709"`. | §5.2 makes it lane-derived. |
| 5 | `kinewright-media/src/export.rs:522` (`DELIVERY_VIDEO_CODEC`) | `libx264`, doc'd "the only video encoder that may carry the managed delivery tags." | Unchanged under Q2's recommendation. |
| 6 | `kinewright-media/src/export.rs:425` | Scaler string pins `out_color_matrix=bt709`. | §5.2 derives from the lane. |
| 7 | `kinewright-media/src/export.rs:1187` | `setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709`. | §5.2, same. |
| 8 | `kinewright-media/src/export.rs:626, :638` | `set_color_primaries(BT709)` on both frame paths. | §5.2, same. |
| 9 | `kinewright-media/src/color_pipeline.rs:1920` (CC5 HSL qualifier) | `grade709_encode(value).clamp(0.0, 1.0)` — **every value above linear 1.0 collapses to one selector.** All HDR highlights read identically to the qualifier. | §3.2 names it as a known limitation; not fixed in CC8. |
| 10 | `kinewright-core/src/color_qc.rs:338, :1209` | Gamut is defined as Rec.709-triangle representability. | §6 adds a lane-aware variant. |
| 11 | `kinewright-core/src/color_qc.rs:120-132` (`bt709_limited_ycbcr`) | The only Y′CbCr reference; asserts 8- or 10-bit; BT.709 coefficients only. | §6 adds a BT.2020 NCL sibling. |
| 12 | `kinewright-core/src/color_qc.rs:395-433` | Skin-band angles derived in `grade709`/Rec.709. | §6 explicitly defers; the numbers do not transfer. |

Two things that are **not** blockers, recorded because they look like they should
be. `grade709` (`color_pipeline.rs:962-985`) is odd, strictly increasing, and
analytically invertible over all reals — it has no upper bound and does not clamp,
so HDR values pass through it losslessly. And the CC4 LUT lattice
(`color_pipeline.rs:1436-1452`) clamps only for interpolation and then adds the
excursion back (`z = y + (e - u)`), deliberately, so over-range highlights are not
collapsed. The working space is already the right shape.

### 0.5 Roadmap edits acceptance would require

`docs/ROADMAP-AND-WORKFLOWS.md` is **not modified by this draft**. On acceptance:

1. **Line ~505**, the deliberate-later-programmes sentence: strike `HDR` from
   "HDR, camera RAW controls, ACES/OCIO integration, calibrated-monitor output,
   and advanced temporal noise reduction are deliberate later programmes," and
   replace with a sentence naming CC8's *bounded* HDR scope, so that
   calibrated-monitor output and the rest keep their standing and CC8 is not read
   as having delivered HDR generally.
2. **The staged table (~line 495-503)**: add a `CC8 — HDR interpretation and
   delivery` row. Deliverable: "HDR source profiles (PQ/HLG Rec.2020) with an
   explicit reference-white anchor; named Rec.2020↔Rec.709 primaries conversion;
   one 10-bit HDR delivery lane with typed rejection; HDR legality and gamut QC;
   labelled tone-mapped preview." Exit gate: "Analytic source-interpretation and
   round-trip fixtures; cross-platform encoded HDR fixture passes tag, legality,
   and difference budgets on both CI operating systems; SDR lanes bit-unchanged."
3. **Line ~461**, current status: extend the completed list to include CC8 and
   restate what "the colour programme table is complete" means now that the table
   has an eighth row.
4. **Colour architecture principles (~line 366-390)**: the principle "Use a
   high-precision intermediate before serious matching, curves, compositing, or
   HDR work" is discharged by CC8 and should say so. Add a principle stating that
   the working space stays BT.709-primaries and that wide gamut is carried as
   out-of-triangle float values, so a later contributor does not "fix" the
   negatives (per Q3).
5. **Windows/Omarchy release evidence (~line 554-566)**: CC8 changes native media
   and export behaviour, so it is release-affecting; note that the HDR lane needs
   the manual Omarchy smoke record and the Windows hands-on equivalent, and that
   neither can be a claim about HDR *appearance* on an SDR panel (Q4).
6. **CC6 §13's deferral bullet** ("HDR, BT.2020, PQ (SMPTE 2084), and HLG") should
   be annotated as superseded-in-part by CC8, naming what remains deferred
   (dynamic metadata, calibrated monitoring, tone-mapped delivery).

### 0.6 Amendments register

*Empty. Amendments from implementation and review are recorded here in CC6 §0 /
CC7 §0 form once this draft is accepted and built.*

---

## 1. In scope and out of scope

CC8 delivers:

- **HDR source interpretation**: two new source profiles (`pq_rec2020` and
  `hlg_rec2020`), their transfer decoding, an explicit named reference-white
  anchor, and honest failure for every HDR tuple that is not one of them;
- **a named, non-identity primaries conversion stage** — the first — carrying
  Rec.2020 into and out of the BT.709-primaries working space losslessly through
  unclamped float;
- **one HDR delivery lane**: HLG, Rec.2020 primaries, BT.2020 non-constant-luminance
  matrix, limited range, 10-bit, in the existing H.264 High 10 encoder
  (§0.2 Q1/Q2), with tags written explicitly and re-probed;
- **typed delivery rejection extended by lane** — the existing
  `DeliveryColorError` vocabulary, widened so an HDR description is accepted on
  the HDR lane and rejected with a named reason on the SDR lanes, and vice versa;
- **QC extensions**: a BT.2020 NCL Y′CbCr legality reference, a lane-aware gamut
  report so wide-gamut content is not reported as an error against the wrong
  triangle, and MaxCLL/MaxFALL measurement **reported, not gated**;
- **a labelled tone-mapped preview** for SDR displays, inspectable and explicitly
  not a monitoring reference; and
- **fixtures** whose central gate is a cross-platform encoded HDR fixture: a
  synthetic HDR source through the production path, re-probed, decoded, and gated
  on tags, legality, and difference budgets, in the default lane on **both CI
  operating systems**.

CC8 does **not** deliver: HDR *grading* controls, or any new node — the CC1/CC3/CC5
node set is unchanged; a wide-gamut working space (§0.2 Q3); **calibrated HDR
monitoring**, in any form or claim; tone-mapped **delivery**, SDR-from-HDR as a
deliverable, or HDR-from-SDR in any form (§0.2 Q6); **ACES, OCIO, ACEScct, RRT,
ODT, view transforms, or any ACES-derived vocabulary** (§0.2 Q5) — this is a
prohibition, not a deferral; Dolby Vision, HDR10+, SMPTE ST 2094 or any dynamic
metadata; HEVC, AV1, or any new encoder (§0.2 Q2); constant-luminance BT.2020,
ICtCp, or Rec.2100 matrices other than BT.2020 NCL; camera RAW or log sources
(CC7's log-*like* carrier is synthetic and stays that way); HDR skin diagnostics
(§6); a second HDR lane; and P3 anything.

An HDR source that is not one of §2.1's two profiles **must** produce a visible
typed status with an explicit override path. It **must not** be silently treated
as Rec.709, and it **must not** be silently tone-mapped.

---

## 2. HDR source interpretation

### 2.1 New source profiles

`classify_source_with_assumption` (`color.rs:665`) gains two arms. A profile match
is on all listed fields; a partial match is not enough, exactly as CC1 §2.1.

| Profile id | Primaries | Transfer | Matrix | Range | White point | Integer depth |
| --- | --- | --- | --- | --- | --- | --- |
| `pq_rec2020` | `bt2020` | `smpte2084` | `bt2020_ncl` or `rgb` | `limited` or `full` | `d65` | 10..=16 bits |
| `hlg_rec2020` | `bt2020` | `arib_std_b67` | `bt2020_ncl` or `rgb` | `limited` or `full` | `d65` | 10..=16 bits |

The 10-bit floor is deliberate and normative: 8-bit PQ or HLG is banding by
construction, and accepting it would be a claim CC8 cannot defend. An 8- or
9-bit HDR tuple is a typed rejection naming the depth, not a warning.

`bt2020_cl` (constant luminance), `ictcp`, `chroma_derived_*`, P3 primaries in any
combination, and `smpte2084`/`arib_std_b67` paired with non-Rec.2020 primaries are
**explicit CC8 failures**, not guesses. As in CC1 §2.1, the error must name the
asset, the unsupported field, the observed value, and the allowed values.

The CC1 D65 rule carries over unchanged: an unknown white point on an otherwise
supported HDR tuple may use the normative D65 value only through an explicit
`profile_assumption` recorded in the colour status and proof. The raw source
metadata stays `Unknown`. No code may rewrite it.

### 2.2 The reference-white anchor — the crux

The working space is scene-referred with diffuse white at `1.0`. PQ is
display-referred and absolute: its code values mean cd/m² directly, up to 10 000.
Mapping one into the other requires a number, and that number **must be stated,
pinned as an integer constant in the authority module, and inspectable in the
colour status**. It must never be a literal buried in a shader.

CC8 pins ITU-R BT.2408's nominal HDR reference white, **203 cd/m²**, as
`CC8_REFERENCE_WHITE_NITS = 203`. A PQ source decodes as:

```text
absolute_nits = pq_eotf(E')                       (ST 2084, peak 10 000)
working_linear = absolute_nits / CC8_REFERENCE_WHITE_NITS
```

so diffuse white lands at `1.0` and a 1 000-nit specular highlight at `≈ 4.93`.

HLG is relative and needs no absolute anchor to *decode*, but its OETF is defined
against a nominal peak with a system gamma that depends on it. CC8 pins
`CC8_HLG_NOMINAL_PEAK_NITS = 1000` and `CC8_HLG_SYSTEM_GAMMA_THOUSANDTHS = 1200`
(γ = 1.2 at 1 000 nits, per BT.2100), and applies the inverse OETF and inverse OOTF
as two separately named stages, so a later slice can vary the peak without
disturbing the curve.

The ST 2084 and ARIB STD-B67 constants are transcribed from their standards into
the authority module and are part of this contract; they must not be delegated to
a platform colour API or an FFmpeg filter. Both transfer functions are decoded in
f32 with sign-preserving negative extension, in the manner CC1 §3.1 establishes
for BT.709, so that undershoot survives to the final clamp.

`CC8_REFERENCE_WHITE_NITS = 203` is a **standards value, not a measurement**, and
is not subject to the measured-tolerance rule. Every *tolerance* in §9 is.

### 2.3 Primaries conversion

The Rec.2020 ↔ BT.709 conversion is the named stage CC1 §3 reserved. It is a
3×3 linear-light matrix, applied after transfer decode and before any grading
node, with its exact coefficients pinned in the authority module (derived from
the two primary sets and D65, transcribed to f32, not taken from a backend).

It is **not** a gamut map. Colours outside the Rec.709 triangle become negative
BT.709 components and **must not be clamped** — CC1 §2.2 invariant 5 already
forbids it, and CC8 restates it because negatives here look like a bug and will
tempt a future contributor to "fix" them. The inverse matrix at delivery restores
them exactly. A fixture asserts the round trip (§9).

### 2.4 Static HDR metadata

Mastering-display primaries and MaxCLL/MaxFALL are read on probe where the
container carries them, stored on the source description with provenance, and
**reported**. They are never invented: absent metadata is `Unknown` and stays
`Unknown`. Under §0.2 Q1's recommendation the HLG lane does not consume them; they
exist so the QC surface can report what a source claimed and so a PQ lane has its
inputs already modelled.

---

## 3. The managed HDR working path

### 3.1 The working space is unchanged

`working` stays BT.709 primaries, `linear` transfer, `rgb` matrix, full range,
D65, `Rgba16Float` — byte-identical to CC1 §2. This is the decision in §0.2 Q3.

Half-float headroom is sufficient and this is stated so it is not re-litigated:
at `CC8_REFERENCE_WHITE_NITS = 203`, PQ's 10 000-nit peak is `≈ 49.3` in working
units, far inside f16's 65 504 maximum, with a ULP of `≈ 0.03` there — about 11
bits of relative precision on the brightest representable specular highlight. The
existing `Rgba16Float` surface is adequate for CC8; no `Rgba32Float` claim is made
or needed, and CC1 §2's warning that `Rgba32Float` blend support cannot be assumed
across backends still stands.

### 3.2 What happens to `grade709` and the SDR-shaped nodes

`grade709` is **unchanged and needs no change.** It is odd, strictly increasing,
unbounded, and an exact analytic inverse of its decode
(`color_pipeline.rs:962-985`), so HDR magnitudes pass through it losslessly. A
linear 4.93 encodes to `grade709 ≈ 2.03`. Nothing clips.

But "nothing clips" is not "the controls behave well," and CC8 must say so
plainly rather than let the distinction pass:

1. **The curve and wheel controls are authored on an SDR-shaped domain.** CC3
   parameterizes curves in basis points of the `grade709` range where 10 000 bp is
   1.0 — which is diffuse white. An HDR highlight at `grade709 2.03` sits at
   20 300 bp, outside the authored domain. The CC4 lattice's add-back rule
   (`z = y + (e - u)`) means such a value is *shifted* by the curve rather than
   *shaped* by it. That is defensible behaviour and it is the existing behaviour;
   CC8 does not change it, does not widen the curve domain, and **does** require
   the colour status to report when a node's input exceeds its authored domain, so
   an editor is told rather than surprised.
2. **The CC5 HSL qualifier is genuinely limited on HDR.**
   `color_pipeline.rs:1920` clamps `grade709` to `[0, 1]` before deriving hue,
   saturation, and luma, so **every value above diffuse white produces the same
   selector**. Qualifying a specular highlight from a mid-tone is not possible.
   CC8 does not fix this — a correct fix is an HDR-aware qualifier domain with its
   own parity gate and its own measured band constants, which is a slice — but it
   **must** be surfaced as a named limitation in matte inspection whenever a
   qualifier node runs on an HDR-profile source, so the matte's behaviour is
   explained rather than merely observed.
3. **Saturation and luma stay Rec.709-weighted.** `saturation_percent` and the QC
   luma use `0.2126 / 0.7152 / 0.0722`, correct for the BT.709-primaries working
   space that §3.1 keeps. No change.

### 3.3 Pipeline order

CC1 §3's canonical order is unchanged; CC8 fills in stages that were identity or
absent. Additions are marked `*`.

```text
source coded samples
  -> source range expansion
  -> source matrix decode to coded RGB              (* BT.2020 NCL added)
  -> source transfer decode to linear light         (* PQ / HLG added)
  -> HLG inverse OOTF                               (* HLG profile only)
  -> reference-white normalization                  (* PQ profile only, §2.2)
  -> primaries conversion to working BT.709 D65     (* now non-identity)
  -> grading nodes, in serialized clip.effects order (unchanged)
  -> non-colour layer operations and linear-light compositing (unchanged)
  -> monitoring transform OR delivery transform
       monitoring: tone-mapped preview (§4) on an SDR display
       delivery:   primaries conversion to Rec.2020 -> HLG OOTF+OETF -> §5
  -> final clamp, quantization, and display/codec packing
```

Every added stage is separately named in the colour status and proof. The
existing SDR path must traverse **byte-identical** stages to today — the two
`*` transfer stages and the primaries conversion are selected by source profile,
and on a Rec.709 source the primaries stage stays the identity matrix it is now.
§9 gates this: every CC1–CC7 fixture must pass unchanged, with unchanged pinned
constants.

---

## 4. Monitoring: what CC8 does not claim

CC8 provides **no calibrated HDR monitoring path** and must not imply one.

On an SDR display, the managed preview applies a named tone-mapping stage from
the working space to the existing Rec.709 monitoring description. Requirements:

1. The stage is named, ordered, and reported in the colour status like any other.
2. Its parameters are pinned integer constants in the authority module.
3. Every UI surface showing it is labelled as a non-calibrated preview of HDR
   content. The specific wording is an implementation decision; the requirement
   that it exist is not.
4. **No CC8 exit gate is a judgment about how it looks.** Its fixtures assert
   determinism, monotonicity, endpoint behaviour, and CPU/GPU parity — properties,
   not aesthetics.
5. It is a *preview* transform. It must not be reachable from the delivery path
   (§0.2 Q6), and a fixture asserts that (§9).

An HDR-capable display is out of scope: CC8 has no display-capability query, no
HDR swapchain, and no metadata handoff to the compositor. On such a display the
preview is still the tone-mapped SDR preview, and the status says so.

---

## 5. The HDR delivery contract

### 5.1 The lane

Exactly one, per §0.2 Q1/Q2:

| Field | Value |
| --- | --- |
| Codec | H.264 High 10 (`libx264`, the existing `DELIVERY_VIDEO_CODEC`) |
| Pixel format | `yuv420p10le` |
| Primaries | `bt2020` |
| Transfer | `arib_std_b67` (HLG) |
| Matrix | `bt2020_ncl` |
| Range | `limited` |
| White point | `d65` |
| Bit depth | `Ten` |

This reuses CC6 §4.1's `DeliveryEncodeDepth::Ten` lane. It adds a *colour
description*, not a codec path, so `DELIVERY_SCALER_FLAGS = "bicubic"`, the
`DELIVERY_INTERMEDIATE_WHITE = 65_280` convention, and the single-pass filter
graph are unchanged and are not re-measured. **Full-range HDR delivery is rejected**
with a typed reason, as full-range SDR already is.

CC6 §13's rule governs any second lane: "the second lane is a slice, not a flag."

### 5.2 Tags are written explicitly, then proven

Per §0.3(b), measured on two encoders: generic codec-context colour fields do not
carry primaries and transfer. Therefore:

1. The encoder colourspace and range **must** be selected from the delivery
   `ColorDescription`, never from a literal. `export.rs:216-217`'s unconditional
   `set_colorspace(BT709)` / `set_color_range(MPEG)` becomes lane-derived.
2. `DELIVERY_X264_PARAMS` becomes a function of the lane. For this lane:
   `colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc`. The SDR lanes'
   string is **byte-identical to today's**, and a fixture asserts that.
3. The scaler's `out_color_matrix` (`export.rs:425`), the `setparams` filter
   (`export.rs:1187`), and both `set_color_primaries` calls (`export.rs:626, :638`)
   are lane-derived from the same single source of truth. A fixture asserts the
   codec context and the filter graph cannot diverge, in the manner CC6 §4.1
   establishes for the depth.
4. **The exported file is re-probed and its tags asserted.** A tag that does not
   survive is a failure, never a warning. This is CC0's rule and CC6 §3.6's, and
   it is the only defence against §0.3(b) recurring on a build change.

### 5.3 Typed rejection

`delivery_color_mismatches` (`delivery.rs:528`) currently hard-codes BT.709 in
four fields. CC8 makes the allowed set **a function of the selected lane** and
keeps everything else: the fixed check order
(`primaries → transfer → matrix → range → white_point → bit_depth → provenance →
confidence`), the `DeliveryColorMismatch` shape (`field` / `observed` / `allowed`),
the `DeliveryColorError` code/recovery vocabulary, and the rule that it is never
applied to a *probed* description.

Required rejections, each named by its own failing-direction test (§9):

- an HDR description on an SDR lane, and an SDR description on the HDR lane;
- HLG or PQ at 8-bit depth;
- Rec.2020 primaries with a BT.709 transfer, and BT.709 primaries with an HLG or
  PQ transfer — mismatched pairs, rejected on the *combination*;
- `bt2020_cl`, `ictcp`, and every matrix outside the lane table;
- full range on the HDR lane; and
- PQ on the HLG lane, with a recovery action naming the deferral rather than
  implying a conversion exists.

---

## 6. QC extensions

The CC6 engine is evidence-only and stays so. CC8 adds:

1. **BT.2020 NCL Y′CbCr legality.** `bt709_limited_ycbcr`
   (`color_qc.rs:120-132`) gets a BT.2020 sibling with `KR = 0.2627`,
   `KB = 0.0593` and the corresponding denominators, pinned in the authority
   module. Legality (EBU R 103-shaped, as CC6 §6.4) is measured against the
   lane's own matrix. Reusing the BT.709 reference on a BT.2020 file would be a
   wrong number, not an approximate one.
2. **A lane-aware gamut report.** CC6 defines gamut as Rec.709-triangle
   representability (`color_qc.rs:338`, `:1209`), which reports legitimate
   wide-gamut HDR content as an excursion. CC8 makes the triangle a property of
   the report, names it in `ColorGamutReport.definition`, and reports Rec.2020
   representability for HDR-profile content. **Both are reported when they
   differ**, because "outside Rec.709 but inside Rec.2020" is exactly the fact an
   editor delivering HDR needs, and collapsing it loses the SDR-compatibility
   signal. CC6 §3.2/§3.3's normative range/gamut relation is restated per triangle
   so the double-counting risk CC6 §14 names does not multiply.
3. **MaxCLL and MaxFALL measurement — reported, never gated.** Computed from the
   working proof over the sampled frames, in the CC6 evidence style, with the
   sampled population and its bounds recorded beside the number. They are reported
   because an editor should be able to see them and because a PQ lane (§0.2 Q1)
   would need them as *inputs*; they are not gated because CC8 does not write them
   into a file and a threshold on them would be invented. Under CC7 §0.3 PM-E12's
   rule the *measured* column is reported, not gated, and this follows it.
4. **HDR skin diagnostics: explicitly deferred.** CC6's band constants were
   derived at `θ(+I) = 123.0000°` in `grade709` against Rec.709 primaries (`P4`).
   Those numbers do not transfer to Rec.2020 or to HDR luminance, and re-deriving
   them is a measurement programme. On an HDR-profile source the skin report is
   **withheld with a named reason**, not silently computed against the wrong
   constants. This is the deferral CC8 is most likely to be asked to reverse, and
   it should be reversed by measurement or not at all.

---

## 7. Core representation and migration

The project needs an explicit managed-colour state for the HDR path, in CC1 §4's
manner: the first CC8 save writes `managed_hdr_v1`; an absent value stays
`managed_sdr_v1` or `legacy`.

1. **Every existing project opens byte-unchanged and exports byte-identically.**
   This is the strongest migration obligation in CC8 and §9 gates it. An SDR
   project must not acquire an HDR field, a changed delivery description, or a
   different exported byte stream.
2. A project whose source is HDR but whose delivery is SDR **loads**, reports the
   mismatch, and blocks managed export with the §5.3 typed reason naming the
   deferral. It is not auto-tone-mapped and its delivery target is not silently
   rewritten (§0.2 Q6).
3. Setting an HDR delivery description is an ordinary undoable, revision-gated,
   journalled operation, validated against §5.1's table.
4. Save/reopen, journal replay, undo, and redo preserve the source HDR metadata,
   the profile assumption, and the managed state, byte-for-byte apart from
   documented JSON defaults.

---

## 8. Human and agent surfaces

No new agent tool. The existing surfaces carry the new facts:

- `get_color_context` reports the HDR source profile, the reference-white anchor
  and its value, the profile assumption, the primaries-conversion stage, and any
  mastering-display/MaxCLL metadata the source declared — with `Unknown` where the
  source said nothing.
- `get_color_qc` reports the lane-aware gamut and legality of §6, the MaxCLL and
  MaxFALL measurements as ungated rows, and the withheld-skin reason.
- The export dialog offers the HDR lane only when the document's delivery
  description selects it, and its post-export verification block shows the probed
  HDR tags.
- The inspector reports §3.2's out-of-authored-domain condition on curve and wheel
  nodes, and the qualifier limitation on matte nodes.

CC8 adds **no** planner, no `auto_hdr`, and no analysis that mutates a grade.
Every surface is evidence-only, as CC1 §5 requires.

---

## 9. Exit fixtures and numeric gates

The gate is a fixture suite. Rules carry over unchanged from CC6 §11.0 and CC7
§11.0 — no invented constant, no vacuous assertion, every failing direction named
by test name, LF checkout discipline per `.gitattributes`, byte-equality where a
generated file is checked in.

Constants authority: `kinewright_core::cc8_hdr`, in the manner of
`cc7_scenarios`. Fixtures: `kinewright_media::cc8_fixtures`. Manifest:
`cc8_manifest.json`. Test names are `cc8_`-prefixed and the manifest asserts the
inventory equals the declared set.

### 9.1 Required fixtures

1. **Transfer identity.** PQ and HLG decode/encode round-trip over a 10-bit ramp,
   including negatives and over-range; monotonic; exact analytic inverse at the
   segment seams, with the seam behaviour recorded explicitly as CC1 §3.1 does for
   BT.709.
2. **Reference-white anchor.** A PQ source at exactly 203 nits lands at working
   `1.0`; 1 000 nits at `≈ 4.93`; 10 000 nits at `≈ 49.3`. Values pinned as
   integers in the authority module.
3. **Primaries round trip.** Rec.2020 → BT.709 → Rec.2020 over a raster including
   out-of-709 primaries; the intermediate carries negatives (asserted present, so
   the fixture cannot pass vacuously on in-gamut content); the round trip is
   within the §9.2 linear budget.
4. **No intermediate clamp on HDR.** An HDR highlight raster is corrected with a
   negative exposure and recovers values that a clamp would have destroyed —
   CC1 §6.1's fixture 4, at HDR magnitudes.
5. **Unsupported HDR metadata.** 8-bit PQ, 8-bit HLG, `bt2020_cl`, `ictcp`, P3
   primaries, and every mismatched primaries/transfer pair block managed proof and
   export and name the recovery action.
6. **SDR regression — the byte-equality gate.** Every CC1–CC7 fixture passes with
   every pinned constant unmoved, and the SDR `x264-params` string, scaler flags,
   and exported bytes for a fixed SDR project are **unchanged**. This is the
   fixture that makes CC8 safe to accept.
7. **Delivery rejection.** One failing direction per §5.3 bullet, each named.
8. **The cross-platform encoded HDR fixture** — the central gate. A synthetic HDR
   source is exported through the production path, re-probed, decoded, and gated
   on: probed tags exactly `bt2020` / `arib_std_b67` / `bt2020_ncl` / `tv` /
   `yuv420p10le` / High 10; decoded native-plane BT.2020 legality; and difference
   budgets against the re-rendered delivery reference. **In the default lane on
   both CI operating systems.**
9. **Preview.** Determinism, monotonicity, endpoint behaviour, and CPU/GPU parity
   of the tone-mapping stage; plus a failing direction asserting the preview
   transform is **unreachable from the delivery path**.
10. **Parity.** CPU reference versus software GPU on HDR magnitudes, under CC1
    §6.2's banded half-float rule, with the over-range band extended to HDR
    values and its own band recorded.
11. **Migration.** An SDR project opens, round-trips, and exports byte-identically;
    an HDR-source/SDR-delivery project loads and blocks with the typed reason.
12. **QC.** BT.2020 legality against hand-derivable analytic patches; the
    dual-triangle gamut report on content inside Rec.2020 and outside Rec.709;
    MaxCLL/MaxFALL emitted as ungated rows; the withheld-skin reason asserted
    present on an HDR source and absent on an SDR one.

### 9.2 Numeric gates

**Every tolerance below is a placeholder to be measured at implementation.** None
may be invented, scaled, or inherited from another lane — CC6 Appendix A's rule,
which CC8 adopts wholesale. The *shape* of each gate is fixed here; the *number*
is not, and a number that appears in this draft is a description of what will be
measured, not a value.

| Gate | Shape | Value |
| --- | --- | --- |
| PQ/HLG transfer round trip | max / P99 / mean absolute, linear domain, banded by magnitude as CC1 §6.2 | **to be measured at implementation** |
| Primaries round trip | max / P99 / mean absolute, linear | **to be measured at implementation** |
| CPU vs GPU, HDR magnitudes | max / P99 / mean, per half-float band | **to be measured at implementation** |
| Decoded HDR delivery | max / P99 / mean luma; RGB mean; PSNR floor | **to be measured at implementation** |
| BT.2020 legality excursion | basis points outside legal range | **to be measured at implementation** |
| Preview parity | max / P99 / mean, monitor codes | **to be measured at implementation** |

Two rules govern how those numbers are taken, both carried forward from CC7:

- **No gate may be an equality against one FFmpeg build's decode output.** CC7
  §0.3 PM-E12 measured the two CI builds decoding the same encode to different
  numbers, and CC6 §0.4 traced one cause to the MSVC build's swscale rounding
  chroma as if `SWS_ACCURATE_RND` were set. What gates is a *constant* asserted
  against the manifest, with both the live and the recorded measurement inside
  that bound in the term's own direction. The per-build measured figures are
  **reported, never gated**. No `cfg(windows)`, no per-OS constant, no window
  invented around one build's output.
- **A budget must carry a real margin, and a margin nothing approaches proves
  nothing.** Where a measured term sits too close to its constant, it is recorded
  with its margin rather than cleared (CC7's `RecordedMargin`), and where a term
  measures zero on the passing source, a deliberately starved fixture bounds the
  constant from above. CC6 §14's rule.

A tolerance may never excuse an unsupported source, a missing or wrong tag, an
intermediate clamp, a wrong stage order, an SDR regression, or a preview
presented as monitoring.

---

## 10. Implementation order

1. **Windows encoder precondition** (§0.3(e)). Confirm the MSVC libx264 accepts
   the HDR `x264-params` and that tags re-probe. **Nothing else starts until this
   is green** — it can invalidate §0.2 Q2 and therefore the whole lane.
2. `cc8_hdr` authority module: transfer constants, matrices, the anchor, budgets.
3. Source profiles and transfer decode, with fixtures 1, 2, 5.
4. Primaries conversion, with fixture 3; then fixtures 4 and 10.
5. **Fixture 6, the SDR regression gate**, before any export change lands.
6. Delivery lane, tags, typed rejection: fixtures 7 and 8.
7. QC extensions: fixture 12.
8. Preview and UI: fixture 9.
9. Migration and serialization: fixture 11.
10. Measure every §9.2 budget; write `cc8_manifest.json`; reconcile the inventory.

---

## 11. Explicit deferrals

- **Calibrated HDR monitoring, HDR display output, and HDR swapchain handoff.**
  The roadmap's own later programme. CC8's preview is not a down payment on it.
- **Tone-mapped SDR delivery from an HDR timeline.** A rendering intent and a
  compression curve, with no defensible objective threshold — CC6 §13's argument
  against gamut mapping, on a different axis. The measurement a future slice would
  consume (§6's MaxCLL/MaxFALL, the dual-triangle gamut) is deliberately produced
  and deliberately unapplied.
- **HDR from SDR.** Not deferred — refused. It invents information.
- **PQ / HDR10 delivery**, pending §0.2 Q1, and with it mastering-display
  provenance and gated MaxCLL/MaxFALL.
- **HEVC, AV1, and every other encoder** (§0.2 Q2). The pinned build has libx265,
  libsvtav1, librav1e, libaom, libkvazaar and libvvenc. **That is not a reason to
  ship them** — CC6 §13's sentence, unchanged.
- **Dolby Vision, HDR10+, ST 2094, and all dynamic metadata.**
- **A wide-gamut working space, ICtCp, and constant-luminance BT.2020** (§0.2 Q3).
- **HDR-aware HSL qualifier domain** (§3.2 item 2) and **HDR skin diagnostics**
  (§6 item 4) — both need measured constants, both are named limitations rather
  than silent ones.
- **HDR-aware curve and wheel authoring domains** (§3.2 item 1). CC8 reports the
  condition; widening the domain is a CC3 amendment with its own parity gate.
- **ACES, OCIO, and camera vendor transforms.** CC1 §7's list, unchanged, and
  §0.2 Q5's prohibition on approaching them sideways.

---

## 12. Risks

- **§0.2 Q2 may not survive contact with a real delivery target.** The whole
  lane's economy rests on HLG-in-AVC being acceptable. If the target platform
  rejects it, CC8 needs HEVC, and the licensing question in Q2 becomes blocking
  rather than avoidable. *Mitigation:* Q2 is the first question in the memo and
  §10 step 1 is a precondition, so this is answered before code exists — but the
  honest mitigation is that the owner checks a real target before accepting.
- **The Windows x264 build may not accept `arib-std-b67`.** It is a later addition
  to x264's tables than the `bt709` values CC6 verified, and the Windows package
  is a different vcpkg port. *Mitigation:* §10 step 1; and per CC6 §11.2.11 the
  check fails typed rather than skipping, so the answer arrives in a red or green
  build.
- **The dual-triangle gamut report multiplies CC6's communication risk.** CC6 §14
  already names two reports over one pixel set as an invitation to double-count;
  CC8 makes it two triangles as well. *Mitigation:* §6 requires both to be
  reported only where they differ, with the relation normative and printed as a
  line. If it still confuses, the fallback is one report with a `triangle` field —
  a rename, not a redesign.
- **The SDR regression gate is the one that must not be weakened.** Every hard
  block in §0.4 is a literal that CC8 makes lane-derived, and a lane-derivation
  bug is invisible on the HDR lane and catastrophic on the SDR one. *Mitigation:*
  §9.1 fixture 6 lands before any export change (§10 step 5) and asserts exported
  bytes, not just tags.
- **`CC8_REFERENCE_WHITE_NITS` is a choice that looks like a fact.** 203 is
  BT.2408's nominal value, not a universal one; content mastered against a
  different diffuse white will sit at the wrong working exposure. *Mitigation:*
  it is a named, inspectable, single-source constant rather than a shader literal,
  so making it per-project is a small change if a measurement ever asks for it —
  and the colour status reports its value, so an editor can see why an HDR clip
  landed where it did.
- **HDR content will make the existing SDR-shaped nodes look broken.** The
  qualifier collapse (§3.2 item 2) especially will read as a bug. *Mitigation:*
  both limitations are surfaced in the UI at the node that has them, not
  documented in a file nobody opens. A named limitation is a different support
  burden from a mysterious one.
- **CC8 is the natural place to accidentally start ACES.** The primaries stage and
  the scene-referred anchor are two-thirds of a colour-management system's
  vocabulary. *Mitigation:* §1's prohibition is stated as a non-deliverable with
  named forbidden terms, so adopting them is a visible contract change.

CC8 is complete only when an editor can import an HDR file, see exactly how it was
interpreted and what was assumed, grade it with the existing nodes knowing which
of them are SDR-shaped and why, export one HDR deliverable whose tags survive a
re-probe on both operating systems, and have the SDR path prove it did not move.
A better-looking HDR picture does not close this gate.
