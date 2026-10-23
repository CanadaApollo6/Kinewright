# Job A S0: coordinated contracts and baseline harness

## 1. Status, authority and scope

**Contracts accepted 2026-10-20** (revision 2). Implementation has not started; every runtime,
native, provider, resource and human gate is NOT RUN.

Part of job A (talking head) in the [product brief](PRODUCT-BRIEF-2026-10.md#programme-mapping).
It reconciles [JA-1](JA1-STUDIO-ORCHESTRATION.md), [JA-2](JA2-PERSON-SEPARATION.md),
[JA-4](JA4-TAKE-CHOICE.md) and [AW2](AW2-CODE-GENERATED-CLIPS.md) at their shared S0 stage.

Review trail, under `target/review/ja/`:
- Astra critic: `ja-s0-contracts-critic.md` (revise; 2 Major, 1 Minor).
- Round 2 on rev2: `ja-s0-contracts-critic-r2.md` (accept; all three closed, no new findings).

Kinewright source pin: `6cbbda2d2e7707d8a3a1076e49576c9741c3ef28` (`git rev-parse HEAD`).
MockingBoard read-only source pin: `aef7c95f1829754c5bc5b8f3bdbea91e175eeb39`.
Kinewright citations below are relative to its root and refer to that HEAD. MockingBoard
citations use its absolute root. Files under `target/review/` are local review inputs at this
inspection, not an assertion that Git tracks them at HEAD.

Binding inputs are `target/review/ja/task-ja-common.md:1`,
`target/review/ja/task-ja-s0-contracts.md`, `target/review/PITFALLS.md:5`, and
`target/review/ja/task-ja-s0-rev2.md`. The rev2 lead decisions accept findings 1–3 and
Q1–Q4's recommended answers in `target/review/ja/ja-s0-contracts-critic.md`;
§§12–13 record those resolutions. This is a complete standalone contract.
The accepted designs are JA-1 revision 3, JA-2 revision 3, JA-4 revision 2 and AW2.
JA-1 is the persistence/lineage authority; JA-3 is an older scoping note, superseded where
JA-1/JA-4 settle its questions (`docs/JA4-TAKE-CHOICE.md:14`).

S0 freezes the interfaces below, prepares a predecessor-compatible observational harness,
and pins existing regressions. It introduces **no production format, host, store, parser,
matcher, decoder lease, effect, scope enforcement, scheduler or delivery semantics**.
All first-write stages in §3 are later implementation stages, not authorization to write
those records during S0. No Cargo, native test, render, provider probe, mutation, performance
measurement or human acceptance ran for this draft. Expected deficiencies are inferred
from inspected code; none is an observed runtime red.

The intended result remains Riel's tight-end video, editable ordinary clips, grounded
Songbird graphics, and 30–60 minutes of Riel's time. The lead's practice dry run precedes
one finished-video watch; automated results cannot certify publishability
(`docs/PRODUCT-BRIEF-2026-10.md:43`, `docs/PRODUCT-BRIEF-2026-10.md:56`,
`docs/PRODUCT-BRIEF-2026-10.md:202`). No new intake interview is required.

**Amendment A1 (2026-10-22).** Independent rescue copy is specified in the approved
[design record](JA-S0-AMENDMENT-A1.md) (A1 rev5), whose §§2–7 are normative
A1 rules referenced below. Rescue is specified but **not implemented until JA-1 S2 lands
and is gated**. The bounded R_ack retention ruling in §4.3 governs the design record's
older-record retention wording.

## 2. Person-visible workflow and responsibility boundaries

Riel supplies the script, recordings, exports and reference material. The agent shows one
short plan, chooses complete takes, makes the cut, adds supplied facts/cards/captions,
improves look/audio, and requests independent output review. Missing evidence remains
visible. A completed cut can be watched immediately; pointing at its displayed frame
creates feedback on that exact version. A requested alternate take produces a successor
edit and retains the original choice/context. Publish-kit derivatives receive their own
checks. These are the accepted workflows, not new S0 UI work
(`docs/JA1-STUDIO-ORCHESTRATION.md:69`, `docs/JA4-TAKE-CHOICE.md:37`).

The lead coordinates one schema. `kinewright-project` owns project persistence and store
publication; the JA-1 run/commit host is its sole durable writer. Agent code owns sessions
and editorial planning. Media owns decoder/analysis/render adapters. The app owns local
manual interaction and actual presentation. JA-4 supplies script/take evidence; JA-3
consumes it; JA-2 supplies person/face evidence. None gets another project writer,
collector or disk ledger (`docs/JA1-STUDIO-ORCHESTRATION.md:109`,
`docs/JA2-PERSON-SEPARATION.md:240`, `docs/JA4-TAKE-CHOICE.md:27`).

The first video uses directly authored ordinary cards/titles and AW2 Part 1 SVG, with
existing numeric motion where needed. It does not require Remotion or reusable MO6
templates; real templates retain their full MO3 dependency
(`target/review/ja/ja3-editorial-scoping-rev1.md:180`,
`target/review/ja/ja3-editorial-scoping-rev1.md:249`,
`target/review/mo/mo6-design-rev3.md:1045`). External's accepted shape is included below
because the shared AW2 format contract must stay coherent, although its first writer is P2a.

## 3. One project-format change and the complete Job A inventory

### 3.1 Meaning of v3 and relationship to today's envelope

**V3 is the project-file/envelope format, including its bundle graph and any new Document
features. It is not a separate `Document.version` or a second graph-format counter.**
Today `ProjectFile` has `format_version` and a flattened `Document`; v1 omits the version
key (`crates/kinewright-project/src/project.rs:218`,
`crates/kinewright-project/src/project.rs:239`). The supported maximum is currently 2
(`crates/kinewright-core/src/model.rs:1254`).

The proposed v3 root preserves that flattened ordinary Document and adds optional `studio`.
Its Document must equal the acknowledged active manual branch checkpoint. Delivering a
candidate changes the version index, not that Document. Fenced adoption changes the active
manual pointer, switch generation, flattened Document and receipt in one publication.
Independent branch/head Documents live in referenced immutable objects. This is one root
with an owned dependency graph, not multiple competing project files
(`docs/JA1-STUDIO-ORCHESTRATION.md:113`, `docs/JA1-STUDIO-ORCHESTRATION.md:223`).

Use a shared typed `ObjectRef = {sha256, byte_length, record_kind}` at durable record edges;
asset-specific format/API/profile tags retain their own meaning within the project version.
Hashes are SHA-256 of exact immutable bytes, not today's incident-pairing FNV digest.
IDs are domain-distinct opaque strings, not runtime revision counters. Occurrence identity
is the JA-1 tuple `(branch_origin, creation_batch_id, result_ordinal)`; WordId is the JA-4
tuple `(transcript_snapshot_hash, original_word_ordinal)`. Do not replace either with text
equality or a reusable Core clip ID (`docs/JA1-STUDIO-ORCHESTRATION.md:282`,
`docs/JA1-STUDIO-ORCHESTRATION.md:442`, `docs/JA4-TAKE-CHOICE.md:239`).

### 3.2 Persisted inventory

This is the single union inventory. Grouped rows enumerate members of one record family;
they are not permission for extra unlisted fields. Payload records are bounded and typed.
An index maps stable identities to ObjectRefs and bounded annotations, with unknown required
kinds refusing writes/resume. Empty optional additions omit themselves on legacy writes.
`S1` etc. below mean **JA-1 stages**, unless prefixed JA-4, JA-2 or AW2. A record first
produced later still uses the S1 graph/publication machinery.

| New persisted field, record, resource or asset kind | Required content / attachment | Owning design; first writer; source |
| --- | --- | --- |
| Root `studio` identity/pointers | `project_id`, `generation`, `creation_brief_ref`, `manual_branch_id`, `manual_switch_generation`, `candidate_branch_ids` | JA-1; S1, switch mutation S2; `docs/JA1-STUDIO-ORCHESTRATION.md:113` |
| Root `studio` indexes/ownership refs | `branch_index_ref`, `version_index_ref`, `feedback_index_ref`, `run_index_ref`, `claim_index_ref`, `input_manifest_ref`, `receipt_index_ref`, `issuer_state_ref`, `store_manifest_ref`; omit absent later-domain indexes | JA-1; S1 infrastructure, individual records at stages below; `docs/JA1-STUDIO-ORCHESTRATION.md:115` |
| Brief/input revision records | Exact supplied creation brief, script/data/style/reference identities; owned byte refs and successor provenance, never credentials or chat transcripts | JA-1; S1, script specialization JA-4 S1; `docs/JA1-STUDIO-ORCHESTRATION.md:109`, `docs/JA1-STUDIO-ORCHESTRATION.md:164` |
| Branch/head checkpoint and history record | Branch/head IDs, document ref/hash, baseline ref, validated ordered batches, cursor/redo order, gesture boundaries/pre-gesture baselines, applied-ops digest, lineage ref, checkpoint/floor generations, exact command-stream cut | JA-1; S1; `docs/JA1-STUDIO-ORCHESTRATION.md:132` |
| Specialist base/proposal record | Target branch, exact base HeadId/document hash, task/attempt/scope, full currently applied merge delta and frozen head/history/lineage refs | JA-1; S1 storage contract, S4 task producer; `docs/JA1-STUDIO-ORCHESTRATION.md:134`, `docs/JA1-STUDIO-ORCHESTRATION.md:159` |
| History-floor / recovery-close checkpoint | Preceding checkpoint, new floor, cut command ID, old/new baseline refs, retained batches/cursor/redo, gesture state, lineage and equivalence digest | JA-1; S1 record, S2 coordinated manual producer; `docs/JA1-STUDIO-ORCHESTRATION.md:151`, `docs/JA1-STUDIO-ORCHESTRATION.md:198` |
| Head-scoped occurrence lineage | Local-ID→occurrence map, parent/range ancestry for splits, operation/batch provenance; chosen head restores its exact map | JA-1; S1; `docs/JA1-STUDIO-ORCHESTRATION.md:440` |
| Version snapshot/index and annotations | Immutable document/head/input/script/data/style refs, parent/run/review refs; separately mutable name/pin annotations. Unreviewed snapshots cannot assert Delivered | JA-1; S3 snapshot, S5 Delivered; `docs/JA1-STUDIO-ORCHESTRATION.md:164`, `docs/JA1-STUDIO-ORCHESTRATION.md:820` |
| Command receipt and issuer state | Issuer/sequence, canonical digest, original target/epoch/expected state, outcome/result identities/generation; high-water, retirement and unresolved evidence. Switch results include old/new branches, heads, checkpoints and switch generation, never a live lease | JA-1; S1, switch extension S2; `docs/JA1-STUDIO-ORCHESTRATION.md:340`, `docs/JA1-STUDIO-ORCHESTRATION.md:225` |
| Store manifest / recovery inventory | Verified object identities, dependency edges, required field/kind/API inventory and durable pins; current root, two verified predecessors, one B3 staging slot and distinct pre-migration backup. Recovery inventories do not recursively retain predecessors | JA-1; S1; `docs/JA1-STUDIO-ORCHESTRATION.md:120`, `docs/JA1-STUDIO-ORCHESTRATION.md:328`, `docs/JA1-STUDIO-ORCHESTRATION.md:704` |
| `rescue_copy_v1` | Recognized schema/role on root-reachable `OwnedEvidence` in `durable_pins`; B/session/origin and full/selected capture bindings; one dedicated optional R_ack `RootCheckpoint` content edge with complete B-owned closure and reopen/recovery semantics; bounded exact omitted-recovery identities/SHAs/causes, unresolved destination-attempt/command identities and bounded warning provenance. R_ack retention is one level with explicit retirement under §4.3 | JA-1 owner; S2 first writer; Amendment A1 §§4, 7 |
| Feedback packet/revisions/batch | Original displayed version/head/document/input, frame/rational fps, raster/orientation/geometry basis, exact text/strokes, optional hit/occurrence/source/beat, frame proof, voice ref, chosen transcript revision; immutable submitted batches | JA-1; S3; `docs/JA1-STUDIO-ORCHESTRATION.md:424` |
| CommentResolution/disposition | Original packet and exact candidate head; separate temporal and spatial result/evidence, unresolved/removal disposition; status/tombstone never deletes original evidence | JA-1; S3; `docs/JA1-STUDIO-ORCHESTRATION.md:449` |
| Run/plan/task/attempt records | State transitions, short plan, assumptions/questions, task scopes/bases/grants, requested/resolved connector/model/options, counters/deadlines, usage event identities and bounded findings/responses/continuation refs | JA-1; S4; `docs/JA1-STUDIO-ORCHESTRATION.md:478`, `docs/JA1-STUDIO-ORCHESTRATION.md:495`, `docs/JA1-STUDIO-ORCHESTRATION.md:516` |
| Attempt cleanup reservation/recovery identity | Nonce, owner instance, service/cgroup or Job/process identity, lifecycle phase and held slots/pins; host imports verified guardian results. Private OS-local guardian records are not alternate project roots | JA-1 reusing AW2; S4, shared guardian AW2 P1b-s; `docs/JA1-STUDIO-ORCHESTRATION.md:375`, `docs/JA1-STUDIO-ORCHESTRATION.md:391` |
| ClaimBinding/validation evidence | Exact export/adapter, typed dataset/cells/scope/completeness/context, finite derivation, units/rounding, expected value/text, typed output slots, output head/definition and validation key | JA-1; S5; `docs/JA1-STUDIO-ORCHESTRATION.md:539`, `docs/JA1-STUDIO-ORCHESTRATION.md:556` |
| Review artifact/coverage/receipt | Encoded artifact hash, frozen head/input/claims/settings, decoder identity, PTS→timeline mapping, checker profiles/results, sampled/full coverage, actual reviewer model/modalities and independent judgment | JA-1; S5; `docs/JA1-STUDIO-ORCHESTRATION.md:613`, `docs/JA1-STUDIO-ORCHESTRATION.md:624` |
| Completion event / publish-kit index | Event/version identity and notification outcome; output hashes/settings, source/claim/review refs, reel→master passage maps, copy/chapters/thumbnail artifacts and per-derivative readiness | JA-1 with JA-3 consumers; S6; `docs/JA1-STUDIO-ORCHESTRATION.md:670`, `target/review/ja/ja3-editorial-scoping-rev1.md:353` |
| Owned-input sealed manifest / entries | Content/length/kind, bounded dependency refs, stream/interpretation identity, capture/seal evidence, original locator as hint, successor links. Decoder-use receipts reference the sealed identity, not a mutable path | JA-1 owner, shared AW2/JA-2 interface; S1 foundation, actual consumer integration before immutable use; `docs/JA1-STUDIO-ORCHESTRATION.md:172`, `docs/JA2-PERSON-SEPARATION.md:155` |
| Owned byte asset kinds | Camera/mic media, exact script/Songbird bytes, supplied references/source cards/styles, fonts/notices, LUT blobs, code sources, generated outputs, original strokes/frame proofs, voice audio/transcripts, review renders/images/PCM evidence, kit copy/thumbnails. Reuse existing LUT/Media records where applicable | JA-1 store; S1 admission, producing domain S3–S6/AW2/JA-2; `docs/JA1-STUDIO-ORCHESTRATION.md:172`, `docs/JA1-STUDIO-ORCHESTRATION.md:287`, `docs/AW2-CODE-GENERATED-CLIPS.md:407` |
| `Effect.resource = PersonMatte` | Optional typed resource only on `person_separation`: bounded recipe descriptor and exact artifact digest; absent serializes away, bypass still counts. Mode/blur/edge remain registered integer controls and ordinary local curves | JA-2; Stage C after Stage A selection; `docs/JA2-PERSON-SEPARATION.md:348` |
| JA-2 separation / Solid-gradient treatment settings | New registered effect descriptors use the existing integer parameter/curve containers for background mode, blur/edge and gradient controls; no new raster backdrop asset or rival gradient clip kind | JA-2; Stage C; `docs/JA2-PERSON-SEPARATION.md:352`, `docs/JA2-PERSON-SEPARATION.md:356` |
| Person recipe/profile and flat matte manifest | Source/stream/selector/rate/orientation/preprocessing, model/runtime/representation/reset identity; ordered direct coverage→chunk/picture refs. Artifact digest equals canonical manifest digest; no parent chain | JA-2; Stage B after A; `docs/JA2-PERSON-SEPARATION.md:247`, `docs/JA2-PERSON-SEPARATION.md:263` |
| Matte chunk / bounded recipe lookup | Independently seekable alpha PNGs, ≤60 pictures/chunk, dimensions/IDs/offsets/lengths/hashes; ≤32 lookup results/recipe, never used as bound edit identity | JA-2; Stage B; `docs/JA2-PERSON-SEPARATION.md:249`, `docs/JA2-PERSON-SEPARATION.md:261` |
| Person job/progress / face evidence | Source/profile-bound job descriptions/progress and geometry/statistics/validity; exact source picture/stage/proof identity. No recurrent tensors, face crops or per-frame foreground RGB assets | JA-2; Stage B jobs, D geometry; `docs/JA2-PERSON-SEPARATION.md:195`, `docs/JA2-PERSON-SEPARATION.md:493`, `docs/JA2-PERSON-SEPARATION.md:608` |
| ScriptRevision with BeatId/LineId/token/cue records | Exact bytes/hash/parent, parser/normalizer/classifications, raw spans, ordered speech/cues and unique/unresolved correspondence; IDs are allocated identities | JA-4; S1; `docs/JA4-TAKE-CHOICE.md:76`, `docs/JA4-TAKE-CHOICE.md:230` |
| TranscriptSnapshot / successor evidence | Owned source/stream/fps/model, actual extraction/language/timing provenance or unknown, immutable words/original word hash and correction links | JA-4; S1; `docs/JA4-TAKE-CHOICE.md:231`, `docs/JA4-TAKE-CHOICE.md:239` |
| CandidatePage / search checkpoint / diagnostic pages | Frozen scope/script/profile/snapshot list, exact WordId paths, stable labels/features/issues and per-candidate `validation_evidence_refs`; frontier/normalizer/coverage/page refs and cumulative work. Diagnostics have a separate bounded quota/status | JA-4; S2; `docs/JA4-TAKE-CHOICE.md:146`, `docs/JA4-TAKE-CHOICE.md:232`; edges frozen in §3.5 |
| JA-4 `TakeValidationEvidence` referenced record | Owned source/stream, snapshot/script/candidate/WordId path, exact frame/sample range and decoder/timestamp coverage; validator/profile identity, boundary/clipping measurements/outcome; source-bound critical-speech observation identity, words/value, interval, observer and actual capability; valid/refused/unresolved status/reasons and successor invalidation (§3.5) | JA-4 inside the existing take-evidence family; S2 validation producer, S3 observation/revalidation successor and choice-ref producer; `docs/JA4-TAKE-CHOICE.md:188`, `docs/JA4-TAKE-CHOICE.md:195`, `docs/JA4-TAKE-CHOICE.md:233` |
| TakeMapRevision / TakePin / addressed pin resolution | Parent/script, ordered choices/status/reasons/alternatives, actor/task and validation profiles; choices/retained alternatives have `candidate_page_ref` and direct `validation_evidence_refs`; original line/beat/context/displayed version/head/comment/candidate/words/command, active/cleared/unresolved state and resolution evidence | JA-4; S3; `docs/JA4-TAKE-CHOICE.md:233`, `docs/JA4-TAKE-CHOICE.md:248`; edges frozen in §3.5 |
| `take_map_revision_ref` on applicable head/checkpoint/version | Exact map/pin/consumer state belongs to that head and undo/redo checkpoint; unavailable pre-map state is absent, never “latest” | JA-4 within JA-1 graph; JA-4 S1 storage attachment, S3 choices; `docs/JA4-TAKE-CHOICE.md:224` |
| PauseIntent / RecordingPair | Prior-authored cue/token/beat/source-gap/hold and provenance; owned streams/time bases/origins/decoder convention, capture A/V observation, fit/uncertainty, all anchors/decoded coverage and admitted/refused ranges | JA-4; S1 parsed pause evidence, S4 assembly/sync evidence; `docs/JA4-TAKE-CHOICE.md:235`, `docs/JA4-TAKE-CHOICE.md:261` |
| AssemblyBinding / consumer inventories | Map/plan digest, occurrences/WordIds/source→project segments, requested/applied bounds/rounding/errors, links/pauses and invalidations; referenced consumer-inventory records persist bounded `FactualOutputInventory` and `AudioIntentInventory` members, their exact output-head/dependency/profile keys and caption/card/chart/layout expectations (§§5.2–5.4) | JA-4 plus JA-3; AssemblyBinding at JA-4 S4, JA-3 inventory producers at JA-4 S5 before JA-1 S5 admission; `docs/JA4-TAKE-CHOICE.md:237`, `docs/JA4-TAKE-CHOICE.md:365`, `docs/JA4-TAKE-CHOICE.md:460` |
| `ClipContent::Code(CodeClip)` / `Document.code_assets` | Closed language/API (`svg`, API 1), immutable source hash/length, declarations/bindings/font refs/provenance; sorted typed parked values and numeric curves. No raw SVG in operations/journal | AW2; P1a, usable only with P1b/P1c; `docs/AW2-CODE-GENERATED-CLIPS.md:364` |
| `ClipContent::External(ExternalClip)` / `Document.external_specs` | Proposed table spelling for AW2's immutable ExternalSpec records: source/input/descriptor/props/dependency/render refs, composition ID, N/dimensions/rational rate and fixed origin O; existing span [a,b) satisfies `0≤O+a<O+b≤N` | AW2; P2a, first video deferred; `docs/AW2-CODE-GENERATED-CLIPS.md:512` |
| AW2 render/input/runtime manifests and outputs | Native source/font/binding provenance; External sorted captured source and runtime-image manifest refs, exact executable/dependency identity, render key and complete PNG/file/raw-pixel manifest | AW2; P1a source assets, P1b-r native reports, P2a offline manifest, P2c executed output; `docs/AW2-CODE-GENERATED-CLIPS.md:445`, `docs/AW2-CODE-GENERATED-CLIPS.md:640`, `docs/AW2-CODE-GENERATED-CLIPS.md:727` |

Machine-local inference libraries/models/notices/install manifests and External execution grants
are not portable project fields. A project may reference a profile identity without embedding
an installation or permission. Staging files and OS leases are transient, not extra root
authorities (`docs/JA2-PERSON-SEPARATION.md:139`, `docs/AW2-CODE-GENERATED-CLIPS.md:560`).

### 3.3 Single minimum-version predicate, legacy preservation and refusal

Retain **one** required-version implementation in `kinewright-project`, extending the function
currently at `crates/kinewright-project/src/project.rs:380` to inspect the whole saved bundle.
Its Document-only entry, if retained for legacy callers, delegates to the same implementation
with explicitly empty bundle state. It must not accept a projection that hides Studio state.
The rule is:

```text
required(saved_bundle) =
  refuse_writes_and_resume, if required version/kind/API is unsupported or graph invalid;
  3, if nonempty Studio state or reachable JA-4 records exist,
        or any saved/reachable Document has Code/External/PersonMatte,
        or the new JA-2 separation/gradient treatment is stored,
        or any corresponding AW2 asset/spec table is nonempty;
  existing_minimum(document), otherwise.
```

The reachable content includes retained branches/versions/history/redo/recovery and parked,
disabled or bypassed content, not merely the displayed clip. Unreachable scratch alone is
not a persisted feature. Existing minimum is precisely the current MO2 union: Adjustment,
Solid, non-Normal blend or a transition outside crossfade/fade_from_black/fade_from_white
requires 2; otherwise 1 (`crates/kinewright-project/src/project.rs:375`). AW2 and JA-2 require
presence-based checks even when disabled (`docs/AW2-CODE-GENERATED-CLIPS.md:432`,
`docs/JA2-PERSON-SEPARATION.md:379`).

Staged v3 landings need **support admission in addition to the version ceiling**. From S1,
bundle IO validates the complete required field/kind/API inventory, including reachable
Document resources, against what that build actually supports. A planned but unimplemented
v3 kind refuses writes/resume; tolerant deserialization must not discard it. Validate the
inventory against the actual raw fields and graph, so omitting a declaration cannot hide
a resource. These are store-manifest/record validation tags within v3, not another format
counter or minimum-version predicate (`docs/JA1-STUDIO-ORCHESTRATION.md:120`). Recognizing
an AW2/JA-2 tag does not expose a usable clip/effect before its renderer and gates land.

Apply this raw-field/graph-derived required field/kind/API admission to `rescue_copy_v1`,
its declared role/schema and `rescue-copy@1` API. Cross-check the dedicated checkpoint
reference against actual content edges and validate its complete closure. Generic opaque
evidence support is insufficient. Missing/mismatched declarations or unsupported semantics
refuse writes/resume at callers before mutation; no second version predicate.

When production v3 support lands, raise the one core `PROJECT_FORMAT_VERSION` ceiling to 3.
Do not change the recovery-journal envelope version; its writer ceiling becomes 3 and
new operations still receive ordinary replay validation. S0 does not change that constant
(`crates/kinewright-core/src/model.rs:1263`, `docs/AW2-CODE-GENERATED-CLIPS.md:436`).

Untouched legacy v1/v2 documents keep the serializer's existing order, omission, defaults and
minimum. Freeze actual writer output before migration and compare full bytes afterward;
no eager write on open, no new `studio:{}` or empty tables. Do not promise arbitrary input
whitespace preservation on normal reserialization: the existing writer pretty-prints.
The stronger preservation obligation is exact original bytes in the pre-migration backup
and on any refused/unsupported write. Existing byte-corpus tests are pins, not new runs
(`crates/kinewright-project/src/project.rs:406`,
`crates/kinewright-project/src/project.rs:730`, `docs/JA1-STUDIO-ORCHESTRATION.md:124`).

Overwrite admission compares retained read version against **reader ceiling**, never the
current Document minimum (`crates/kinewright-project/src/project.rs:247`). The actual older
reader may reject Code/External parsing, or parse a plain-Document v3 envelope while dropping
Studio fields; in the latter case `can_overwrite_save(3)` is false in this max-2 build.
Test both, including raw original bytes after refusal. Old readers cannot be retrofitted to
preserve unknown graph objects. Their existing lossy Save As is only an edit-only derivative,
never a preserved Studio bundle. New bundle-aware save/Save As refuses unsupported required
kinds; an explicitly labelled derivative is a distinct action
(`docs/AW2-CODE-GENERATED-CLIPS.md:439`, `docs/JA1-STUDIO-ORCHESTRATION.md:126`).

### 3.4 CC8/MO6 collision check; no speculative fields

No Job A field is reserved for HDR metadata, new colour-management settings, Bezier, title
reveal, Sequence/Compound, GraphicTemplate, presets or authored-duration metadata. Existing
linear/ease curves and direct titles suffice for the first cut. The table includes only
Job A's shared records plus AW2's explicitly required Code/External contract.

Later CC8 extends the same minimum predicate with its enumerated feature tests, retaining
default omission and ceiling-based overwrite checks; it does not get a second writer
(`docs/CC8-FLEXIBLE-COLOUR-AND-YOUTUBE-HDR.md:223`,
`docs/CC8-FLEXIBLE-COLOUR-AND-YOUTUBE-HDR.md:245`). Later MO6 extends that same policy when
its actual active-curve/reveal/template features land; dormant sequence/library content
must be traversed then. No MO6 data is smuggled under v2 and no independent reservation of
v3 is made (`target/review/mo/mo6-design-rev3.md:132`,
`target/review/mo/mo6-design-rev3.md:1054`).

Thus the union remains extensible by the one owner, while a released reader never silently
acquires unsupported semantics under its existing ceiling. Version-assignment collisions
are recorded in §12; this draft does not invent future fields or reserve their next number.
MO0's inert tangents stay inert until the accepted MO6 evaluator work, and templates still
use MO3's Sequence representation (`docs/MO0-MOTION-PROGRAMME.md:179`,
`docs/MO0-MOTION-PROGRAMME.md:406`).

