# CC8 — Flexible colour pipeline + YouTube HDR (HLG) delivery: design

Status: **accepted design** (recipe v2; worker rev 3 2026-09-25 + lead
promotion edits P1–P8 from the rev-3 completeness check, §0.3; verification
findings + Edits 1–8 accepted as written, N6 binding).
Conventions: prose cites symbols, never `file:line`. Rules **R1–R36** are
numbered, testable, and each names its owning stage. Gates **G1–G10** are named
behaviours that fail today, tagged **[new]** (new behaviour), **[regression]**
(must-not-move evidence), or **[invariant]** (standing property re-asserted).
Production budgets count new-or-changed nonblank production lines incl.
production comments (replacements once, pure moves zero); tests, fixtures, docs
counted separately; every rule owns at least one test. Base: main `6c2bdec` +
AW1 serialisation (§15). **The numerical contract is approved at promotion
(completeness check: promote-with-edits; P1–P8 incorporated) — S0 and S1 may
start.**

## 0. Inputs and binding decisions

Accepted: port critic except N2 overrides; design critic in full (N4 binds);
verification table + Edits 1–8 as written (N6 binds). SDR insertion default
appearance-preserving, scene-matching explicit (provisional YES pending Riel).

- N2.1 **YouTube** target; N2.2 SDR/stills/titles placeable at ref white (§4,
  R9, R36); N2.3 flexible pipeline now, genuinely scene-referred (§2, B1);
  N2.4 correct MaxCLL/MaxFALL in scope (§9, B5); N2.5 Windows via CI + Riel's
  machine/VM (§17).
- Port-critic B1/B4/B5/B6/S1/S3/S4/S5/S6/S7/S8 stand, discharged inline.
  Reused from archive: PQ EOTF / HLG OETF maths, luminance-coupled OOTF
  (output-side only), 2020↔709 matrices, 2020-NCL + dual-triangle QC,
  x264-params tag mechanism + re-probe rule, HEVC patent reasoning. Not reused:
  integration, fixed light model, Reinhard preview, stage orders.
- All numbers independently recomputed by the review-round numeric check (`scratch/check.py`, kept with the review notes, not shipped) — see App. N
  (supersedes rev-2 figures). Registry 153/60/93 after AW1's five inspector
  capabilities (subject to merged surface).

## 0.1 Changes in revision 2 (finding → section)

| Finding | Change → section |
|---|---|
| B1 | Genuine scene intermediate: HLG input = inverse OETF only; PQ/SDR via named inverse rendering interpretation; OOTF at output; legacy SDR = separate branch → §2, §5, R28 |
| B2 | Authoritative persisted data model (structs, precedence, atomicity, HLG γ/black) → §3, R29 |
| B3 | EETF on normalized PQ coordinates; separate ceilings/anchor/knee; beyond-ceiling rule; input-intent placement + no-double-application → §5, §6, R14 |
| B4 | Pinned gamut compressor + HLG signal ceiling; saturated-colour tests → §6, R30 |
| B5 | CTA-861.3 MaxCLL/MaxFALL; display-nit post-render stage; active area; 203/203/53.33 fixture → §9, R20 |
| B6 | Complete node table (encoding, coeffs, ranges, extrapolation, selectors); HDR qualifier equations + units → §7, R16 |
| B7 | Enumerated v2 predicate; max-readable vs min-required; `ColorContextWire` round trip; journal/Save-As notes → §3, R2 |
| B8 | New `color_node_domain_unsupported` code (off-allowlist) + typed context/render routes → §7, §11, R17, R23 |
| S1 | Float HDR scope path (stage, units, routing); SDR scope bytes pinned → §10, R31 |
| S2 | Complete LUT-wrapper order + authored-space tags → §7, R32 |
| S3 | Placement equality assertions; 75% HLG / 58% PQ reference; two insertion intents → §4, R9, R36 |
| S4 | Untouched SDR execution branch; transform-aware cache keys; warm-renderer test; AW1-explicit baselines → §5, §14, R11 |
| S5 | Measurement lifecycle/binding/storage; CC9 re-measures → §9, §18, R33 |
| S6 | Riel = upload operator; fixture+hash+manifest; inconclusive state; PQ metadata = recommended w/ fallback → §8, §16–§17, §19 |
| S7 | Conformance over exported interval + animated enable; production preflight path → §8, R34 |
| S8 | Pinned f16 precision budgets; non-finite = typed refusal → §14, R35 |
| S9 | Re-estimate + contingency (ceiling 8,500); rule→stage owners; gate tags; G2 = regression gate; native-plane assertions; HDR eval outcomes → §12, §15–§17 |
| Nits | Stale refs repaired; one `eetf_to_target` + versioned composed intents; preview/enable = selection; N2.3/N2.4 undeferrable → §§5–6, §16, §19 |

## 0.2 Changes in revision 3 (finding/edit → section)

Every "pinned"/"defined" claim now carries its value/equation/schema/mapping;
all numbers recomputed by the review-round numeric check (`scratch/check.py`, kept with the review notes, not shipped) (App. N). Verification discharged
B5, S6, S7 outright; the rest close via Edits 1–8:

| Edit | Change → section |
|---|---|
| E1 | S1 stays gated on the numeric contract; S0 may proceed → §0, §15 |
| E2 | Dimensionless scene normalization (`s_white`, `working=scene/s_white`); forward/inverse rendering equations; SDR insertion equations + white scaling → §2, §4–§5, R28, R36 |
| E3 | Corrected EETF (source-span denorm, explicit Hermite, defaults, identity, clip+flag past Cs); 203→88.243641 anchor → §6, R14 |
| E4 | Destination-RGB compressor (destination coeffs, U bounds, t rule, Y=0); HLG ceiling + post-OETF clamp → §6, R30 |
| E5 | Correct extended γ; N3 overwrite guard (file version vs app ceiling; min-required only when writing); enumerated defaults/omission/predicate → §3, R2, R29 |
| E6 | `ENC/DEC(input_encoding_token)` + bypass/zero-mix returns; qualifier equations + ranges + nits↔selector; per-kind domain map → §7, R16–R17, R32 |
| E7 | Actual precision limits (PB1–PB4); scope equation/bounds/bins/predicate/routing; refusal→code→policy + recovery tables; report/cache-only measurement storage → §§9–12, §14, R23, R31, R33, R35 |
| E8 | Programme fixture 400/203/67.8; exact HDR eval outcomes; equation-tied tests; budget retained → §§9, §12, §§15–17 |
| N1–N4 | N1 EETF fix (E3); N2 γ fix (E5); N3 guard fix (E5); N4 LUT encoding fix (E6) — all independently recomputed |

## 0.3 Promotion edits (rev-3 completeness check → lead, normative)

Verdict promote-with-edits; the checker supplied exact text, inserted by the
lead with repository names verified against main `6c2bdec` (`RecoveryAction`,
`RecoveryKind::Operation`, `ParamValue::Integer`, `SetEffectParam`,
`SetColorContext`, `ColorContext::sdr_rec709`, `AddAsset`, `AddLutAsset`,
`ColorEvalRequest`, `ColorEvalEvidence`, `ColorEvidenceError`). Plus the lead
BT.1886 erratum (§4, App. N).

| Edit | Change → section |
|---|---|
| P1 | Complete `ColorManagement` model, defaults, validation, `gamma_rule_version`, asset-removal coupling; R29 stated → §3 |
| P2 | `DeliveryLane`; per-lane target-peak resolution; render at project reference, encode HLG at resolved target peak; adapter returns `D709/Ct`, never scene into the EETF; R19 scope; `q0 = PQ_OETF(0)` → §§3, 5, 6, 8, App. N |
| P3 | Signed rendering/inverse with scale; `VersionedIntent`/`IntentParams` closed set; chain order → §§2, 3, 6 |
| P4 | `LutAsset.authored_space`, `MediaAsset.hdr_source_metadata`/`HdrMetadataRecord`; omission + v2 predicate additions → §3 |
| P5 | Wheels = unbounded CDL (no Curves extrapolation); qualifier stored units; `matte_domain_token` → §7, R17 |
| P6 | Recovery via real `RecoveryAction` payloads; invented action shapes removed → §11 |
| P7 | `PostGradeSceneFloat` semantics, channels, bins, comparability, `scope_incomparable` as query error → §10 |
| P8 | `ColorEvalRequest.hdr` / `ColorEvalEvidence.hdr` fields, failure behaviour, pinned HLG probe strings → §12 |

## 1. Scope and the CC8 / CC9 cut

