# CC4 look management

Status: implementation contract, 2026-08-25
Depends on: [CC0](ROADMAP-AND-WORKFLOWS.md), [CC1 managed SDR primary](CC1-MANAGED-SDR-PRIMARY.md), [CC2 scopes and matching](CC2-SCOPES-AND-MATCHING.md), [CC3 curves and wheels](CC3-CURVES-AND-WHEELS.md), [M41 offline/relink](M41-OFFLINE-RELINK-CACHE-VISIBILITY.md)
Scope: ordered technical and creative LUT nodes on the CC3 node stack, backed by project-owned, content-hashed LUT assets.

CC4 does not change CC1's input, working, monitoring, or delivery contract. It adds
two node kinds to the stack CC1 declared ordered and inspectable, an asset model
that makes a look portable, and the human file-picker workflow the roadmap records
as missing. Every CC1 invariant — especially the no-intermediate-clamp invariant
and "the serialized vector order is the execution order" — is preserved verbatim.

The words **must**, **must not**, and **may** in this document are normative.
Measured facts cited below (adapter limits, bake errors, JSON sizes) were
gathered on 2026-08-25 on Mesa lavapipe and an NVIDIA RTX 3080 and are recorded
so later readers can tell a measurement from a design choice.

## 1. In scope and out of scope

CC4 delivers:

- `Document.lut_assets`: project-owned, content-hashed LUT asset records with
  typed availability and recovery;
- a project-relative sidecar store, so copying the project file plus one
  directory reproduces every look bit-identically on another machine;
- two serializable ordered nodes, `technical_lut` (role `technical`) and
  `creative_look` (role `creative`), with an integer asset reference, adjustable
  mix, per-node bypass, and an explicit input encoding;
- a normative stage-ordering rule — technical → correction → creative — enforced
  by Core *rejection* rather than by silent reordering;
- normative tetrahedral interpolation, identical on the CPU reference and the GPU
  compositor, with a defined out-of-domain rule that keeps over-range values
  recoverable;
- the four legacy built-in looks re-expressed as deterministic, content-hashed,
  binary-embedded **built-in generated LUT assets**, so human and agent use
  exactly one node kind;
- one positional `InsertEffect` operation, two LUT-asset operations, and one
  explicit legacy conversion operation;
- a human import/browse/mix/bypass/A-B workflow and an agent surface
  (`list_look_assets`, `import_lut_asset`, `plan_technical_lut`,
  `plan_creative_look`, extended `color_nodes` manifests and
  `render_color_proof`); and
- a fixture suite covering parsing, interpolation anchors, ordering, mix,
  relocatability, missing/changed recovery, parity, and serialization.

CC4 does **not** deliver HSL qualifiers, windows, mattes, tracking, or
node-scoped secondaries (CC5); gamut mapping, legal-range policy, per-look gamut
warnings, skin diagnostics, HDR, or delivery beyond the CC1 Rec.709 contract
(CC6); ACES, OCIO, camera RAW, or log source profiles (CC6/CC7); LUT authoring or
export; look groups and managed group apply; still stores; or any automatic look
selection. CC4 adds no `auto_grade` operation and no analysis tool that mutates a
document.

**1D shapers are reserved, not delivered.** The asset model carries `kind` with
a reserved `cube_1d` value so a shaper needs no schema migration, and the parser
reports a typed `unsupported_lut_format` for `LUT_1D_SIZE`. A shaper needs its
own evaluation contract (interpolation, extrapolation, per-channel versus
shared) and its own gates; it is not trivially supported by the 3D asset model
and is therefore deferred.

## 2. Asset model

### 2.1 The `LutAsset` record

`Document` gains one field:

```rust
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub lut_assets: Vec<LutAsset>,
```

Absent in every pre-CC4 project, so those projects load byte-unchanged and
re-save without the field until a look is added.

```json
"lut_assets": [
  {
    "id": 1,
    "sha256": "3f5c9d0b…64 lowercase hex…",
    "title": "Kodak 2383 D65",
    "kind": "cube_3d",
    "size": 33,
    "byte_len": 1174896,
    "domain_min_millionths": [0, 0, 0],
    "domain_max_millionths": [1000000, 1000000, 1000000],
    "source": { "imported": { "source_path": "/home/riel/LUTs/k2383.cube" } }
  },
  {
    "id": 2,
    "sha256": "9ab1…",
    "title": "Warm",
    "kind": "cube_3d",
    "size": 17,
    "byte_len": 137979,
    "domain_min_millionths": [-1000000, -1000000, -1000000],
    "domain_max_millionths": [2000000, 2000000, 2000000],
    "source": { "builtin": { "name": "warm" } }
  }
]
```

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | `LutAssetId(u64)`, `1..=9007199254740991` | Stable project-local identity. Allocated as `max(existing)+1`. The upper bound is `2^53 - 1` so the id survives every JSON consumer, including the agent's, without precision loss. |
| `sha256` | 64 lowercase hex chars | The content identity of the LUT file bytes. Validated exactly as M41 validates `MediaSourceFingerprint` (length 64, `[0-9a-f]`). |
| `title` | non-empty `String` | The `.cube` `TITLE` keyword when present, otherwise the file stem. Informational; never an identity. |
| `kind` | `cube_3d` \| `cube_1d` | `cube_1d` is reserved and rejected on import in CC4. |
| `size` | `2..=65` | Lattice edge length `S`. |
| `byte_len` | `u64`, non-zero | Diagnostic and cheap preflight, never proof by itself (M41's rule). |
| `domain_min_millionths` | `[i64; 3]` | Informational mirror of `DOMAIN_MIN`, rounded half away from zero. |
| `domain_max_millionths` | `[i64; 3]` | Informational mirror of `DOMAIN_MAX`. |
| `source` | `{"imported":{"source_path":…}}` \| `{"builtin":{"name":…}}` | Provenance. `source_path` is **informational only**: it is never opened by the renderer and never resolved relative to anything. |

**The hashed bytes are the authority, not the record.** `size` and the domain
mirrors exist so a human or agent can read a project without touching the store.
The renderer **must** use the `size` and domain parsed from the verified bytes,
and **must** fail with `lut_asset_metadata_mismatch` (naming `field`,
`observed`, `allowed`) if the parsed values disagree with the record. Because the
bytes are hash-verified, a mismatch can only mean the JSON was hand-edited; it is
a typed error, never a silent preference for one side.

Integer-only storage extends to the domain mirrors deliberately: no float reaches
project JSON, and no reader is tempted to render from a lossy mirror. Rejected
alternative: `[f32; 3]` domain fields — rejected because they would look
authoritative.

### 2.2 The project-relative store

For a project file `<dir>/<stem>.kinewright`, the store root is

```text
<dir>/<stem>.kinewright-assets/
    luts/
        <sha256>.cube
```

The `<stem>` is the project file's stem regardless of extension (a `foo.json`
project derives `foo.kinewright-assets`). The store path is **derived from the
project path at runtime and never stored**,
absolute or relative, in any document, journal entry, or recovery record. Core
has no filesystem or project-directory concept (it never did: `Document` does not
know its own path); the app and media layers derive the root and pass verified
data into Core, exactly as M41's `RelinkCandidate` does. Cache clearing never
touches the store: M41 already states that scoped cache clears never remove
LUTs.

Relocatability rule, normative: **copying the project file and its
`<stem>.kinewright-assets` directory to another machine, another user account, or
another path must reproduce every look bit-identically.** Nothing else may be
required. §10.3.11 is the proof.

The file name is the content hash, so the same LUT imported twice writes one
file, and the store is inherently deduplicated. Because the name is 64 validated
hex characters plus `.cube`, no user-supplied string ever reaches a path
component: directory traversal through import is structurally impossible, not
merely checked. That covers the leaf only; the store root, its `luts/`
directory, and every store file are inspected with `fs::symlink_metadata`, and a
symlinked root, `luts/`, or entry is refused with `lut_store_root_invalid
{ path, reason }` for every write (import, restore, Save As copy), exactly as
`derived_cache.rs` refuses a symlinked cache root. Availability treats a
non-regular store file as `missing`. This, not the hash filename alone, is what
makes "written only under the project directory" true. A project path with no
parent or stem, or a root that exists as a non-directory, is the same typed
error.

Rejected alternative: embedding samples in the project JSON — rejected on
measurement: the cheapest embeddable form (base64 of raw f32) is 575 KB per 33³
look and 4.39 MB per 65³ look, every `Document` clone in the actor, journal,
branch, and recovery path would carry it, and `Document` is cloned per operation
for atomic validation. A content-addressed sidecar puts a 64-character digest in
the JSON instead. Rejected alternative: a user-level shared LUT library —
rejected because it is exactly the non-relocatable behaviour CC4 exists to
remove.

Built-in assets are **not** written to the store. They are generated in the
binary (§2.6).

Save As **must** copy every store file referenced by the document to the new
store root before reporting success, and **must** report, per asset, any file it
could not copy as `lut_store_copy_failed { lut_asset, reason }`. A project saved to a new path with an unavailable asset is still
saved; the asset is simply `missing` there, with the ordinary recovery path.

A project that has never been saved has no store root. `import_lut_asset` on such
a project fails with `project_not_saved` and the recovery action "save the
project first". Rejected alternative: a session temp store migrated on first save
— rejected because a crash between import and save would silently lose the bytes
the project claims to own.

### 2.3 Availability and recovery

Availability is **runtime state, not project state**, exactly as in M41. It is
never serialized.

| State | Condition |
| --- | --- |
| `verified` | The store file exists, is a regular file, and its bytes hash to `sha256`. |
| `missing` | The store file is absent or is not a regular file. |
| `changed` | A file exists but its bytes hash to something else. |
| `unreadable` | The path exists but bytes or metadata cannot be read. |
| `changed` (metadata) | The bytes hash-verify but the record's `size` or domain mirror disagrees with the parsed lattice (`lut_asset_metadata_mismatch` in `reason`); the asset is withheld from the library. |