### 3.5 Durable take-validation content and exact retention edges

`TakeValidationEvidence` is one narrowly typed referenced record in the existing JA-4
take-evidence family. Its immutable ObjectRef identifies its exact bytes. It contains:

- Owned source/stream and TranscriptSnapshot refs, ScriptRevision/LineId, the complete
  candidate key/WordId path, rational source-frame interval, sample rate/half-open sample
  indices, timestamp coverage and decoder-open identity. Candidate identity is a value;
  the evidence does not point back to its containing CandidatePage.
- Validator/build and validation-profile identities, source/range/profile key, measured
  boundary/clipping results and their measurement basis, and valid/refused/unresolved
  disposition with blockers. Missing timing or measurements remain explicitly unavailable.
- For disputed critical speech, a separate immutable observation member: observation
  identity, observed words/value, exact source interval, named human or certified-audio
  observer, actual evidence capability and any audio-connector certification identity/outcome.
  Preserve original ASR and correction refs separately. Missing observations, uncertified
  audio capability or contradictory evidence leaves
  `critical_speech_unresolved`; a script/Songbird match or transcript correction cannot
  substitute for the observation (`docs/JA4-TAKE-CHOICE.md:188`,
  `docs/JA4-TAKE-CHOICE.md:195`, `docs/JA4-TAKE-CHOICE.md:239`).