CC8 delivers: the flexible colour-managed pipeline (per-source interpretation,
timeline working space, named rendering intents, per-deliverable output
transforms); the **YouTube HLG** HDR lane; the **SDR-from-HDR deliverable** lane
with the single pinned default intent; correct measured MaxCLL/MaxFALL
(reported + stored, feeding QC and CC9); HDR-aware QC/scopes; the labelled
preview output transform; CC1–CC5 node behaviour in both working spaces; typed
refusals; agent + app surfaces; SDR byte-identity.

CC9 (boundary in §18) delivers: the **PQ/HDR10 YouTube lane**, consuming CC8's
measurement plus a mastering-display provenance decision. The cut is there
because (a) HLG is complete with three tags and no absolute mastering claim
while PQ carriage is recommended-with-fallback (S6); (b) measurement is CC8's
but PQ's lane membership was this design's call; (c) one HLG artefact keeps
evidence closable. SDR-from-HDR stays in CC8 (listed transform; machinery
shared with preview; M34 local-deliverable need).

Out (deferred): calibrated HDR monitoring / swapchain / display query;
HEVC/AV1/VP9 (revisit only on named recognition failure); dynamic metadata
(HDR10+, ST 2094, Vision); P3; camera RAW/log; ICtCp/CL-2020; ACES/OCIO
vocabulary (prohibition stands); HDR skin re-derivation (withheld, R17);
HDR-specific new grading controls. N2.3/N2.4 are **not deferrable** at closure
(nit): §19 deferrals never cover them.

## 2. Light-domain model and terminology (B1)

- **Working scene**: linear-light, D65, ref diffuse white = 1.0. A *constructed*
  intermediate — the design never claims original-scene recovery. Nodes,
  compositing, alpha run here. No inter-stage display clamp (CC1 §2.2 inv. 5).
- **HLG input is scene light**: inverse OETF only, no OOTF at input (BT.2390-12
  §6.2: scene light ≠ OOTF display-light result).
- **Display-mastered input via inverse rendering interpretation**: PQ (ST 2084
  EOTF → absolute nits) and SDR (relative decode → SDR display light) pass a
  declared, named, versioned `display_to_scene` inverting output rendering at
  the input's declared peak — inspectable, reported, explicit construction.
- **Scene→display rendering at output**: every output transform applies OOTF at
  output peak + intent (§6) before signal encode. "Tone map" = named intent
  only; anything else is a clamp, rescale, or bug.
- **Reference white** (default 203, BT.2408) anchors working 1.0; **peak**
  (default 1000) parameterises OOTFs/knees, never a clamp.
- **Legacy SDR branch**: SDR projects run a separate untouched branch (S4,
  R11), not the flexible path with identity stages.

Scene normalization and rendering equations (Edit 2 — dimensionless; W =
`reference_white_nits`, P = peak, γ = §3 gamma rule, α = 1, β = 0 pinned):

- Reference scene: `s_white = (W/P)^(1/γ)`; working `w = s/s_white` where s is
  normalized scene (s = 1 ⟺ peak). Defaults: s_white = 0.26479719, w_peak =
  3.776475 (App. N). HLG 0.749877365 decodes to scene s_white → working 1.0
  exactly; nominal 0.75 → scene 0.26496256 → working 1.000625 (App. N).
- Forward rendering (scene→display, per component c, 2020 luminance Y_s of
  normalized scene): `D_c = α·P·Y_s^(γ−1)·s_c`; achromatic: `D = P·s^γ`.
  Y_s = 0 → D = 0 exactly. Non-finite input → typed render refusal (R35).
- Inverse rendering (display→scene, `display_to_scene` v1): `Y_s =
  (Y_d/(α·P))^(1/γ)`; `s_c = D_c/(α·P·Y_s^(γ−1))`; achromatic: `s =
  (D/P)^(1/γ)`. Y_d = 0 → s = 0; non-finite → refusal.
  Forward∘inverse = identity to float precision (gated round trip).
- Signed handling (P3): for the nonnegative rendering function F at reference
  peak P, signed rendering is `D = F(max(s,0)) + P·min(s,0)` componentwise;
  inverse rendering is `s = F⁻¹(max(D,0)) + min(D,0)/P`. The zero-luminance
  shortcut applies to the nonnegative part. Negatives are never fed to powers.
- Source locks pin (ref_white_nits, ref_peak_nits, γ at lock); locked sources
  evaluate the equations with locked parameters regardless of project edits.

**R1 [S1].** Every stage boundary in §5 names its light domain (signal /
working-scene / display); a stage that cannot name its input and output domains
fails review, and wrong-domain placement fails its test (wrong-transform
controls, §17). **R28 [S1+S3].** HLG input applies inverse OETF only (no OOTF
gain at input, asserted); PQ/SDR pass `display_to_scene` v1 above; output
renders per forward equations; no surface claims original-scene recovery.

## 3. Per-project colour-management settings and format policy (B2, B7)

Authoritative persisted model (exact structs; `SetColorContext` replaces and
revalidates the whole payload atomically — all-or-nothing, undoable,
revision-gated, journalled):

- `WorkingReference { space: Sdr709 | Hdr2020 (default Sdr709),`
  `reference_white_nits: u32 (default 203, range 100..400, pinned ints),`
  `peak_nits: u32 (default 1000, range 400..10_000, pinned ints) }`.
  Authoritative over the legacy `working` description: validation **refuses**
  a context whose legacy `working` contradicts `WorkingReference`.
- `SourceInterpretation` per asset: `Auto` (probe + timeline space decide) or
  `Explicit { transfer, primaries, white, ref_white_nits, ref_peak_nits,`
  `insertion: AppearancePreserving | SceneMatching, input_intent }` (R8). An
  explicit interpretation **locks its reference**: later project white/peak
  changes re-render but never reinterpret the locked source; the lock and any
  resulting mismatch are reported, never silent (B2).
- `OutputTarget { lane: DeliveryLane, target_peak_nits: Option<u32>,
  tone_intent: VersionedIntent, gamut_intent: VersionedIntent }`, one per
  deliverable plus one `preview` target. `DeliveryLane = PreviewSdr |
  SdrFromHdr | YoutubeHlg` (P2). Target-peak resolution (P2, replaces rev 3's
  single-inheritance rule): absent targets resolve to lane defaults — SDR /
  preview peak 100; HLG peak inherited from the project. `target_peak_nits =
  None` follows that lane rule. Explicit SDR peaks admit 50..10_000; explicit
  HLG peaks admit 400..10_000. Project `peak_nits` is always the
  grading/rendering reference.
- Intents (P3): `VersionedIntent { id: IntentId, version: u16, params:
  IntentParams }` admits only the §6 IDs plus `display_to_scene`; version
  defaults to 1 and other versions refuse. `IntentParams = Empty | Eetf {
  source_ceiling_nits: Option<u32> }`; an omitted ceiling inherits the
  resolved rendering/source-reference peak; an explicit ceiling lies between
  that peak and 10_000. Ct comes solely from the target; knee and black
  parameters are derived/pinned, not independently editable. Other intents
  require `Empty`. Lane defaults select EETF plus gamut compression; encoding
  is lane-derived. Resolved chains are ordered input interpretation,
  rendering, tone, gamut, encoding.
- HLG gamma (N2, BT.2100-3 Table 5 Note 5f): P≤2000:
  `γ=1.2+0.42·log10(P/1000)`; P>2000: `γ=1.2×1.111^log2(P/1000)`. Pinned:
  γ(400)=1.032865, γ(1000)=1.2, γ(2000)=1.326433 (log rule owns the boundary;
  power rule gives 1.333200 there — not used), γ(4000)=1.481185,
  γ(10000)=1.702316 (App. N). HLG black β=0, gain α=1.
- Enclosing payload (P1): `ColorContext.management: ColorManagement`,
  `ColorManagement { working_reference: WorkingReference, sources:
  BTreeMap<AssetId, SourceInterpretation>, outputs: BTreeMap<DeliveryLane,
  OutputTarget>, preview: OutputTarget }`. Missing members reconstruct the
  stated working defaults, empty maps, and the default SDR preview. A missing
  source entry means `Auto`, resolving W/P from the current working reference.
  `Explicit` requires typed `ColorTransfer`, `ColorPrimaries`,
  `ColorWhitePoint`, and `u32` reference-white/reference-peak values;
  insertion defaults to `AppearancePreserving`. Reference bounds equal
  `WorkingReference` bounds; W ≤ P. Store `gamma_rule_version: u16 = 1`,
  deriving locked gamma from that version and locked peak. Reject unsupported
  profiles, unknown referenced assets, duplicate source/lane entries,
  incompatible intents, and contradictory legacy descriptions atomically.
  Asset removal also removes its source entry, with undo restoring both.