M41's `online_unverified` has no CC4 equivalent: a LUT asset can only be created
with a hash, so there is no legacy unverified state. Built-in assets are
`verified` when the embedded bake hashes to the recorded `sha256`; if a future
release changes a bake, older projects report `changed` and are not silently
re-rendered.

Recovery is typed and explicit:

- **Restore** — `restore_lut_asset(lut_asset_id, candidate_path)` reads the
  candidate, hashes it, and writes it into the store only when the hash matches
  exactly. A mismatch is `lut_relink_hash_mismatch` with `expected`, `observed`,
  and the recovery action. This is a **media repair action, not a Core
  operation**: content addressing means the document stores no locator to
  change, so restoring bytes changes no document state, does not dirty the
  project, and needs no revision gate. This is a deliberate, stated departure
  from M41, whose asset locator *is* document state.
- **Replace** — a different LUT is a *different asset*. It is one visible,
  undoable batch: `AddLutAsset` for the new asset, one
  `SetEffectParam{ name: "lut_asset_id" }` per node being retargeted, and
  optionally `RemoveLutAsset` for the old one. No operation ever rewrites an
  asset's `sha256` in place.

A `missing`, `changed`, or `unreadable` asset referenced by an **active** node
blocks managed proof and export with `missing_lut_asset` / `changed_lut_asset` /
`unreadable_lut_asset`, naming the asset id, title, hash, expected store path,
and recovery action. Preview shows the same typed status rather than an invented
frame; it **must not** silently drop the node. An asset referenced only by
nodes that can never be active does not block; the status is still reported.
Because `bypass` and `mix_basis_points` are keyframable, "active" is evaluated
conservatively: a node counts as active unless it is inactive at its static
fallback **and** at every keyframe value of `bypass` and `mix_basis_points`
(`lut_node_may_be_active`). The export preflight uses the export document; QA
and the preview status use the whole timeline.

Availability can only be resolved by the layer that owns the store, so it is
**injected** into Core exactly as M41 injects media availability. Rendering
resolves lattices **per document**: the media engine keeps a content-addressed
table of published lattices keyed by `sha256`, and every proof, export, or
playback render builds a document-local library from that document's own
`lut_assets`, so two open projects that both use `LutAssetId(1)` can never
alias each other's looks; an unpublished hash fails typed `missing_lut_asset`.
The injection is: Core defines
`LutAvailabilityKind`, `LutAvailabilityStatus`, and
`export_lut_preflight_with(document, availability_for)` in `media.rs`,
mirroring `export_media_preflight_with`, and the export queue and the export
dialog inject the store's resolver. `qa_document` and `delivery_conformance`
check only what is structural — a dangling `lut_asset_id` is `missing_lut_asset`
— and **must not** derive or probe a store path. Today no layer detects a
missing LUT before render time; the preflight is what changes that.

### 2.4 Import flow

Import is filesystem work, so it lives in the media crate and Core stays pure and
I/O-free:

1. `kinewright_media::lut_store::import_lut_asset(store_root: &Path, source: &Path) -> Result<LutAssetImport, MediaError>`
   reads the file, strips a leading UTF-8 BOM, parses it (§2.5), computes
   SHA-256 over the **original file bytes**, creates `<store_root>/luts/` if
   needed, and writes `<sha256>.cube` by writing a temporary file in the same
   directory and renaming it into place. An existing file with the same name
   whose bytes already hash correctly is left untouched; one that does not is
   overwritten (the hash is the truth).
2. It returns `LutAssetImport { sha256, title, kind, size, byte_len, domain_min_millionths, domain_max_millionths, source_path }`
   — metadata only, no samples.
3. The caller allocates the next `LutAssetId` and submits
   `Operation::AddLutAsset { asset }` through the ordinary Core path: validated,
   journaled, revision-gated at the agent boundary, undoable.

**No LUT sample bytes ever enter the journal, the document, a branch, or a
recovery record.** Undoing an `AddLutAsset` removes the record and leaves the
store file; the store is content-addressed and idempotent, so a redo re-registers
the same hash without touching the filesystem. Store files are never deleted by
an undo; garbage collection of unreferenced store files is out of scope for CC4
and is stated as such in the UI.