JA-4 S2 writes boundary/clipping validation and any available critical-speech observation.
JA-4 S3 writes immutable successors when a new observation or revalidation is supplied,
rechecks eligibility and publishes the choice/map refs through JA-1. A refreshed page is
also an immutable successor; S3 never edits an S2 page in place. Each CandidatePage's
candidate entry retains its exact `validation_evidence_refs`. Every selected choice and
retained alternative in TakeMapRevision retains its `candidate_page_ref` and direct
`validation_evidence_refs`; it never consults the latest profile or mutable cache entry.
An unresolved pin retains its original candidate/page/evidence and blocker, without making
that choice eligible (`docs/JA4-TAKE-CHOICE.md:233`, `docs/JA4-TAKE-CHOICE.md:248`,
`docs/JA4-TAKE-CHOICE.md:457`).

The ordinary JA-1 graph is sufficient: branch/head/history/version → exact TakeMapRevision →
retained CandidatePage and validation record → frozen script/snapshot/source and any owned
observation/proof bytes. Before a map exists, the retained search checkpoint pins its pages
and their evidence. AssemblyBinding's consumer-inventory refs retain the exact evidence
those consumers used. Context keys such as output HeadId, map revision identity and the
assembly-plan/mapping digest are values, not hashes of the containing records that reference
the inventory. Frozen input
and evidence ObjectRefs are retention edges; head/map consumer bindings retain the exact
AssemblyBinding and its inventories. Undo/redo, recovery, Save As and retained versions
traverse these same edges under the existing object/metadata caps; quota exhaustion pauses
admission, never silently drops evidence (`docs/JA1-STUDIO-ORCHESTRATION.md:704`,
`docs/JA4-TAKE-CHOICE.md:243`).
A1's separately consented B-only history trim and omission of unavailable source recovery roots may remove only edges exclusive to those selections; all remaining independent evidence edges stay required. Its bounded rescue-provenance transformation may replace older warning records with a counted unresolved summary, never independent receipt/evidence roots or current-selection omissions. Quota exhaustion alone authorizes no silent loss.

Relink, damage repair, retranscription/correction, script/candidate/range or validation-profile
changes invalidate dependent eligibility and consumer/review keys for the successor. Publish
new evidence refs after validation; preserve the old bytes/refs and unresolved pins for
retained heads. Never retarget an observation to new WordIds or assume equal text/duration
makes it reusable (`docs/JA4-TAKE-CHOICE.md:239`, `docs/JA4-TAKE-CHOICE.md:370`).
The later JA-4 G7/G10 cache-delete/reopen witness is specified in §8.3. No general evidence
framework, new writer or production type is introduced in S0.

## 4. Shared owned-input interface

### 4.1 Owner and typed handoff

Freeze these conceptual boundary types now, without adding production Rust types in S0:

| Boundary value | Content and invariant |
| --- | --- |
| `InputRequest` | Project or unsaved-session identity/owner epoch, job/command, requested content hash/length/kind, source locator hint, dependencies and requested stream/interpretation. A locator never supplies identity. |
| `OwnedInputRef` | Sealed manifest ObjectRef and exact entry/dependency identity. Copying it does not acquire mutation authority. |
| `OwnedInputLease` | Owner-issued live pin plus verified read-only snapshot/root/file handles and epoch. It is runtime authority, never serialized handle/PID state. |
| `DecoderOpenReceipt` | Manifest/entry hash, actual opened object/root/file identity, selected stream/time base/origin/selector/profile and use outcome. Bind the successful decoder instance and subsequent seeks to that leased snapshot. |
| `DerivedResult` | Project/session identity, epoch/job/source/recipe/intent identity, measured bytes, sealed output refs and coverage. A candidate result is not an edit bind. |
| `SuccessorEvidence` | Old and new identities, reason (relink, damage repair, retranscription or deliberate regeneration), dependency invalidations and expected head/bound-resource identity. |

The JA-1 host is the **single writer** of the authoritative manifest/index/root; before
first save, the same project-store primitives have one session owner as frozen in §4.3.
It reserves and copies/clones to exclusive owned storage while hashing, checks requested
digest/length and dependency closure, seals, then lends pinned handles. Named bundle refs
become durable only through B1–B5; §4.3 defines the unsaved-session state and first publication.
Workers receive bounded staging authority only. Media opens from those handles or an
equivalently protected namespace, rather than re-resolving the external pathname. Today SourceSpec still stores and
opens a PathBuf; this interface is new work, not already inherited from PF1
(`crates/kinewright-media/src/render.rs:305`, `docs/JA2-PERSON-SEPARATION.md:149`).

The platform adapter must certify byte stability **through use**, including writes through
aliases or pre-existing mappings. Read-only opens, chmod, an advisory writer lock or hashes
before/after decode do not establish that. If a decoder API requires a name, the name must
resolve within the leased stable snapshot namespace. An unavailable stability mechanism
refuses immutable use. S1 owns admission/publication; decoder-open enforcement must land
before S3 immutable playback/review or JA-2 analysis claims this contract is satisfied
(`docs/JA1-STUDIO-ORCHESTRATION.md:172`, `docs/JA2-PERSON-SEPARATION.md:155`).

### 4.2 Closure and lifecycle

Recordings, fonts, LUTs, SVG/code, supplied data/styles/cards/reference/proof bytes all use
the same owner and dependency pins. Font fallback cannot choose ambient different bytes;
generated output keys include source/bindings/fonts/build/profile. Model/runtime binaries
remain machine-local certified profiles. External additionally needs AW2's two image sets:
source/input and reusable runtime/dependency, with actual Node/browser identity protected
before probe/import/exec and through cleanup. Its SquashFS/fs-verity or read-only VHDX
profiles are AW2 P2c certification, not a prerequisite to import plain camera files and
not permission to build another snapshot system (`docs/AW2-CODE-GENERATED-CLIPS.md:407`,
`docs/AW2-CODE-GENERATED-CLIPS.md:640`, `docs/AW2-CODE-GENERATED-CLIPS.md:690`).

All producers follow **reserve → write → seal → pin → retire**. Reserve measured unique
objects and all simultaneous temp/copy/root/backup overlap before admission. A shared blob
has one physical charge; each live consumer holds a pin. Transfer pins/charges without a
gap or double release. Roots, undo/redo, retained branches/versions, recovery, comments,
claims, plans and running exports retain their dependency closure. Owner death requires
cleanup/reader reconciliation; EOF or missing discovery is not retirement proof
(`docs/JA1-STUDIO-ORCHESTRATION.md:605`, `docs/JA1-STUDIO-ORCHESTRATION.md:704`,
`docs/JA2-PERSON-SEPARATION.md:281`).
A1 rescue preserves the complete closure of its selected history, with only §4.3's disclosed B-only oldest-first trim, and includes A's last acknowledged-root checkpoint by default when one exists in the rescue record's one dedicated recovery-reference slot, with complete closure and reopen/recovery validation, outside the unchanged two-predecessor inventory. Rescue may, with explicit consent, omit unavailable source recovery roots and records them. Record each omitted root's identity, root SHA and unavailability cause as provenance; never omit an available root or any dependency still needed by current state, selected history or independent roots. Evaluate full-history omission alone before offering Undo loss, and offer omission alone if it fits and is self-contained. Recompute closure after omission.

Missing/changed bytes produce damaged/unavailable state and block affected review/resume or
enabled output. Preserve the original identity/evidence. Relink and regeneration create
successors; they cannot mutate a delivered input, retarget a pin or overwrite an artifact.
For mattes, coverage extension C directly shares A's unchanged chunks/entries, gets a new
flat manifest digest, and binds only through a reviewed expected-head/resource rebind.
A completed job cannot attach to “latest” (`docs/JA2-PERSON-SEPARATION.md:273`,
`docs/JA2-PERSON-SEPARATION.md:297`, `docs/JA4-TAKE-CHOICE.md:239`).

Save As freezes/pins the source generation, try-acquires destination authority before any
destination store mutation, copies/verifies the whole retained graph and publishes the
destination root last. New ProjectId, historical origin IDs and paused runs transfer no
live leases/grants/credentials. Source/prior destination remain exact on interrupted copy.
Use JA-1's root/pin collector; no JA-2 or AW2 independent collector
(`docs/JA1-STUDIO-ORCHESTRATION.md:258`, `docs/JA2-PERSON-SEPARATION.md:335`).
A1 rescue uses §4.3's separately consented selected-history trim and unavailable-source-
recovery omission choices, in A1 §4's order, and copies/verifies the recomputed complete
selected closure while preserving prior-destination protection.

### 4.3 Unsaved session and first-save ownership handoff

**Owner/stage:** the project/JA-1 owner implements this lifecycle in JA-1 S1's common store
and bundle-publication primitives; AW2 P1a consumes them for code/font admission. It does
not wait for a scheduler or create an AW2 store lifecycle. AW2 R12 already requires a
session-owned temporary store and copy-first first save; JA-1 anchors named-project
publication to the destination lock and pinned namespace. Today's LUT helper returns no
store for an unsaved project, so it does not already implement this handoff
(`docs/AW2-CODE-GENERATED-CLIPS.md:425`, `docs/JA1-STUDIO-ORCHESTRATION.md:181`,
`docs/JA1-STUDIO-ORCHESTRATION.md:746`,
`crates/kinewright-project/src/project.rs:195`).

For A1, S2 adds the JA-1 `Ja1EditingSession` lifecycle owner to the existing app editing
context, shared by agent/headless entry points. The originating session runs from New/Open
through explicit Close or exit; rescue handoffs and ordinary focus/project switches do not
end it or reset its slots. Closing a destination status view is not session teardown.

An unnamed edit has one session owner with an exclusive temporary root, pinned handles and
session/epoch-scoped authority. It uses the same reserve/write/seal/pin/retire primitives,
validation and object/byte limits as the named store. Bound temporary objects, staging and
undo/redo/live-reader pins within the existing quotas; no separate cache allowance or
collector. In-memory acceptance permits usable edits/undo but makes no durable **Saved**
claim and publishes no named project root. Live handles/epochs are runtime authority, not
portable persisted credentials.

First save freezes/pins the exact session generation, Document, undo/redo/history and full
required dependency closure; pause mutations during the handoff. Resolve the destination
read-only and try-acquire its authority/pinned namespace before any destination copy or
store mutation, including parent creation under the existing pinned-parent claim rule.
Contention refuses without changing the edit or destination. Reserve the simultaneous
source/destination/staging/root/backup overlap before copying (§4.2;
`docs/JA1-STUDIO-ORCHESTRATION.md:260`).
For A1 rescue, exact history and required source closure admit only A1 §4's separately
consented B-only oldest-first trim and unavailable-recovery omission, in its stated order.
Keep the capture frozen through the initial outcome and validated handoff, acquire authority
before any mutation, and copy/verify the complete final selected closure. Prior-destination
protection remains mandatory; unknown B resumes the original dirty edit.

Copy/verify the complete required closure, including code/fonts/render outputs, source
inputs, retained undo/redo and reachable bundle evidence, into destination-owned objects.
Never leave a named root dependent on temporary-store paths. Preserve the prior destination's
reachable graph and verified recovery root. Finish B1–B4 for the copied graph and publish
the destination root last through B5. Only verified durable success can acknowledge Saved
and transfer the active named-project writer association to the destination authority.
Do not transfer serialized live leases; issue destination-bound authority for subsequent
work (`docs/JA1-STUDIO-ORCHESTRATION.md:267`, `docs/JA1-STUDIO-ORCHESTRATION.md:287`).
Only immediately verified A1 success with validated handoff seals A's actor and associates editing with B's fresh authority/Core. Unknown B keeps the original dirty edit; later B commit is a capture copy and never changes active editing. A's authority/Core and pending state remain session-owned; no A authority transfers. Reconciliation of a rescue-retained destination settles its receipt/pending state and may only settle that same attempt's own ticket on the live, unsealed Core through A1 §5's S1 exact-ticket path, preserving content, post-floor Undo and Redo. No other floor, restore, install or history replacement on any Core is allowed; sealed A's ticket stays until session end. A separate B open is read-only while its owner is retained; an editable open requires explicit release of B's completed destination owner after proven commit.