- Persisted attachment points (P4): `LutAsset.authored_space: Sdr709 |
  Hdr2020` defaults to `Sdr709`, is written through existing `AddLutAsset`, and
  governs every node referencing that asset. Title/graphic `authored_signal` is
  interpretation-report evidence, not an additional persisted flag.
  `MediaAsset.hdr_source_metadata: Vec<HdrMetadataRecord>` defaults to empty
  and enters through `AddAsset`; `HdrMetadataRecord { scope: Stream |
  Frame(u64), kind: MasteringDisplay | ContentLight, fields:
  BTreeMap<String,String> }` preserves raw probed field/value strings,
  including malformed values. Reports distinguish missing, partial, malformed,
  and valid records; prefer a valid stream record, otherwise the earliest valid
  frame record, and report every conflict. These records never supply
  programme measurements.
- Omission (P4): apply default omission only to new members, preserving
  existing serialized fields. Default management is omitted wholesale;
  `ColorContextWire` reconstructs every member before validation (explicit
  round-trip arms + test for each). `input_encoding_token` omitted → 0 =
  `display709` (CC4 default); HDR metadata omitted → empty (never
  synthesised).

**R29 [S2].** These defaults, reference resolutions, and validation rules apply
identically to operations, deserialization, replay, and rendering.

Format policy (after AW1 S1, through `kinewright-project`). v2 predicate =
**any** of: `space=Hdr2020`; non-default white/peak; any source override;
retained HDR source metadata; any non-default `OutputTarget`/intent selection;
any `authored_space=Hdr2020` tag (B7/E5); nonempty HDR metadata and every
nondefault new node-domain tag, incl. `matte_domain_token` (P4). No persisted programme derived
records exist in CC8 (report/cache-only, R33 — rev-2 predicate item dropped).
Minimal-version writing: v1 iff no v2-only content. Overwrite guard (N3):
`can_overwrite_save` compares the retained file version against the
**application's supported format ceiling** (max readable), never against the
document's minimum-required version — removing the last HDR field re-enables
saving; minimum-required is used **only when writing**. IN2B newer-file
advisory-open/overwrite-refusal behaviour preserved; no new format code.
Through AW1's shared serializer (sidecar-before-project, digests); cover
save/reopen, undo, journals, eval loading, actual IN2B-era older-reader
refusal. Stated: journal protection may reject even an SDR journal in an older
build; Save As stays intentionally lossy (B7).

**R2 [S2].** The enumerated v2 predicate, version separation, and wire
round-trip behave as above; SDR saves stay v1 byte-identical. **R3 [S0+S2].**
Ordinary SDR saves are byte-identical pre/post CC8 (same envelope, same v1
bytes); the G2 corpus asserts it. **R4 [S2].** Changing working reference,
white, peak, intents, or targets is undoable, revision-gated, journalled, and
re-renders; it never rewrites source probe data (R7).

## 4. Source interpretation

Accepted HDR profiles (closed-set discipline, via
`classify_source_with_assumption` + `ColorSourceProfile` arms):

| Profile | Primaries | Transfer | Matrix | Range | White | Depth |
|---|---|---|---|---|---|---|
| `pq_rec2020` | bt2020 | smpte2084 | bt2020_ncl / rgb | limited / full | d65 (or assumed, R8) | 10..=16 |
| `hlg_rec2020` | bt2020 | arib_std_b67 | bt2020_ncl / rgb | limited / full | d65 (or assumed, R8) | 10..=16 |
| SDR (existing) | bt709 | bt709 / srgb… | … | … | … | unchanged |

**R5 [S2].** 8/9-bit PQ/HLG, `bt2020_cl`, `ictcp`, P3 primaries with HDR
transfers, and every mismatched primaries/transfer pair are typed refusals
reusing the existing `ColorSourceError` vocabulary (field / observed / allowed
/ recovery); no new source code is minted. **R6 [S3].** One assumption
predicate shared by renderer (`open_scaled_managed`, `decode_source_rgb`,
`FrameRenderer::render`), `normative_d65_assumption`, and QC; one test asserts
all three agree on every profile × white tuple. **R7 [S3].** Raw probed
descriptions are never rewritten: `white_point=Unknown` survives
probe→import→preview→export; D65 enters only via a recorded
`profile_assumption` (`assumed_from` / `set_asset_color_description` revert
semantics preserved); the end-to-end HDR test uses the untouched probed
description. **R8 [S2+S3].** Per-source override per §3 (with lock + reported
mismatch); overrides are undoable and reported in `get_color_context`/proofs.

Placement matrix (N2.2 + N4: default appearance-preserving, scene-matching
explicit — both designed, R36):

| Content | SDR timeline (legacy branch) | HDR timeline |
|---|---|---|
| SDR video / stills (`MediaKind::Image`, EXIF applied, alpha preserved) | native | inserted at ref white via the insertion intent |
| Titles / captions / graphics | native | inserted at ref white, authored-signal flag recorded |
| Freeze (`ClipContent::Freeze`) | native | inherits the frozen frame's interpretation |
| HDR video | input intent `eetf_to_target`→SDR at decode, reported (never silent) | native (HLG scene / PQ via inverse interpretation) |
| Disabled clip (`is_enabled_at` false) / off-time | skipped before colour (MO1 `visual_layers_at` etc. unchanged) | skipped before colour |

**R9 [S3].** Placement asserts **equality** at the anchor (not ≤, S3): inserted
SDR white encodes exactly to the HLG signal of working-scene 1.0 (same code
path both sides); plus black, midtones, super-whites, coloured graphics, and
partial-alpha vectors. External anchors are **approximate** (S3, recomputed):
HLG signal of exactly 203 nits = 0.749877365 (≈75%); PQ E′ of 203 nits =
0.580688881 (≈58%); HLG 0.75 → 203.152145 nits (App. N). Custom white/peak
combos report their actual mapping. **R10 [S3].** Freeze/titles/captions/
images/disabled follow the table; preview/output selection never scans the
timeline (by space + deliverable only, R22).

Insertion equations (Edit 2; V = SDR signal 0..1; s_white per §2):

- Inverse SDR OETFs. bt709 (piecewise): `E=V/4.5` for V<0.08124286, else
  `E=((V+0.0992968)/1.0992968)^(1/0.45)`. sRGB: `E=V/12.92` for V≤0.04045,
  else `E=((V+0.055)/1.055)^2.4`. Both exact at 0 and 1.
- `appearance_preserving` (default): SDR display light from the reference
  DISPLAY EOTF applied to the signal V (not to the inverse OETF — lead erratum
  at promotion, rev 3 double-linearised bt709): bt709 → BT.1886 with Lb = 0,
  `D_sdr = 100·V^2.4`; sRGB → `D_sdr = 80·E_srgb(V)` (the piecewise sRGB curve
  above, no further power). White-scale `D_h = D_sdr·(W/P_sdr)` (P_sdr = 100 /
  80); scene `s=(D_h/P)^(1/γ)` (luminance-coupled inverse per §2 for chromatic);
  `w=s/s_white`. Anchors (bt709): V=0→0.000000, V=0.5→0.250000, V=1→1.000000;
  sRGB: V=0.5→0.276746, V=1→1.000000 (App. N, lead-recomputed).
- `scene_matching` (explicit, BT.2408 scene-light insertion): `s =
  s_white·E` with E the selected inverse SDR OETF above; `w = s/s_white = E`.
  Anchors (bt709): V=0→0.000000, V=0.5→0.259719, V=1→1.000000; sRGB:
  V=0.5→0.214041, V=1→1.000000 (App. N).
- The intents differ in midtones by construction, agree at black and white;
  super-white V>1 follows the power branches; V<0 clamps to 0 with flag.