Resolution at render time is not a filesystem concern for the compositor. The
app (or the agent's server) builds a `LutLibrary { BTreeMap<LutAssetId, Arc<Lut3d>> }`
from `Document.lut_assets` plus the store root and the built-in table, verifying
each hash, and publishes it with a new `Playback::set_lut_library`. The
compositor and the CPU reference consume verified sample data and never open a
file. This also replaces today's path+mtime cache with a content-addressed one:
the parse cache is keyed by `sha256`, so a same-path replacement can never serve
stale samples.

### 2.5 `.cube` parsing

`crates/kinewright-media/src/lut.rs` keeps its structure and gains typed errors
and two behaviour changes:

- `TITLE` is captured into the asset title instead of being discarded.
- `MAX_CUBE_SIZE` becomes `65` (from `64`), because 65 is the most common vendor
  export grid and `65 - 1 = 64` is a power of two, which matters for §3.5's
  exactness claim.

Errors become a structured `LutParseError { code, line: Option<usize>, observed, allowed }`
(and, for the store, `LutStoreError { code, detail, observed, allowed }`), both
public. Recorded departure: `MediaError` gained no `Lut` variant in CC4 —
adding one would have changed a type consumed across every crate boundary
mid-slice — so both errors map to `MediaError::Backend` with the stable prefix
`"<code>: <detail>; observed=<…>; allowed=<…>"`, which the agent and the app
parse back into `field`/`observed`/`allowed`/`recovery_action`; callers that
hold the typed error use its fields directly. The codes are:

| Code | Cause |
| --- | --- |
| `unsupported_lut_format` | `LUT_1D_SIZE` or `LUT_2D_SIZE`. |
| `lut_size_out_of_range` | `LUT_3D_SIZE` outside `2..=65`. |
| `malformed_lut_file` | Repeated/missing `LUT_3D_SIZE`, a non-triple data line, an unparsable number, non-UTF-8 bytes. |
| `lut_domain_invalid` | Non-finite domain, or `DOMAIN_MIN >= DOMAIN_MAX` on any channel. |
| `lut_sample_count_mismatch` | Sample count `!= 3 * S³`. |
| `lut_sample_not_finite` | Any NaN or infinite sample. |

Comments (`#`), blank lines, CRLF (`str::lines` strips the trailing `\r`),
scientific notation, and keyword case-insensitivity are all supported and are
fixtures. Sample values are **not** range-limited: negative and above-1 lattice
values are legal and are preserved. Import and restore reject a source whose
length exceeds `LUT_MAX_FILE_BYTES = 16 MiB` (roughly twice the 65³ worst case)
with `lut_file_too_large { observed, allowed }` **before** reading it, and the
bytes hashed are the same in-memory buffer that is written to the store — the
source is never re-read between hashing and writing.

### 2.6 Built-in generated LUT assets

The four legacy `look_lut` presets become built-in generated assets so that a
human and an agent apply looks through exactly one node kind. The presets have
never had names in the code, the UI, or the agent (only `preset_token` 0..=4,
where 0 is the neutral identity and the descriptor default); CC4 coins them
here, and the token mapping is normative:

| `preset_token` | Built-in name | Title |
| ---: | --- | --- |
| 0 | `identity` | Identity |
| 1 | `warm` | Warm |
| 2 | `cool` | Cool |
| 3 | `monochrome` | Monochrome |
| 4 | `bleach_bypass` | Bleach bypass |

Each is baked from the formulas the legacy shader used, evaluated in the
`display709` encoding, where `e` is the display-coded RGB triple and
`Y = 0.2126·r + 0.7152·g + 0.0722·b`:

```text
identity        L(e) = e
warm            L(e) = (e - 0.5) * 1.08 + (0.54, 0.50, 0.46)
cool            L(e) = (e - 0.5) * 1.12 + (0.46, 0.50, 0.55)
monochrome      L(e) = (Y, Y, Y)
bleach_bypass   m    = (Y,Y,Y) + (e - (Y,Y,Y)) * 0.35
                L(e) = (m - 0.5) * 1.35 + 0.5
```

The bake **must not** clamp lattice values. The legacy stage clamped to `[0, 1]`
in display space; the managed node does not, because CC1 §2.2 invariant 5
forbids an intermediate clamp. That is a real, documented behaviour difference
and is one reason conversion is explicit (§9).

**Bake domain.** The four looks are baked at **S = 17** over
`DOMAIN_MIN = -1`, `DOMAIN_MAX = 2` on every channel, not over `[0, 1]`. This is
a measured requirement, not a preference: the CC3 §10.2 raster encodes to
display values spanning `[-0.71, 1.95]` (72 of 192 samples outside `[0, 1]`),
and with a `[0, 1]` lattice the §3.5 out-of-domain rule clamps *before* the look
is applied. Because none of the four looks has a unit Jacobian, the additive
delta restoration is not the identity outside the domain; the measured maximum
linear divergence from the closed form on the raster is 0.35 (`warm`), 0.54
(`cool`), 1.74 (`bleach_bypass`), and 2.56 (`monochrome`), on 63–72 of the 192
samples. With the `[-1, 2]` domain every raster sample lies inside the lattice.
`identity` is baked at `S = 2` over `[0, 1]`, where every lattice value is
exactly representable and the reproduction is bit-exact.

All five formulas are affine in `e`, and tetrahedral interpolation reproduces an
affine function exactly on every simplex in exact arithmetic, so the only error
floor is the 1e-6 decimal lattice grid mandated by the serializer below: the
measured maximum is `4.85e-7` in display code and `1.43e-6` in linear light at
`S = 17` over `[-1, 2]` on the raster, against the §10.3.10 gate of `2e-6` in
display code. `S = 17` is chosen because it stays faithful if a future built-in
is non-affine, is 59 KB of raw samples per look, and `17 - 1 = 16` is a power
of two.

Determinism of the hash requires a pinned serializer. The generated text
**must** be exactly:

```text
TITLE "kinewright.look.<name>.v1"\n
LUT_3D_SIZE <S>\n
DOMAIN_MIN <min> <min> <min>\n
DOMAIN_MAX <max> <max> <max>\n
<r> <g> <b>\n            (S³ lines, red-fastest, each value "{:.6}")
```

LF only, no trailing blank line, no locale-dependent formatting, `{:.6}` fixed
decimal for every number including the domain lines (`-1.000000`, `2.000000`).
Each lattice value is computed in f64, stored as f32, and **the f32 is what is
formatted**; the pinned hashes depend on that intermediate rounding (3657 of
14739 `monochrome` components sit on a half-millionth decimal tie), and its
extra cost is `<= 9.4e-8`, inside the §10.3.10 gate.
Each built-in's `sha256` is a pinned literal constant in the source and
re-asserted in the fixture (§10.3.10); changing a bake is a visible test
failure, never a silent re-render. The text round-trips through the production
parser in both LF and CRLF form (verified).

Built-ins are registered in `Document.lut_assets` on first use, with
`source: {"builtin":{"name":"warm"}}` and the pinned hash. The project is
therefore self-describing — a reader learns the exact look bytes a project used
without the binary — and a bake change is detectable as `changed`. Rejected
alternative: a separate `builtin_look` node kind — rejected because it doubles
the UI, the manifest, the planner, and the gate for no capability.

### 2.7 Operations

| Operation | Behaviour |
| --- | --- |
| `AddLutAsset { asset: LutAsset }` | Metadata only. Rejects a duplicate id (`DuplicateLutAsset`), a malformed hash (`InvalidLutAssetHash`), an empty title, `byte_len == 0`, `size` outside `2..=65`, `kind == cube_1d`, or a non-increasing domain mirror (`InvalidLutAssetMetadata { field, observed, allowed }`). |
| `RemoveLutAsset { lut_asset }` | Rejected with `LutAssetInUse { lut_asset, clip, effect }` when any effect on any clip references it, including a bypassed node and a `Hold` keyframe value. Never cascades. |
| `InsertEffect { clip, index, effect }` | Positional sibling of `AddEffect`, with identical validation plus `EffectIndexOutOfRange { clip, index, len }`. Required because `AddEffect` appends, and a stage-ordered stack must be able to place a `technical_lut` before an existing correction node without deleting it. |
| `ConvertLegacyLook { clip, effect }` | Replaces one legacy `look_lut` / `cube_lut` at its exact vector position with the equivalent managed node (§9). Rejected with `NotALegacyLook` for any other effect, and with `MissingLutAsset` if the required asset is not already registered — so conversion is always the visible two-operation batch `[AddLutAsset, ConvertLegacyLook]`. |

`validate_document` gains three invariants: every `lut_asset_id` referenced by
a node — its stored parameter value **and every value in its keyframe curve** —
exists in `lut_assets` (`MissingLutAsset { clip, effect, lut_asset }`); a LUT
node whose `lut_asset_id` is omitted or `0` is rejected with
`MissingLutAsset { lut_asset: 0 }` by `AddEffect`, `InsertEffect`, and
`validate_document` alike; and `lut_assets` ids are unique.
`LutAssetId` allocation past `2^53 - 1` fails with `LutAssetIdExhausted`.

## 3. Node model

### 3.1 Kinds and roles

Two effect names join `MANAGED_COLOR_NODE_NAMES`:

| Effect name | `ColorNodeKind` | Role | Stage | Storage tag |
| --- | --- | --- | ---: | ---: |
| `technical_lut` | `TechnicalLut` | `technical` | 0 `input` | 4 |
| `primary_correction`, `color_wheels`, `color_curves` | `Primary`, `Wheels`, `Curves` | `correction` | 1 `correction` | 1, 2, 3 |
| `creative_look` | `CreativeLook` | `creative` | 2 `look` | 5 |

Both new kinds are managed nodes: `effect_compatibility_stage` returns `None`
for them, they are inside the CC1 conformance claim, and they never report
`legacy_lut_stage`. The two kinds are mathematically identical; they differ only
in stage, role, mix bounds, and UI placement. That separation is the roadmap's
requirement that input transforms, corrections, and creative looks stay
separately inspectable, and it is what makes "no display/output transform is
mistaken for a creative LUT" checkable rather than aspirational.

`COLOR_NODE_LIMIT_PER_LAYER` stays 16 for all managed nodes combined. A new,
tighter limit applies to LUT nodes only: `LUT_NODE_LIMIT_PER_LAYER = 4`, counting
technical and creative together, enforced by
`TooManyLutNodes { clip, limit, actual }`. The limit exists because each LUT
node needs a texture atlas slot (§4).

### 3.2 Stage ordering

Normative rule: **the subsequence of managed colour nodes in `clip.effects` must
have non-decreasing stage rank.** All `technical_lut` nodes come first in vector
order, then all correction nodes in vector order, then all `creative_look` nodes
in vector order. Within a stage, the vector order is the execution order and
there is no inter-kind precedence, exactly as CC3 §3.1 states for corrections.
Non-colour effects (crop, mask, key, reframe, transitions) are unconstrained and
keep their positions.

A vector order that contradicts the stage order is **rejected**, not reordered:

```rust
OpError::ColorStageOrderViolation {
    clip: ClipId,
    effect: EffectId,          // the offending node
    kind: String,              // "technical_lut"
    color_stage_rank: u8,      // 0
    previous_effect: EffectId,
    previous_kind: String,     // "color_curves"
    previous_color_stage_rank: u8, // 1
}
```

Enforced in `add_effect`, `insert_effect`, `convert_legacy_look`, and
`validate_document`. Because every pre-CC4 project has only correction nodes,
all at rank 1, the document-level invariant is trivially satisfied by every
existing project.

This is the choice that keeps two principles simultaneously true: "the stored
order is the execution order" (CC1 §3, CC3 §3.1) and "keep input transforms,
corrections, and creative looks ordered" (the roadmap's architecture
principle). Rejected alternative: sort by stage at render time and leave the
vector alone — rejected because the inspector, the proof manifest, and
`clip.effects` would then disagree about what runs when, which is precisely the
hidden-engine-state failure the roadmap forbids. Rejected alternative: let a
creative look run before a correction if the user places it there — rejected
because "no output transform is mistaken for a creative LUT" needs a stage a
reader can trust.

### 3.3 The asset reference as an integer parameter

`lut_asset_id` is an ordinary `ParamValue::Integer` descriptor parameter whose
value is the `LutAssetId`. Bounds `0..=9007199254740991`, neutral `0`. `0`
means *unbound*, which makes the node inactive (§3.6). A valid document never
contains `0`, because `validate_document` requires every referenced id to
exist; the value exists only so a resolved node can never index a missing
asset — the same defensive posture CC3 §2.3 takes for a non-positive curve
span.

No new `ParamValue` variant is introduced, for exactly CC3 §2.4's reasons: the
descriptor, keyframe, validation, journal, undo, and agent-operation machinery
all work unchanged on integers. This is also the first effect parameter that
references another document entity by id; every earlier cross-reference is a
typed struct field. The integer parameter is chosen because a typed field would
need a new retargeting operation and a second keyframe mechanism for no gain,
and because `validate_document` makes a dangling reference impossible in a
loadable project. `lut_asset_id` uses `EffectUniform::ColorNode`, so it is never
materialized into `LayerParams`; the compositor's generic `parameter_value`
cast computes and discards it for `ColorNode` parameters, and its
`cast_precision_loss` justification must say so, because `2^53 - 1` is exact in
`i64` but not in `f32`.

### 3.4 Input encoding

| Token | Name | Encode `ENC` | Decode `DEC` |
| ---: | --- | --- | --- |
| 0 | `display709` | CC1 `encode_bt709` (sign-preserving) | `decode_display709` (below) |
| 1 | `linear` | identity | identity |
| 2 | `grade709` | CC3 `grade709_encode` | CC3 `grade709_decode` |

Default `0` for both node kinds. Published `.cube` looks are authored against
display-coded Rec.709 values in `0..1`, and a Rec.709 look fed scene-linear
values would be visibly wrong; the default must match the overwhelmingly common
authoring assumption.

**`decode_display709` is new and required.** CC1's `decode_bt709` is a *source*
decode: for a negative argument it takes the linear branch unconditionally, so
`decode_bt709(encode_bt709(x)) != x` for `x < -0.018`. CC4 therefore defines the
exact sign-preserving inverse of `encode_bt709`, with CC1's rounded constants:

```text
decode_display709(e) = sgn(e) * |e| / 4.5                       if |e| <  0.081
                     = sgn(e) * ((|e| + 0.099) / 1.099)^(1/0.45) otherwise
```

`sgn(0) = 0`; implementations must not use `f32::signum`. In exact arithmetic
`decode_display709(encode_bt709(x)) = x` for every finite `x`, including across
the documented 0.018/0.081 seam. This function is a **node-internal grading
parameterization**. It **must not** replace CC1's `decode_bt709` for source
decode, and neither it nor `encode_bt709` may be used here as a monitoring or
delivery transform: the monitor transform still happens once, at the named
boundary, after compositing.

**Log sources are out of scope, honestly.** A technical LUT authored for a
camera log curve expects the *source-coded* value, but CC1 §2.1 accepts only
`rec709_video` and `srgb_full` — a log transfer is an explicit CC1 failure, so a
log source cannot reach the node stack at all. Re-encoding a decoded Rec.709
value into a log curve to feed such a LUT would be inventing a transform the
pipeline never performed. CC4 therefore **restricts technical LUTs to the three
encodings above** and defers log-source normalization to the slice that adds
log source profiles (CC6/CC7). A `.cube` file whose author intended log input
will produce a wrong-looking result; that is a source-profile limitation stated
in the UI and the manifest, not a silent approximation.

### 3.5 Evaluation

For one node, per pixel, with the verified lattice `L` (edge `S`, domain
`dmin`/`dmax` from the bytes), `mix = mix_basis_points / 10000`:

```text
e_c  = ENC(x_c)                              for c in {r, g, b}
u_c  = clamp(e_c, dmin_c, dmax_c)
y    = tetrahedral(L, u)
z_c  = y_c + (e_c - u_c)                     out-of-domain delta restoration
x'_c = DEC(z_c)
out_c = x_c + (x'_c - x_c) * mix             mix in linear light
```