Retain source leases, pins and physical charges until the handoff succeeds **and** every
reader still using the temporary store retires. A live proof/export may finish on its exact
source handles; it cannot publish into a new epoch by relabelling its result. Transfer
ownership/accounting without a missing pin, duplicate charge for the same physical object
or early deletion; charge distinct simultaneous copies. Retire temporary bytes exactly once
through the same owner after the last reader, not merely after root publication.
A1 overrides this ordinary retirement: the same `Ja1EditingSession` owner retains all
rescue source holds, pins, charges and temporary-store bytes until originating-session end,
even through handoffs, switches, reconciliation and last-reader retirement. Four slots limit
rescue-retained attempts only; ordinary saves retain ordinary S0 rules. Previews, re-probes
and cancel consume nothing. After the second probe and all required explicit consent,
source adoption and source-slot consumption are atomic with the first B mutation. Explicit
release of proven-committed B's completed destination owner permits a separate editable open
without retiring rescue source holds or refunding consumed slots.

A refused or failed copy/verification/prepublication first save preserves the usable unsaved
edit, undo/redo, reader handles and source pins; any prior destination root/assets remain
intact. Release only this attempt's destination claims/staging when safe; retry from the
same usable session. If replacement or its barrier has an uncertain outcome at A, classify it as
`durability_unknown` and retain both authorities/pins. Reconcile A's exact root/receipt
and required durability before acknowledging A, writing there again, or another ordinary
save/copy attempt from that unresolved owner. An uncertain publication is not a reported
failed save or proof of rollback (`docs/JA1-STUDIO-ORCHESTRATION.md:334`).

Exception A1: after an eligible failed reconciliation and a fresh selection-time probe,
the person may explicitly choose **Save rescue copy** to fresh, independent B. Writes require
btrfs, ext4, or XFS; Windows writes stay refused. First-save sources remain in the local
temporary store. Opened-A rescue requires every selected owned byte to remain readable
through retained handles/local state and hash-verified. Otherwise return A1 §3's missing-object
refusal. B must be separate by native path
and pinned namespace identity from every retained destination and the session temporary
store. B receives a fresh ProjectId, fresh destination authority, and a genuinely separate
Core actor. Historical origins remain; A's authority never transfers to B.

Freeze the accepted live edit and all required history/input/lineage/evidence roots at one
cut. Full history, a checkpoint of A's last acknowledged root when one exists, and all
selected source recovery roots are the default. A1 §4 first evaluates whether omission
alone makes B fit and be self-contained; if so, offer it without an Undo-trim preview.
Only otherwise offer an eligible oldest-first history trim, alone or with separately
consented omission of exactly unavailable source recovery roots, recorded by identity,
root SHA and cause as provenance. Available recovery roots and all objects still needed by current state, selected
history or independent roots remain required. Recompute, copy and verify the complete
closure of the final selected roots, including LUTs, before B5-last publication. R_ack is
one dedicated recovery reference in the durably pinned rescue record, with its complete
content closure in B; it never occupies a third automatic-predecessor entry. B validates
and offers this recoverable root on reopen (§4). A durable, root-reachable rescue record binds
the full capture, B's selected result and disclosed loss, and A's unknown publication and
pending-command identities. B has no graph edge to A or to temporary-store paths.

Only a rescue verified durably successful at the time of its initial outcome can switch
editing to B's fresh authority/Core. Seal A's actor against all editing handles at that
handoff; editing and ordinary saves then continue in B. If B returns unknown, resume the
original dirty edit associated with A. A later B commit only records/shows a rescue copy as
of its capture; it never switches the active edit, even if histories happen to match.
The originating editing session's JA-1 owner retains A's original owner/Core, attempt/tag
and settlement status, authorities, source leases, pins, physical charges, and all source
temporary-store bytes until that session ends, even after reader retirement or independent
A reconciliation. Only §5 may settle the live attempt's own ticket/barrier; sealed A's
ticket/barrier stays until session end.
A stays independently reconcilable; B's success never settles A. Reconciliation of any
rescue-retained destination settles its own receipt/pending state. Its sole permitted Core
operation is S1 exact-ticket settlement of that same attempt's ticket on the live, unsealed
Core, after its own destination proof (§5). Commit replays current edits and preserves
content, post-floor Undo and Redo. Sealed A's ticket stays until session end. No other floor,
restore, install or history replacement is allowed on any Core. Report the destination
contents while preserving current edits, dirty/Saved baseline and writer association.
Cancel, failure, or an uncertain B changes none of
A's Core, ticket, floor, histories, or holds.

An unknown publication at B gets its own tag, Core state, and independent holds, with no
Saved message or destination writer exposed. Explicit rescue to C may follow under the
same rules. Four rescue-retained uncertain-publication slots belong to the originating
editing session (§5). Previews and admission use provisional reservations only. After the
second probe and all required explicit consent, atomically adopt A's unknown attempt into
rescue retention and consume its source slot when the first B mutation begins. Preview,
re-probe, cancel and any abort before mutation consume nothing. Reserve the destination
slot at that boundary; it is consumed only if the rescue-destination outcome is unknown.
Verified rescue success and definite refusal return the destination reservation.
Ordinary saves never reserve or consume a rescue slot; a later unknown ordinary attempt
counts only when adopted as the source of a new explicit rescue. Consumed slots remain
consumed until session end, even after settlement. Existing byte/object/metadata and
cleanup quotas still apply. No hold is evicted to admit another attempt.

**Binding A1 R_ack retention ruling.** R_ack retention is bounded at one level: each rescue
record carries only its own R_ack as a pinned recoverable root. When a later rescue starts
from a session whose active project is itself a rescue copy, that project's own R_ack
becomes provenance only (identity, root SHA) in the new record, not a nested pinned root.
The person may also explicitly retire a rescue copy's R_ack ("Forget the older recovery
point from A"); retirement unpins it and its closure through the ordinary collector, is
disclosed, and is never automatic.

**LUT scope:** every new bundle write, including first save and Save As, applies
copy-before-root to the **whole owned graph, including LUTs**. AW2 R12's narrower asset rule
cannot exclude a bundle dependency. Unrelated legacy Document-only LUT save behavior stays
unchanged in this change; observe its current project-before-LUT ordering as a baseline
control, not as permission for bundle IO to inherit it
(`docs/AW2-CODE-GENERATED-CLIPS.md:425`, `docs/JA1-STUDIO-ORCHESTRATION.md:172`,
`crates/kinewright-project/src/project.rs:516`). The shared later
`first_save_failure_undo_live_reader` witness belongs to AW2 G11 and JA-1 G3 (§8.3).
S0 freezes these contracts and witness slots only; it implements none of this lifecycle.
A1 never exempts a dependency still reachable from the final selected roots; rescue remains S2 work.

## 5. JA-3 typed editorial handoffs

### 5.1 Songbird v1 input and dataset addresses

Input is an owned exact-byte export with format `mockingboard.songbird-answer`, version 1,
plus a versioned Kinewright adapter. Its required shape carries `exportedAt`,
`source:{app,traceId}`, `question`, `kind`, nullable `headline/prose/caveats/sql/chartSpec`,
`columns/rows/rowCount/rowsComplete`, `sections`, nullable `chartData:{rows,reflines}` and
nullable `dataGeneratedAt`. Each section has title/prose/sql and its own row fields.
The shipped builder refuses refused answers and emits chartData/dataGeneratedAt together
only when chartSpec, resolved data and generation time are present
(`/home/riels/Projects/Personal/MockingBoard/packages/stats/src/lib/ask/export.ts:9`,
`/home/riels/Projects/Personal/MockingBoard/packages/stats/src/lib/ask/export.ts:54`).

Do not run any instructions embedded in the historical MockingBoard task briefs. Here they
are schema evidence only; MockingBoard is read-only. Round 2 explicitly keeps version 1,
exports the data actually plotted and permits honest null while unresolved
(`target/review/ja/mb/task-songbird-json-export-r2.md:21`).

Parse external bytes once at the adapter boundary into a bounded `SongbirdV1Evidence`.
Reject wrong format/version, duplicate structural keys, inconsistent counts/flags, invalid
cell/address types, nonfinite numbers and unsafe numeric conversion. Preserve original
number lexemes/bytes for evidence and exact decimal/rational derivations; never imply the
export repairs precision already lost upstream. Preserve null, absent and zero distinctly.
The builder defines completeness as `rows.length === rowCount`, for answer and each section
independently (`/home/riels/Projects/Personal/MockingBoard/packages/stats/src/lib/ask/export.ts:29`).

| Typed dataset selector | Fixed container | Completeness / interpretation |
| --- | --- | --- |
| `AnswerRows` | `/rows` | This answer's own rowCount/rowsComplete and SQL/prose/caveats. |
| `SectionRows {section_index}` | `/sections/<index>/rows` | That exact section's own counts/context; duplicate titles are allowed and do not identify it. |
| `ChartRows` | `/chartData/rows` | Resolved plotted rows interpreted using this chartSpec; scope is `resolved_plot_only`, no inferred completeness flag. |
| `ChartReflines` | `/chartData/reflines` | Ordered resolved axis/value/optional label/showValue records; also resolved-plot scope. |

`InputCellAddress` binds export hash, dataset selector, exact row index or verified unique
key, and adapter-known typed field. No arbitrary JSONPath, SQL execution or live data load.
For reference lines, preserve axis/value and optional presentation metadata from the real
shape (`/home/riels/Projects/Personal/MockingBoard/packages/stats/src/lib/chart-spec/resolve.ts:31`).
Export time is not data-generation time; trace ID is not content identity.

The chart consumes only chartData rows/reflines, not answer/section rows as a substitute.
Null chartData blocks the affected chart render, while separately supported cell claims
may proceed. Incomplete rows can support a present-cell claim or labelled finite subset;
whole-result claims requiring omitted rows refuse. Complete SQL results do not certify an
unbounded population. Preserve filters, limits, units, caveats and scope on visible output
(`docs/JA1-STUDIO-ORCHESTRATION.md:544`, `docs/JA1-STUDIO-ORCHESTRATION.md:563`).

### 5.2 Claim and output addresses

JA-3 supplies a complete `FactualOutputInventory`; JA-1 owns ClaimBinding and independent
validation/admission. **Persist this inventory** as a bounded member of the existing
consumer-inventory family in §3.2, referenced by the exact AssemblyBinding and retained with
the applicable head/map/version. It records its output HeadId/definition identity, full typed
slot list, frozen export/adapter/dataset/scope/derivation keys, ScriptRevision refs, TakeMap
identity and validation-evidence refs, and chart/code/style/profile identities. JA-3 first
writes it at JA-4 S5 before JA-1 S5 admission. It is not reconstructed from live data or the latest edit;
missing/stale inventory blocks the affected admission until a successor is published
(`docs/JA4-TAKE-CHOICE.md:237`, `docs/JA4-TAKE-CHOICE.md:370`,
`docs/JA4-TAKE-CHOICE.md:460`). An `OutputFieldAddress` binds exact output HeadId/definition hash,
head-scoped occurrence, and either a registered title/caption/effect property with explicit
static/stored-key address, or an AW2 CodeAsset identity plus declared typed parameter key.
Future nested addresses use MO3/MO6 relative sequence/clip/effect identities; this flat cut
does not invent a second addressing language (`target/review/mo/mo6-design-rev3.md:350`).

Inventory every factual label, tick/category, qualifier, reference value and data/geometry
parameter. Generate from bound inputs, then independently read the actual addressed output
and compare value **and** text with expected derivation/units/rounding. An intact export
hash cannot excuse a modified label or bar geometry. Unsupported chartSpec features refuse
rather than silently disappear; actual feature inventory awaits supplied exports
(`target/review/ja/ja3-editorial-scoping-rev1.md:191`,
`docs/JA1-STUDIO-ORCHESTRATION.md:576`).

Claim validation binds export/adapter/dataset/cells/count/scope/context/chart interpretation,
finite derivation, units/rounding, script/style and exact addressed output. Any relevant
change invalidates it. Invalid/unbound factual fields or unsupported script claims cause
**zero affected preview-proof, review and final render starts**. Already validated delivered
playback remains valid; working placeholders cannot acquire a review receipt. Semantic
script claims require source-linked lead/independent review, not a universal prose truth
detector (`docs/JA1-STUDIO-ORCHESTRATION.md:583`).

### 5.3 Script/take-map, assembly and consumer keys

Freeze `EditorialSelection` as `(ScriptRevisionRef, TakeMapRevisionRef, AssemblyBindingRef,
exact branch/HeadId, owned transcript/source refs, exact TakeValidationEvidence/profile
refs)`. Consumers never select “latest map”. Its ordered items carry LineId/BeatId, candidate/WordIds, exact
half-open source stream/frame/sample intervals, applied project segments, pauses/links,
occurrences, pin status and unresolved evidence. Captions consume **retained** words and
eligible speech; cues are nonspoken. Script agreement cannot establish recorded wording
(`docs/JA4-TAKE-CHOICE.md:224`, `docs/JA4-TAKE-CHOICE.md:370`).

The existing assembly owner supplies one typed cleanup result preserving today's ordered
`selections` and kept-source ranges, including repeated asset occurrences. It incorporates
required-word/PauseIntent protection after merged cuts. JA-3 and the paired planner consume
that one result, not independent silence/filler computations. Current assembly builds one
track's operations from already supplied envelopes; it is a successful baseline control,
not existing take discovery or two-asset synchronization
(`crates/kinewright-agent/src/server.rs:8564`, `crates/kinewright-agent/src/server.rs:8644`,
`docs/JA4-TAKE-CHOICE.md:344`).