**R36 [S3].** Both intents implemented exactly as above (no "relative scene
linear directly" shortcut); all anchors asserted ±1e-6 in f64 reference.

## 5. Working spaces and pipeline order (B1, B3, S4)

- `Sdr709`: existing SDR inputs + SDR settings execute the **legacy branch** —
  same functions, arithmetic order, storage boundaries, byte-identical (S4).
  HDR sources on SDR timelines pass a named **HDR-input adapter** before
  joining the legacy branch (P2): it decodes HDR to **display nits** using the
  resolved source reference, applies its EETF, converts/fits to 709, and
  returns relative linear RGB `D709/Ct` (legacy-kind decoded frames). It never
  feeds scene values directly to the EETF.
- `Hdr2020`: linear Rec.2020, D65 working scene. Linear 2020 keeps P3/2020
  content positive, makes HLG delivery conversion identity, and is Rec.2100
  vocabulary (no ACES terms).

Canonical HDR order (CC1 §3 + named CC8 stages; `*` = new/selectable):

```text
source signal → range expansion → matrix decode (* 2020 NCL added)
 → transfer decode to scene-linear (* HLG inverse OETF → scene;
      PQ EOTF → display nits → inverse rendering interpretation → scene;
      SDR relative decode → display light → inverse interpretation → scene)
 → HDR→SDR input adapter (* SDR timelines only, display-nit domain:
      decode → nits → eetf_to_target → 709 fit → D709/Ct, P2)
 → reference-scene normalization, divide by s_white (* HDR timelines;
      dimensionless: working = scene/s_white, §2 — never scene÷nits)
 → primaries conversion to working space (* non-identity across spaces)
 → grading nodes in serialized effects order (unchanged order; §7 behaviour)
 → MO1 layer ops + linear-light composite (unchanged; params_for, enabled skip)
 → monitoring OR delivery output transform (* scene→display rendering at
      the PROJECT working reference, then the target's tone/gamut intents;
      HLG inverse OOTF + OETF at the RESOLVED TARGET peak, §6, P2)
 → final clamp, quantization, codec packing
```

Double application is structurally excluded (B3): the input intent exists only
on SDR timelines, the output intent only renders HDR timelines to SDR/monitor
targets — both conditions cannot hold on one frame — plus a debug assertion on
the resolved intent chain. Cached transformed frames are keyed by resolved
input-transform identity: `VideoSourceKey` and `TitleCacheKey` gain a transform
digest (S4); one warm-renderer test runs space/white/peak/override changes and
undo across video, titles, and stills.

**R11 [S0+S3].** SDR inputs + SDR settings execute the untouched legacy branch
(G2); the HDR-input adapter is the one new pre-stage, named and tested.
**R12 [S3].** No intermediate clamp on either space; an HDR highlight raster
corrected with negative exposure recovers values a clamp would destroy.
**R13 [S1].** Matrix round trips (2020→709→2020, 709→2020→709) over saturated
primaries + near-black preserve negatives and meet pinned budgets;
"approximately reversible" replaces "lossless" (S1 of port critic), with
repeated storage-boundary + full 10 000-nit-peak coverage.

## 6. Rendering intents (B3, B4; nit: one EETF)

One EETF intent, `eetf_to_target`, configured per use (SDR input, preview, SDR
deliverable, pre-HLG roll-off). Corrected construction (N1/Edit 3, BT.2390-4
§5.4.1 Hermite — denormalization uses the **source** span):

```text
q0 = PQ_OETF(0); s = PQ_OETF(Cs) − q0
x = (PQ_OETF(L) − q0)/s; m = (PQ_OETF(Ct) − q0)/s; ks = 1.5·m − 0.5
x < ks:            H = x
x ≥ ks:  T = (x − ks)/(1 − ks)
  H = (2T³−3T²+1)·ks + (T³−2T²+T)·(1−ks) + (−2T³+3T²)·m
return PQ_EOTF(q0 + s·H)
```

Defaults: Cs = `IntentParams::Eetf.source_ceiling_nits`, else the resolved
rendering/source-reference peak (§3, P3); Ct = the resolved target peak (§3
lane rule, P2); black lift 0/0. `q0` is defined exclusively as `PQ_OETF(0)`
(P2). Identity: Ct ≥ Cs (i.e. PQ(Ct) ≥ PQ(Cs)) → return L exactly, no
compression. Domain: L ≥ 0, exact 0→0; EETF negatives pass unchanged in nits
(P3 — never fed to PQ); non-finite → typed refusal (R35). Beyond Cs (x > 1): **clip to Ct
and flag** (B3 — a monotone bounded curve cannot keep rolling off). Anchors
(Cs=1000, Ct=100): 10→10.000000, 100→69.454403, 203→88.243641, 1000→100.000000,
4000→100.000000+FLAG; (Cs=10000, Ct=100): 203→63.306209, 1000→91.070396,
4000→99.437356, 10000→100.000000 (App. N). The white mapping is a **reported
property**, asserted within ±0.5 nits of the computed anchor.

| Intent id | Use | Definition |
|---|---|---|
| `none` | in-space mapping | identity; mismatched use refuses |
| `appearance_preserving` / `scene_matching` | SDR→HDR insertion | §4, R36 |
| `eetf_to_target` | HDR→SDR input, preview, SDR deliverable, pre-HLG roll-off | PQ-normalized Hermite, Cs/Ct/knee pinned, white reported |
| `gamut_compress` | 2020→709 output, pre-HLG volume fit | below (B4) |
| `hlg_output` | HLG delivery | volume fit → inverse OOTF + OETF at the resolved target peak (P2) |
| `hard_clip_inspect` | inspection only | per-channel clip; labelled, never a deliverable/default |

`gamut_compress` (B4/Edit 4, pinned) operates in **destination RGB with
destination luminance coefficients** (709: 0.2126/0.7152/0.0722 for the SDR
cube; 2020: 0.2627/0.6780/0.0593 for HLG): resolve Y into range first (clamp +
flag); Y = 0 → black exactly; then per-pixel `c′ = Y + t·(c−Y)` with `t =
min(1, Y/(Y−cᵢ) for cᵢ<Y, (U−Y)/(cᵢ−Y) for cᵢ>Y)` (channels equal to Y impose
no constraint). Bounds: SDR cube `U = Ct`; HLG display RGB `U =
P·(Y/P)^((γ−1)/γ)`. Order: per-component tone first, then compress toward the
resulting luminance. `hlg_output`: pre-OETF volume fit per above (no preserved
super-whites on the YouTube lane), then OETF, then a declared post-OETF clamp
to [0,1] before quantization (covers float residue only). Anchors (P=1000,
γ=1.2): pre-fit signals 1.040708 (R) / 1.011855 (G) / 1.085829 (B); U bounds
800.283 / 937.285 / 624.466; post-fit 0.999999996 all three (App. N).

**R14 [S1].** The EETF construction, defaults, identity, domain, clip+flag, and
anchors above hold exactly (±1e-6 f64 reference; white ±0.5 nits of anchor).
**R15 [S1].** No output/preview path uses per-channel Reinhard or any unnamed
curve; wrong-transform controls (§17) assert Reinhard/sRGB-gamma substitutes
fail. **R30 [S1].** The compressor/ceiling behave as above on saturated colours
*and* neutral ramps; peak-saturated primaries assert U bounds + in-volume
signals + reported compression.

## 7. CC1–CC5 nodes in the flexible space (B6, B8, S2)

Nodes evaluate in the timeline working space. Each actual `ColorNodeKind` maps
to exactly one domain row (B6 — no CDL/wheels overlap: Wheels *is* the ASC CDL
slope/offset/power kind; compile-forced, SDR semantics unchanged):

| Kind | Encoding / coeffs | Authored range | Above-range behaviour | Failure |
|---|---|---|---|---|
| Curves | `grade709` scalars | pts −0.2..1.2 (CC3) | existing evaluator extrapolation, measured: w 4.926→2.153580, 49.261→6.250231 (App. N) | never; measured report (R16) |
| Wheels (CDL slope/offset/power) | unbounded `grade709` coordinates | per-control ranges (CC3 §4.1) | CC3 sign-preserving CDL on unbounded coordinates — **not** Curves' endpoint extrapolation (P5) | never; measured report (R16) |
| Primary (exposure/gain/offset, lift/gamma/gain, contrast/pivot, saturation) | linear math; luma coeffs per space (709: 0.2126/0.7152/0.0722; 2020: 0.2627/0.6780/0.0593) | unbounded math; tonal weights saturate above w=1.0 (declared) | exact math; saturation holds, reported | non-finite → R35 refusal |
| TechnicalLut / CreativeLook | stored `input_encoding_token` (N4 — never forced `grade709`) | authored | wrapped (R32) | never silently reinterpreted |
| MatteQualifier (CC5 selectors) | grade triple, space coeffs; HDR range [0, Gmax] | § below; stored CC5 integers, nits advertised on HDR | equations below | active `matte_domain_token = 1` → R17 refusal |
| Skin diagnostics | 709 constants | SDR only | **withheld with named reason** on HDR | re-derivation out of scope |

Qualifier equations (Edit 6; `Gmax = grade709_encode(1/s_white)` = 1.899662 at
defaults, App. N): `n_c = clamp(grade709_encode(max(w_c,0)), 0, Gmax)/Gmax`
(negative-clamped fraction reported); maximum/minimum/chroma on the normalized
triple; `saturation = chroma/maximum` (maximum≤0 → 0); `luma = KR·r+KG·g+KB·b`
(space coeffs above); hue = CC5 hexagonal formula verbatim (branch order +
`==` tie-break retained); legs = CC5 `band()` unchanged. Nits↔selector
(achromatic reference; chromatic is measured, not converted): `sel(D) =
grade709_encode(((D/P)^(1/γ))/s_white) / Gmax`, inverse `D(sel) =
P·(grade709_decode(sel·Gmax)·s_white)^γ`. Anchors: sel(100)=0.391468,
sel(203)=0.526409, sel(1000)=1.000000 (App. N).

Stored units and preset identity (P5): HDR qualifier storage retains CC5
integers — hue in centidegrees, saturation/luma in basis points. Convert luma
endpoints from advertised reference nits using `round(10000·clamp(sel(D),0,1))`;
convert back with `D(bp/10000)`. Softness remains selector basis points and is
labelled accordingly. These nits describe the achromatic reference, not
chromatic pixel luminance. Add Hold-only `matte_domain_token: i64`, range
0..1, default 0: 0 generic range, 1 SDR skin preset. Preset creation writes
token 1 using existing `SetEffectParam`; active token-1 qualifiers refuse on
HDR. Never infer preset identity from ordinary numeric thresholds.

LUT wrapper order (S2/N4 — no tone-map/inverse pair, never implied lossless):
working scene-linear → primaries matrix to authored (linear, negatives kept) →
exposure normalization (identity scale, stated) → `ENC(input_encoding_token)`
(0 = `encode_bt709`, 1 = linear identity, 2 = `grade709_encode`; CC4 stored
selection honoured) → LUT + excursion add-back → `DEC` (exact inverse) →
linear-light mix → inverse matrix. Bypass or mix = 0 returns the input frame
**without entering the wrapper** (bit-exact, no matrix round trip). Legacy
defaults: omitted `authored_space` → `Sdr709`, omitted token → 0. Tests:
identity, bypass, zero mix, negatives, excursions, non-identity LUTs × all
three encodings × both spaces.

**R16 [S5].** Curves/Wheels never refuse on HDR magnitudes; out-of-authored-
domain evaluation is *measured* (actual evaluated-node input) and reported.
**R17 [S2].** Skin withheld-with-reason on HDR, present on SDR; active
qualifiers with `matte_domain_token = 1` on `Hdr2020` refuse via new typed
`OpError::UnsupportedColorNodeDomain` → new code (R23). **R32 [S3].** Wrapper
order, ENC/DEC selection, bypass/zero-mix returns, tags, and tests as above.

## 8. Delivery lanes (YouTube; S6, S7)

YouTube requirements (fetched 2026-09-25,
[Upload HDR videos](https://support.google.com/youtube/answer/7126552?hl=en)):
Rec.2020 + PQ/HLG; tags transfer/primaries/matrix; MOV/MP4; VP9 P2 / AV1 /
HEVC recommended, **H.264 10-bit working**; PQ mastering metadata
recommended-with-BVM-X300-fallback (S6); HDR transcodes **plus SDR
downconversion**.

CC8 lanes (both MP4, both re-probed — generic codec-context fields do not
carry primaries/transfer, so tags go through the encoder's own params):

| Lane | Codec/profile | Tags | Notes |
|---|---|---|---|
| `youtube_hlg` | H.264 High 10, `yuv420p10le` | bt2020 / arib-std-b67 / bt2020nc / tv | primary HDR lane; `DELIVERY_X264_PARAMS` lane-derived; HLG needs no static metadata |
| `sdr_from_hdr` | H.264 High / High 10 (depth lane as today) | bt709 / bt709 / bt709 / tv | `eetf_to_target` + `gamut_compress` at project defaults; SDR lanes byte-identical for SDR timelines |

Why HLG first: tag-complete with no absolute mastering claim (honest without a
calibrated HDR monitor); PQ lane is CC9 (§18). HLG-in-AVC is unusual but
well-defined + documented-working; only G3 retires recognition risk.
Conformance (S7): over the **exported interval incl. animated enable** (not the
stills-missing, enable-ignoring scan); preview/SDR-delivery share intent
implementation with separate selection (R22/R34).

**R18 [S4].** Tags lane-derived from one source of truth (codec context, scaler
`out_color_matrix`, `setparams`, stamped frames cannot diverge); SDR params
byte-identical; exports re-probe exact or fail (never warn). **R19 [S4].**
`delivery_color_mismatches` / `DeliveryColorError` widen **by lane** (order,
shape, vocabulary unchanged): HDR-on-SDR refuses and vice versa; PQ on the HLG
lane refuses naming CC9; full-range HDR refuses; 8-bit HDR refuses; each with
its failing test. R19 refusals concern requested output descriptions/codec
settings (P2): PQ sources may feed HLG output, and HDR timelines may feed
`sdr_from_hdr`. Export preserves `ExportReport`, `normalize_master`,
`mix_audio`, cancellation, temp-file publication, loudness/true-peak reporting,
queue verification. **R34 [S4].** The production preflight→render→export path is
exercised for MO1 transforms, EXIF, alpha, freezes, disabled effects/clips;
AU normalization on/off, audio bytes, cancellation, publication retained.

## 9. QC, scopes input, and MaxCLL/MaxFALL (B5, S5)

QC gains: BT.2020 NCL Y′CbCr legality sibling of `bt709_limited_ycbcr`
(`KR=0.2627`, `KB=0.0593`, pinned); lane-aware gamut report naming its triangle
in `ColorGamutReport.definition`, both triangles where they differ;
`get_color_qc` carries the new rows. (Scope *rendering* with a nits scale is
§10/R31; this section is measurement.)

MaxCLL/MaxFALL per CTA-861.3 (B5): MaxCLL = programme max of per-pixel
max(R,G,B); MaxFALL = programme max over frames of mean over **active-area**
pixels of per-pixel max(R,G,B); weighted frame-mean luminance is a separate
row, never MaxFALL. Stage: absolute display-nit samples **after** output
rendering (OOTF + tone + gamut), **before** transfer-function encode
("pre-transfer" banned as ambiguous). Population: every active-area pixel of
every frame (decoded frame area; signalled padding excluded when signalled;
full-frame default recorded); ROI/sampled = **estimates**. Written to QC
evidence + `ExportReport.programme_light` (report/cache-only, below). Source
mastering is provenance-tagged, **never silently the programme's**; conflicts
recorded with selection rule; partial/missing/malformed distinguished.

Lifecycle/binding (S5): accumulate through the exact export frame stream or a
cancellable equivalent pass; bind to content/settings/source identities,
transform, range/rate, area, completeness; rendered vs decoded distinguished;
invalidate on binding change. Storage is **report/cache-only** (Edit 7 — §12
forbids new operations, so no journalled document mutation): results live in
the `ExportReport` section `programme_light { max_cll_nits: f64,
max_fall_nits: f64, max_frame_mean_luminance_nits: f64, frames_measured: u64,
active_area: Rect, transform_id: String, complete: bool }` plus a QC cache
keyed by the binding tuple; a read-only inspector never edits. CC9 re-measures
through its own rendering (§18).

Distinguishing programme fixture (Edit 8): frame A = uniform 203-nit
Rec.2020 red; frame B = 25% 400-nit Rec.2020 green + 75% black. Pinned:
**MaxCLL = 400, MaxFALL = 203, max frame-mean luminance = 67.8** (frame means:
maxRGB 203/100; luminance 53.3281/67.8000 — App. N).

**R20 [S5].** Corrected definitions/stage/population as above; uniform-red
reports 203/203/53.3281; the Edit-8 fixture reports 400/203/67.8 (all ±1e-6
f64 reference; production f32 ±0.5 nit). **R21 [S5].** Estimates cannot satisfy
programme-maximum gates; a test asserts refusal. **R33 [S5].** Lifecycle,
binding, invalidation, and report/cache-only storage as above; one test mutates
a binding input and asserts invalidation.

## 10. Preview, viewer, and HDR scopes (S1; no HDR display assumed)

The SDR-display viewer applies an explicit labelled **output transform**:
working scene → monitor via `preview_intent` (default `eetf_to_target` +
`gamut_compress` to Rec.709). Selection by space + target (R10 — no scan);
"independent of enable" = transform **selection**, not pixels. All surfaces
carry label + intent + peak; `render_monitor*`/`render_delivery*` stay
separate; preview unreachable from delivery (tested).

HDR scopes use a new float measurement path (S1/Edit 7 — relabelling the
tone-mapped SDR monitor cannot recover HDR values): input = post-grade
working-scene float proof; measurement equation = §2 forward rendering at
project peak (same equation as output, pinned here: `D = OOTF(w·s_white)`);
units = display nits. Histogram: 257 bins — underflow [0, 0.01), 255 log bins
with edges `e_i = 0.01·(1.2e6)^(i/255)` over [0.01, 12000] (e_1 = 0.010564,
e_128 = 11.259279, e_254 = 11359.031910, App. N), overflow [12000, ∞).

Scope semantics (P7): `PostGradeSceneFloat` measures the composited float
raster after enabled grading/layer operations and before output
rendering/tone/gamut processing. Measure R/G/B and `Y = 0.2627R + 0.6780G +
0.0593B` after the stated reference rendering, within the requested
ROI/matte population. Negative samples increment a separate counter;
non-finite samples refuse. Log bins are `[e_i, e_(i+1))`; exactly 12000
belongs to overflow. HDR requests accept omitted bins or 257, otherwise
refuse. Comparison predicate `scope_comparable(A,B)`: same working space, same
project peak, same active area, same stage, **and** equal reference white,
rendering/gamma version, units/bin definition, and all existing ROI,
matte-population, and evidence-shape guards; else `scope_incomparable {
field }` — a structured query error, **not** a new incident-policy code.
Route both app scope measurement and agent requests through this float
producer; never source HDR values from monitor bytes. Routing: app
`preview_ui` scope panel reads the new `PostGradeSceneFloat` proof stage; agent
`get_video_scopes_v2` gains `stage` (`monitoring_post_composite` default
unchanged; `post_grade_scene_float` explicit) with nits units in the response
schema. Existing SDR scope bytes and reference-shot behaviour stay pinned.

**R22 [S3].** Preview is deterministic, monotone, endpoint-exact, with CPU/GPU
parity measured independently on both sides; no exit gate judges appearance.
On an HDR-capable display the preview is still the SDR preview and the status
says so. **R31 [S5].** The float scope path (equation, 257 bins, predicate,
routing) behaves as above; SDR scope bytes bit-identical.

## 11. Typed errors and incidents (B8)

Refusal → code → policy mapping (Edit 7; no `Backend` strings for new refusals):

| Typed refusal | `incident_code()` | POLICY row / class / severity | Investigator |
|---|---|---|---|
| `ColorSourceError` (13 existing, R5) | existing source codes | existing rows | per existing decision |
| `DeliveryColorError` (4 existing, R19) | existing delivery codes | existing rows | per existing decision |
| new `OpError::UnsupportedColorNodeDomain { node, space, supported }` (family `ColorPolicy` subject-hint + per-variant code override, IN1b §3.1r6 precedent) | new `color_node_domain_unsupported` | 75 / Explain / Blocks | **off** (restates card) |
| new `OpError::InvalidColorManagement { field, observed, allowed }` (bad white/peak/intent id+version/lane dup/working contradiction; same family+override pattern) | new `color_context_invalid` | 76 / Explain / Blocks | **off** (restates card) |
| `MediaError::ColorContextInvalid { field, observed, allowed }` (render-time context/intent; replaces `validate_managed_context` `Backend`) | `color_context_invalid` via explicit `from_media_error` arm | 76 | off |
| render-time colour failure with asset+frame (incl. non-finite, R35) | source cause → existing source code; else `color_context_invalid` with asset/frame in `observed` | per code | per code |
| format v2 in v1 reader | existing `project_newer_format` | existing | existing |

Recovery (P6 — repository payloads; replaces rev 3's invented action shapes).
Both new codes use `PolicyClass::Explain`, `Blocks`, and investigator
exclusion. Their evidence carries `field`, `observed`, `allowed`, plus
optional typed `asset`, `frame`, `clip`, and `effect` identities; render
failures distinguish invalid settings from non-finite samples. The sole
recovery producer remains `policy_recovery`:

- identified unsupported qualifier → `RecoveryAction { label: "Disable
  qualifier", kind: RecoveryKind::Operation(Operation::SetEffectParam { clip,
  effect, name: "matte_qualifier_enabled", value: ParamValue::Integer(0) })
  }`, followed by an explanation;
- invalid settings → `RecoveryKind::Operation(Operation::SetColorContext {
  color_context: ColorContext::sdr_rec709() })` labelled "Reset colour
  settings to SDR";
- non-finite render failures and cases lacking a buildable payload →
  explanation-only (`RecoveryKind::Explain`).

Nothing executes automatically; no `use_range_qualifier`,
`switch_timeline_space`, or `choose_compatible_override` shapes exist.
Allowlist stays 54; `POLICY` count is not a design goal (B8).

**R23 [S2].** `incident_family()` exhaustive with no wildcard over the grown
enum (both new variants `ColorPolicy` + code overrides, covered by the
override-agreement test); `from_code` round-trips over 76; exhaustiveness,
headline, and serialised-size tests extended; both new codes asserted absent
from the allowlist with reasons recorded.

## 12. Agent surface

No new capability/operation/served tool. `SetColorContext` payload grows;
`ColorDescription`/state schema changes move registry bytes: 148/60/88 →
**153/60/93** after AW1's five inspectors (subject to merged surface),
re-pinned + ledger-appended per M36. `get_color_context` reports space,
white/peak, intents, assumptions, locks, wraps, node conditions;
`get_color_qc` reports §9 rows + estimate labels;
`ColorEvalRequest` / `measure_color_evidence` / `color_workflow_suite` gain
exact HDR outcomes (Edit 8): `hdr_import { profile: "hlg_rec2020",
assumption: "d65", raw_white: "unknown" }` (exact match); `insertion { white_bit_equal_working_1: true,
hlg_signal: 0.749877365 ±1e-5 }`;
`programme_light { max_cll_nits: 400, max_fall_nits: 203,
max_frame_mean_luminance_nits: 67.8 }` (±0.5 nit production); `lane_tags {
primaries/transfer/matrix/range/pix_fmt/profile }` (exact strings);
`sdr_regression { byte_equal: true }` (exact). Audio-workflow-v7, typed
failures, version checks, AW1 auth kept; old serialized outcomes preserved
when HDR fields absent.

Executable fields (P8): add `ColorEvalRequest.hdr: Option<HdrEvalRequest> =
None`, where `HdrEvalRequest { source_asset: AssetId, insertion_frame:
TimeCode, programme_range: Range<TimeCode>, lane: DeliveryLane }`; reject
missing assets, invalid ranges, or unsupported lanes. Add optional
`ColorEvalEvidence.hdr` containing the five result records above — import,
insertion, programme light, lane tags, SDR regression — with their listed
fields and expectations. Each result is independently optional; measurement
failure records a quantity-specific `ColorEvidenceError`, leaves that result
absent, and fails its assertion. Omit the entire HDR block when unrequested.
Pin HLG probe strings to `bt2020 / arib-std-b67 / bt2020nc / tv / yuv420p10le
/ High 10`; assert the listed numeric tolerances and booleans, not merely
field presence.

**R24 [S5].** Served quad stays 7 / 5,660 B / 3,510 B / 998 B in all pin sites.
**R25 [S5].** Registry re-pins land **after** AW1's lifecycle surface
stabilises; the M36 ledger records CC8's row with the byte split.

## 13. App surface

Import-to-export: inspector overrides, space/white/peak/intent settings, lane
selection + post-export tag check, §10 labels, §7 node wording + nits units,
MO1 disabled cases. No new panel; edit `color_ui`, `color_qc_ui`,
`preview_ui`, `export_ui` in place.

**R26 [S6].** Every UI-visible assumption/lock/wrap/placement/preview names its
intent/setting + recovery; nothing HDR-shaped renders unlabeled.

## 14. Budgets (S8)

Transform cost **measured, then pinned with margin** (CC6 App. A; margin shapes
pass §17 controls first):

- B1 decode+intent+grade uplift per 1080p frame vs the SDR path, lavapipe lane.
- B2 same, NVIDIA RTX 3090 lane (this machine's GPU lane).
- B3 export-throughput floor for `youtube_hlg` (1080p24) on both lanes.
- B4 MaxCLL/MaxFALL full-programme measurement pass: streaming accumulators,
  O(1) frame stores — no frame-cache growth; HDR cache keys are transform
  digests (S4), and no HDR stage touches the legacy display-coded path.
- B5 Windows rows (CI + Riel VM/machine) recorded separately per environment
  (never cross-OS file equality).

Precision limits after scene rendering (S8/Edit 7 — recomputed, App. N; the
rev-2 display-linear figures are superseded): f16 ULP at w=1.0 is 0.0009765625
→ 0.237891 nits via the OOTF slope at white; at w_peak=3.776475, ULP
0.001953125 → 0.620618 nits; one working-ULP at white = 0.168 HLG 10-bit
codes; W=100 puts the 10 000-nit input at working 46.4159.

- **PB1 storage boundary**: each f32→f16 store ≤ 0.5 ULP(w) (round-to-nearest);
  each encode/decode stage-pair round trip ≤ 2 ULP (RGB triplets: ULP of the
  triplet's max |channel|, erratum CE4).
- **PB2 working domain**: w ≥ 2^-10: end-to-end relative error ≤ 0.3%,
  absolute ≤ 4 ULP(w) (RGB triplets: relative ‖err‖∞/‖w‖∞ and ULP of the
  triplet's max |channel|, erratum CE4).
- **PB3 display absolute** (post-render): white ±10% ≤ 1.0 nit; peak ≤
  max(2.0 nits, 0.1% × P) (erratum CE3); below 1 nit ≤ 0.05 nits.
- **PB4 final code**: 10-bit delivery anchors (black/18%/white/peak/saturated)
  ≤ 2 codes max, ≤ 0.5 mean; 8-bit SDR anchors ≤ 1 code.
- **S1 precision errata (2026-09-25, lead ruling after the S1 reviews).**
  CE3: the 2.0-nit peak bound was derived at P = 1000; one f16 store of the
  W=100/P=10 000 working peak (≈14.96) alone costs 3.46 nits, so the peak bound
  scales with P above 2000 nits (P ≤ 2000 unchanged). CE4: once a matrix mixes
  channels, a small channel beside a large one inherits the large channel's
  storage quantum ([1, 2^-10, 2^-10] 2020→709→f16→2020 errs 8.6 ULP of 2^-10
  but < 0.01 ULP of 1.0), so triplet ULP/relative limits use the max |channel|.
  f32 intermediates were rejected (memory). CE1 accepted: the HLG compressor
  resolves Y into [0, P]. CE2 withdrawn: EETF identity/clip decide in nits.
- Non-finite/overflow in working values: typed render refusal with asset/frame
  identity (fail closed — never a quiet clamp).

**R27 [S7].** B1–B5 measured with margins and starved controls at exit;
software-only CI never implies hardware coverage. **R35 [S1].** PB1–PB4 +
non-finite refusal as above hold at every storage boundary, near-black,
saturated conversion, white extreme (100/400), and repeated-node chain.

## 15. Staging by crate (S9 re-estimate; tests separate)

Honest re-estimate retained (Edit 8): ceilings sum to 7,300; **+1,200
contingency (≈16%) → ceiling 8,500**. **S0 and S1 may start** (numerical
contract approved at promotion, §0.3). **S2's persisted
fields + format policy land after AW1 S1 merges** (one GUI/headless
implementation); AW1 pins intact, CC8 format cases added separately; re-pins
after AW1 lifecycle (R25).

| Stage | Crate / ceiling | Rules → discriminating tests |
|---|---|---|
| S0 contract + SDR baseline | docs/test infra / **100** | R1, R3, R11 — unchanged SDR corpus captured pre-change (base = main + accepted AW1 cutover, stated); baseline detects an intentional pixel change |
| S1 numeric kernel | core / **1,000** | R13–R15, R28, R30, R35 — App. N anchors (EETF 88.243641, U bounds, s_white, γ table) vs independent vectors; forward∘inverse identity; saturated primaries, seams, near-black, 10k peak, white extremes; wrong-transform controls fail; PB1–PB4; non-finite refusal |
| S2 model, incidents, format | core + project / **1,400** | R2, R4–R5, R8, R17, R23, R29 — structs/precedence/atomicity/locks; save/reopen, undo/redo, journal replay, wire round trip, older-reader overwrite refusal (actual old reader), SDR saves byte-identical, family+code mapping |
| S3 input, render, preview | media / **1,800** | R6–R7, R9–R10, R12, R22, R28, R32, R36 — untouched-probe imports, insertion equality + both intents, full matrix, images/freezes/titles/disabled, legacy branch, transform-aware caches + warm renderer, wrapper tests, preview parity + unreachability from delivery |
| S4 delivery | media + core delivery / **1,000** | R18–R19, R34 — lane table, refusal matrix, exact tags + native planes (**explicit legality assertions**, not warning-only `range_exceptions`, S9), real encode/decode, AU behaviour, SDR pre/post equality |
| S5 QC, measurement, agent | core + agent / **1,200** | R16, R20–R21, R24–R25, R31, R33 — Edit-8 fixture 400/203/67.8, programme-vs-estimate, binding invalidation, report/cache-only storage, dual-triangle, 257-bin scopes + predicate, selector equations + nits units, exact eval outcomes + preserved old outcomes, served quad, re-pins |
| S6 human workflow | app / **600** | R26 — import-to-export, undoable settings, labels, nits units, node wording, post-export verification |
| S7 evidence + closure | agent/eval/CI / **200** | R27, G-gates — lavapipe/3090/Windows measurements, YouTube artefact, manifest, roadmap status |

Scoped checks in development, dependent-crate checks per boundary, full gate per
stage. Final: workspace tests, fmt, clippy `-D warnings`, Windows/Linux media,
admitted Kani lane (no float HDR proofs to populate it).

## 16. Exit gates (tagged; all fail today)

- **G1 [new]** untouched-probe HDR round trip: tagged BT.2020 PQ/HLG imports
  (`Unknown` white survives), previews labelled, exports HLG, one D65
  assumption — fails today (managed decode refuses).
- **G2 [regression]** SDR byte-identity pre/post: same fixtures/deps/OS/adapter,
  bytes + full SDR exports equal before/after; per-environment baselines — red
  today as missing evidence (S9).
- **G3 [new]** YouTube-accepted HLG upload: **Riel uploads** (S6); implementer
  supplies fixture + hash + manifest + checklist; `inconclusive/pending`
  explicit. Fails today (no lane).
- **G4 [new]** light levels match vectors (203/203/53.3281 + 400/203/67.8
  fixtures); estimates refused — fails today (no measurement).
- **G5 [new]** intent gates: EETF construction exact (10→10.0, 203→88.243641,
  1000→100.0 at Cs=1000/Ct=100; clip+flag past Cs); Reinhard/sRGB/input-OOTF
  substitutes fail — fails today.
- **G6 [new]** SDR-from-HDR deliverable: legal tagged SDR file (explicit
  native-plane assertions, S9) via pinned default intent; SDR exports unchanged
  — fails today.
- **G7 [new]** node table classified by measurement; qualifier above white per
  equation; skin withheld on HDR; refusal typed — fails today.
- **G8 [new]** preview *selection* independent of later HDR clips + enable; all
  surfaces labelled; unreachable from delivery — fails today.
- **G9 [new]** B1–B5 + PB1–PB4 with margins on lavapipe, RTX 3090, Windows —
  fails today.
- **G10 [new]** served quad unchanged; re-pinned post-AW1 (153/60/93) + ledger;
  v1 identical; v2 refused by old reader — fails today. Standing
  **[invariant]** re-assertions in S4/S5: tags, scope bytes, exhaustiveness.

## 17. Evidence plan

- **Vectors**: ST 2084 / ARIB STD-B67 anchors; BT.2100 γ; BT.2390-4 EETF;
  BT.2408 white; hand-derived matrices. Every gate pairs a vector with a
  control (round trips alone prove nothing).
- **Wrong-transform controls**: Reinhard, sRGB-gamma decode, 709-luma
  saturation, pre-conversion MaxRGB, input-side HLG OOTF must FAIL
  R13–R15/R20/R28/R30.
- **Platforms**: lavapipe + RTX 3090; Windows CI + scripted Riel checklist
  (per-environment baselines only).
- **Artefact**: G3 upload record (hash + manifest + client evidence + date, or
  explicit `inconclusive`); `ffprobe` necessary, never sufficient.

## 18. CC9 one-page boundary — PQ/HDR10 YouTube lane

**Goal**: `youtube_pq` (H.264 High 10 or HEVC — named recognition requirement
decides; HEVC re-takes the patent call). **Consumes**: intent registry
(`pq_output`: ST 2084 + ST 2086 mux), lane/re-probe machinery, SDR corpus —
but **re-measures** light levels through its own rendering (S5). **New**:
mastering-provenance UX (declared or explicitly absent; BVM-X300 fallback
documented, never written; source mastering never copied); muxing + re-probe;
PQ QC rows; re-pin; second G3 artefact. **Out**: dynamic metadata, calibrated
monitoring, HEVC-without-cause. **Size**: ~800–1,200 + 10%, one stage post-CC8.
**Why not CC8**: §1 — HLG tag-completeness vs PQ mastering carriage.

## 19. Riel's end-to-end production session (checklist material)

1. Import real PQ + HLG clips: probe untouched, one D65 assumption each,
   preview labelled. 2. Mixed timeline: insertion visible; enable never
   changes selection. 3. Grade (curves/wheels/look/qualifier): domain reports,
   skin withheld, wrap reported, nits units. 4. Export `youtube_hlg`, tags
   exact; **Riel uploads unlisted** (G3) → HDR badge + SDR version or
   `inconclusive`. 5. SDR export from same timeline legal + tagged; SDR
   project byte-identical. 6. QC: gamut, nits scopes, levels vs known values,
   estimate labels. 7. Change white/peak/space: undoable; locks hold + report;
   save → v2; v1 build → card, no overwrite. 8. Incomplete until findings
   discharged or deferrals recorded — never N2.3/N2.4.

## 20. Risks

- **YouTube HLG-in-AVC recognition** (open): only G3 retires it.
- **EETF parameters read as fact**: pinned + inspectable; white reported.
- **Foreign masters vs white**: locks + overrides (R8), reported not silent.
- **SDR regression**: legacy branch + S0 baseline + G2; cache digests (S4).
- **Scope discipline**: §1 prohibition + Rec.2100-only vocabulary.

## Appendix N — numeric check (Edits 1–8 + P2; values from the review-round `scratch/check.py`, lead-rechecked; f64 unless noted)

| quantity | value | how |
|---|---|---|
| q0 = PQ_OETF(0) | 7.3e-7 (c1^m2; not 0) | ST 2084 inverse at 0 — P2; anchors below unchanged at 1e-6 (lead re-check) |
| PQ_OETF(203) | 0.580688881 | ST 2084 inverse |
| PQ_OETF(100) | 0.508078422 | ST 2084 inverse |
| PQ_OETF(1000) | 0.751827096 | ST 2084 inverse |
| PQ_OETF(10000) | 1.000000000 | ST 2084 inverse (=1) |
| gamma(400) | 1.032865 | log rule |
| gamma(1000) | 1.200000 | log rule |
| gamma(2000) | 1.326433 | log rule (owns boundary) |
| gamma(4000) | 1.481185 | power rule |
| gamma(10000) | 1.702316 | power rule |
| s_white (203/1000/1.2) | 0.26479719 | (W/P)^(1/g) |
| w_peak = 1/s_white | 3.776475 | 1/s_white |
| HLG signal of exactly 203 nits | 0.749877365 | OETF((203/1000)^(1/1.2)) |
| nits at HLG 0.75 | 203.152145 | 1000*OETF^-1(0.75)^1.2 |
| OETF(1.0) | 0.999999996 | upper branch at E=1 |
| scene at HLG 0.75 | 0.26496256 | OETF^-1(0.75) |
| working at HLG 0.75 | 1.000625 | OETF^-1(0.75)/s_white |
| f32: scene at 203 nits | 0.26479718 | f32 (W/P)^(1/g) |
| f32: OETF of that | 0.749877334 | f32 upper branch |
| EETF(0; Cs=1000, Ct=100) | 0.000000 | corrected Hermite, source-span denorm |
| EETF(10; Cs=1000, Ct=100) | 10.000000 | below-knee identity |
| EETF(100; Cs=1000, Ct=100) | 69.454403 | corrected Hermite |
| EETF(203; Cs=1000, Ct=100) | 88.243641 | corrected Hermite |
| EETF(1000; Cs=1000, Ct=100) | 100.000000 | Cs→Ct endpoint |
| EETF(4000; Cs=1000, Ct=100) | 100.000000 +FLAG | above Cs: clip to Ct |
| EETF(203; Cs=10000, Ct=100) | 63.306209 | wide-span roll-off |
| EETF(1000; Cs=10000, Ct=100) | 91.070396 | wide-span roll-off |
| EETF(4000; Cs=10000, Ct=100) | 99.437356 | wide-span roll-off |
| EETF(10000; Cs=10000, Ct=100) | 100.000000 | Cs→Ct endpoint |
| EETF(203; Cs=100, Ct=1000) | 203.000000 | Ct>=Cs identity |
| saturated R: pre-fit signal | 1.040708 | OETF(invOOTF(1000-nit primary)) |
| saturated R: U bound | 800.283 | P*(Y/P)^((g-1)/g) |
| saturated R: post-fit signal | 0.999999996 | after affine fit; clamp to 1.0 |
| saturated G: pre-fit signal | 1.011855 | OETF(invOOTF(1000-nit primary)) |
| saturated G: U bound | 937.285 | P*(Y/P)^((g-1)/g) |
| saturated G: post-fit signal | 0.999999996 | after affine fit; clamp to 1.0 |
| saturated B: pre-fit signal | 1.085829 | OETF(invOOTF(1000-nit primary)) |
| saturated B: U bound | 624.466 | P*(Y/P)^((g-1)/g) |
| saturated B: post-fit signal | 0.999999996 | after affine fit; clamp to 1.0 |
| insert bt709 V=0: appearance | 0.000000 | 100*V^2.4 (BT.1886), x2.03, invOOTF, /s_white |
| insert bt709 V=0: scene-match | 0.000000 | invOETF_709(V) |
| insert bt709 V=0.5: appearance | 0.250000 | 100*V^2.4 (BT.1886), x2.03, invOOTF, /s_white (lead erratum: rev 3 had 0.067454 from invOETF^2.4) |
| insert bt709 V=0.5: scene-match | 0.259719 | invOETF_709(V) |
| insert bt709 V=1: appearance | 1.000000 | white equality |
| insert bt709 V=1: scene-match | 1.000000 | invOETF_709(1)=1 |
| insert sRGB V=0.5: appearance | 0.276746 | 80*srgbInv, x(203/80), invOOTF, /s_white |
| insert sRGB V=0.5: scene-match | 0.214041 | srgbInv(V) |
| insert sRGB V=1: appearance/scene | 1.000000 | white equality both |
| uniform 203 red: MaxCLL/MaxFALL | 203 / 203 | max / mean maxRGB, uniform |
| uniform 203 red: mean lum | 53.3281 | KR20*203 |
| mixed frame: frame-mean maxRGB | 100.0 | 0.25*400+0.75*0 |
| mixed frame: frame-mean lum | 67.8000 | 0.25*KG20*400 |
| programme MaxCLL / MaxFALL | 400 / 203 | max(203,400) / max(203,100) |
| programme max frame-mean lum | 67.8 | max(53.3281, 67.8) |
| f16 ULP at w=1.0 | 0.0009765625 | binary16 spacing |
| display nits per ULP at w=1.0 | 0.237891 | P*g*s^(g-1)*s_white*ULP |
| f16 ULP at w=3.7765 | 0.0019531250 | binary16 spacing |
| display nits per ULP at w=3.7765 | 0.620618 | P*g*s^(g-1)*s_white*ULP |
| W=100: s_white | 0.146780 | (100/1000)^(1/1.2) |
| W=100: working of 10k-nit input | 46.4159 | s/s_white |
| HLG 10-bit codes per working-ULP at white | 0.1680 | OETF slope*ULP*876 |
| grade709(4.926) | 2.153580 | 1.0992968*x^0.45-0.0992968 |
| grade709(49.261) | 6.250231 | 1.0992968*x^0.45-0.0992968 |
| Gmax = grade709(1/s_white) | 1.899662 | selector range top |
| selector at 100 / 203 / 1000 nits | 0.391468 / 0.526409 / 1.000000 | grade709(w)/Gmax |
| hist edges e[0] / e[1] / e[128] / e[254] / e[255] | 0.01 / 0.010564 / 11.259279 / 11359.031910 / 12000.0 | 0.01*1.2e6^(i/255) |

Cross-checks: all critic/lead values reproduced exactly (PQ/HLG anchors, B4
signals, EETF 10→10.0/203→88.243641/1000→100.0, γ pair, grade pair, 400/203/67.8);
f32 agrees with f64 to ~3e-8 on the HLG anchor.

---
*Rules: R1 §2; R2–R4 §3; R5–R10, R36 §4; R11–R13, R28 §5; R14–R15, R30 §6;
R16–R17, R32 §7; R18–R19, R34 §8; R20–R21, R33 §9; R22, R31 §10; R23 §11;
R24–R25 §12; R26 §13; R27, R35 §14; R29 §3. Gates §16 (G2 regression, rest new,
invariants in G10/S4/S5); evidence §17; CC9 §18; checklist §19; App. N.*