**Lattice coordinates.**

```text
t_c = (u_c - dmin_c) / (dmax_c - dmin_c)     in [0, 1]
s_c = t_c * f32(S - 1)
i_c = min(u32(floor(s_c)), S - 2)
f_c = s_c - f32(i_c)                         in [0, 1]
```

`V(dr, dg, db) = L[(i_b + db) * S * S + (i_g + dg) * S + (i_r + dr)]`,
red-fastest IRIDAS order, matching the parser and the texture layout.

**Tetrahedral interpolation, normative, with this exact branch structure so tie
handling is identical on both implementations:**

```text
if f_r > f_g {
    if f_g > f_b {  out = c000 + f_r*(c100-c000) + f_g*(c110-c100) + f_b*(c111-c110) }
    else if f_r > f_b { out = c000 + f_r*(c100-c000) + f_g*(c111-c101) + f_b*(c101-c100) }
    else {          out = c000 + f_r*(c101-c001) + f_g*(c111-c101) + f_b*(c001-c000) }
} else {
    if f_b > f_g {  out = c000 + f_r*(c111-c011) + f_g*(c011-c001) + f_b*(c001-c000) }
    else if f_b > f_r { out = c000 + f_r*(c111-c011) + f_g*(c010-c000) + f_b*(c011-c010) }
    else {          out = c000 + f_r*(c110-c010) + f_g*(c010-c000) + f_b*(c111-c110) }
}
```

All six formulas agree analytically on the shared faces, so a tie is well
defined; the fixed branch structure removes the f32 association difference that
would otherwise let CPU and GPU disagree by a ULP at a tie.

Tetrahedral is chosen because it is the interchange convention for 3D `.cube`
LUTs (the default in the major grading and colour-management tools, so a look
authored elsewhere reproduces here), because it uses 4 rather than 8 weighted
vertices per pixel, and because its fixed-branch form is deterministic across
two independent implementations. It is **not** chosen on an accuracy claim:
measured against non-affine references, tetrahedral has lower *mean* error than
trilinear (1.02×–1.16×) but can have *higher maximum* error (up to 1.7× on a
saturating S-curve at 17³), and both are exact for affine functions such as the
built-in looks. Any contract text claiming tetrahedral is uniformly more
accurate would be false. Rejected alternative: hardware trilinear filtering via
`textureSample` — rejected because Vulkan guarantees only a few bits of
sub-texel filter precision, so the GPU could not be compared against an exact
CPU reference at all (the same class of failure the CC1 §6.2 pixel-exact
sampling clause was added for). The legacy `cube_lut` compatibility stage keeps
its existing manual trilinear evaluation unchanged.

**Out-of-domain rule.** The lookup uses the clamped coordinate, and the
excursion is *added back* in the encoded domain: `z = y + (e - u)`. Chosen over a
pure clamp because a pure clamp collapses every over-range highlight onto one
lattice value, destroying information CC1 §2.2 invariant 5 exists to preserve;
the additive rule is the identity outside the domain when the LUT's boundary is
the identity, preserves ordering, and keeps a specular highlight recoverable by a
later correction or by the monitor clamp. It is stated as an explicit deviation
from the common "clamp to domain" implementation, and the fixture pins its
numbers. For a channel-mixing LUT the rule is an approximation outside the
domain (the excursion is restored per channel, not mixed); that is why the
built-in bakes cover `[-1, 2]` (§2.6) and why an imported `[0, 1]` look reports
its domain in the manifest.

**Mix in linear light.** `out = x + (look(x) - x) * mix`. Chosen because the
node's contract is scene-linear in, scene-linear out (CC1 §3), because linear
light is the project's blending space, and because a mix in the encoded domain
would need a *fourth* normative transfer decision and is undefined when
`input_encoding = linear`. Rejected alternative: mixing in the encoded domain
(what the legacy `lut_intensity` path did) — rejected for those reasons; the
endpoints `mix = 0` and `mix = 1` are identical either way, so no look is
unreachable.

**Exactness claim.** When `input_encoding = linear`, `domain = [0, 1]`, and
`S - 1` is a power of two, the lattice coordinates, the fraction, and the
interpolation weights are all exact binary fractions, so an identity lattice
reproduces the input **bit-exactly**, and the fixture asserts `to_bits`
equality. `S ∈ {2, 17, 33, 65}` all satisfy this. For any other `S`, or for
`display709` / `grade709` (whose f32 `pow` round trip is not bit-exact even
though the pair is an exact analytic bijection), the identity gate is
`LINEAR_CPU_GPU_MAX`, not bit equality. Both statements are asserted separately
in §10.3.2; neither is used to excuse the other.

### 3.6 Inactive nodes

Keyframes are resolved first (`Effect::evaluated_at`). A LUT node is
**inactive** when any of:

- evaluated `bypass >= 1` → reason `bypassed`;
- evaluated `mix_basis_points == 0` → reason `neutral`;
- evaluated `lut_asset_id == 0` → reason `unbound` (a new
  `ColorNodeInactiveReason` token; unreachable in a valid document).

An inactive node is the exact identity function: it **must not** be written to
the GPU buffer, **must not** occupy an atlas slot, and **must** be skipped by
the CPU reference. Neutrality is tested on the stored integers, never on floats.
That is what makes bypass and `mix = 0` **losslessly** identical to removing the
node, bit-for-bit, on CPU and GPU — the same mechanism CC3 §3.3 uses.

An identity *asset* is deliberately **not** a short-circuit condition. Rejected
alternative: skipping a node whose asset hash equals the pinned
`builtin:identity` hash — rejected because it would make the render depend on
content sniffing, and because the numeric identity path is exactly what §10.3.2
needs to prove.

## 4. GPU mechanism

### 4.1 One 3D texture atlas

Binding 3 is already a `texture_3d<f32>` with
`sample_type: Float { filterable: false }`, used only through `textureLoad`.
CC4 keeps that single binding and turns it into a **slot atlas**, so no new
binding is introduced. The measured reasons: WGSL has no `texture_3d_array`
(naga rejects the identifier), separate bindings are capped by
`max_sampled_textures_per_shader_stage = 16` in every wgpu profile (measured
failure at the 17th sampled texture on both lavapipe and NVIDIA), and a
depth-packed atlas addressed with `textureLoad` works on both adapters at 17³,
33³, and 65³.

- The atlas cache retains an `Arc` to every lattice it was built from and
  compares by `Arc::ptr_eq` plus size, so a cache hit can only mean the same
  verified allocation; keying on a raw pointer value would alias a freed and
  reused address. The process-wide parse cache is sha-keyed and bounded (MRU
  with a byte budget); a miss re-parses hash-verified bytes.
- Dimensions `(Smax, Smax, Σ S_k)` where `Smax` is the largest bound LUT edge
  and the sum runs over bound slots in slot order. Each slot `k` occupies
  `z ∈ [z_origin_k, z_origin_k + S_k)`; a slot smaller than `Smax` simply leaves
  the trailing texels of its `x`/`y` rows unused. Slot `k` is read as
  `textureLoad(atlas, vec3<i32>(x, y, z_origin_k + z), 0)` and uploaded with one
  `write_texture` per slot at origin `(0, 0, z_origin_k)` and extent
  `(S_k, S_k, S_k)`.
- Format `Rgba32Float`, the format already in production for the legacy LUT.
  Chosen over `Rgba16Float` on measurement: an f16 lattice costs up to
  `1.07e-3` linear error on a 33³ look (71% of the CC1 §6.2 in-gamut maximum
  and about half of the P99 and mean budgets) before any other stage, whereas
  an f32 lattice contributes `<= 1.5e-7`. With f32 the texel values are
  bit-identical to the parsed samples the CPU reference uses, so the only
  CPU/GPU divergence in a LUT node is the arithmetic order of the tetrahedral
  blend and the transfer `pow`. `textureLoad` on `Rgba32Float` needs no device
  feature on either adapter; only hardware filtering would, and hardware
  filtering is forbidden here.
- Slots: `COMPOSITOR_LUT_SLOTS_PER_LAYER = 4` managed slots plus
  `COMPOSITOR_LEGACY_LUT_SLOT = 4`, giving `COMPOSITOR_LUT_ATLAS_SLOTS = 5`.
  Worst case `65 × 65 × 325` texels = 20.95 MiB (21.97 MB); typical (one 33³
  look) 575 KB. The atlas is cached in the compositor keyed by the ordered
  `(sha256, size)` list of bound slots, so playback rebuilds it only when the
  set changes; the cache is not optional.
- Limit: the worst-case depth `5 × 65 = 325` exceeds the 3D limit of both
  `wgpu::Limits::downlevel_defaults()` and `downlevel_webgl2_defaults()` (256,
  measured; the existing limit-contract test uses the WebGL2 profile), so
  `compositor_required_limits` gains `COMPOSITOR_REQUIRED_TEXTURE_DIMENSION_3D = 512`
  raised the same way the storage-buffer constants are, and the limit-contract
  test asserts `COMPOSITOR_LUT_ATLAS_SLOTS * MAX_CUBE_SIZE <= COMPOSITOR_REQUIRED_TEXTURE_DIMENSION_3D`
  and that the downlevel base is raised to it. Production negotiates
  `wgpu::Limits::default()` (2048), so no production adapter changes behaviour.

The legacy external `cube_lut` becomes slot 4 of the same atlas. `LayerParams`
stays exactly 48 floats: the two existing `_uniform_padding` words become
`external_lut_z_origin` and `external_lut_size`, and `sample_external_lut`
reads them instead of calling `textureDimensions`. The legacy stage's trilinear
evaluation is otherwise unchanged.

### 4.2 Node record

The CC3 §3.2 record layout is unchanged — 64-byte stride,
`[kind, payload_word_offset, bypass, reserved, v0..v11]`, offsets indexing
`words`, one storage buffer, `COMPOSITOR_REQUIRED_STORAGE_BUFFERS_PER_SHADER_STAGE = 1`.
LUT nodes use **no payload region**, so `payload_word_offset` is `0` and
`COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE` stays `16384` (the worst case
in fact shrinks, because each LUT node displaces a 784-byte curve node).