RecordingPair admission retains selected stream indices, rational clocks, actual decoder
origins/coverage, separate capture A/V observation and every uncertainty component. Use the
accepted conversion `c_v(m)=a+b*m+(O_a-O_v)+delta_AV`, then actual decoder-grid rounding.
Unknown is not zero. Offset-only segments must satisfy the full conservative error bound
≤one project frame, including interior evidence/K/h, decoder/clock and cumulative rounding.
Refuse discontinuity/unverified coverage before paired commit; one slate or PCM length does
not establish it (`docs/JA4-TAKE-CHOICE.md:261`, `docs/JA4-TAKE-CHOICE.md:297`,
`docs/JA4-TAKE-CHOICE.md:309`).

Prepare linked C920 video-only and QuadCast audio-only routes together, scratch unrouted.
Round shared cumulative project boundaries once; one ordinary atomic plan/undo/receipt
contains both routes, map and consumer bindings. Take replacement preserves unaffected
choices/occurrences, shifts dependent placements once and invalidates captions, facts and
output review even at equal duration. Fact consumers separately retain script, ASR, actual
sound observations and selected data; disputed critical speech without source-bound human
or certified-audio evidence blocks the affected render
(`docs/JA4-TAKE-CHOICE.md:195`, `docs/JA4-TAKE-CHOICE.md:360`).

### 5.4 One shipped audio profile and existing meter

Use the existing streaming/social target for master/reels: **−14 LUFS integrated ±1 LU;
true peak ≤−1 dBTP**. Read the profile from `DeliveryProfile::loudness_target`, with
`STREAMING_PLATFORM_TARGET`; do not hardcode a rival −16 target
(`crates/kinewright-core/src/delivery.rs:194`, `crates/kinewright-core/src/delivery.rs:233`).
The same identity drives planning, export normalization and acceptance. Export normalization
remains an explicit job setting, not an automatic Document edit. Reuse the existing decoded
export loudness/true-peak report; successful export alone is not conformance or Delivered
(`crates/kinewright-agent/src/server.rs:13866`,
`docs/JA1-STUDIO-ORCHESTRATION.md:635`).

`AudioIntentInventory` carries selected speech/cut/pause/room-tone intervals and profile to
JA-1's whole-output checks. **Persist it** as a bounded member of the same consumer-inventory
family in §3.2, referenced by the exact AssemblyBinding and retained with the head/map/version.
Its frozen keys name the output head, ScriptRevision/TakeMap/AssemblyBinding, owned sources
and selected streams, retained WordIds/applied segments, PauseIntent/room-tone evidence,
TakeValidationEvidence refs, delivery profile and explicit export-normalization/settings
identity. JA-3 first writes it at JA-4 S5 before JA-1 S5 checks; it is not a reconstruction
from current analysis caches. Missing/stale intent prevents sealing until an explicit
successor is published. Neither inventory adds an independent durable kind, index or writer;
both use the consumer-inventory retention edges in §3.5
(`docs/JA4-TAKE-CHOICE.md:237`, `docs/JA4-TAKE-CHOICE.md:367`,
`docs/JA1-STUDIO-ORCHESTRATION.md:624`). It does not implement new DSP or a duplicate meter.
Click, dropout/gap scans and calibrated thresholds are later S5 work; missing required measurements
block sealing. Model listening stays optional, while unresolved disputed critical speech
retains JA-4's separate evidence requirement. No listening certification is claimed here.

### 5.5 JA-2 geometry for cards/captions/reframes

Freeze a paged read-only `PersonGeometryInspection` boundary returning source/profile/recipe
identity, exact source-picture/PTS/frame/raster/orientation, face box and five landmarks,
validity/unreliable intervals and measurement coverage. The intended detector samples at
5 Hz, interpolates only verified short spans and rechecks difficult intervals at source
rate. It supplies geometry, not gaze/eyes-open/identity or model-confidence claims
(`docs/JA2-PERSON-SEPARATION.md:487`, `docs/JA4-TAKE-CHOICE.md:216`).

JA-3 maps geometry through AssemblyBinding's source→project segments and the production
crop/reframe/host transform at that exact output frame, preserving coordinate basis and
visibility. Card/caption occupancy combines those mapped boxes with actual title bounds,
card rectangles and chart-label inventory. Invalid/multiple-face/long-unverified spans use
the wider or explicitly reviewed static layout; they do not fabricate a track or add a
second detector. An impossible layout recomposes or blocks; no required caption vanishes
(`docs/JA2-PERSON-SEPARATION.md:462`,
`target/review/ja/ja3-editorial-scoping-rev1.md:135`,
`target/review/ja/ja3-editorial-scoping-rev1.md:280`).

Raw geometry is source-bound before creative nodes. Face-look measurements separately name
`host_post_color_pre_legacy`, the recovered F branch, colour context and valid patch counts;
do not confuse a geometric box with a measurement stage or CC5's colour matte. Inspection
obeys JA-2's bounded pages/proof identities; no face crops become durable model input
(`docs/JA2-PERSON-SEPARATION.md:493`, `docs/JA2-PERSON-SEPARATION.md:606`).

## 6. Agent and app surfaces frozen by S0

The harness drives the real compact server and existing prepared-plan/confirmation path.
Future production actions remain registry capabilities behind the compact tools, scoped at
prepare, actual commit, replay and merge; fixture JSON never becomes an unguarded production
HTTP/file API. Scripts, source prose and tool results remain data. Existing destructive
confirmation is a separate regression (`docs/JA1-STUDIO-ORCHESTRATION.md:488`).

App observers distinguish selected/requested identity, Core-accepted pending state, applied
PF1 stamp, actually bound DisplayCell and paint acknowledgment. Today's DisplayCell contains
stamp/frame/frame_id/stale, with no VersionId (`crates/kinewright-app/src/presenter.rs:13`).
The harness must record that absence rather than synthesize a version from selection.
S0 adds no Studio layout, feedback capture, lease controls or “Saved”/“Delivered” behavior.

## 7. Implementable S0 baseline harness

### 7.1 Placement and compilation boundary

Use **the existing `kinewright-agent` eval-harness boundary**, with no new workspace crate:

| Proposed placement | Responsibility |
| --- | --- |
| `crates/kinewright-agent/src/ja_harness/{mod,protocol,baseline,controller,recording,report}.rs` | Harness-only fixture/wire records, current-API adapters, participant controller, recording session wrapper and observations/assertions. Gated by existing `eval-harness`. |
| `crates/kinewright-agent/src/bin/kinewright-ja-baseline.rs` | Thin native CLI with `required-features=["eval-harness"]`; controller and worker roles, selected by fixed arguments, not shell snippets. |
| `crates/kinewright-agent/tests/fixtures/ja/` | Versioned fixture/action descriptors, independently labelled oracle refs and hash-slot inventory. No private practice media or model weights in Git. |
| `crates/kinewright-app/src/ja_harness.rs`, included only under `cfg(test)` | App-owned adapters for private presenter/App/GUI-journal seams and one exact test entry that reads a fixture path and writes the common report protocol. |
| Existing core/project/media test-util boundaries | Narrow read-only trace/barrier adapters beside private state when current APIs cannot expose an observation. No new restore, receipt, lock or decoder semantics. |

Agent already owns eval infrastructure and depends on core/media/project under this feature;
the shipping app uses agent with default features off
(`crates/kinewright-agent/Cargo.toml:9`, `crates/kinewright-agent/src/lib.rs:16`,
`crates/kinewright-app/Cargo.toml:20`). Enable eval-harness only in the app's dev dependency
for shared wire/report access, and project/test-util only for harness builds. Do not make
agent depend on app or lower crates depend on agent. App-private tests stay in the binary
crate; it currently has no exported App library (`crates/kinewright-app/src/main.rs:1`).

Protocol records may use strings, enums, bounded JSON payloads and ObjectRefs; they must
compile at the pinned predecessor. Current operation payloads use existing serde types.
Future bundles/receipts/maps/leases are opaque fixture bytes and expected fields,
never imports of future production Rust types. Revised mode later supplies a different
adapter using the same protocol; it must not change fixtures/oracles to manufacture green.
The harness cannot implement a toy host, receipt deduplicator, matcher, map persistence,
decoder snapshot, fact validator or delivery gate on behalf of missing production behavior
(`docs/JA1-STUDIO-ORCHESTRATION.md:725`).

### 7.2 Fixture/action JSON protocol, version 1

The fixture is one UTF-8 JSON object, duplicate keys rejected. Every run freezes its exact
bytes/hash plus referenced input/oracle hashes. Proposed field names below define the S0
wire contract, not production records:

| Field | Shape / validation |
| --- | --- |
| `protocol` / `protocol_version` | Literal `kinewright.ja-harness` / integer 1; unknown versions refuse before actions. |
| `fixture_id` / `gate_ids` | Stable bounded label and qualified gate list, e.g. `JA1.G2`; gate names never imply success. |
| `lane` / `requirements` | `portable`, `media`, `app-paint`, `linux-native`, `windows-native`, `provider` or `practice`; explicit binary/OS/media/device/connector prerequisites. |
| `inputs` | Map of role→`{relative_file,sha256,byte_length,kind}`; paths resolve beneath the fixture root, never caller project roots. Pending hash slots are catalog-only and make a run NOT RUN. |
| `initial` | Existing project/Document fixture ref, runtime seed revision and participant descriptors; any future record fixture is an opaque input ref. |
| `actions` | Ordered list of `{id,participant,action,args,after?}`; strictly increasing action IDs; bounded args, typed per-action validation. |
| `barriers` | Named observation points and explicitly ordered release/death/reply-loss events with prerequisite positive-control IDs; not time-based race guesses. |
| `oracles` | Independent expected-byte/path/landmark/field refs and positive/negative assertion IDs, each qualified by evidence class. |
| `limits` | Declared input/output/control quotas and setup/action deadlines within harness ceilings; overflow reports incomplete/NOT RUN, never truncates evidence into pass. |

Runtime selection is a CLI `--mode baseline|revised`; mode is recorded in the run header and
does not change fixture hash. Baseline runs only recognized current actions. The action
catalog and result semantics are:

| Action family | Baseline execution / observation |
| --- | --- |
| `core_start`, `core_send`, `core_query`, `branch_create`, `branch_compare`, `branch_merge`, `branch_cherry_pick` | Real Core / TimelineBranch. `core_send` carries referenced current JournalCommand JSON, converted using its existing `From` implementation. Optional expected-revision guards apply only to Do/DoBatch via current guarded commands; reject guarded Undo/Redo/coalesced payloads as invalid fixture actions. Query Snapshot/AppliedOperations/OpLog; record exact results/errors. |
| `project_load`, `project_serialize`, `project_save`, `project_reopen`, `headless_save`, `lock_acquire`, `lock_verify`, `lock_release` | Actual shared project APIs and held lock handles. Record full file bytes/asset inventory before/after; never add harness locking that would repair the baseline. |
| `server_start`, `scripted_session`, `prepared_commit`, `export_observe` | Actual loopback McpServer, ScriptedDriver/pump, existing confirmation broker, tools/registry and export jobs; retain exact request/result/event traces. |
| `media_seek`, `media_proof`, `media_export`, `media_decode`, `media_stats` | Actual engine/shared render/export/decode paths on supplied controls. Retain requested/returned stamps, raw landmark bytes, PCM/report refs and resource counters. |
| `app_fixture` | Invoke app test adapter through its built test executable; drive actual coalesced App input/finalization/paint path or GUI journal path. Report what was bound/painted; headless simulation alone is labelled as such. |
| `barrier_wait`, `barrier_release`, `drop_reply`, `participant_exit`, `participant_kill`, `inspect_files`, `assert_observed` | Controller-only scheduling, transport reply suppression, process lifecycle and read-only evidence checks. Kill only recorded fixture-owned identities, never bare-name process matching. |
| Later-domain action | Baseline reports `surface_absent` with the successful current control's evidence refs. It does not claim a runtime red unless an actual asserted missing property was observed through a compatible seam. |

Current Core exposes Snapshot/AppliedOperations/OpLog and accepted `JournalCommand` events
(`crates/kinewright-core/src/actor.rs:82`, `crates/kinewright-core/src/actor.rs:107`).
JournalCommand is serializable and already converts into Command; Command itself is not
a JSON protocol (`crates/kinewright-core/src/journal.rs:13`,
`crates/kinewright-core/src/journal.rs:36`).
It does not expose the full private history entries/cursor/pre-gesture state
(`crates/kinewright-core/src/actor.rs:250`). Add a test-util read-only observation queried
at the actor boundary, carrying current undo/redo order, entry operations, baseline bytes
and coalesce keys. This is an observational seam, not S1's future history export/restore.

Use JSON Lines for controller/worker control and event streams: one complete bounded record
per line, explicit sequence/participant/action IDs, stdout reserved for protocol, stderr
captured separately. A control record is at most 256 KiB, at most 64 admitted actions per
fixture, and report indexes stay within 64 MiB; large bytes are streamed into quota-bound
artifacts, never inline base64 video/PCM. A larger suite is separate fixtures. These are
S0 harness admission choices based on JA-1 ingress/control bounds, not measured RSS claims
(`docs/JA1-STUDIO-ORCHESTRATION.md:687`). Exact i64 frame/sample values and rational
numerator/denominator pairs must round-trip without floating conversion.

CLI entry is `kinewright-ja-baseline --mode baseline --fixture <file> --output <directory>`;
the controller launches the same binary with its fixed `--role worker` protocol. App work
uses the built app test executable's exact entry
`ja_harness::tests::run_fixture_from_env`, with `JA_HARNESS_FIXTURE` and
`JA_HARNESS_OUTPUT` pointing into the controller's private tree. These are proposed S0
entrypoints, not commands or endpoints already present at HEAD.

### 7.3 Two-process controller and recording ScriptedDriver

For races, the native controller launches two real fixture participants A/B, records their
PID plus OS start identity/executable hash and establishes named pipes/control barriers.
The controller itself is the test supervisor, not a production host. It starts no provider
until that lane explicitly requires one. Participants acknowledge actual lock/actor/IO
points before a swap, kill or release. Existing lock tests already launch claimants, and
`REV2_HOOK` is test/test-util-only (`crates/kinewright-project/src/lockfile.rs:1225`,
`crates/kinewright-project/src/lib.rs:40`). Each fixture gets a private working/store tree;
no action mutates the real checkout or another project.