`kind` is `4.0` for `technical_lut`, `5.0` for `creative_look`. Values:

| Word | Meaning |
| --- | --- |
| `v0` | `lut_slot`, `0..=3` |
| `v1` | `mix`, `0.0..=1.0` |
| `v2` | `input_encoding` token, `0.0`, `1.0`, or `2.0` |
| `v3..v5` | `domain_min` r, g, b (from the verified bytes) |
| `v6..v8` | `domain_max` r, g, b |
| `v9` | `size` `S` |
| `v10` | `z_origin` in the atlas |
| `v11` | reserved, `0.0` |

`GRADE_ABI_VERSION` goes `1 → 2`, because the buffer now carries kinds whose
interpretation depends on a companion texture binding. The shader's
`apply_color_nodes` dispatch treats an unrecognized kind as the identity; the
host must never write one, and a fixture asserts that every `ColorNodeKind` has
a shader branch.

### 4.3 Shader

`compositor.wgsl` gains `decode_display709` (the sign-preserving inverse of
the existing `encode_bt709`, which is reused unchanged; the source decode
`decode_bt709` is untouched), `lut_fetch(slot_size, z_origin, ir, ig, ib)`,
`lut_tetrahedral(...)` transcribing §3.5's branch structure verbatim, and
`apply_lut_node(values, rgb)`. The CPU and WGSL functions share the name
`decode_display709`. Fetches
use `textureLoad` on the atlas — **never** `textureSample`, and the sampler at
binding 1 is never used for the atlas.

### 4.4 CPU reference independence

`color_pipeline.rs` gains `Lut3d`, `LutNode`, and
`ColorNode::{TechnicalLut, CreativeLook}`. The CPU tetrahedral evaluator
**must** be written independently from the compositor's atlas builder and from
the shader, per CC3's rule that parity fixtures compare two implementations of
the written contract rather than one implementation with itself.

`resolve_color_nodes(effects)` becomes `resolve_color_nodes(effects, &LutLibrary)`;
the old symbol is kept as a wrapper over an empty library that fails with
`missing_lut_asset` when a LUT node is present, so no caller silently renders a
look-free frame.

## 5. Controls

All parameters are `ParamValue::Integer` with `uniform: EffectUniform::ColorNode`.
Bounds are inclusive; an omitted parameter resolves to its neutral.

### 5.1 `technical_lut`

| Parameter | Stored unit | Min | Max | Neutral | Meaning |
| --- | --- | ---: | ---: | ---: | --- |
| `lut_asset_id` | `LutAssetId` | 0 | 9007199254740991 | 0 | The project LUT asset this node applies. `0` is unbound and inactive. |
| `mix_basis_points` | 1/10000 | 10000 | 10000 | 10000 | Pinned at full strength. |
| `input_encoding_token` | enum token | 0 | 2 | 0 | `0` display709, `1` linear, `2` grade709. |
| `bypass` | boolean token | 0 | 1 | 0 | `1` makes the node the identity. |

`mix_basis_points` is pinned by its bounds rather than by a special case: a
partially applied *technical normalization* is not a meaningful state — the
source is either interpreted correctly or it is not — and identical descriptors
keep the manifest, the reset path, and the planner uniform across both kinds.

### 5.2 `creative_look`

| Parameter | Stored unit | Min | Max | Neutral | Meaning |
| --- | --- | ---: | ---: | ---: | --- |
| `lut_asset_id` | `LutAssetId` | 0 | 9007199254740991 | 0 | As above. |
| `mix_basis_points` | 1/10000 | 0 | 10000 | 10000 | Look strength. `0` is inactive; `10000` is the full look. |
| `input_encoding_token` | enum token | 0 | 2 | 0 | As above. |
| `bypass` | boolean token | 0 | 1 | 0 | As above. |

The neutral of `mix_basis_points` is `10000`, not `0`: a look node created with
only `lut_asset_id` set must show the look, and CC3 §2.4's "insert only the
values the operator touched" convention would otherwise create an invisible
node.

## 6. Reset, bypass, and keyframing

**Bypass** is the CC3 §5 mechanism unchanged: a serialized, undoable,
keyframe-able integer per node, set with `SetEffectParam`, reported in every
manifest as `"bypass": 1, "active": false` with reason `bypassed`. A bypassed
node keeps its position and its values. Bypass must never be implemented by
removing the effect, zeroing the mix, or a UI-only flag.

**Reset** uses the existing `color_node_reset_operations` generalization: one
`SetEffectParam` per descriptor parameter set to its neutral, plus
`ClearEffectKeyframes` for each automated parameter, emitted as one batch.
Resetting a LUT node would *unbind* it (`lut_asset_id → 0`), which
`validate_document` rejects — so the reset batch for a LUT node **must** exclude
`lut_asset_id` entirely, both its `SetEffectParam` and its
`ClearEffectKeyframes`, so a Hold-automated binding survives a reset; the
inspector's Reset is labelled "Reset look controls". A
`SetEffectParam` that would unbind a referenced node is rejected with
`MissingLutAsset`. No new operation kinds are needed for reset or bypass.

**Keyframing:**

| Parameter | Policy |
| --- | --- |
| `mix_basis_points` | Fully keyframable with any interpolation. This is the audition/blend-in control. |
| `lut_asset_id` | `Hold` only, enforced with the existing `OpError::NonHoldKeyframeParameter`. Interpolating between two asset ids is meaningless. Every id appearing in a Hold curve counts as a reference for `RemoveLutAsset` and for availability. |
| `input_encoding_token` | `Hold` only, same reason. |
| `bypass` | `Hold` recommended; `Linear` legal and resolved by the `>= 1` test, exactly as CC3 §6. |

A layer whose keyframed `lut_asset_id` resolves to different assets at different
frames still obeys the 4-slot limit *per rendered frame*, because slots are
assigned after keyframe evaluation.

## 7. Human UI

The roadmap records the human `.cube` workflow as the missing piece; CC4 closes
it.

- **Import.** `Look → Import LUT…` opens `rfd::FileDialog` filtered to `cube`,
  matching `choose_media` and `choose_relink_for_asset`. Parse, hash, and
  store-copy run on a worker thread with a `LutImportResponse` channel, exactly
  like the M41 relink probe; the UI never blocks. Success submits `AddLutAsset`
  and, when a clip is selected, `InsertEffect` for a `creative_look` bound to
  it, as one batch. Failure shows the typed code, the offending line, and the
  recovery action. Dropping a `.cube` onto the window routes through the same
  path as dropped media.
- **Look browser.** A list showing built-ins first, then project assets, each
  with title, size, provenance (`built-in` / file stem), and an availability
  chip (`verified` / `missing` / `changed` / `unreadable`) using the media
  card's warning treatment. Selecting a look on a clip with an existing
  `creative_look` retargets that node; `Add as new look` stacks a second one.
- **Mix.** One slider on the `creative_look` card only, `0..=100 %`, writing
  `mix_basis_points = percent * 100`; `technical_lut` hides `mix_basis_points`
  (pinned) alongside `lut_asset_id` and `input_encoding_token`.
  Live operations during the drag carry a gesture key so the actor replaces the
  top history entry; the release sends the final un-keyed commit. One gesture
  is one undo step; producing an entry per frame is a defect (CC3 §7).
- **Bypass and A/B.** Every node card has a bypass toggle. The look card
  additionally has a press-and-hold **A/B** control that sets `bypass = 1`
  while held and restores it on release, through the same coalesced gesture
  path, so the comparison is the real bypass and is provably lossless rather
  than a preview shortcut.
- **Sections.** The inspector renders colour nodes in stage order under three
  headings — **Input transform**, **Correction**, **Creative look** — each
  showing the node's stage index in `clip.effects`. Inserting from a heading
  computes the correct `InsertEffect` index, so a user can never author a
  stage-order violation. The generic slider loop hides `lut_asset_id` and
  `input_encoding_token`, which have dedicated controls
  (`should_render_effect_parameter` gains `technical_lut` and `creative_look`).
- **Status and recovery.** A `missing` or `changed` asset shows an inline banner
  on every node that references it, with `Locate file…` (restore, hash-checked)
  and `Replace…` (import a different LUT and retarget). Export and proof
  buttons report the same blocking status rather than failing at render time.
- `is_effect_insertable` excludes `look_lut` in addition to today's
  `color_grade` and `cube_lut`, because the managed kinds now cover it. A
  legacy node already in a project shows a **Convert to managed look** button
  (§9).

## 8. Agent surface

`INSPECTOR_TOOL_NAMES` grows from 66 to 71 (`list_look_assets`, `import_lut_asset`, `convert_legacy_look`, `plan_technical_lut`, `plan_creative_look`). Errors follow the CC1/CC2 shape:
`field`, `observed`, `allowed`, `recovery_action`; a refused or timed-out
confirmation is `import_refused { reason }`.

- **`convert_legacy_look`** — the only path that performs §9's
  `[AddLutAsset, ConvertLegacyLook]` batch for an agent (an agent cannot
  submit `AddLutAsset` itself and cannot register a built-in through
  `import_lut_asset`). Arguments `expected_revision`, `clip_id`, `effect_id`;
  for a `look_lut` it registers the built-in (reusing an existing record with
  the same hash) and converts; for a `cube_lut` it imports the effect's path
  into the store through the same confirmation gate as `import_lut_asset`
  first. `get_color_context.legacy_look_conversions[]` reports `ready` only
  when this tool can perform the batch, and every status carries a
  `recovery_action`. Because the generated operation tool for
  `Operation::ConvertLegacyLook` would carry the same name, that variant joins
  `RelinkAsset` and `AddLutAsset` in the ungenerated list; the raw operation
  itself remains accepted by `prepare_edit_plan`/`apply_edit_plan`.
- **`list_look_assets`** — read-only. Returns the timeline revision, the
  built-in catalogue (name, title, size, pinned sha256), and every project asset
  with `lut_asset_id`, `title`, `sha256`, `kind`, `size`, `byte_len`,
  `provenance`, `availability`, `store_path`, and the clip/effect ids
  referencing it. Compact: no samples, no domain arrays beyond the two integer
  triples.
- **`import_lut_asset`** — the only mutating media action CC4 adds. Arguments
  `expected_revision`, `path`, optional `title`. It routes through the same
  authorization path as the other destructive tools: the server calls
  `ConfirmationBroker::confirm("import_lut_asset", …)` with a description
  naming the file, its size, and the store it will be written to, before any
  byte is written. It then submits `AddLutAsset` through `apply_operation`
  under the given revision. Like `relink_media`, `AddLutAsset` **must not** be
  submittable through `apply_edit_plan` or `prepare_edit_plan` (the plan path
  has no way to write the store, so a plan-supplied record could reference
  bytes that do not exist); both reject the variant with that reason, and
  `AddLutAsset` **must** also be filtered out of the generated
  `operation_tools()` by name in `schema.rs` and rejected in the operation
  dispatcher, exactly as `RelinkAsset` is — `import_lut_asset` is the only path
  that can create a `LutAsset` record. The store root reaches the server
  through a shared handle owned by the project session (the saved project
  path; the server derives the store per use); when it is absent, the tool
  returns `project_not_saved`. `restore_lut_asset` is a human UI action in
  CC4 and is not an agent tool.
- **`plan_technical_lut`** and **`plan_creative_look`** — evidence-only,
  revision-gated, exactly modelled on `plan_color_wheels`. Arguments
  `expected_revision`, `clip_id`, `lut_asset_id`, optional `mix_basis_points` /
  `input_encoding_token` / `bypass` / `append`. They return `expected_revision`,
  `clip_id`, `target_effect_id`, `insert_index`, `source_profile`,
  `profile_assumption`, `requested_parameters`, `resolved_parameters`,
  `existing_color_node_count`, `lut_asset` (title, sha256, availability), and
  `operations` — the exact `InsertEffect` or `SetEffectParam` list the caller
  must submit through the ordinary edit-plan path. They apply nothing.
  Following the CC2 rule, an existing node of the requested kind is targeted
  with `SetEffectParam` unless `append: true`. The emitted `insert_index` is the
  first index that satisfies §3.2, so a plan can never be rejected for
  ordering. A plan referencing a `missing`/`changed` asset is returned with the
  availability status and a `recovery_action`, not silently. Built-in assets
  resolve to `verified` from the embedded bake without a store; only imported
  assets need a store root, and without one they report `unknown_no_store`.
  Branch and isolated agent servers receive the same project-path handle as
  the live server so agent sessions are never store-blind on a saved project.
- **`render_color_proof`** today proofs only a planned `primary_correction`.
  `RenderColorProofArgs` gains optional `effect_id` and
  `look_comparison: "before" | "after" | "bypass"`; when `effect_id` is present
  the tool proofs the *stored* node at that id (`parameters` must be absent),
  producing the full managed path with the node's stored `mix`, with the node
  absent, and with `bypass = 1` on a scratch copy of the document. The manifest
  states which variant it rendered and asserts the bypass variant is the
  byte-identical twin of the absent variant. The existing primary-only
  behaviour is unchanged when `effect_id` is absent.
- **`color_nodes` manifest** entries gain, for LUT kinds: `kind`, `role`
  (`technical`/`correction`/`creative`), `color_stage`
  (`input`/`correction`/`look`) with `color_stage_rank`, `stage_index`, `lut_asset_id`, `lut_title`, `lut_sha256`, `lut_size`,
  `lut_provenance`, `lut_availability`, `mix_basis_points`, `input_encoding`,
  `bypass`, `active`, `inactive_reason`. `get_color_context` and
  `get_qa_report` report `missing_lut_asset` the same way they report
  `legacy_lut_stage`.
- **Schema compactness (M36).** `schema.rs` must not enumerate the huge
  `lut_asset_id` range in the tool description; it emits
  `lut_asset_id (project LUT asset id; see list_look_assets)` and a one-line
  encoding-token legend, the same special-casing `color_curves` already
  receives.

## 9. Migration

1. Pre-CC4 projects load unchanged. No effect is renamed, no node is inserted,
   no `lut_assets` array is written until a look is added.
2. Legacy `look_lut` and `cube_lut` remain post-primary compatibility stages
   with unchanged rendering, unchanged `EffectCompatibilityStage::PostPrimaryLut`,
   and unchanged `legacy_lut_stage` reporting in `qa.rs`, `delivery.rs`, and
   `color_status.rs`. They are not managed nodes, they are not stage-ordered,
   and they still execute in the legacy branch after every managed node.
3. **Conversion from legacy to managed is always explicit.** The batch is
   `[AddLutAsset?, ConvertLegacyLook]` when the legacy node's position is legal
   for a `creative_look` under §3.2, and `[AddLutAsset?, RemoveEffect,
   InsertEffect]` — the same node, same effect id, inserted at the first
   legal look-stage index — when a legacy node sits before a managed
   correction node (legal in a pre-CC4 project because legacy stages are
   unordered). Both are one journaled, undoable batch and neither is
   automatic. For a `look_lut`, the client resolves `preset_token` to the
   built-in asset (§2.6 table, including token `0` → `identity`) and
   `intensity_percent` to
   `mix_basis_points = percent * 100`; for a `cube_lut`, it imports the external
   `path` into the store first. The batch is `[AddLutAsset, ConvertLegacyLook]`,
   journaled and undoable, and the UI shows a before/after proof before
   committing. It is **not** bit-identical to the legacy stage: the legacy path
   clamped to `[0, 1]` in display space, mixed intensity in the encoded domain,
   and used the non-invertible `decode_bt709` on the way back, while the managed
   node does none of those. That difference is exactly why CC1 §4's "no silent
   visual change" rule applies, and why conversion is never automatic on load.
4. `ColorPipelineState` stays `managed_sdr_v1`. Rejected alternative:
   `managed_sdr_v2` — rejected for CC3 §9's reason: `pipeline_state` describes
   the source → working → monitoring → delivery contract, not the inventory of
   nodes. CC4 changes no colour description and adds nodes inside a stage CC1
   already declared ordered and extensible; a bump would immediately fail
   `delivery.rs`'s managed-delivery check for every existing project with no
   semantic gain.
5. Save/reopen, journal replay, branch, undo, redo, and recovery preserve
   `lut_assets`, node positions, parameters, and keyframes byte-for-byte apart
   from documented JSON defaults.

## 10. Exit fixtures and numeric gates

The gate is a fixture suite in the style of `cc1_fixtures.rs` and
`cc3_fixtures.rs`, recorded as `crates/kinewright-media/src/cc4_fixtures.rs`.
Every fixture records the git revision, backend, adapter, software-fallback and
GPU-claim flags, OS, source profile, node stack, asset hashes, resolved
parameters, and output hashes.

### 10.1 Fixture-quality rules (from the CC1/CC2/CC3 reviews — normative)

1. Expected values are written out analytically from the §2/§3 equations,
   either as literal constants in this document or transcribed independently
   in f64 in the fixture. A fixture **must not** obtain an expected value by
   calling `Lut3d::apply`, `apply_color_nodes`, the compositor, or the shader.
2. Every control at minimum, maximum, and a representative interior value has a
   numeric expected value. `is_finite()` alone is never a sufficient assertion.
3. Parity rasters must contain samples that exercise every control, including
   out-of-domain samples for a LUT node. The raster asserts its own coverage.
4. Manifest tolerances are asserted equal to the code constants
   (`MONITOR_CPU_GPU_MAX`, `LINEAR_CPU_GPU_P99`, …), not restated as literals.
5. GPU fixtures run on a hardware adapter when no software fallback is present,
   recording honest provenance, instead of panicking or silently claiming GPU
   coverage. The software-fallback lane and the hardware lane stay distinct, and
   **the software lane is the default lane**.
6. Error assertions check `field`, `observed`, and `allowed`, not just the error
   variant or the field name.
7. A precision gate must use a **non-dyadic** LUT: an identity lattice at 33³ or
   65³ is exactly representable in f16 and would make a lattice-precision gate
   vacuous (measured `0.0` error). The parity fixture therefore uses a real 33³
   look with non-dyadic samples.