Record exact child exit/cleanup and reaping; absence of IPC is not zero descendants. The
controller can drop an outward result after an actual accepted edit/save, then issue the
original action again and observe undo/file effects. It must not cache a successful reply
or deduplicate that action itself. Pre-host baseline restart is real process restart/load,
not a fake host receipt API. Multi-client leases and guardian containment stay pending.

Use the existing ScriptedDriver, which makes real rmcp calls over its own loopback client
and needs no account (`crates/kinewright-agent/src/scripted.rs:1`,
`crates/kinewright-agent/src/scripted.rs:217`). A harness-owned recording wrapper forwards
AgentDriver/AgentSession calls unchanged and records messages/events/interrupts alongside
server request/result counters. SessionObserver records pump decisions. ScriptedCost's
optional categories remain optional; no missing cost becomes zero. Do not create a fake
model implementation of future orchestration (`crates/kinewright-agent/src/scripted.rs:53`,
`crates/kinewright-agent/src/session.rs:314`, `crates/kinewright-agent/src/session.rs:398`).

### 7.4 Observed outputs and result classes

Emit `run.json`, ordered `events.jsonl`, quota-bound artifact files and `summary.json`.
The run index binds fixture/oracle/input hashes, source HEAD, dirty-diff identity if any,
binary/test-executable SHA-256, compiler/profile, OS/driver/FFmpeg identity, mode, lane,
requirements, time bounds, controller and participant identities, and command lines with
secrets removed. Existing app/media test traces map to this same output schema.

| Observation | Required content |
| --- | --- |
| Document/files/graph | Exact serialized bytes and reopened state; every affected source/destination file's hash/length; directory names and actual graph refs where available. Missing graph is explicit. |
| History | Accepted JournalCommands, runtime revisions, applied ops versus append-only log, observed entry/pre-baseline/coalescing/cursor/redo trace and undo/redo snapshots. Missing private-state observation is pending, not inferred from counts. |
| Side effects | Actual accepted-operation, undo-entry, file-write/copy/rename/sync, render-start, bind, process-spawn/exit and pin/charge counters **where instrumented**; unobserved values are null/unavailable, never invented zero. |
| Presentation | Input/request timestamps, issued/applied FrameStamp, bound DisplayCell, paint mark/ack time and actual raw-frame landmark identity; durable version/input association absent on baseline. |
| Session/process | Exact scripted tool requests/results, events/usage/stop reason, process tree identities/lifecycle and barriers; provider certification remains separate. |
| Audio/review | Existing export job plus decoded PCM/loudness/true peak and settings/profile identity; output-frame proof refs. Structural QA, sampled imagery and full-output coverage remain different evidence classes. |

`pass_control` means a named existing behavior passed. `baseline_red` requires a successful
positive control followed by a literal failing property assertion with retained observations.
`pending_baseline_witness` means a required seam cannot yet observe the property. `not_run`
names missing prerequisites/setup failure. `mutation_red` and `restored_green` are later
separate runs with exact source/mutation identity. A compiler error, unsupported response,
missing binary, setup timeout or unavailable OS/provider/device is never a behavioral red.
Keep AW2 capability-gap observations separate under §12 item 5; they earn no JA-1
`baseline_red` or invariant-closure credit. Unsupported capability alone is `surface_absent`
with a pending baseline witness, even when AW2 calls it a capability-open red.
Exit 0 means all **selected control assertions** passed; the summary must still enumerate
pending/not-run gates. Exit 1 means an exercised property assertion failed; exit 2 means a
selected lane could not run or its protocol/setup failed; exit 3 means selected work has
only a pending baseline witness. `summary.json` carries protocol/version, fixture/run IDs,
mode/lane, positive-control results, per-gate disposition/reason, assertion IDs,
observation/artifact refs and missing requirements. No aggregate exit code certifies Job A
(`docs/JA1-STUDIO-ORCHESTRATION.md:727`, `docs/JA4-TAKE-CHOICE.md:467`).

## 8. G-gate observer allocation and exact regression pins

### 8.1 JA-1 gates: S0 observation versus pending witnesses

Every gate receives a fixture slot and expected E/P/B/M definition. “Observer at S0” below
means a current-path control/trace can be implemented; it does **not** say the new gate
passes or a red was run. All runtime evidence is NOT RUN in this draft. Qualified IDs
avoid confusing JA1.G14 usage with PF1 G14 held-age evidence.

| JA-1 gate | Compatible observer to land in S0 | Baseline witness still pending / later repetition |
| --- | --- | --- |
| G1 / G1-W graph durability | Real changed-Document save/reopen, byte inventory and test-only persistence-call trace/rename-refusal seam | Trace absence/fallback can yield a runtime assertion after control. Full graph, dropped-unflushed-write model and certified native Windows storage-crash lane remain pending S1; SIGKILL proves only process death. |
| G1-R receipts | Accepted edit, suppressed result, retry; count exact undo/history effects and saved bytes | Edit missing-dedup property is reachable once observed. Durable comment receipt and branch-switch cases need actual comment/S1/S2 seams; no synthetic receipt service. |
| G2 history/base | Current branch/undo/redo/coalesced edit, private read-only history trace, save/reopen at changed runtime revision | Demonstrate document-only history loss. Durable HeadId/base/floor and shared recovery-close semantics await S1/S2. |
| G3 migration/Save As | Legacy writer corpus, copy order/fault trace, two participant destination contention and exact asset inventories | Studio graph migration/backup/unknown-kind publication and §8.3's `first_save_failure_undo_live_reader` remain pending JA-1 S1/AW2 P1a. Existing legacy LUT behavior is observed, not fixed in S0; new bundle writes include LUTs in copy-before-root. |
| G4 writer/AW1 | Real alias lock/save participants, existing hook barriers, App-owned journal trace/control | Post-verify pinned-root use and real GUI post-scan journal exclusion need minimal observers before claimed red; full S4/S5 obligations remain S2. |
| G5-L / G5-A manual/adoption | Existing coalesced App→Core/presentation and two diverging branches/history controls | No lease/history bridge, shared floor/recovery barrier or adoption seam today; baseline witnesses pending those minimal observable boundaries. No host emulation. |
| G6 containment/resume | Record current session ownership/direct-child lifecycle; native fixture child/grandchild positive control | Actual production guardian pre-exec/host-death/guardian-death/cleanup/no-overlap/resume witnesses pending S4 integration and OS prerequisites. |
| G7-S / G7-D versions | Save/load/select two distinguishable ordinary Documents/owned-source control refs; record missing immutable index | Full frozen-input snapshot selection awaits real lease/index seam; Delivered seal refusal needs S5 coverage/claims/host. |
| G8 displayed A/B | Existing finalization/paint-marker and delayed-frame tests, one-player observations | Durable version/input/LUT association observer pending S3. Never label requested identity as displayed identity. |
| G9 drawn/typed batches | Current displayed stamp and independent letterbox landmark control | Version-bound comment store/capture/ack baseline seam pending; no fake comment persistence. |
| G10 feedback lineage | Core delete/recreate/split/undo and allocator ID-reuse demonstration | Occurrence-safe resolution/temporal-spatial status observer pending S1/S3 records. |
| G11 voice | Existing Whisper/source transcription control, model-installed lane only | Anchored note capture/cancel/publication observer pending S3; missing model is NOT RUN. |
| G12 specialist scope | Real server/ScriptedDriver allowed edit and forbidden-attempt traces; preserve confirmation/investigator controls | Role-bound footprint policy and certified real connector ambient-tool restrictions pending S4. Existing investigator denial alone is not G12. |
| G13 serial merge | Current TimelineBranch compare/merge and applied-op traces | Durable base and actual freeze/drain/task-completion race observer pending S1/S4. |
| G14 usage/limits | ScriptedCost/pump two-session events, missing categories and replay input traces | Persistent multi-attempt run accounting/limits/rate-wait/no-fallback witness pending S4; recorded events do not implement it. |
| G15 admission/storage | Current queue ingress, file/byte inventory, lifecycle and existing PF1 counters where exposed | Graph/receipt/pending-history/recovery inventory, host execution claim, exact slot/pin retirement and decoder-open integrity observer pending owning production seam S1/S4. |
| G16 output review | Real clean export→decode, current structural QA and existing loudness report | New whole-output seeded-defect checkers, certified reviewer modalities and seal observer pending S5. A successfully rendered defect is only input evidence until checker runs. |
| G17 facts | Opaque Songbird/expected-slot fixtures and real ordinary title/solid render-start control | Versioned adapter, AW2/MO6 actual-slot validator and pre-render spy seam pending consumer implementation; unsupported Code load alone is not red. |
| G18 completion/kit | Existing export/artifact identity control and native notification prerequisite inventory | Durable completion→notification→exact-version activation, derivative readiness and headless execution pending S6. |
| G19 practice | Hash slots and qualified workflow/evidence checklist only | Actual supplied material, prior technical/model review and Riel's one watch pending S7. No invented practice content or human red. |
| G20 performance/parity | Direct coalesced input→Core→actual paint timing boundaries/process identity sampler | End-to-end host/children/version/switch/resource and native parity measurements pending S6. No claim that current baseline already misses latency. |

Allocation derives from the gate entries/positive controls and explicit pending-witness rule
in `docs/JA1-STUDIO-ORCHESTRATION.md:734` and
`docs/JA1-STUDIO-ORCHESTRATION.md:756` through `docs/JA1-STUDIO-ORCHESTRATION.md:907`.
S0 may land the minimum read-only trace seam that makes an expected property observable;
if that still requires future production behavior, leave it pending with its exact next owner.

JA-4 G1–G11/E1 get the independent fixture slots in §9 and existing supplied-envelope
assembly/cache controls. Its parser, unique correspondence, exact matcher, map/pin/history,
critical-speech, stream-coverage/sync and paired-commit witnesses remain pending the seams
explicitly named by `docs/JA4-TAKE-CHOICE.md:478`. JA-2 model/precision/foreground/native
profiles remain Stage A trials, not S0 witnesses; observe no inferred confidence or quality
(`docs/JA2-PERSON-SEPARATION.md:689`). AW2 P0's G0a–G0e transport/refusal work stays its own
stage and does not get silently implemented by this harness
(`docs/AW2-CODE-GENERATED-CLIPS.md:1066`).

### 8.2 Exact existing regression tests to retain

Names below are literal test function names at the source pin. The crate/module or test
target is part of each pin; implementation must use exact filters/full names and retain
the raw test summary. They have not been run for this draft.

| Pin | Crate / module or integration target | Exact existing tests and source |
| --- | --- | --- |
| R-M31 divergence/atomic merge/cherry-pick | kinewright-agent, `branch::tests` | `sibling_branches_diverge_without_touching_live_state` — `crates/kinewright-agent/src/branch.rs:450`; `merge_is_atomic_and_rejects_a_stale_base` — `crates/kinewright-agent/src/branch.rs:418`; `cherry_pick_preserves_branch_order_and_validates_indices_first` — `crates/kinewright-agent/src/branch.rs:476` |
| R-M31 history/one-entry batches | kinewright-core, `actor::tests` | `applied_operations_follow_undo_and_redo_instead_of_the_append_only_log` — `crates/kinewright-core/src/actor.rs:993`; `batch_broadcasts_once_appends_each_op_and_undoes_as_one_snapshot` — `crates/kinewright-core/src/actor.rs:1033`; `coalesced_batches_under_one_key_form_a_single_undo_entry` — `crates/kinewright-core/src/actor.rs:1166`; `coalesced_history_replays_faithfully_through_applied_operations` — `crates/kinewright-core/src/actor.rs:1258` |
| R-PF1 stamp/final consumer, I8 | kinewright-media, `engine::tests` | `i8_media_0`, `i8_media_1`, `i8_media_2`, `i8_media_3` — macro invocation `crates/kinewright-media/src/engine.rs:5680`; `i8_media_real_audio` — `crates/kinewright-media/src/engine.rs:5685`; `only_a_current_stamped_failure_stops_playback` — `crates/kinewright-media/src/engine.rs:4433`; `a_request_issued_before_a_new_document_is_superseded_not_relabelled` — `crates/kinewright-media/src/engine.rs:4465` |
| R-PF1 actual paint, I18 | kinewright-app, `presenter::tests` | `the_marker_marks_what_is_bound_when_it_paints` — `crates/kinewright-app/src/presenter.rs:513`; `a_marker_that_never_paints_is_never_acked` — `crates/kinewright-app/src/presenter.rs:548`; `the_real_paint_callback_acks_only_painted_images` — `crates/kinewright-app/src/presenter.rs:602`; `seeded_interleavings_bind_and_ack_only_current_frames` — `crates/kinewright-app/src/presenter.rs:705` |
| R-PF1 resources/teardown | kinewright-media, `engine::tests` | `an_idle_engine_holds_no_preview_reader_or_decoder` — `crates/kinewright-media/src/engine.rs:4287`; `dropping_the_engine_stops_the_preview_thread` — `crates/kinewright-media/src/engine.rs:4347`; `sync_decoders_counts_open_decoders_until_teardown` — `crates/kinewright-media/src/engine.rs:5151` |
| R-PF1 shared preview/export | kinewright-media integration target `generated_media` | `two_track_effect_export_matches_preview_after_h264_redecode` — `crates/kinewright-media/tests/generated_media.rs:495`; `title_export_pixels_match_preview_after_h264_redecode` — `crates/kinewright-media/tests/generated_media.rs:625`; `cancelled_export_writes_no_output` — `crates/kinewright-media/tests/generated_media.rs:864` |
| R-lock alias/exclusion/release | kinewright-project, `lockfile::tests` | `r19_second_acquire_refuses_while_held` — `crates/kinewright-project/src/lockfile.rs:1014`; `r19_release_frees_the_lock_for_a_clean_reacquire` — `crates/kinewright-project/src/lockfile.rs:1061`; `f4_symlink_alias_shares_one_lock` — `crates/kinewright-project/src/lockfile.rs:1607`; `g9_fd_check_detects_a_swapped_object` — `crates/kinewright-project/src/lockfile.rs:2109` |
| R-lock two-process/release/journals | kinewright-project, `lockfile::race_tests` | `guard_missing_discovery_with_held_lock` — `crates/kinewright-project/src/aw1_race_tests.rs:934`; `guard_alias_spellings_race_one_owner` — `crates/kinewright-project/src/aw1_race_tests.rs:1028`; `h7_stale_same_endpoint_release_spares_the_successor` — `crates/kinewright-project/src/aw1_race_tests.rs:2156`; `j3_link_retargeted_after_acquire_refuses_the_next_save` — `crates/kinewright-project/src/aw1_race_tests.rs:2595` |
| R-lock shared IO/sidecar/recovery | kinewright-project, `headless::tests`; kinewright-app, `recovery::tests` | `headless_save_preserves_an_unreplayed_journal` — `crates/kinewright-project/src/headless.rs:196`; `g3_headless_occupied_save_as_preserves_and_pairs` — `crates/kinewright-project/src/headless.rs:278`; `f6_failed_project_write_restores_the_prior_sidecar` — `crates/kinewright-project/src/headless.rs:454`; `journal_round_trip_preserves_agent_ops_and_undo_redo_history` — `crates/kinewright-app/src/recovery.rs:1077` |
| Format control alongside R-lock | kinewright-project, `project::tests`; kinewright-app, `project::tests` | `r1_save_bytes_are_byte_identical_pre_post_cutover` — `crates/kinewright-project/src/project.rs:730`; `v2_stamps_only_when_used` — `crates/kinewright-project/src/project.rs:821`; `mo2_inactive_features_still_stamp_v2` — `crates/kinewright-project/src/project.rs:956`; `mo2_old_reader_drops_an_unknown_blend_and_refuses_overwrite` — `crates/kinewright-project/src/project.rs:1100`; `can_overwrite_save_gates_on_the_read_version` — `crates/kinewright-app/src/project.rs:2582` |