**The lavapipe lesson applies unchanged.** A layer whose source raster has the
output raster's shape with no geometric stage **must** sample with point
filtering on every adapter (CC1 §6.2's pixel-exact sampling clause). Mesa
lavapipe blends `2^-15`–`2^-14` of a neighbouring texel into a
bilinear-filtered "identity" layer where the NVIDIA adapter returns every texel
exactly; a tetrahedral lookup near a lattice boundary amplifies such a
perturbation into a visible branch flip, so the CC4 parity gate depends on that
clause, not on tolerance width. No epsilon guard is added to the interpolation.

### 10.2 Raster

CC4 reuses `cc3_parity_raster()` from CC3 §10.2 verbatim — 24 linear levels × 8
channel patterns = 192 samples, spanning negatives, `0..1`, and values to 4.0 —
and adds one assertion: after `display709` encoding, at least 40 of the 192
samples fall outside `[0, 1]` (72 do, measured), so the §3.5 out-of-domain rule
is exercised non-vacuously.

### 10.3 Required fixtures

1. **Parsing.** 3D sizes 2, 17, 33, 65; `DOMAIN_MIN`/`DOMAIN_MAX` including a
   negative domain; `TITLE` capture including a quoted title; `#` comments;
   blank lines; CRLF; lowercase keywords; scientific notation; a UTF-8 BOM.
   Rejections with `field`/`observed`/`allowed` asserted: `LUT_1D_SIZE` →
   `unsupported_lut_format`; size 1 and 66 → `lut_size_out_of_range`; a
   two-value data line and a repeated `LUT_3D_SIZE` → `malformed_lut_file` with
   the 1-based line number; `DOMAIN_MIN == DOMAIN_MAX` → `lut_domain_invalid`;
   `3*S³ ± 1` samples → `lut_sample_count_mismatch`; `NaN` and `inf` →
   `lut_sample_not_finite`; non-UTF-8 bytes → `malformed_lut_file`.
2. **Identity.** (a) An identity lattice at `S ∈ {2, 17, 33, 65}`, domain
   `[0, 1]`, `input_encoding = linear`, `mix = 10000`, on the §10.2 raster
   restricted to `[0, 1]`: output is bit-identical to the input, asserted with
   `f32::to_bits`, on the CPU reference and on the GPU. (b) The same at
   `display709` and `grade709`: identical within `LINEAR_CPU_GPU_MAX`, with the
   maximum observed deviation recorded. (c) A bypassed non-neutral LUT node, a
   `mix = 0` node, and an unbound node each produce output bit-identical to the
   same stack with the node removed, in linear working values and monitor
   RGBA8, on CPU and GPU. The unbound case is constructed below the operation
   layer, directly against `resolve_color_nodes`, because §3.3 makes it
   unreachable through Core; the fixture also asserts that `AddEffect` /
   `InsertEffect` of a LUT node with `lut_asset_id` omitted or `0` is rejected
   with `MissingLutAsset { lut_asset: 0 }`.
3. **Interpolation anchors.** LUT B, `S = 2`, domain `[0, 1]`, lattice
   `V(0,0,0)=(0,0,0)`, `V(1,0,0)=(.5,0,0)`, `V(0,1,0)=(0,.5,0)`,
   `V(1,1,0)=(.5,.5,0)`, `V(0,0,1)=(0,0,.5)`, `V(1,0,1)=(.5,0,.5)`,
   `V(0,1,1)=(0,.5,.5)`, `V(1,1,1)=(1,1,1)`:

   | Input `e` | Branch | Expected `lut(e)` |
   | --- | --- | --- |
   | `(0.75, 0.50, 0.25)` | `f_r > f_g > f_b` | `(0.500000, 0.375000, 0.250000)` |
   | `(0.25, 0.50, 0.75)` | `f_r <= f_g`, `f_b > f_g` | `(0.250000, 0.375000, 0.500000)` |
   | `(0.50, 0.50, 0.50)` | tie → final else | `(0.500000, 0.500000, 0.500000)` |

   The first case is asserted to differ from trilinear interpolation of the
   same lattice, whose value is `(0.421875, 0.296875, 0.171875)`, so the fixture
   proves tetrahedral is actually implemented. The tie case is additionally
   evaluated through all six formulas and asserted equal, confirming ties are
   well defined. (All three rows were re-derived by hand during contract
   review.)

   LUT C, `S = 3`, domain `[0, 1]`, separable lattice
   `V(i,j,k) = (f(i), f(j), f(k))` with `f = (0, 0.25, 1.0)`:
   `lut(0.75, 0.25, 0.50) = (0.625000, 0.125000, 0.250000)`.

   LUT D, the same lattice with `DOMAIN_MIN = -0.5`, `DOMAIN_MAX = 1.5` on every
   channel: `lut(0.50, 0.00, 1.00) = (0.250000, 0.125000, 0.625000)`. This is
   the domain-mapping anchor.

   Every number above is an exact binary fraction, so the assertions are
   equalities within `LINEAR_CPU_GPU_MAX` and the CPU case is exact.
4. **Out-of-domain.** With LUT D: `e = (2.0, 2.0, 2.0)` clamps to
   `(1.5, 1.5, 1.5)`, whose lookup is `(1, 1, 1)`, so the node output is
   `(1.5, 1.5, 1.5)`: the `0.5` excursion above `dmax` is restored on top of the
   boundary value, which for this lattice is `1.0`, not `1.5` — the output is
   neither the clamp result nor the input, which is what keeps the ordering of
   over-range highlights. `e = (-1.0, -1.0, -1.0)` clamps to
   `(-0.5, -0.5, -0.5)`, lookup `(0, 0, 0)`, output `(-0.5, -0.5, -0.5)`. A pure-clamp implementation would
   return `(1,1,1)` and `(0,0,0)`; the fixture asserts the additive result and
   asserts the difference from the clamp result, so the rule cannot silently
   regress. Monotonicity across the domain boundary is asserted on the 8-bit and
   10-bit neutral ramps.
5. **Mix.** With LUT B, `input_encoding = linear`, `x = (0.75, 0.50, 0.25)`,
   `look(x) = (0.5, 0.375, 0.25)`: `mix = 0` → `(0.75, 0.5, 0.25)` bit-identical
   to the node removed; `mix = 5000` → `(0.625000, 0.437500, 0.250000)`;
   `mix = 10000` → `(0.500000, 0.375000, 0.250000)`. Endpoints and midpoint
   asserted on CPU and GPU.
6. **Ordering.** A stack
   `[technical_lut, primary_correction, color_wheels, color_curves, creative_look]`
   renders identically on CPU and GPU and matches the CPU reference evaluated in
   that vector order. `[creative_look, technical_lut]` with the same values
   produces a *different* result when evaluated in vector order (asserted
   directly against the CPU reference, since the document cannot store it), and
   storing it is **rejected** with `ColorStageOrderViolation` asserting
   `color_stage_rank`, `previous_color_stage_rank`, and both effect ids — through `AddEffect`, through
   `InsertEffect`, and through `validate_document`. `InsertEffect` at a legal
   index for a technical LUT ahead of an existing primary succeeds and preserves
   every other effect's relative order.
7. **CPU/GPU parity.** The CC1 §6.2 numbers are reused verbatim and asserted
   equal to the code constants: monitor max `<= 2`, P99 `<= 1`, mean `<= 0.50`;
   neutral identity max `<= 1`, P99 `<= 1`, mean `<= 0.25`; linear (on samples
   with `|value| <= 1`) max `<= 1.5e-3`, P99 `<= 7.5e-4`, mean `<= 2.5e-4`; the
   `(1, 2]` band uses the banded half-float numbers `<= 9.765625e-4`; samples
   with `|linear| > 2` are excluded from the linear gate, counted, recorded, and
   remain subject to the monitor-code, finiteness, and monotonicity gates. Run
   on the §10.2 raster with a real, non-dyadic 33³ look at three mix values and
   with the full five-kind stack, on the software fallback by default and on a
   hardware adapter in the explicit `--ignored` lane. A non-neutral case that
   changes fewer than 5 % of CPU-reference samples fails as vacuous. No new
   tolerance is invented.
8. **Slots and limits.** Four LUT nodes on one layer render correctly with
   distinct atlas slots and distinct `z_origin` values; a fifth is rejected with
   `TooManyLutNodes { limit: 4, actual: 5 }`. A mixed-size stack (2, 17, 33, 65)
   produces an atlas of `65 × 65 × 117` and each node reads its own slot.
   `COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE` and
   `COMPOSITOR_REQUIRED_STORAGE_BUFFERS_PER_SHADER_STAGE` are asserted unchanged
   at `16384` and `1`; `COMPOSITOR_REQUIRED_TEXTURE_DIMENSION_3D` is asserted
   `>= 5 * 65`; `GRADE_ABI_VERSION` is asserted `2`; every `ColorNodeKind` is
   asserted to have a shader branch.
9. **Legacy coexistence.** A clip carrying a managed `creative_look` and a
   legacy `cube_lut` renders both with the legacy stage last **regardless of
   their relative order in `clip.effects`** (the legacy branch runs after every
   managed node, as CC1 §3 already scopes); the fixture asserts this for both
   orderings, with the legacy stage in atlas slot 4, and reports
   `legacy_lut_stage` exactly once in each.
10. **Built-in bake determinism.** Each built-in's serialized bytes hash to a
    pinned literal sha256 asserted in the fixture; the bake is byte-identical
    across two invocations and across platforms (LF endings and `{:.6}`
    formatting asserted directly on the text). Each built-in node's output on
    the CPU reference is compared against the closed-form formula of §2.6 on
    the §10.2 raster to within `2e-6` in display code (measured 1.1e-7 to
    4.5e-7), proving the affine reproduction claim and the `[-1, 2]` domain
    coverage. The GPU cannot meet a `2e-6` display-code gate — one
    `Rgba16Float` storage step at a mid-grey display code is ~1e-4 — so the
    GPU is gated against the same f64 closed form carried through the
    normative f16 quantization under the CC1 §6.2 banded gate, with the
    display-code deviation recorded (measured 3.2e-4 to 7.6e-4).
11. **Relocatable project proof** (owned by `crates/kinewright-app`, because
    save, open, and Save As live there; `save_project` **must** be refactored so
    path selection is separated from a dialog-free
    `write_project(&mut self, path) -> Result<ProjectSaveReport, _>` that the
    fixture drives). Import a LUT, apply a look, save the project, and render a
    full-raster monitor proof; hash it. Copy the `.kinewright` file
    *and* the `.kinewright-assets` directory into a fresh temp directory with a
    different name for the parent, reopen, render, and assert the output hash
    is **bit-identical**. Copy the project file **without** the store: reopen
    succeeds, the asset reports `missing`, `render_color_proof` and export are
    blocked with `missing_lut_asset` naming the asset, hash, expected store
    path, and recovery action, and no frame is produced. Then
    `restore_lut_asset` with the original file: the render hash returns to the
    first value bit-identically. Save As into a third directory copies the store
    and reproduces the same hash again.
12. **Recovery rejections.** `restore_lut_asset` with a different file →
    `lut_relink_hash_mismatch` with `expected`/`observed`; the store is left
    untouched. Corrupting one byte of a store file → `changed`, with the same
    blocking behaviour and the observed hash reported. `RemoveLutAsset` while
    any node references it — active, bypassed, or via a `Hold` keyframe value —
    → `LutAssetInUse` naming the clip and effect; after the referencing nodes
    are removed, it succeeds.
13. **Serialization and history.** `AddLutAsset`, `RemoveLutAsset`,
    `InsertEffect`, `ConvertLegacyLook`, `SetEffectParam`, `SetEffectKeyframes`,
    `ClearEffectKeyframes` all save/reopen, journal-replay, undo, and redo with
    values and vector positions preserved exactly. A pre-CC4 project round-trips
    without a `lut_assets` key. Rejections asserted atomically with
    `field`/`observed`/`allowed`: duplicate id, malformed hash, `byte_len == 0`,
    size 66, `kind: cube_1d`, dangling `lut_asset_id`, `lut_asset_id` out of
    range, a non-`Hold` keyframe on `lut_asset_id` and on
    `input_encoding_token`, an out-of-range `mix_basis_points` on
    `technical_lut` (min = max = 10000), `kind: cube_1d` as
    `InvalidLutAssetMetadata { field: "kind" }` at the operation layer (the
    parser reports it as `unsupported_lut_format`), and a
    `lut_asset_metadata_mismatch` from a hand-edited `size`. These operation
    cases live in `crates/kinewright-core/tests/cc4_core.rs`.
14. **Agent plan-not-apply and authorization.** `plan_technical_lut` and
    `plan_creative_look` return exact operations, bind to the analyzed
    revision, fail closed on a stale revision, and leave the source document
    byte-identical. `import_lut_asset` refused by the confirmation broker writes
    **no** store file and changes no document. `AddLutAsset` submitted through
    `apply_edit_plan` or `prepare_edit_plan` is rejected, and the generated
    `add_lut_asset` operation tool is absent from the registry. The refusal
    case uses the real `ConfirmationBroker` with `wait_for_request` /
    `invoke_in_background` and `reject`, as the existing export tests do; no
    test double is needed. These cases live in `crates/kinewright-agent`.
    `list_look_assets` and `render_color_proof` mutate nothing. Manifests list
    ordered stages with role, stage, asset hash, availability, mix, encoding,
    and bypass, and the `render_color_proof` bypass variant is byte-identical to
    the node-removed variant.

No tolerance may be used to excuse a missing asset, a hash mismatch, an
intermediate clamp, a wrong stage order, hardware-filtered LUT sampling, or a
stale legacy stage.

## 11. Explicit deferrals

- 1D shaper LUTs — reserved in `LutAssetKind` with no schema migration
  required, but they need their own interpolation, extrapolation, and
  per-channel contract and their own gates. Not delivered.
- LUT authoring, baking a grade to a LUT, and `.cube` export.
- Look groups, managed group apply, and copy-paste of a look between clips
  (post-CC5 workflow).
- Per-look gamut and legal-range warnings, and any "this look clips" diagnostic
  (CC6).
- Log and camera-native source profiles, which are what a genuinely technical
  camera LUT needs (CC6/CC7).
- ACES, OCIO, `.3dl`/`.look`/`.dat` formats, and CDL sidecar interchange.
- Store garbage collection of unreferenced files; the UI states that removing
  an asset leaves its bytes.
- Automatic look selection or ranking. CC4 planners are request-driven and
  evidence-only.

CC4 is complete only when an editor can import a `.cube` from a file picker,
audition it at adjustable strength against a lossless bypass, see exactly where
it sits relative to the technical transform and the corrections, move the
project to another machine with one directory, and recover explicitly when the
bytes are not there.

## 12. Implementation order

1. **Core model and operations.** `crates/kinewright-core/src/model.rs`
   (`LutAssetId`, `LutAsset`, `LutAssetKind`, `LutAssetSource`,
   `Document.lut_assets`, dangling-reference invariants);
   `crates/kinewright-core/src/effect.rs` (`technical_lut` / `creative_look`
   descriptors, `ColorNodeKind::{TechnicalLut, CreativeLook}`, storage tags
   4/5, `ColorStage`, `ColorNodeInactiveReason::Unbound`,
   `MANAGED_COLOR_NODE_NAMES` 3→5, `LUT_NODE_LIMIT_PER_LAYER`);
   `crates/kinewright-core/src/operation.rs` (`AddLutAsset`, `RemoveLutAsset`,
   `InsertEffect`, `ConvertLegacyLook`, stage-order validation, `Hold`-only
   keyframe rules, the new `OpError` variants); `qa.rs`/`delivery.rs`
   (structural `missing_lut_asset`); `media.rs` (`LutAvailabilityKind`,
   `LutAvailabilityStatus`, `ExportLutPreflightReport`,
   `export_lut_preflight_with`); core unit tests in `tests/contracts.rs` and a
   new `tests/cc4_core.rs`.
2. **Parser, store, and library.** `crates/kinewright-media/src/lut.rs` (typed
   `LutParseError`, `TITLE`, max size 65, BOM); new
   `crates/kinewright-media/src/lut_store.rs` (`LutStore`, `import_lut_asset`,
   `availability`, `restore`, atomic write, hash-keyed parse cache); new
   `crates/kinewright-media/src/builtin_looks.rs` (pinned serializer, five
   bakes, pinned hashes); `LutLibrary`, the store's availability resolver, and
   `Playback::set_lut_library`.
3. **CPU reference math.** `crates/kinewright-media/src/color_pipeline.rs`
   (`decode_display709`, `Lut3d`, `LutNode`, tetrahedral evaluation written
   independently, `ColorNode` variants, `resolve_color_nodes` taking a
   library).
4. **GPU ABI and shader.** `crates/kinewright-media/src/compositor.rs` (atlas
   builder and cache, slot assignment, node record words,
   `GRADE_ABI_VERSION = 2`, the two reclaimed `LayerParams` padding words,
   `COMPOSITOR_REQUIRED_TEXTURE_DIMENSION_3D`, new slot constants and limit
   assertions); `crates/kinewright-media/src/compositor.wgsl`
   (`display709_encode/decode`, `lut_fetch`, `lut_tetrahedral`,
   `apply_lut_node`, dispatch for kinds 4/5, legacy slot addressing).
5. **Fixtures.** New `crates/kinewright-media/src/cc4_fixtures.rs`, registered
   in `lib.rs`, reusing `cc1_fixtures.rs` helpers for provenance, diff metrics,
   the banded linear gate, and evidence emission, and `cc3_fixtures.rs`'s
   raster; `tests/fixtures/cc4_manifest.json`.
6. **Agent surface.** `crates/kinewright-agent/src/color_status.rs`
   (`plan_technical_lut`, `plan_creative_look`, `list_look_assets`, LUT entries
   in `color_node_value` / `color_node_manifest`);
   `crates/kinewright-agent/src/server.rs` (dispatch, the `import_lut_asset`
   confirmation path, the store-root handle, the `apply_edit_plan` /
   `prepare_edit_plan` / generated-tool rejections, `render_color_proof` look
   comparison with the `RenderColorProofArgs` extension in `color_status.rs`);
   `crates/kinewright-agent/src/export_queue.rs` (LUT preflight blocking
   variant);
   `crates/kinewright-agent/src/schema.rs` (tool names 66→71, compact
   descriptor summarization, legacy labels for `look_lut`/`cube_lut`
   unchanged).
7. **Human UI.** `crates/kinewright-app/src/inspector_ui.rs` (stage headings,
   look card, mix slider with coalesced undo, bypass, A/B hold, availability
   banner, reset excluding `lut_asset_id`, `is_effect_insertable` excluding
   `look_lut`); new `crates/kinewright-app/src/look_browser_ui.rs`;
   `media_workflow.rs` (import/restore worker threads and channels);
   `export_ui.rs` (LUT preflight rendering and gate);
   `app.rs` / `project.rs` (store root derivation, dialog-free
   `write_project`, Save As store copy, library publication, the shared
   store-root handle for the MCP server, `.cube` drop); the §10.3.11
   relocatability fixture.
8. **Docs.** This file; `docs/ROADMAP-AND-WORKFLOWS.md` current-status bullets
   and the CC4 staged row; `CHANGELOG.md`.

Steps 1 → 2 → 3 → 4 are strictly ordered. Step 5 depends on 3 and 4. Steps 6
and 7 depend on 1 and 2 and may proceed in parallel with 3–5.

## 13. Risks

- **Texture memory and downlevel adapters.** The atlas can reach 21 MiB at the
  worst case. Mitigation: allocate depth from the *bound* slots only, cache by
  the `(sha256, size)` slot signature so playback does not re-upload, keep the
  binding count unchanged, raise and assert
  `COMPOSITOR_REQUIRED_TEXTURE_DIMENSION_3D`, and keep the sample type
  `Float { filterable: false }` so no filtering capability is newly required.
- **Store/document divergence.** Someone hand-edits `size` or a domain mirror
  and the render disagrees with the manifest. Mitigation: the verified bytes
  are the sole rendering authority and a disagreement is the typed
  `lut_asset_metadata_mismatch`, asserted in §10.3.13.
- **Import path security.** The store must only ever be written under the
  project directory. Mitigation: the file name is the 64-character validated
  hash, never user text, so traversal is structurally impossible; the store
  root is derived from the project path's parent and created with
  `create_dir_all`; an import when the project path has no parent, or when the
  root exists as a non-directory, fails typed. The import reads the source path
  the user chose and writes nowhere else.
- **Hashing and parsing large files.** A 65³ `.cube` is ~7.5 MB of text;
  parsing plus hashing on the UI thread would stutter. Mitigation: both run on
  the same worker-thread pattern M41 established for relink probes, with a
  typed response channel; the fixture times a 65³ import to keep the budget
  honest.
- **Agent import authorization.** `import_lut_asset` writes to disk before any
  document change, so a refused confirmation must leave nothing behind.
  Mitigation: confirmation is requested before the first byte is read, the
  write is a temp-file rename, and §10.3.14 asserts a refused import leaves no
  store file and no document change.
- **Ordering rejection ergonomics.** A user or planner that appends a technical
  LUT to a graded clip gets a rejection instead of a result. Mitigation:
  `InsertEffect` plus the inspector's stage headings and the planners' computed
  `insert_index` mean no ordinary path can produce the rejection; the error
  still names both nodes and both stages so a hand-written plan is diagnosable.
- **Legacy conversion expectations.** A user converting a legacy look will see a
  small change wherever the old display clamp or encoded-domain mix was active.
  Mitigation: the conversion shows a before/after proof, the difference is
  documented here, and the operation is undoable — the alternative, converting
  silently on load, is forbidden by CC1 §4.