Unix symlink pins and native Windows pins are separate lanes; a platform skip is not a
passing alias witness. The existing ignored
`lockfile::race_tests::defect_journal_appearing_after_the_scan_is_missed` at
`crates/kinewright-project/src/aw1_race_tests.rs:1194` is the **open S5-OBL**, not a green
R-lock pin. Port it to real GUI journal ownership later. Existing
`project::tests::project_atomic_write_falls_back_when_rename_is_refused` at
`crates/kinewright-project/src/project.rs:583` documents legacy fallback; its passing does
not certify G1 strict Studio writes. Preserve pending PF1 H-T/G14 and decoder-bound A-1;
no regression group discharges the new Studio gates
(`docs/JA1-STUDIO-ORCHESTRATION.md:909`).

### 8.3 Later witnesses for retained evidence and first save

These extend existing gates only. S0 records their fixture/observer slots and current-path
controls; the new production seams and all red/green results remain pending.

1. **JA-4 G7/G10 — `critical_word_evidence_cache_delete_reopen`.** At JA-4 S2/S3,
   retain a selected take whose disputed critical word has an independently source-bound
   human/certified-audio observation, plus its exact CandidatePage, TakeMapRevision and
   validation refs. At G7 save, delete reconstructible caches and reopen through the real
   JA-1 bundle writer/reader; repeat through recovery, Save As and undo/redo. Assert the
   same observation identity/content/capability, source/snapshot/WordIds, frame/sample range,
   validation-profile identity and choice refs. At JA-4 S5 G10, the correct supported take
   is the positive render control; correct script/chart/ASR with independently observed wrong
   spoken value must still block. Missing/damaged owned evidence or an unavailable checker
   yields explicit unresolved/refused state and zero affected render starts, never an
   invented replacement observation. Relink/retranscription produces successors and stale
   eligibility/review refusal. Targeted later mutations drop a retention edge or substitute
   ASR/script agreement for the observation; require the affected assertion red and restored
   green. Preserve G10's separate NOT RUN checker lane and G7's native/storage-crash boundary
   (`docs/JA4-TAKE-CHOICE.md:512`, `docs/JA4-TAKE-CHOICE.md:535`).

2. **AW2 G11 / JA-1 G3 — `first_save_failure_undo_live_reader`.** The common
   store/handoff owner is JA-1 S1; AW2 P1a supplies actual Code/font admission. Begin with a
   usable unnamed edit, required Code/font assets retained only by undo/redo, and a live
   proof/export reader pinning the source temporary store. Include a LUT dependency and a
   distinguishable prior destination root/asset closure. The positive control copies the
   whole closure, publishes root last, reopens the destination and restores the exact
   undone/redone content. Inject destination contention and copy/verification/quota/faults
   before root publication. Each definite failure must retain the original usable edit,
   undo/redo, source handles/pins and exact prior destination root/assets; retry succeeds
   without reimporting from mutable external paths. After successful ordinary handoff, keep the
   source alive while the old reader finishes; inspect its actual handles/output and exact
   pin/charge retirement, then reclaim temporary bytes only after reader retirement.
   Cache clear cannot remove required undo/reader assets. Inject a B5 outcome ambiguity
   separately: require `durability_unknown`, a wholly old/new graph and reconciliation
   before acknowledgment or retry at that destination; A1 independent rescue has separate S2 witnesses.
   Do not report a fictitious failed-save rollback. Later mutations copy
   before destination authority, omit an undo/LUT dependency, publish before verification,
   or retire a source reader pin early; each gets its own targeted red/restored green.
   Ordinary existing save/undo/reader controls can run in S0, but absent Code support or
   the session-store seam cannot earn a JA-1 behavioral red
   (`docs/AW2-CODE-GENERATED-CLIPS.md:425`, `docs/AW2-CODE-GENERATED-CLIPS.md:1033`,
   `docs/JA1-STUDIO-ORCHESTRATION.md:782`).

## 9. Fixture slots: independent adversarial labels and pending practice material

These are **hash slots only**, not fabricated fixture contents or measured hashes. Each
text fixture gets independent input and oracle SHA-256 slots; its oracle records exact
WordId paths/spans/identities/statuses as applicable. Do not derive expected paths from the
matcher/parser being tested. Tiny exhaustive enumeration may validate the exact-language
fixture oracle; it is test code, not production eligibility logic. Text timing labels do
not establish acoustic observations (`docs/JA4-TAKE-CHOICE.md:487`,
`docs/JA4-TAKE-CHOICE.md:564`).

| Small fixture slot | Independently labelled distinction; input hash / oracle hash |
| --- | --- |
| `text-lossless-cues` | Soft wraps, hard breaks/beat cues, links, literal `a[0]`, quoted brackets, code role and unknown/nested/unbalanced brackets; `pending` / `pending`. |
| `text-unique-move-rewrap` | Unique surviving line/beat and structured identity edit positive controls; `pending` / `pending`. |
| `text-duplicate-delete-first` | Original duplicate pin/context with first deletion history; `pending` / `pending`. |
| `text-duplicate-delete-second` | Indistinguishable raw revision, different survivor history; automatic transfer stays unresolved; `pending` / `pending`. |
| `text-duplicate-insert-move-context` | Insert before pinned duplicate, ambiguous moves/splits/joins and cue/beat-context changes; `pending` / `pending`. |
| `text-out-of-order-retakes` | Flub/clean/later repeat, script order differing from recording order; all full paths and stable labels; `pending` / `pending`. |
| `text-anchor-fillers` | All selected anchors broken by legal isolated fillers in valid path, exact anchors in distant wrong/partial path; `pending` / `pending`. |
| `text-window-crossing-long-line` | Accepted JA-4 96-required-token/35-isolated-filler counterexample across prior window boundaries; exact original associations; `pending` / `pending`. |
| `text-critical-values-units` | Correct written/spoken equivalence versus wrong value/sign/decimal/unit; synthetic labels only; `pending` / `pending`. |
| `text-critical-names-negations-comparisons` | Shared wording with a changed required name, negation or comparison; `pending` / `pending`. |
| `text-authored-filler` | Authored filler required versus permitted isolated insertion; consecutive/edge/substantive insertion negatives; `pending` / `pending`. |
| `text-late-valid-after-diagnostic-saturation` | Many early near-misses cannot consume exact registry capacity or hide late valid path; `pending` / `pending`. |
| `text-resume-frontier` | Same exhaustive path set across varied chunk/slice/cancel/resume boundaries, no false coverage completion; `pending` / `pending`. |
| `text-long-gap-unprompted` | Complete lexical delivery across a labelled 2.2 s gap remains complete, pacing review separate; `pending` / `pending`. |
| `text-true-restart` | Substantive intervening speech/restart cannot splice prefix and suffix; `pending` / `pending`. |
| `text-long-gap-authored-pause` | Same lexical/gap case with prior-authored protected cue; `pending` / `pending`. |
| `text-word-identity-successor` | Equal text with different snapshot/ordinals, corrected snapshot successors and unresolved pin retention; `pending` / `pending`. |

The three pause slots remain separate cases through JA-4 G5/G9. The two duplicate-deletion
histories and structured-survivor control must remain independently labelled. Future PCM,
selected-stream clock/coverage and independent decoded flash/clap/mouth landmark fixtures
need their own hashes/oracles before acoustic/sync acceptance; text cannot substitute for
them (`docs/JA4-TAKE-CHOICE.md:499`, `docs/JA4-TAKE-CHOICE.md:517`).

| Pending practice material | Hash slots only |
| --- | --- |
| Riel's tight-ends script | `script_sha256: pending`; `script_label_oracle_sha256: pending`. |
| Songbird JSON exports | `songbird_export_sha256[]: pending`; `dataset_scope_output_oracle_sha256[]: pending`. |
| C920 takes with scratch audio | `camera_sha256[]: pending`; `stream_coverage_landmark_oracle_sha256[]: pending`. |
| QuadCast takes | `mic_sha256[]: pending`; `word_critical_speech_oracle_sha256[]: pending`. |
| Capture-pair calibration evidence | `capture_clock_bound_evidence_sha256[]: pending`; independent audio/video landmark oracle hashes pending. |

No file count, script text, football values, expected word interval, font licence clearance,
chosen model hash or redistribution permission is invented. The shipped export shape is
available; actual practice exports are still pending. Default practice media is private;
public-fixture permission cannot block a private video
(`docs/JA4-TAKE-CHOICE.md:575`, `docs/JA2-PERSON-SEPARATION.md:917`).

## 10. Performance, memory and evidence bounds

S0 measures observer overhead separately and streams large artifacts. It adds no background
service, inference runtime, external package installation, second playback pipeline or
production memory allowance. Reuse existing dependencies and test-util surfaces. Any new
dependency or production mechanism needs a lead ruling, not an accidental harness import.

Retain JA-1's proposed 64 MiB control metadata, bounded ingress, 10,000-object/depth-32 graph,
bounded receipts/issuers/recovery roots and explicit admission pause rather than eviction of
unresolved evidence (`docs/JA1-STUDIO-ORCHESTRATION.md:367`,
`docs/JA1-STUDIO-ORCHESTRATION.md:683`). JA-4's 1 MiB script, bounded scope/pages, separate
exact/diagnostic quotas, cumulative counters and full workload matrix remain its own gates
(`docs/JA4-TAKE-CHOICE.md:413`, `docs/JA4-TAKE-CHOICE.md:436`). JA-2 precision/capacity remain
Stage A decisions, with host quota and feature ceilings both enforced; S0 cannot choose
Gray8/Gray16, approve full-source storage or add foreground RGB artifacts
(`docs/JA2-PERSON-SEPARATION.md:203`, `docs/JA2-PERSON-SEPARATION.md:645`).

Record actual bytes, RSS per process and simultaneous aggregate samples, VRAM, work/cells,
decode/verification bytes, p50/p95/p99 and displayed identities where the lane measures them.
Do not sum nonconcurrent peaks into an aggregate peak or call callback/submission timing
presentation. No PF1 whole-process RSS allowance or H-T/G14 pass follows from these records
(`docs/JA1-STUDIO-ORCHESTRATION.md:716`). Numerical targets remain proposed until the
owning native lane supplies measured evidence.

## 11. Ordered S0 implementation plan: five commits

These are **future commits**, not commits made by this task. S0 may land only contracts,
harness infrastructure, observational test hooks, fixture/hash catalogs and regression pins.
Do not raise the production format ceiling, expose unrenderable clip kinds or implement
the missing semantics. JA-1 S1 follows S0; JA-4 uses its records/owned-input seam, AW2 P0
remains independent, AW2 P1a consumes the one store, and JA-2 A trials precede B/C
(`docs/JA1-STUDIO-ORCHESTRATION.md:745`, `docs/JA4-TAKE-CHOICE.md:455`,
`docs/AW2-CODE-GENERATED-CLIPS.md:1066`, `docs/JA2-PERSON-SEPARATION.md:689`).

| Commit, in order | Bounded deliverable | Gate in addition to the common per-commit gate |
| --- | --- | --- |
| S0-1: freeze shared contracts and source pins | Promote the reviewed field/owner/version/handoff inventory, exact test manifest and fixture/hash-slot catalog; no production types | Citation/schema cross-check and legacy source/reader pins; all five resolutions in the complete §12 conflict ledger adopted before S0-1 acceptance; affected format/control tests retained. |
| S0-2: native protocol and current Core/project adapter | Eval-gated wire/runner/report modules, current commands/branch/save/load and read-only history/IO traces; full-byte controls | Protocol wrong-version/duplicate/oversize/path-escape failures classified as setup; current edit→undo/redo→save/reopen positive controls; demonstrate reachable history-loss/legacy-IO deficiencies with literal assertions, retain inaccessible rows pending. |
| S0-3: two-participant races and session recording | Real worker controller/barriers/reply-drop/exit inventory, recording wrapper forwarding existing ScriptedDriver/pump/server | Two-process R-lock controls, accepted-edit/retry trace and exact R-M31 history; real loopback prepared-plan/confirmation control; controller cleanup and missing-prerequisite classification checked. No receipt/lease implementation. |
| S0-4: media/App observations and editorial fixture slots | Shared preview/export/decode controls, cfg(test) App finalization/paint/journal adapters, independent adversarial label slots | R-PF1 I8/I18, resource/teardown and shared preview/export pins; direct coalesced gesture/actual-paint control; every JA-4 fixture has separate oracle identity or explicit pending slot. GPU/device/provider absence stays NOT RUN. |
| S0-5: freeze baseline evidence and pending witness ledger | Per-lane literal results/artifact hashes, exact regression manifest, gate→observer→owner ledger and runnable unchanged fixture protocol | R-M31/R-PF1/R-lock and format corpus; every claimed baseline red has successful control and failing assertion; targeted observer mutations detect trace/assertion defects, then restored controls green. No pending gate silently promoted. |

**Every commit's common gate is build + clippy + fmt + affected tests.** During actual
implementation, use the repository's FFmpeg environment and constrained serialized commands:
source `scripts/setup-ffmpeg.sh` in the same shell as Cargo; `nice -n 19`, `-j 4`,
`RUST_TEST_THREADS=4`. Build the workspace and explicit harness/test features/targets;
clippy the workspace/all affected targets with `-D warnings`; check touched Rust files with
`rustfmt --edition 2024 --config skip_children=true --check <touched files>`; run the exact
affected tests and required fixture cases. No `cargo fmt --all`. Check slow/ignored-test
manifest consistency if test classification changes. Do not substitute harness-only
compilation for normal shipping-feature compilation
(`target/review/PITFALLS.md:10`, `target/review/PITFALLS.md:24`,
`target/review/PITFALLS.md:66`).

The implementation lead must add the explicit harness feature/target to verification so a
`required-features` skip cannot look like a build pass. New read-only hooks must be absent
from shipping App/Core paths. Run one heavy command at a time and obey the available-memory
rule. Fast/slow CI requirements remain; a full slow gate runs through the established
`[slow-tier]` CI route, not an unauthorized workstation full-suite run. Preserve the tested
source/binary identities/raw summaries, not a saved result relabelled as a rerun.

S0's stage review tier is **two independent reviews: Astra + Muse**, explicitly required
for JA-1 stages. Review the final S0 head and evidence ledger before S1; fixes receive
targeted rechecks. This draft starts neither reviewer and claims neither review completed
(`docs/JA1-STUDIO-ORCHESTRATION.md:7`, `docs/JA1-STUDIO-ORCHESTRATION.md:919`).

## 12. Conflicts, resolutions and remaining risks

1. **V3 cohort versus future MO6/CC8 version assignments.** JA-1 names a coordinated v3
   with MO6; MO6 says use the next joint version if v3 was allocated, while CC8 calls its
   later feature additions v2 (`docs/JA1-STUDIO-ORCHESTRATION.md:113`,
   `target/review/mo/mo6-design-rev3.md:139`,
   `docs/CC8-FLEXIBLE-COLOUR-AND-YOUTUBE-HDR.md:245`). Today's ceiling is already 2 and
   current predicate contains only MO2 (`crates/kinewright-core/src/model.rs:1263`,
   `crates/kinewright-project/src/project.rs:380`). **Resolution (accepted for rev2):** freeze Job A v3 now;
   reserve no future fields. The shared owner integrates any actually ready later feature
   before its first writable release or assigns a later jointly reviewed version. A
   released v3 reader must refuse a later unsupported feature rather than drop it. CC8's
   old “v2” label cannot waive its own actual-old-reader refusal/legacy-byte gates.
   This settles the owner/predicate, not the future version number or HDR implementation.

2. **Shared-store construction stage/owner.** AW2 assigns generic blobs/pins/copy-first
   Save As to P1a; JA-1 requires a durable graph/store in S1, and JA-2 explicitly consumes
   the host (`docs/AW2-CODE-GENERATED-CLIPS.md:1076`,
   `docs/JA1-STUDIO-ORCHESTRATION.md:746`, `docs/JA2-PERSON-SEPARATION.md:240`).
   **Resolution (accepted for rev2):** JA-1 S1 provides the common project-owned publication/pin primitive;
   AW2 P1a implements its code/font validation/admission through it. AW2's stage retains
   its gates and complete renderer before exposure. Build the shared mechanism once,
   with host ownership for Studio and the same project writer boundary for other callers.
   S1 also owns the bounded session-temporary-store → named-project handoff in §4.3;
   AW2 P1a supplies admission and the shared AW2 G11/JA-1 G3 witness in §8.3. A failed
   copy preserves the usable unsaved edit and prior destination; source reader pins survive
   successful handoff until retirement (`docs/AW2-CODE-GENERATED-CLIPS.md:425`). Apply
   copy-before-root to every owned dependency, including LUTs, on new bundle writes;
   unrelated legacy LUT behavior remains unchanged.

3. **Legacy lossy Save As versus bundle preservation.** Current ProjectFile/CC8 allow a
   newer-file edit-only Save As; JA-1 says generic Save As cannot claim to preserve unknown
   records (`crates/kinewright-project/src/project.rs:247`,
   `docs/CC8-FLEXIBLE-COLOUR-AND-YOUTUBE-HDR.md:251`,
   `docs/JA1-STUDIO-ORCHESTRATION.md:126`). **Resolution (accepted for rev2):** preserve old behavior as an
   explicitly labelled derivative only. New generic bundle save/Save As refuses unsupported
   graph state and retains source bytes. No assertion that an already shipped old reader
   preserves or renders new features.

4. **JA-2 cleanup timing versus shared guardian deadline.** JA-2 lists a 5 s worker watchdog
   plus 5 s child-only cleanup; JA-1's later accepted shared guardian requires OS emptiness/
   reaping within 2 s of the kill decision or `cleanup_blocked`
   (`docs/JA2-PERSON-SEPARATION.md:649`, `docs/JA1-STUDIO-ORCHESTRATION.md:391`,
   `docs/AW2-CODE-GENERATED-CLIPS.md:818`).
   **Resolution (accepted for rev2):** distinguish inference watchdog/cooperative response from post-kill cleanup;
   use JA-1's stricter shared 2 s post-kill contract for owned children. At the deadline keep
   pins/charges/permit and `cleanup_blocked`; never report success or start replacement
   work merely because a timeout elapsed. No second JA-2 cleanup owner.

5. **AW2 capability-open “runtime red” versus JA-1 behavioral red.** AW2 explicitly
   labels missing Code admission/render, External admission/describe and cached External
   export as capability-open reds (`docs/AW2-CODE-GENERATED-CLIPS.md:996`,
   `docs/AW2-CODE-GENERATED-CLIPS.md:1005`). JA-1 requires a successful existing control
   followed by an actually observed failing durability/authority/evidence assertion;
   unsupported operation alone earns no behavioral-red credit
   (`docs/JA1-STUDIO-ORCHESTRATION.md:727`, `docs/JA1-STUDIO-ORCHESTRATION.md:734`).
   **Resolution (accepted for rev2):** retain AW2 capability-gap observations as separate,
   explicitly labelled evidence with their original assertions/source/fixture identities.
   They earn **no JA-1 behavioral-red or invariant-closure credit**. In the shared S0
   protocol an unsupported surface stays `surface_absent`/`pending_baseline_witness`;
   setup/prerequisite failures remain NOT RUN. Once support exists, invariant claims
   require a successful valid control, the relevant actual behavior assertion, targeted
   mutation red and restored green. No new harness service or production semantics.

All five ledger resolutions are adopted for this revision under the binding lead decisions;
S0-1 acceptance requires this complete ledger, not only a subset of its conflicts.

The old −16 LUFS row in JA-3 is a **resolved stale reference**, not an open design choice:
JA-1's promotion amendment and coverage row choose the shipped −14 target
(`target/review/ja/ja3-editorial-scoping-rev1.md:314`,
`docs/JA1-STUDIO-ORCHESTRATION.md:17`, `docs/JA1-STUDIO-ORCHESTRATION.md:635`).
Likewise JA-4 closes JA-3's request for a take-choice design; first-video flat cards do not
claim MO6 C/D template completion (`docs/JA4-TAKE-CHOICE.md:14`,
`target/review/ja/ja3-editorial-scoping-rev1.md:249`).

Largest remaining risks are a writer that accepts a Document projection and strips the
bundle; a decoder that reopens an unsealed path; history traces mistaken for restore
semantics; incomplete dataset/geometry inventories; and a harness that implements the
property it purports to test. The field union, owner/lease interfaces, independent oracles
and pending-witness classifications address those risks without adding production scope.
Windows namespace durability remains a release blocker where uncertified, model/provider
modalities stay uncertified, actual practice hashes/clock bounds remain pending, and PF1
H-T/G14 stays independent (`docs/JA1-STUDIO-ORCHESTRATION.md:317`,
`docs/JA1-STUDIO-ORCHESTRATION.md:617`, `docs/JA4-TAKE-CHOICE.md:595`).

## 13. Accepted lead answers and remaining questions

### 13.1 Accepted answers for the lead

The rev2 lead decisions adopt Q1–Q4's critic recommendations. These answers freeze scope;
actual adapter/connector certification and implementation evidence remain pending.

1. **Who owns the shared schema/store and the single assembly cleanup result?** Accepted
   answer: **the project/JA-1 owner** owns the union, evidence/retention edges, version
   admission, root/pins/publication, decoder-bound inputs, temporary-to-durable store
   handoff and head→TakeMap attachment. **The existing assembly owner** supplies the one
   cleanup result with durable associations; JA-3 consumes its exact applied mappings.
   Maintainer names are not required in this doc. Adopt all five §12 resolutions before
   S0-1 acceptance.

2. **Which native snapshot and Windows durability work comes first?** Accepted answer:
   certify one concrete ordinary-media snapshot adapter on Linux first, binding the actual
   decoder open and stable bytes through use. Test original/alias mutation, relink
   successors and lost ownership; “stable handles” alone is not an adapter selection or
   native result (§4.1). The actual adapter and its certification remain implementation
   work. Keep **G1-W** explicit as the Windows durable-write release blocker. AW2 P2
   source/runtime-image and executable certification stays separate and deferred for this
   video. Assign guardian packaging once through the shared AW2 P1b-s/JA-1 owner seam;
   JA-2 execution and AW2 helpers wait for their actual containment prerequisite,
   irrespective of which programme lands it first
   (`docs/JA1-STUDIO-ORCHESTRATION.md:317`,
   `docs/AW2-CODE-GENERATED-CLIPS.md:1076`).

3. **Who certifies visual review, disputed speech and practice oracles?** Accepted answer:
   use the configured independent visual connector only after actual image/required-clip
   evidence transport is certified. For disputed critical words, retain the separate named
   human/certified-audio source observation and durable identity in §3.5. Freeze supplied
   script/export/camera/mic hashes and independent text, acoustic/landmark/clock-bound
   oracles when available. Keep optional listening optional, unresolved speech blocked and
   pending practice slots honest; no fabricated football data or listening receipt
   (`docs/JA1-STUDIO-ORCHESTRATION.md:611`, `docs/JA4-TAKE-CHOICE.md:195`).

4. **What evidence must S0 have before S1 starts?** Accepted answer: the five commits'
   common build/clippy/fmt/affected-test checks, actual selected controls and accessible
   literal behavioral assertions, plus **Astra + Muse review of the final S0 head**.
   Every claimed baseline failure names its assertion and successful positive control;
   setup failures and AW2 capability-gap labels cannot satisfy it. Run explicit harness
   targets/features and a normal App configuration; retain exact selected test counts,
   source/binary identities and raw summaries. Leave unavailable native/provider/practice
   lanes and genuinely inaccessible seams pending with their owners/stages. S0 introduces
   no ceiling bump or production host/store/restore/matcher/decoder semantics and does not
   wait for the first video's entire G1–G20 implementation (§§7–8, 11).

### 13.2 For Riel

1. **No design question now.** If indispensable material is still absent when a real run
   begins, use JA-1's one batched missing-file prompt. Defaults, host ownership, v3, the
   audio target, exact discovery, conservative sync and no invented facts are already
   decided (`docs/JA1-STUDIO-ORCHESTRATION.md:934`, `docs/JA4-TAKE-CHOICE.md:585`).

## 14. Changes from rev1

| Critic finding | Addressed in | Resolution |
| --- | --- | --- |
| 1 — Major: durable take-validation evidence and implicit inventories | §§3.2, 3.5, 5.2–5.4, 8.3 item 1, 13.1 Q3 | Added one typed take-evidence record with exact page/map/JA-1 retention edges, S2/S3 producers and successor invalidation; both editorial inventories persist within the existing consumer family. JA-4 G7/G10 now require cache-delete/reopen preservation of the critical-word observation or explicit unresolved/refused output. |
| 2 — Major: unsaved store, first save and LUT scope | §§4.1–4.3, 8.1 G3, 8.3 item 2, 12 item 2, 13.1 Q1–Q2 | JA-1 S1 owns the bounded session-to-named handoff consumed by AW2 P1a; authority/copy/verification precede root, failures preserve usable unsaved state/prior destination, and reader pins outlive handoff. Named AW2 G11/JA-1 G3 witness; whole new-bundle graph includes LUTs while unrelated legacy behavior stays unchanged. |
| 3 — Minor: AW2 runtime-red terminology | §7.4, §12 item 5, §11 S0-1, §13.1 Q4 | Preserve capability-gap evidence separately with no JA-1 behavioral-red/invariant credit. Adopt all five conflict resolutions and the critic's Q1–Q4 answers; correct S0-1 to reference the complete ledger. All three findings accepted; nothing rejected. |
