# JA-2: person separation and a face-aware look

Status: **design accepted 2026-10-20** (revision 3). Implementation has not started. Every model,
fixture, runtime and performance gate is NOT RUN. Numerical limits are proposed targets until the
Stage A measurements replace them.

Part of job A (talking head) in the [product brief](PRODUCT-BRIEF-2026-10.md#programme-mapping).
It consumes the JA-1 Studio host's single project writer and store ownership; JA-1 is still in
design review, so the seam in §§4–6 follows JA-1 as accepted.

Review trail, under `target/review/ja/`:
- Astra critic: `ja2-person-separation-critic.md` (revise; 1 Blocker, 5 Major, 1 Minor).
- Round 2 on rev2: `ja2-person-separation-critic-r2.md` (revise; one new Major).
- Narrow round 3 on rev3: `ja2-person-separation-critic-r3.md` (accept).
- Lead verification of licences and runtime versions: `ja2-lead-verification.md`.
- Lead decisions: `task-ja2-rev2.md`, `task-ja2-rev3.md`.

Bare file names below such as `ja1-studio-orchestration-design-rev1.md` refer to that directory.

## 1. Outcome, scope and boundaries

R1. Give Riel a believable talking head from the C920 recording in an ordinary room, with stable
hair and hands, a natural face, and either subtle blur or an editable replacement backdrop.
Songbird charts and captions remain ordinary layers above or beside the presenter.
Riel reviews the finished tight-end video and points at changes without opening the timeline.
The 30–60 minutes of Riel's time, publish-without-timeline bar, script retakes and grounded data are
binding in `docs/PRODUCT-BRIEF-2026-10.md`, “What done means”, “How it works” and “The first video”.

R2. Deliver reusable source analysis, editable separation/background effects, conservative face
measurements, revision-bound plans and QA evidence. Model inference runs before playback or export.
Changing a backdrop, blur strength or framing reuses valid source analysis.
The initial domain is one seated presenter in opaque camera footage, SDR, at most 2,073,600 source
pixels, normally 1080p30. Transparent source media is unsupported by the initial estimator.
Glasses, flyaway hair, beard, fingers, a microphone and the actual chosen crops are acceptance cases.
Do not promise arbitrary people, transparent props, motion-blur recovery or repair of clipped footage.
The job order and H-T dependency follow `docs/ROADMAP-AND-WORKFLOWS.md`, “Job-driven order”.

R3. Colour decontamination of recorded person pixels at fractional edges is allowed by lead
decision 1 in `task-ja2-rev2.md`. It precedes nonlinear grading and cannot create a person region
absent from the source. At zero person coverage, foreground RGB is zero and contributes nothing.
No generative fill, face replacement, beauty reshaping, AI image/video generation or invented facts.
Riel may supply a backdrop image. Newly authored backdrops are solid, gradient or vector/code graphics.
Never bundle Python, Node, OpenCV, a browser engine or a training environment.

R4. Use JA-1's single persistent project host and its project-store ownership, not a second owner.
No general cache daemon, general tracking system or runtime framework is part of this design.
Physical depth, physical relighting, live camera inference and general rotoscoping remain deferred.
Each mechanism below exists for a named rule or gate. Local fill and below-host wrap, including
moving-window fitting and extra passes, are deferred. Re-admit either only if the real tight-end take
demonstrates a benefit. The default blur plus natural-look cut must clear G14 without either.

## 2. What Riel sees

1. Riel supplies the script, free retakes and Songbird JSON. The agent picks usable takes and prepares
   a short look comparison while other independent editing proceeds.
2. The agent records “natural appearance, subtle room blur, preserve the visible microphone” as
   reversible defaults. It inspects gestures, hair, lighting and intended framing before asking anything.
3. “Preparing background” shows progress, usable ranges and cancellation. The previous usable cut
   remains available. Incomplete separation is never labelled Ready.
4. The finished cut uses subdued blur. A second version may show a restrained MockingBoard gradient
   with room for the supplied tight-end charts. Riel sees the actual take in each version.
5. “My hair disappears here” or “less warm” targets the displayed version and frame. The agent
   changes ordinary effects, shows nearby motion and reruns the affected QA.
6. Enabled separation requires complete, matching analysis for export. An intentional Off/bypass
   edit can instead be reviewed and exported as the chosen original-room look.

An enabled effect with missing analysis is a preflight failure, never an automatic export fallback.
Choosing Off is an explicit undoable edit with an updated review receipt. It is a valid product outcome,
not a claim that separation passed. Do not block a private practice video on public-fixture permission.
Completion names passed, failed and not-run checks plus version/frame/artifact identity, following
`target/review/refs/synthesis-hyperframes.md`, items 5 and 14.
Studio feedback and headless completion consume JA-1's version/host contracts, not a separate JA-2 UI owner.

## 3. Model, runtime and installation decision

### 3.1 One initial model and one shipping runtime

R5. Trial RVM MobileNetV3 with full source raster and a 0.25 internal downsample ratio at 1080p.
Use FP32 CPU as the reference and admit FP16 CUDA only after parity and quality checks.
RVM's recurrence, separate alpha/foreground outputs and suggested portrait ratio are documented by
the upstream inference page cited in `ja2-person-separation-critic.md`, “Source spot-checks”.
No local C920 quality, speed, RAM or VRAM result exists in this design round.

| Candidate | Decision and quality judgment | Code, weights and bytes |
| --- | --- | --- |
| RVM MobileNetV3 | First trial. Recurrence fits video edges, inferred. Hair/noise/punch-in quality and CPU/3090 costs need measurement. | Lead records code as GPL-3.0 and released weights as GPL-3.0 by provenance. They are assets of the GPL repository with no separate weight terms stated. Reported ONNX sizes are 14,975,696 bytes FP32 and 7,503,483 FP16. Hash actual files before use. |
| MODNet | Alternative if RVM fails clearance or quality. Independent-frame temporal quality must pass the same gates. | Lead verified Apache-2.0 for code and models, excluding demo GIFs. Exact export, bytes, hashes and local costs need verification. Trial admission ceiling 32 MiB, not a size claim. |
| MediaPipe selfie segmentation | Deferred. Category confidence does not establish fractional hair coverage; another conversion/runtime route is unnecessary. | Exact selected code/model terms, model bytes and CPU/3090 costs are not established here. |
| BiRefNet | Deferred still-image challenger. Detail alone does not establish stable video alpha. | Rev1 cites MIT code/model labels. Exact variant, notices, hashes, size and local costs need verification before any trial. |
| SAM 2-class models | Deferred. Prompted object masks still need soft-edge matting and bounded state. | Rev1 cites Apache-2.0 code/checkpoints. Selected export, size and local memory/speed need verification. |

The RVM, MODNet and YuNet provenance decisions come from `ja2-lead-verification.md`, items 1–3.
Do not describe the RVM decision as a separately published weight licence. Record its provenance verbatim
in the installation manifest and carry the applicable source/notices with redistribution.
Upstream tensor throughput does not certify Kinewright end-to-end throughput.
The wrapper is MIT OR Apache-2.0 per lead verification item 4. Rev1 §3.2 cites native ORT's MIT
licence; verify the selected native release and every provider/redistributable notice in Stage A.

R6. The binding first trial is optional `ort = "=2.0.0-rc.15"`, with default features disabled.
This is a prerelease, not stable ort 2.x. Keep its API behind one media-owned person-analysis boundary
that accepts immutable input/recipe handles and returns typed analysis results. Core, project files,
playback and planners contain no ort types. Preserve the local unsafe-code prohibition in `Cargo.toml:50`.

The trial feature allowlist is `std` and `load-dynamic`, plus `cuda` only in the Linux CUDA lane.
CPU is the explicit provider in Linux CPU and Windows MSVC CPU lanes. DirectML and Windows CUDA
certification are deferred. No download/copy-binary default features or implicit provider fallback.
Before the trial builds, Stage A must verify these names against the exact RC manifest and explicitly
record any API-level feature required by that RC. No unreviewed default/API feature is admitted.
The supplied evidence does not identify that feature or a numeric native ORT/CUDA/cuDNN combination.
Those exact pins remain a Stage A entry deliverable, not guessed compatibility facts.

R7. Every executable trial profile requires a sealed native manifest containing all of these values:

- Exact wrapper and resolved ort-sys versions, complete feature sets, target triple and build flags.
- Exact ORT release/build, C API level, library filenames, lengths, SHA-256 and publisher/source URLs.
- Explicit CPU or CUDA provider, provider library/dependency hashes, exact CUDA/cuDNN versions and
  minimum driver requirement where applicable. Record the provider actually assigned to graph nodes.
- Model hashes, graph opset, input/output layouts, recurrent-state shapes, precision and preprocessing.
- Required notices, install/download bytes, installed bytes, build time/peak RSS and cold-start result.

Empty fields mean no approved runtime profile. The manifest must name exact artifacts, never “latest”,
an ABI family alone or a library found opportunistically on PATH. Reject mismatches before session creation.
Stage A must resolve the numeric native pins from the RC's compatibility data and then test them.
There is no accepted native manifest in this design-only output. A deliberate upgrade creates a new
profile and reruns install/offline, graph parity, quality, resource and old-artifact reopen checks.
This resolves the runtime choice while preserving the verification gap identified in the lead's note.

| Runtime route | Decision |
| --- | --- |
| ort / native ORT | Selected first trial because it consumes upstream ONNX without a model port. Native packaging and Windows MSVC costs are early rejection criteria. |
| tract | Cost-driven fallback if ORT fails Stage A. Verify selected-release RVM/YuNet graphs, recurrence, actual CPU/CUDA support and MSVC build before substitution. No assumed drop-in parity. |
| burn | Deferred. ONNX import and portable GPU backends are credible, but graph parity and allocation sharing require work. The word wgpu does not establish shared memory. |
| candle or direct neural wgpu kernels | Deferred. Owning a model port or inference kernels is unnecessary for Job A. Use wgpu for ordinary filtering/compositing. |

There is one shipping runtime. A fallback replaces the selected implementation after the same gates;
it does not add a second installed runtime or a generic backend registry.
CPU analysis may take unattended hours, permitted by the product brief, “What done means”.

R8. Model/runtime setup is an explicit first-use action or a verified person-installed path.
Show publisher, licence, bytes, destination and hashes before installation. Import, open, playback and
export do not download anything. Incomplete installs are not visible as installed; work is offline after setup.
Model/native libraries stay machine-local, outside projects and portable packages.
Old-project open remains functional without them. Complete cached mattes can render without inference installed.
The repo already centralizes dependency pins and includes PNG support (`Cargo.toml:18`, `Cargo.toml:34`).
No new codec is selected before Stage A demonstrates that the existing PNG route fails.

## 4. Source identity, ranges and recurrent analysis

R9. Add the narrow owned-input lease accepted by lead decision 3 and critic answer Q2.
Reuse an equivalent subsequently verified facility, but the inspected path stores a pathname and opens
it later (`crates/kinewright-media/src/render.rs:307`, `render.rs:325`). It is not already an immutable lease.
PF1 explicitly leaves decoder-bound snapshots unresolved in `docs/PF1-PLAYBACK-PERFORMANCE.md`,
“PF1-LH R97 checkpoint”, A-1. Pre/post hash or stat checks cannot replace this contract.

The project host reserves disk, copies source bytes into an exclusive owned object while hashing them,
compares the copied digest to the requested identity, seals it, and lends read-only snapshot handles.
Analysis, separated preview, proofs and export decode those same pinned bytes. Verify owned-root/file
identity at admission/use. Reject external-resource containers unless all dependencies can be owned.
Mutation of the original path cannot alter an admitted snapshot. A digest mismatch returns `source_changed`.
Reuse the same owned input across consumers; do not copy per frame or charge an already-owned blob twice.

R10. Analyze selected pictures in source presentation order using the production selector.
Greatest-PTS selection, stable equal-PTS ordering, effective rational rate, seeks and rotation follow
`docs/MEDIA-POLICY.md`, “VFR mapping” and “Rotation metadata”. Store picture PTS plus stable ordinal
and the effective-frame mapping. Advance state once per distinct selected picture; held pictures reuse alpha.
Never feed timeline cut order, graded images, a chart or a proxy to RVM.
Reset at source start and recipe-pinned discontinuities, not at cuts, chunk boundaries or random seeks.

R11. Whole-source coverage is allowed only if Stage A's capacity calculation admits the recording.
Assume 20–40 minutes total across files and test 60 minutes as the upper case, per lead decision 4.
If whole-source mattes do not fit, store the union of source ranges used by the edit, with two-second
handles on each end, clipped to source boundaries. Merge overlap before charging or scheduling.
The two-second handle is an initial lead-owned engineering choice, recorded in the recipe, not an Riel question.

Range-limited storage must preserve whole-source recurrence. Without persisted recurrent checkpoints,
replay from the preceding pinned reset to the last requested picture, discarding unrequested outputs.
Several ranges in one job share that replay. Cancellation/resume likewise replays from the reset.
This saves disk, not necessarily inference time. Record replayed and retained durations separately.
An edit outside stored coverage queues the additional range and exposes `analysis_missing` there.
The host can durably save that edit with pending coverage, retaining its exact bound artifact A; saving
does not make A cover the extension or mark the new edit Ready. Enabled export remains blocked.
R15 extends coverage as a new flat manifest C sharing A's chunks, followed by R18's reviewed,
expected-revision rebind. Completion alone cannot change the bound digest. There is no automatic reset
at the new range start and no silent use of a nearby matte.

R12. Use one native analysis child of the project host, with an eight-job coalescing queue.
One machine-local advisory execution permit serializes JA-2 inference across project hosts. It grants
only worker execution, never project-store authority. It is not a cache service or daemon.
Acquire it in the worker after the project owner has admitted the job. Hold it until that worker exits.
Do not inherit the project lock into the child. A new host cannot infer worker death from a PID string.
Parent death closes an authenticated control pipe and invokes JA-1's child containment, Linux supervisor
and Windows kill-on-close Job Object (`ja1-studio-orchestration-design-rev1.md` §7.2).
Until child death and execution-permit release are confirmed, wait rather than launch a duplicate.

Requests/results carry project ID, owner epoch, job ID, source digest, recipe digest and generation.
The child writes only its exclusive staging directory. Only the host can seal or attach results.
At frame/write boundaries observe cancellation and playback/export priority. Release inference GPU
resources before timing playback. Replay on resume is acceptable; concurrent inference is not assumed safe.
Persist job descriptions/progress, not recurrent tensors. No late result crosses an owner epoch.

## 5. Representation, capacity and the single store owner

### 5.1 Select precision before building the store

R13. Gray8 and Gray16 lossless alpha PNG payloads are Stage A candidates, not accepted formats.
Compare actual model outputs through identical black/white/colour edges, temporal tests, 150% punch-ins
and the actual video/reel framing. Choose Gray8 if it passes every gate. Retain Gray16 only if its
quality advantage is needed and its capacity/read costs pass. Alpha is coverage, never gamma-coded.
Reject nonfinite/out-of-range values before pinned round-to-nearest quantization.
Version precision, quantization, estimator and resampling independently in their recipes.

| Raw alpha at 1080p30 | Per minute | At 20 / 40 / 60 minutes |
| --- | --- | --- |
| Gray8 | 3,732,480,000 bytes, about 3.48 GiB | about 69.5 / 139.0 / 208.6 GiB |
| Gray16 | 7,464,960,000 bytes, about 6.95 GiB | about 139.0 / 278.1 / 417.1 GiB |

These are arithmetic, not expected PNG sizes. At rev1's target of 384 MiB/min, matte alone is
7.5 / 15 / 22.5 GiB for 20 / 40 / 60 minutes. A 16 GiB source plus the 60-minute matte already
exceeds the 32 GiB feature-store ceiling, before staging, retained old artifacts or indexes.
Therefore that target cannot justify whole-source storage for the upper case.

Stage A records encoded bytes per minute and worst chunk, cold/warm random reads, and decode/resample/
upload p95/p99 for each precision. Calculate peak overlap using actual owned source bytes, all retained
manifests and unique chunks, new output/staging, relocation copies and scratch. Include every file in the
60-minute case and both the feature ceilings and JA-1's user-selected host media quota.
Run the bounded range policy before approving the format if whole-source storage fails. Include an
evolving-edit case: an old version and live export retain A while a trim needs additional coverage,
C shares every old entry/chunk and adds only missing pictures, then bind/undo/redo/reopen retain both.
Charge each shared physical object once, but all simultaneous staging/copy bytes and both manifests.
The critic's illustrative 12 GiB owned input + 14 GiB existing matte + 1 GiB extension is about 27 GiB
plus manifests/temporary overlap, with about 15 GiB unique completed matte. Copying a 15 GiB union
alongside A would instead need 41 GiB before scratch and 29 GiB completed matte. These are arithmetic,
not measured capacity or permission to ignore overhead. Stage A must establish headroom from actual
encoded outputs and the chosen overlap; Stage B/C must exercise this case through the real host path.
Prefer used ranges to a larger disk allowance. If even needed ranges/snapshots cannot fit, return
required/available bytes and use an explicit Off edit or another take, rather than raise the ceiling silently.

### 5.2 Immutable addresses and ownership

R14. JA-1's project host owns reservations, indexes, publication, pins and retirement under its
never-unlinked advisory project lock. Its one-writer and durable-root contracts are dependencies,
not claims that the inspected tree already implements the full host (`ja1-studio-orchestration-design-rev1.md`
§§6–7 and 10; `docs/AW1-HEADLESS-KINEWRIGHT.md` §0.1 AF1).
Existing derived-cache threads/mutexes do not supply cross-process ownership
(`crates/kinewright-media/src/derived_cache.rs:21`, `derived_cache.rs:91`).

Use the host's verified project asset root and these logical locations:

- `person-analysis/v1/artifacts/<artifact-digest>/` for one immutable flat coverage manifest.
- `person-analysis/v1/chunks/<chunk-digest>` for shared immutable chunk objects.
- `person-analysis/v1/recipes/<recipe-digest>` for a bounded lookup record of available artifact IDs/ranges.
- `person-analysis/v1/staging/<owner-epoch>/<job-id>/` for exclusive incomplete output.

The recipe includes source bytes/stream, selector/rate/orientation, source interpretation/preprocessing,
model and runtime profile, resets, inference settings and representation. The artifact digest hashes
the canonical sealed coverage manifest, including its direct ordered chunk references/hashes;
artifact digest and manifest digest name the same identity. Coverage is part of that manifest.
It is flat, with no parent manifests, generation chains or recipe lookups needed to resolve a picture.
Same recipe with different output bytes has a different artifact address. A recipe lookup is not the
identity of an edit and cannot retarget undo, a prepared plan, retained version or an export.
Bound lookup history to 32 results per recipe; remove only lookup entries, never a pin needed by a root.

R15. Start with independently seekable PNG pictures packed in at most 60-picture chunks.
Each immutable chunk records format, dimensions, picture IDs, bounded offsets/lengths and hashes.
The bounded flat manifest has source/recipe/format identity and ordered coverage entries mapping
source-picture intervals/effective frames directly to chunk digests and their picture entries.
Validate checked arithmetic, dimensions, decoded size, nonoverlap, ordering, dependency identities and
exact payload lengths before allocation. Reject missing/trailing bytes. The same bounds in §11 apply
to the complete union manifest, not a fresh allowance per extension.
No inter-frame codec or cached foreground RGB is introduced. The selected foreground estimator uses
owned camera RGB at render time, avoiding a second three-channel per-frame disk artifact.

An extension copies A's existing canonical entries byte-for-byte into C, adds entries only for newly
needed pictures and publishes a new manifest digest. Do not split/repack old chunks, copy their payloads,
replace their picture mappings or mutate A. Insert new entries in canonical source order without changing
any old entry bytes. C references every chunk directly, including shared old ones. Source/recipe changes
are regeneration, not extension. Replay may traverse old coverage, but recomputed overlapping outputs
are discarded; they never replace reviewed entries. Replacing old output needs a separately reviewed
expected-revision change and QA, even if the recipe is unchanged.

The host performs this initial-publication/extension handoff, with bounded work off the UI thread:

1. Capture owner epoch, expected edit revision/head, bound A (if extending), source/recipe and requested
   coverage. Under JA-1's single disk authority, reserve unique new bytes plus every simultaneous temp,
   manifest/root/backup and snapshot overlap against both §11's feature ceilings and the host media quota.
   Shared retained chunks stay charged once by that owner; duplicate staging bytes stay charged until
   removed. Pin all dependencies, including A's shared chunks and the owned input, before work starts.
   Register the job and issue staging-only authority. Authorize each chunk's actual encoded bytes before
   writing; stop before any projection overrun exceeds either quota and report incomplete coverage.
2. The worker writes only new-picture chunks in exclusive staging and returns hashes/coverage. It has
   no manifest/root binding authority. Validate the result's epoch/job/source/recipe and declared ranges.
3. The owner verifies dependencies through owned handles, syncs/publishes immutable chunks and pins
   each newly verified chunk before sealing the manifest. It then seals/publishes the flat manifest
   and syncs affected directories/ancestors. An existing digest must match bytes. Preserve all dependency
   pins across this interval; transfer reservations to sealed ownership without a gap or double charge.
   A sealed reusable candidate is not a bound edit or a Saved/Ready cut.
4. Prepare the ordinary resource-rebind plan against the captured expected revision and exact A.
   R18 preflights the full candidate mapping and pins C plus its dependencies. QA the proposed edit,
   including newly exposed pictures, neighbor motion and every old/new coverage boundary. Preserve exact
   old pixels and retain version/source/picture/manifest-bound evidence. A seam failure cannot be repaired
   by replacing old entries silently. Only the reviewed plan may commit the rebind through JA-1's
   durable-root path, including its idempotency receipt and failure/recovery semantics (§§6.2, 7.1).
5. Recheck expected revision/head, bound digest and all identities at commit. A stale completion may
   remain a reusable sealed candidate under existing bounded host retention, but cannot attach to a newer
   edit, retry against a fresh revision or mark that edit Ready automatically. Reinspect/replan and QA
   the current mapping before any later bind. Recipe lookup publication never retargets an edit.
6. Only a durable root commit acknowledges the bind. Transfer dependency pins to that root/history and
   live readers before releasing job/plan pins. Retire staging only after worker death and outstanding
   reads. JA-1's existing collector retains manifests and traverses their direct chunk dependencies from
   its root/backup/version/branch/history/recovery/export/comment references plus live-reader and existing
   job/plan pins. It deletes only unreachable owned objects after a completed root commit. Add no second
   garbage collector, disk ledger or store owner. An unrooted candidate has no permanent retention right.

A crash after seal but before root commit leaves the prior durable edit authoritative, possibly saved
with pending coverage. Recovery may validate/reuse sealed output; its existence never completes a bind.
A crash after root publication but before reply follows JA-1's persisted command receipt, without a
second rebind. Uncertain durability retains pins/charges and follows JA-1's stopped-mutation recovery.

On a second opener, attach to JA-1 or refuse contention. Missing discovery never permits a second writer.
The host serializes disk admission across GUI/headless/proof/export. Readers hold host-issued pins plus
verified file handles. Reader disconnection does not release an export the host still owns.
On owner death, a successor takes the same project lock, validates the durable root, reconciles abandoned
staging/reservations and confirms old child cleanup before admitting new writes or garbage collection.
Missing recovery accounting fails closed to retention. PID checks cannot authorize deletion.
GUI/proof readers stop new reads when their owner connection dies and drain existing CPU/GPU work.
The successor requires old reader leases to be drained or re-registered before retiring their artifacts.
Unconfirmed old readers keep those artifacts charged and retained; reconnecting uses the new owner epoch.

R16. Pinned manifests and their chunks are never evicted. Retain old/new same-recipe manifests and
shared dependencies concurrently when undo, versions or exports require them. Regeneration cannot
overwrite an old artifact. Rebinding different bytes requires a new expected-revision plan and QA.
Missing/corrupt analysis is a typed state.
If the cap is pinned full, refuse admission with required/available bytes and preserve every existing edit.

Save As, relocation and packaging use JA-1 §7.3's frozen root generation, destination lock and verified
copy-before-root order. Copy all retained matte manifests, their reachable chunk objects and owned
inputs needed by the copied root, including history/version references. Copy each shared chunk once
within the destination store. Validate path components, case collisions and file identities.
Reserve destination overlap before starting. Publish the destination root last. Interrupted copy leaves
source and prior destination roots valid; do not claim portability after a partial copy.
Packages disclose included private media and need explicit inclusion authority. Runtime/models stay local.
Opening incomplete external projects reports precise missing inputs/artifacts; enabled output cannot bypass them.
The existing project writer copies LUTs after writing the document (`crates/kinewright-project/src/project.rs:522`),
so it is not the portability implementation for this contract.

## 6. Editable effects and coordinated format

R17. Add optional typed `EffectResource::PersonMatte` on `Effect`, containing a bounded recipe descriptor
and immutable artifact digest. It is valid only for `person_separation`. Omit the field when absent.
The current `Effect` and `EffectWire` lack it (`crates/kinewright-core/src/model.rs:187`, `model.rs:215`).
Rasters, faces, model state, queues and host ownership never enter `Document` or its operation journal.
Store mode/blur/edge as registered integer controls with normal enable/bypass and clip-local curves.
Wrap controls and local-fill windows/fitting remain deferred under R4, R21 and R24.
Hashes and paths do not belong in scalar parameters.

R18. Add typed bind/unbind operations, one separation descriptor and a gradient treatment for Solid.
“Apply background” is one ordinary validated batch with a lower backdrop layer, effect and resource bind.
Permit at most one separation effect on a video Media or Freeze host. Initially refuse adjustment,
audio-only and generated hosts. Core checks types, bounds and eligible mapping.
For an initial ready bind or a replacement/extension bind, media/host preflight pins and verifies the
exact candidate manifest, owned input, every required chunk and complete coverage for the proposed
mapping/declared ranges before commit. That is R18's availability-preflight scope; it does not forbid
saving an ordinary trim/freeze/reuse/speed edit that exceeds coverage of its already-bound manifest.
Such an edit retains A and saves as pending coverage under R11, with readiness derived from the saved
mapping and bound digest. Queues/progress stay host-owned, outside Document. It cannot earn Ready or
an updated review receipt from an unbound job result. Structural load/replay validation remains required;
opening an incomplete project reports missing dependencies rather than requiring inference installation.

Split/duplicate retain source-bound resource identity. Trim/freeze/reuse/speed mapping selects source
pictures and may request more coverage; it does not rebase analysis as clip-local metadata. R15's rebind
is a distinct ordinary expected-revision batch, with undo/redo retaining A and C. QA/review applies to
the proposed new version, newly exposed boundary and exact digest; old review receipts remain tied to
their old version. Recheck source/artifact identity during rendering. Readiness, proofs and export must
resolve the digest actually bound in the selected edit and check its required source pictures, never a
recipe lookup or the newest completed job. Enabled export fails on a missing picture/dependency even
if a sealed union exists elsewhere. Intentional Off remains the explicit reviewed choice in §2.
Backdrop edits invalidate composite QA, not the matte. Keep original source/audio and ordinary undo.

The lead owns one format change across JA-2/AW2/CC8/MO6, per binding decision 3.
Extend serialization, wire parsing, constructors, operation/load/replay validation, journal/undo and
the one minimum-required-version predicate together. Presence of a stored feature counts even bypassed.
No independent “v3” reservation. Current maximum is 2 (`crates/kinewright-core/src/model.rs:1263`);
the shared writer predicate is `crates/kinewright-project/src/project.rs:380`.
Legacy documents keep their prior bytes/meaning. Older readers refuse overwrite of new-format projects.
AW2 is accepted design work, not an available clip renderer (`docs/AW2-CODE-GENERATED-CLIPS.md` §1).
Solid, gradient and supplied-still backgrounds must work without AW2 or MO3.

## 7. Foreground colour and the shared render pass order

### 7.1 Required colour decontamination

R19. Select a bounded closed-form/multilevel foreground estimator for the first trial, rather than
storing RVM's predicted foreground. This is a design selection, not a claim of Stage A evidence.
Stage A must validate it with actual alpha outputs before the route can be accepted. If it fails,
return the route to the lead; do not silently switch to original RGB or ship two estimators.
The reasons are testability in managed linear space and avoiding a three-channel matte-store expansion.
No new neural RGB path or foreground disk format is required.

The opaque camera pixel is `S = a*F_true + (1-a)*B_old`. It is not a straight foreground sample.
For replacement, `a*S+(1-a)*B_new` retains old-room colour even with perfect alpha.
Use source-profile-decoded working-linear S, before any technical LUT or creative/correction node.
Choose foreground/background colour priors only from this source picture's trusted foreground/background
samples. A bounded normalized pyramid, with candidate source-pixel supports 8, 16, 32 and 64, supplies
the nearest valid priors. Initial trusted alpha thresholds are 0.98 and 0.02. Pin all constants in Stage A.
If a fractional edge lacks valid support, mark that interval untrusted; do not fabricate a background/person.

One bounded closed-form solve per channel projects those priors onto the observed mixture:

```text
r = S - a*F_prior - (1-a)*B_prior
d = a*a + (1-a)*(1-a)
F_est = F_prior + a*r/d
B_est = B_prior + (1-a)*r/d
```

Here `d >= 0.5`, avoiding division by tiny alpha. At a=1 preserve S. At a=0 set foreground RGB to zero
and preserve the observed background. This estimates colour, not missing detail. Reject nonfinite output
and record support/reconstruction error. Stage A fixes correction/support bounds from independent
references before candidate scoring. It must reject a route that changes opaque detail or adds coverage.
Real camera compression and imperfect priors limit recovery; F1's algebra alone cannot clear real hair.

The original-RGB route is a declared approximation available for labelled inspection only by default.
It cannot clear the fractional-edge gate. A replacement case that fails edge QA is refused on that route;
the agent may explicitly choose Off or a reviewed lower-blur version, never call that a replacement pass.
Wrap remains deferred and cannot compensate for failed foreground recovery.

### 7.2 One explicit pass order

R20. Extend `FrameRenderer` and the shared compositor, used by preview/proof/export
(`docs/M4-COMPOSITING-EXPORT.md` opening paragraphs). The required separation pass order is:

1. Resolve the owned source picture and source profile. Decode/orient once into straight working-linear
   S at the full supported source raster, alongside its exact alpha. Model preprocessing remains a
   separate pinned display-referred view used only during analysis.
2. Estimate F before grading. For replacement form `P=a*F_est`. Filter/resample P and a together with
   identical kernels and pixel centres, retaining that pair until the output sample in step 5.
   Never filter arbitrary straight RGB in transparent pixels.
3. For blur, derive visible-room blur by normalized convolution of `(1-a)*S` and `(1-a)`, with a
   conservative foreground exclusion. Use the nearest coarser valid support if needed. Refuse an
   unsupported region rather than inpaint it. Background support is required only where `(1-a)>0`;
   fully opaque person pixels bypass that lookup. Reconstruct `T=a*F_est+(1-a)*B_blur` in linear space.
   T is an opaque room-plus-person image; person alpha is internal to this reconstruction.
4. Reduce source-local results to the admitted preview raster, or retain full raster for proof/export.
   Use area integration for reduction, bilinear reconstruction for enlargement and identity otherwise,
   with pinned pixel-centre conventions. Replacement reduces P and a together. Blur reduces T as an opaque
   image. Apply source crop/reframe consistently to RGB and alpha when sampling the output quad.
5. At each output-quad sample, recover replacement F=P/a only above epsilon, initially 2^-16,
   and set F=0 otherwise. Then run the existing ordered colour stack
   once on F, or once on T for blur. Keep technical input nodes, stored correction order and creative
   look order. Do not run a second prepass grade or reorder primary/curves/wheels.
6. Keep the existing legacy compatibility stage, key/spill and fade in their established order.
   Preserve crop, mask and geometric coverage. For replacement multiply person coverage into the
   final host alpha after the fade override, exactly once, immediately before over/blending.
   For blur keep the normal host alpha; multiplying person coverage again would erase the retained room.
7. Composite into the existing opaque-over-black accumulator. Replacement background is an ordinary
   lower layer. Upper charts/captions stay above the host. Monitoring/delivery conversion remains last.

Current working storage is linear binary16 with f32 arithmetic (`crates/kinewright-media/src/frame.rs:18`;
`docs/CC1-MANAGED-SDR-PRIMARY.md` §1). The current shader samples before grading at output-quad UV and
can force alpha to one during fades (`crates/kinewright-media/src/compositor.wgsl:850`, `:966`).
The new prepass must preserve that grading coordinate, rather than evaluate windows at source UV.
For source point p, use the production crop/reframe inverse to obtain layer-output q, then the host
transform for composite position. CC5 receives q and the output-raster aspect, as specified in
`docs/CC5-SECONDARIES.md` §§2.3–2.4. Its qualifier still reads the value entering each node in grade709.
Pass mapping/context explicitly through the prepass and face-region proofs. A missing/invertibility
failure is unsupported, not a guessed coordinate conversion. Freeze/portrait/punch-in use the same mapping.

Full-source decode for enabled separation is a new cost relative to today's 1280-wide preview
(`docs/MEDIA-POLICY.md`, “Preview memory and decode strategy”). Admit and measure that cost in Stage C;
do not hide it in the matte lookup target or claim PF1 already covers it. An optimization to a smaller
prepass is allowed only after the same independent resampling/edge gates pass.
Keep the opaque accumulator and alpha semantics in `docs/MO2-BLEND-ADJUSTMENT-TRANSITIONS-SOLIDS.md` §3.

R21. Below-host light wrap is deferred, including its snapshot, fitting and extra render passes.
Re-admit only if the real tight-end take demonstrates a benefit; G14's default blur/natural look must
pass without it. If re-admitted, use only a below-host stack snapshot, never upper charts or the host
itself, mix at most 10% low-frequency backdrop colour into an inside edge, default to zero and preserve
alpha. Require its own admission/pass budget and nonzero-strength witness before enabling it.
Depth-like falloff is deferred. No silhouette distance is labelled physical depth.
All geometry/raster/artifact/estimator/colour-context identities enter cached prepass keys.
Future proxy files must retain original picture mapping; never analyze proxy pixels or export proxy alpha.

## 8. Face evidence and idempotent ordinary colour edits

### 8.1 Analysis and measurement stages

R22. Trial a pinned YuNet ONNX export through the same runtime, without OpenCV. Lead verification
item 3 records MIT for the model directory. Exact export layout, bytes, hash and local costs remain
Stage A verification; admit at most 2 MiB for the trial. Five landmarks support conservative cheek
patches, not a semantic skin mask. Detect at 5 Hz, interpolate only verified short intervals and recheck
difficult intervals at source rate. Exclude glasses/eyes/lips/beard, clipping, turns and occlusion.
Ambiguous/multiple faces or unreliable placement yield insufficient evidence. No identity embeddings.
Store geometry/statistics/validity, not face crops. Do not add a parsing model to rescue a failed gate.

Cache source geometry and raw measurements before creative nodes, keyed by immutable source/profile.
Solving measures the current isolated host at a new typed `host_post_color_pre_legacy` stage, after its
managed colour stack and before legacy/key/fade/compositing. It excludes charts and the room using
valid face patches plus person coverage. For both Blur and Replace, measure the isolated recovered F
branch, not the reconstructed room T. Missing trustworthy alpha/foreground yields unchanged/insufficient
evidence. Reference and candidate use their own mapped patches.
The stage is new. Existing scopes expose only monitoring-post-composite (`docs/CC2-SCOPES-AND-MATCHING.md`,
“Outcome”); CC6 explicitly does not detect faces (`crates/kinewright-core/src/color_qc.rs:351`).
CC0 source interpretation and CC1's managed transform remain authoritative. CC2 supplies comparisons,
CC3/CC4 the ordinary correction/look nodes, CC5 windows/qualifiers and CC6 output diagnostics.
CC7 evaluates these workflows and adds no filter (`docs/CC7-WORKFLOW-EVALUATION.md` opening paragraphs).
After a proposal, reuse monitoring scopes and CC6 checks on matching rendered output. Their existing
monitoring stage is not renamed to the new isolated-host stage.

Measurements use linear Rec.709 `Y=0.2126R+0.7152G+0.0722B` relative to working reference white,
with exposure differences in stops `log2(Y_ref/Y_current)`. Report clipping/noise and valid pixel counts.
Hue in degrees and chroma as max-minus-min come from a local clamped grade709 RGB copy, using CC5's
hexagonal hue convention. Chroma is unitless in [0,1]. Exclude achromatic pixels from hue and report them.
These summaries are not perceptual Delta E, skin classification or a complexion-independent target.

### 8.2 One deliberately restricted solver for both entrypoints

R23. `plan_face_look` and `plan_shot_match(measurement_region=face_analysis)` call one solver.
V1 permits a managed neutral stack with either no primary or exactly one enabled, globally active,
unmasked static primary. That primary may already contain static exposure/temperature/tint correction.
All its other controls must be neutral. Other enabled managed nodes must be neutral identities.
An active nonlinear curve/look, nonneutral wheel/technical LUT, legacy stage, disabled or CC5-masked
primary, multiple primaries, colour/enable automation affecting the measured interval or unknown stage
returns a typed conflict unchanged.
Do not enable a disabled primary, remove a mask, erase keys or insert a competing correction automatically.
This restricted contract is the critic's allowed simpler alternative to a general grade-stack solver.
Manual colour editing remains available. A separately reviewed new-node proposal is explicit; local
fill remains deferred under R24.

The existing primary planner writes absolute static integers and warns about multiple nodes/keys
(`crates/kinewright-agent/src/color_status.rs:1476`, `:1521`). Existing shot matching adds deltas measured
through the current grade (`crates/kinewright-agent/src/color_scopes.rs:1771`). Neither calculation may
be fed unchanged pre-grade face statistics and treated as this new solver.

For both entrypoints, use this contract:

1. Bind source/geometry/reference/intent, expected revision, colour context and eligible node identity.
   Pin the reference evidence and the baseline parameter values for the look request. Retry does not
   select a new reference or reset the safety baseline. Keep this bounded record in the host's existing
   run/evidence objects, not Document or an unbounded solver cache. Manual node edits invalidate that
   request; continuing requires a fresh reviewed request rather than overwriting the manual change.
2. Measure the current isolated host at the named stage. Without reliable face evidence return unchanged.
   Without a separately validated neutral surface preserve temperature/tint exactly in both entrypoints.
   Face hue alone never supplies white balance. Do not fall back to whole-frame matching.
3. Solve residual exposure, and optional neutral-supported WB, against current measured output using
   the existing primary forward transform. Use a deterministic bounded parameter search, at most
   32 evaluations of the fixed patches, to find absolute descriptor integers. Quantize with pinned rules.
4. Limit exposure movement to 0.5 stop from the request baseline and inferred per-channel WB gain
   movement to 5%. Also guard resulting face hue change to 3 degrees and relative chroma change to 5%,
   excluding chroma below the measurement floor. Descriptor ranges bound final stored values.
   These are limits on the whole requested correction, not a fresh allowance on every retry.
5. Evaluate the proposed document through the real isolated-host path before returning operations.
   Require exposure error within 0.02 stop and, when WB is solved, channel-ratio error within 1%,
   provisionally. Verify quality bounds and no worsened clipping. If the target is unreachable inside
   the limits, return unchanged/target_out_of_bounds, not a partially converged edit to repeat later.
6. Emit absolute parameters through `plan_primary_correction` and the ordinary prepared-plan path.
   A current result already in the same convergence band returns no operations. Replanning after commit
   with the same source/reference/intent must be unchanged. Stale revisions or evidence cannot commit.

Pin convergence thresholds before candidate scoring in Stage D. The source geometry/raw cache is reusable,
but current-stage measurements and before/after evidence are revision-bound and must be recomputed after edits.
For matching, process each candidate with its own evidence and baseline. Preserve the legacy geometric-ROI
branch unchanged. A missing face/neutral never invokes that branch implicitly.
Natural reference-relative appearance is the default. No universal IRE or “correct skin” hue target.

R24. Local skin correction/soft fill is deferred, including moving-window fitting, key generation and
extra passes. Re-admit only if the real tight-end take demonstrates a benefit; the conservative global
natural look and blur must clear G14 without it. If re-admitted, emit ordinary CC5 windows, qualifiers
and curves after the global result. Map landmarks to layer-output quad coordinates via §7, not composite
UV, fit within one source pixel, disable invalid intervals and cap keys. Omit fill if fitting fails.
Keep the +0.2 stop maximum, alpha preservation and R23 conflict behavior; it is a look, not physical
relighting. No new per-pixel skin mask or neural colour renderer. Its placement/admission/quality gates
must pass before enabling it. CC5 qualifier input/stored node order stay authoritative
(`compositor.wgsl:719`; `effect.rs:2458`).

## 9. Agent capabilities and QA evidence

R25. Add capabilities to the existing compact runtime, not new always-advertised MCP tools.
`docs/M36-AGENT-RUNTIME-EFFICIENCY.md`, “Outcome and requirements” and “Prepared-plan lifecycle”,
defines discovery and revision-bound prepare/commit. Proposed capability names are:

| Capability | Result |
| --- | --- |
| `analyze_person` | Source/ranges/recipe, admitted job identity, estimated bytes and execution state. Writes derived data only. |
| `get_person_analysis` | Bounded progress, bound/candidate manifest identities, stored/required coverage, pending bind, replay duration, model/provider and actual bytes. |
| `plan_person_background` | Unapplied effect/resource/backdrop batch, coverage preflight and expected revision. |
| `inspect_person_matte` | Original, alpha, edge and composite evidence at exact pictures, with neighbor frames. |
| `qa_person_separation` | Per-segment risk measurements, flagged intervals and proof references. |
| `plan_face_look` | Reference/assumptions, shared solver result and ordinary unapplied correction operations. |

Extend `plan_shot_match` only with the explicit face branch in §8. All document changes use normal plans.
States include not_installed, queued, running, paused, partial, ready, cancelled, failed, missing_analysis,
source_changed and unsupported_input. Separate stored coverage, analysis availability and QA outcome.
Job-ready means a sealed candidate has complete validated coverage for its declared ranges; it may
still be unbound. Edit-ready requires the exact bound digest to cover the current mapping, with the
separate reviewed-plan/QA checks satisfied. Report pending coverage/bind explicitly; never infer edit
readiness from job-ready or silently advance an edit to a later manifest. Neither promises whole-source
coverage.

R26. Alpha is coverage. Return `model_confidence: null` unless a separately justified confidence exists.
Per one-second segment report boundary changes, subject loss, temporal residual, validity/sample counts
and worst frames. A versioned heuristic risk is not a probability, IoU estimate or proof of quality.
For unlabelled takes, bounded block-matching residual is a heuristic with unmatched/disoccluded counts.
Do not use that estimated correspondence as ground truth. With no hand detector, report suspected
face occlusion or unreliable face region, not a known “hand crossed face” event.

Return at most eight worst frames per page, stable cursors, neighbor motion and bounded proof strips.
Default JSON is at most 16 KiB, status at most 4 KiB. Images use artifact handles, not inline rasters.
Every proof names revision/version, clip/effect/source, project frame, source picture/PTS, artifact/model/
recipe digests, raster, colour stage/context and sampled/full coverage. No sampled check clears unseen frames.
Background changes invalidate composite QA. Source/model/preprocessing changes invalidate raw analysis too.
Microphone/desk intent and other semantic errors need rendered review even when numerical QA looks good.

## 10. App controls

R27. Background offers Off, Blur and Replace, strength, backdrop and before/after. Preparing, Ready,
Needs review and concrete recovery status must reflect the same host state the agent reads.
The advanced inspector exposes alpha, edge, black/white/colour, checkerboard and full-resolution views.
Main controls show useful look choices, not runtime/library details. Show estimated/actual disk use in setup
and advanced status. All manual controls use the same operations and undo as agent edits.

Person alpha is a distinct diagnostic source from CC5's colour-selection matte. Keep one diagnostic
view active, labelled with frame/stage/revision, and preserve blocked-source precedence.
Existing Matte view precedence is documented at `crates/kinewright-app/src/preview_ui.rs:225`.
Coalesce seeks, discard late mismatched results and release their buffers. Never decode/infer or hold a
long store lock in a UI callback. Face-look controls name the reference, assumptions and unchanged choice.
Keep fill/wrap controls deferred until R21/R24's real-take benefit decision and independent gates.
An incomplete enabled effect may display a labelled draft original view, but that view can neither
pass export preflight nor become a review receipt.

## 11. Resource budgets and reservation handoffs

R28. All numbers here are unmeasured trial criteria. Report each OS/provider/precision independently.
Do not derive resident memory from weight size or end-to-end speed from tensor inference.

| Resource | Initial target/ceiling and failure behavior |
| --- | --- |
| 1080p30 analysis on 3090 | End-to-end p95 at most 33.33 ms/picture, including decode, transfers, inference, QA and writes. Report p99/mean/replay/cold setup. A miss means offline-slow, not real time. |
| CPU analysis | p95 target 250 ms/picture, with CPU/thread count. Slower unattended operation can remain useful if disclosed; no silent quality reduction. |
| Analysis memory | Child peak RSS at most 2 GiB, incremental dedicated VRAM at most 4 GiB, parent overhead at most 96 MiB. Measure total host/GUI/child peaks separately. |
| Cached alpha work | Lookup/decode/resample/upload p95 at most 2 ms, also report cold/warm p99. Full-source RGB/estimator costs are additional named measurements. |
| Render overhead | Added estimator/filter/composite GPU time p95 at most 3 ms at the actual preview output, at most 8 ms for full-raster proof/export before encoding. Misses require review. |
| Playback | 240-second 30 fps host+charts workload: at least 29.7 delivered fps, p95 presentation at most 50 ms, misses at most 1%. Paired enabled/bypass rate loss at most 1%, extra p95 at most 5 ms. Absolute PF1 held-age gate remains binding. |
| CPU preview storage | Source, alpha, compressed input, PNG scratch, conversions, prepass handoff and generated rasters share PF1's 224 MiB required-set admission. No extra matte allowance. |
| GPU feature demand | Added live+idle feature resources at most 96 MiB and 32 pooled resources for one 1080p host, inside any stricter shared owner policy. Count padding, mips, staging and retirement overlap. |
| Disk | At most 24 GiB unique completed matte chunks/manifests across retained generations within 32 GiB total feature-owned source/matte/temp overlap, also within JA-1's host media quota. Snapshot at most 16 GiB is an individual ceiling, not a reserved free allowance. Charge shared owned inputs/chunks once; charge all simultaneous temporary copies and manifest/root overhead. |
| Artifact bounds | At most 2,073,600 pixels/frame, 8 MiB encoded payload/picture, 512 MiB/chunk, 216,000 indexed source frames and 8 MiB manifest/index per artifact. Reject before allocation. |
| Jobs/metadata | One active inference child globally, eight queued/project, 32 retained job summaries, 256 KiB queue/summary bytes. At most 32 MiB live analysis metadata/index buffers and 4,096 covered ranges/artifact. |
| Plans/evidence | At most 16 host clips/plan, 512 generated keys/parameter, 4,096/clip. At most 32 proofs and 64 MiB aggregate. Refuse/paginate excess. |
| Interaction/cancel | Queue/status p95 at most 100 ms, cooperative cancellation acknowledgment within 250 ms between frames. Worker watchdog 5 s plus 5 s child-only cleanup, cold init watchdog 30 s. Report actual native completion separately. |

R29. Extend PF1's real admission path, not merely its counters. Preview planning enumerates the
whole required set before alpha prefetch begins: full source RGB for enabled separation, resident alpha,
compressed payload read, bounded PNG decoder scratch, resampling buffers and handoff overlap.
Acquire one atomic required-set reservation under the existing scheduler. Give the alpha reader a
bounded token for its share, then transfer the charge with the decoded buffer into the renderer lease.
Shrink an estimated reservation only after the prior allocation is released. Coalescing/eviction cannot
release a buffer still held by another consumer. Lookahead uses only surplus after the required set.
If a set cannot fit, return the existing explicit fallback/unsupported outcome with full accounting;
never run an uncharged alpha path in a synchronous renderer.

PF1 already counts last-owner working allocations and atomically admits source/generated sets
(`docs/PF1-PLAYBACK-PERFORMANCE.md` §5 K-1/K-2; `crates/kinewright-media/src/preview.rs:1913`).
Alpha/PNG/full-source-prepass integration is new work. Synchronous proof/export uses its approved
owner policy and explicit reporting, not an inferred extra 224 MiB allowance.

R30. Extend the existing compositor owner with pre-allocation reservation for the complete §7 pass
graph. Enumerate peak lifetimes for source upload, alpha, foreground/background priors/pyramids,
premultiplied foreground, resampling, blur ping-pong and staging. Wrap snapshots and local-fill passes
are deferred; if re-admitted under R21/R24, extend this same admission graph before enabling them.
Reserve before texture/buffer creation. Reuse scratch only after its final GPU dependency retires.
Transfer a pooled allocation's existing charge without double counting. Keep charges until both GPU
completion and final host handle release. Admission must precede any optional pass as well.
The existing ledger counts allocations (`crates/kinewright-media/src/compositor.rs:194`), and its
256 MiB pool cap applies to idle textures (`compositor.rs:850`). Neither admits new live demand.
Stage C must validate actual pass liveness under the stricter shared policy before cached playback is accepted.

Measure paired enabled/bypass workloads on the shared machine as it is. Do not wait for quiet,
change settings or overlap Kinewright builds/tests with timing. Retain executable SHA/hash, source/model
hashes, provider/driver, foreign-load annotation and actual presentation/hold-event chronology.
Report mean/p95/p99, maximum holds, frame identities and missing outcomes, not just average fps.
PF1 H-T has no established completion date. Its held-event chronology and combined host+charts
acceptance remain pending; active whole-process RSS has no accepted numerical PF1 allowance here
(`docs/PF1-PLAYBACK-PERFORMANCE.md`, “PF1-LH R97 checkpoint”, G14, H-T and unresolved items).

## 12. Ordered delivery stages

| Stage | Required result before proceeding | Gates |
| --- | --- | --- |
| A | Produce cleared F2/F3 assets and reference uncertainty. Seal exact model/native manifests. Trial Linux CPU/CUDA and Windows MSVC CPU. Select the foreground route and Gray8/Gray16 from real outputs; accept whole-source or used-range capacity for 20/40/60 minutes, including retained A plus shared-chunk extension C, actual unique/temporary bytes and both feature/host quotas. Measure package/build/startup/read/decode/resource costs. No store/effect implementation proceeds with these choices unresolved. | G1, G2, G7 foreground/precision trial, G13 capacity/read/retained-generation trial |
| B | Consume JA-1 host ownership, implement owned-input leases, selector/replay, unique staging, flat digest manifests/shared chunks and bounded inspection. Exercise reservation/pinning/seal/recovery through the real host publication path; no second owner, collector or ledger. | G3a/G3b, G4, G5 host publication/recovery |
| C | Lead-coordinated format, ordinary effects/backgrounds and the complete shared render path. Exercise pending trim, reviewed expected-revision rebind, stale refusal and retained-version/export undo/redo/reopen through JA-1's actual durable-root path. Required-set CPU admission and GPU pre-allocation cover all passes. Accept cached playback only after real delayed-read, retention and hold witnesses. | Full G5/G6 extension transaction, G7, G8, G9, G13 retained-generation/cached playback |
| D | Face geometry/evidence, the restricted shared solver, absolute primary edits and independent before/after evaluation. Local fill, moving-window fitting and extra passes remain deferred under R24. | G10, G11 |
| E | Agent/app QA, version-bound feedback and encoded practice-video review with Songbird charts. Measure the integrated path and each supported lane. | G12, full G13, G14 |

Stage A is an experimental, design-selecting trial, not construction of the durable store.
If the selected estimator or a precision fails, revise that decision before Stage B/C, rather than
add unmeasured fallbacks. Keep one shipping runtime. AW2 animation waits for its own real renderer.
Default review count is one per stage. Persistence/data-loss/security boundaries follow the product
brief's two-review requirement. This design task runs none of these trials or witness mutations.

## 13. Fixtures, metrics and discriminating exit gates

### 13.1 Fixture ownership and temporal definitions

All gates below are NOT RUN. The implementation owner must retain a literal failing behavior with
each targeted guard/fix removed and restored green. Compile/setup/provider absence is not a witness red.

F1 is generated from owned primitives in Rust under GPL-3.0-only: independently known F_true,
B_old, fractional alpha, B_new, labelled pictures and explicit transforms. The production input is
the opaque mixture S, not pre-isolated F_true. Inject known alpha through the artifact reader.
The oracle composites independently known F_true over B_new. A black strand, white old room,
alpha 0.5 and black new room expects zero, exposing the incorrect result 0.25 from F=S.
Include colour texture, nonlinear grades, fade, crop/reframe, CC5 windows and known generated motion.
This proves mapping/math behavior only; a model is not expected to recognize a drawn person.

F2 is a consenting self-recorded C920 presenter, with moving hair, noise, glasses, gestures,
microphone and lighting changes. F3 is owned real-person foreground/green-screen material with
independently keyed and manually reviewed reference alpha and recorded uncertainty.
Stage A's implementation owner captures or acquires them, documents camera/source transforms,
hashes every artifact and obtains explicit repository redistribution permission before accepting them.
No cleared F2/F3 artifact is established by this draft. Their deliverables cannot be deferred to Stage E.
Riel's practice take stays private unless separately authorized; another consenting person can supply fixtures.

Spatial metrics are alpha MAE on [0,1], SAD=sum absolute error/1000, IoU at alpha 0.5 and boundary
MAE in the reference five-pixel edge band. Report uncertain/excluded regions and counts separately.
For generated or explicitly transformed plates only, use their known warp and validity mask.
For genuinely recorded motion use reference temporal residual
`mean(abs((a_t-ref_t)-(a_prev-ref_prev)))` on independently valid reference pixels in fixed image
coordinates, plus static-region flicker and moving-edge spatial errors. This compares error changes,
not a claim of known physical correspondence. Do not label arbitrary F3 recorded motion a known warp.
Any optional measured flow has a separately validated mask and remains secondary evidence.

Provisional spatial targets are MAE at most 0.02, IoU at least 0.97, boundary MAE at most 0.08,
and temporal residual mean at most 0.01. Stage A's reference reviewer freezes thresholds from
reference uncertainty before candidate scores. If reference error prevents discrimination, replace
or improve the reference, not the score. Each planted fault must exceed its intended frozen threshold.
Do not claim temporal quality from a frozen moving matte that happens to have low raw frame difference.

### 13.2 Gates

G1. Install/packaging. Old projects open with inference absent on Linux and Windows MSVC CPU.
The exact sealed RC/native/model profile works offline, and Linux CUDA reports actual provider placement.
Record download/install/build/startup/RSS/VRAM costs. Wrong model byte, native/API mismatch or missing
provider refuses its specific preflight. No unexpected compile-time binary fetch or hidden CPU fallback.
Upgrades rerun parity, quality and cached-artifact reopen. Windows GPU remains explicitly deferred.

G2. Real matte and representation. Run RVM F2/F3, both alpha precisions and the selected estimator
on the same black/white/colour composites at 100%, 150% and actual crops. Score spatial/temporal error
and review motion. Erode edges 3 px, alternate alpha by 0.1 and freeze a moving matte. Demonstrate
separately that each spatial/flicker/moving-edge witness fails above its threshold, with a clean control.
Retain captured assets, rights, reference uncertainty and frozen thresholds. Tiny wrapper success is insufficient.

G3a. Picture mapping. F1's injected alpha tests production selection/seek/rotation/proxy mapping with
VFR holds, tied PTS, freezes, trims, reuse and fractional output rates. Random seeks
`[0,90,30,90,0,last]` match sequential retained pixels and picture IDs. A one-picture offset must fail.
This gate intentionally makes no claim about neural recurrence.

G3b. Recurrence. Use a deterministic stateful adapter through the production worker whose later alpha
demonstrably changes if state is reset. Compare uninterrupted source analysis, cut/reused timeline uses,
pause/replay and disjoint range replay. Reset-at-cut, reset-at-range and discard-state mutants must fail
pixel equality. Separately run selected RVM on F2/F3, establishing state-sensitive frames and actual
temporal quality with a reset/discard-state mutation. A test adapter cannot clear the real-model quality gate.

G4. Owned inputs/generations. Replace the original path with same-length bytes and mutate it in place
during analysis, preview and export. All admitted decoders retain the snapshot's exact bytes/pixels.
Model/profile/generation changes plus delayed old completion cannot attach the old artifact.
Remove snapshot binding or epoch validation separately and observe the intended wrong-identity failure.

G5. Store ownership/recovery. Race two real openers through aliases against the same file identity.
Exactly one JA-1 owner reserves/publishes; the other attaches/refuses. Kill the owner while its worker
is alive. The successor cannot race cleanup or duplicate inference. Kill/truncate at payload/manifest/root
boundaries, inject sync/publication failures and reopen. Preserve exact acknowledged roots/artifact pixels.
Generate different output bytes under the same recipe while old undo/export pins survive: both digest
locations and retained pixels remain distinct. Clear a pinned-full store and interrupt Save As/relocation.
Assert prior source/destination validity and exact retained inputs, not matching counts alone.
Extend through the real host path in B/C: an old version and live export pin A while a trim is durably
saved beyond coverage. Seal C, complete QA and commit the reviewed expected-revision rebind, then
undo/redo/reopen. Assert A's exact canonical entry bytes, chunk/file identities and retained pixels,
C's complete union with direct shared references, actual charged unique/temporary bytes and no early
release while either root/export survives. Make overlap replay differ deliberately; A's pixels must
remain exact. Crash between C seal and root commit: recover the prior acknowledged root, pins and
charges without auto-binding C. Deliver completion after a newer edit: preserve that edit and refuse
the stale rebind. Remove sharing, dependency pinning, durable publication or revision checks separately;
each targeted witness must fail on the real storage/binding result, not an adjusted counter.
Process-death tests establish process recovery; durability-barrier failures need separate storage evidence.

G6. Editable format/portability. Apply/reopen/split/trim/freeze/bypass/undo/redo a background batch and
compare canonical edits and pixels. Exercise every operation/load/replay path, a bypassed stored resource,
legacy bytes, future-version refusal and copied history/version dependencies. Remove resource serialization,
validation or the shared feature predicate independently; the intended round-trip/refusal witness must fail.
Save/reopen the pending-coverage trim with A bound; it must remain pending and enabled export must
refuse even when unbound C is sealed/job-ready. After reviewed rebind, verify complete new coverage,
exact old pixels and version-bound proofs, including newly exposed pictures and the old/new boundary.
Undo/redo/reopen must restore the respective A/C bindings; an export pinned to A must retain its pixels.
Race a fresh edit with the bind and require stale refusal, not automatic attachment. Exercise seal/root
crash recovery and measured shared/temporary charges jointly with G5/G13 through the real host path.
Remove the bound-digest coverage check or substitute recipe/latest-job lookup: the pending-edit export
witness must fail. Interrupted package/relocation never leaves a published root claiming missing
dependencies are portable; shared chunks needed by copied old/new roots must be present exactly.

G7. Foreground and colour order. Feed opaque F1 mixture S with independently known alpha, F_true,
B_old and B_new through the production estimator. Compare to the independent F_true composite, not to
another F=S implementation. Provisional working-linear error is max 0.002 and mean 0.0002, with monitor
error at most two codes excluding separately declared codec loss. Exercise nonlinear curves/looks,
CC5 layer-quad windows under crop/reframe, alpha-zero/one, premultiplied resizing and fade override.
Bypass decontamination, grade before estimation, use source UV for CC5, gamma-filter, double-alpha or
apply alpha before fade: each mutant needs its own visible failing vector. Blur must exclude bright face
leakage into the room. If wrap is later re-admitted under R21, its nonzero witness must reject an
upper-chart-fed backdrop; that deferred witness does not gate the default. Real F2/F3 review
remains required even if synthetic colour math passes.

G8. Preview/proof/export parity. Same source picture and mapping, independently resampled oracle,
proxy/full geometry, final 1080p encode/decode and selected vertical crops. Retain pre-encode alpha/edge
evidence separately from codec error. Neighbor-picture alpha and proxy-alpha export mutants must fail.
No thumbnail/PNG-only proof clears encoded delivery.

G9. Actual reservation ownership. Barrier-controlled tiny budgets overlap source/PNG decode, required
set/prefetch, conversion, eviction, GPU submit/retire, cancellation, late completion and shutdown through
the production paths. Observe padded/scratch allocations, final-owner charges and teardown to zero.
Removing a reservation, releasing before a reader/GPU fence, or retaining a resource must fail a real
over-admission/leak witness. Cover every current live pass in §7. If wrap/fill is re-admitted, gate
its additional resources through this same owner before enabling it.

G10. Face placement. F2/F3 include moving heads, glasses, occlusion, two faces, reference neutral patch
and changing room colours. Valid cheek patches follow the selected face and exclude invalid intervals.
Geometry is checked at actual crop/punch-in. Freeze the region or use the room as a neutral and require
the evidence/parameter witness to fail. Separate generated patch math from detector recognition quality.
If five landmarks fail, return insufficient evidence and defer fill rather than add skin parsing.

G11. Idempotent colour planning. Controlled exposure/WB changes produce ordinary absolute parameters,
improve the target within bounds and survive replay. After commit, replan the identical request at the new
revision and require zero operations. Include an existing static correction and a target outside safety
bounds. Disabled/masked primary, multiple nodes, keys and nonlinear curve/look return the specified
unchanged conflict. With no neutral, both entrypoints preserve WB. A chart covering the face must not
affect isolated-host measurements. Stale revision/reference/source rejects. Residual-as-absolute and
repeated-pregrade-delta mutants must fail. Local fill is deferred; if re-admitted under R24, its separate
witness must disable invalid intervals and preserve alpha before it can be enabled.

G12. QA discrimination. Plant missing alpha, a 3-pixel boundary jump, a 0.1 alternating-alpha segment
and lost face evidence in otherwise clean fixtures. Each signal identifies its interval and actual worst
proof pixels; clean intervals remain below the frozen heuristic threshold. Disable each signal and bind
proofs to a wrong generation separately. Preserve sampled/full coverage and manual semantic review limits.

G13. Capacity and runtime, split across stages. In A, measure both precision/read paths, full 20/40/60
minute projected peak overlap from actual recordings/bytes, and used-range/replay admission before
building the store. Include an evolving trim with A retained by an old version and live export while
C shares old chunks and adds only the missing coverage. Record actual unique completed bytes, both
manifests and peak temporary/reservation overlap under both feature ceilings and host media quota.
Do not treat a 240-second playback run as long-recording capacity evidence.
In B/C exercise the real host extension transaction from G5/G6: save pending, complete, QA/rebind,
undo/redo and reopen with exact old pixels and complete new coverage. Assert actual charged unique/
temporary bytes, stale-result refusal and crash recovery between seal and durable-root commit. A fit
calculated from unique bytes alone does not pass if temporary overlap or the host quota prevents it.
In C, run real production alpha reads and delay one using a barrier, retaining resources across a real
GPU submission. Capture presentation identities, hold start/end/ages and release chronology. Delay a
single frame enough to violate the absolute hold gate even if mean rate/p95 still pass. A retained-resource
mutant must trip admission or leak detection without manually changing an alarm counter.
Then measure the full 240-second host+charts blur/replacement path and relevant existing PF1 workloads.
Stage E repeats integrated affected paths only as needed. Report every OS/backend independently and
mark pending/shared-load/NOT RUN outcomes honestly. H-T remains pending until its actual evidence exists.

G14. Finished tight-end video. The lead makes the serious script/retake practice cut with Songbird JSON
charts, cards, captions, cuts/punch-ins, natural correction and blur/replacement variants. An independent
model reviews encoded edges/look/motion against supplied references before Riel watches once.
The default blur plus natural-look cut must clear this gate without local fill or below-host wrap.
Re-admit either only after the real tight-end take demonstrates a benefit, with its own review/gates;
moving-window fitting and extra passes are not Stage D/E prerequisites.
Record Riel's own minutes, publishability judgment, failures and unrun checks. Solid/gradient and a
supplied still are required supported backdrops; AW2 animation stays pending until real integration.
Test intentional Off export as a reviewed choice, and missing-analysis enabled export as a refusal.
No public-fixture consent or optional fill/wrap blocks the private dry run.

## 14. Risks, reductions and deferrals

The estimator can fail on similar foreground/room colours, large unsupported fractional regions,
camera sharpening/noise or motion blur. More feathering is not recovered hair. Reject failing replacement
cases and show an honest original-room/lower-blur alternative or use a better retake/key light.
The product brief, “Jobs A”, permits lighting improvements; that is not a software quality pass.

The dominant storage cost may be matte bytes and immutable source overlap. Flat shared-chunk extension
avoids copying retained payloads, but added unique chunks, manifests, pins and temporary overlap must
still fit both quotas. A boundary quality failure or stale plan leaves the saved edit pending until a
fresh reviewed bind or explicit Off choice. The range policy limits stored output while replay can
still be expensive. Do not substitute unsafe source-path hashing for an owned input or increase disk
allowances to conceal a failing representation decision.
Full-source prepass decode may miss playback budgets. Stage C decides from actual reservations and
held-frame chronology before an enabled cached effect is accepted.

ORT's RC/API and native provider package are release risks. Exact install cost and graph support
remain Stage A obligations, not resolved by small model files. A Rust-only fallback must earn parity.
The restricted face solver intentionally refuses complex grades. This is preferable to silently damaging
manual nodes, accumulating retries or guessing WB. Five landmarks may be insufficient even for global evidence.

Deferred: physical depth, physical/neural relighting, semantic skin/hand parsing, arbitrary multi-person
selection, general rotoscoping/paint, live camera inference, persisted recurrent checkpoints, 4K/HDR
certification, transparent project export, Windows GPU and additional shipping runtimes.
Local fill and below-host wrap, their moving-window fitting and extra passes are deferred; re-admit
only for a demonstrated benefit on the real tight-end take and then require independent gates. G14
must pass with default blur and natural correction without them. AW2 animation, CC8 context integration,
JA-1 host delivery and PF1 H-T keep their own owners. None is claimed implemented by this design.

## 15. Numbered open questions and remaining evidence

### 15.1 For Riel, only after seeing the take

These are visual choices to resolve with variants if needed, not an intake checklist or engineering gate.

1. After seeing your room softly blurred beside the designed backdrop, which fits this video?
   Default: the softly blurred room. The agent can change it without re-recording.
2. If the framing makes it unclear, should the visible microphone and desk stay in this shot?
   Default: preserve the microphone and the intended recording setup. If separation cannot do that,
   show the original-room version rather than silently remove objects.
3. If a tighter crop improves a particular moment, which shown version looks more like the video you
   want? Default: natural appearance and the safer framing. Extra fill remains deferred unless the real
   take demonstrates a benefit under R24.

Separately, and only if useful: may a chosen short clip be redistributed as a public fixture?
Default is private. A refusal or no answer never blocks the private video; use F2/F3 from another
consenting person. The lead owns 150% testing, crop quality thresholds, codecs and resource limits.

### 15.2 For the lead and implementation owners

1. Stage A evidence remains open: exact RC feature/API/native/provider pins and hashes, package/build
   cost, real C920 quality, chosen precision/estimator acceptance, cleared F2/F3 and reference thresholds.
   The runtime and route are selected for trial; no quality/capacity/performance result is accepted yet.
2. Confirm the JA-1 host/store dependency at the implementation base, its durable-object/root and child
   containment behavior, and the narrow owned-input lease. No alternate JA-2 writer is permitted.
3. Allocate the coordinated JA-2/AW2/CC8/MO6 format number and approve shared CPU/GPU pre-allocation
   policy, including synchronous proofs/export. Counting ledgers and idle pools are insufficient.
4. Freeze the range handle, estimator bounds, actual recording/crop workload and solver convergence
   thresholds from the independent references. Prefer used ranges before any larger disk allowance.
5. Supply PF1 H-T held-event chronology and run the combined path before claiming playback acceptance.
   No completion date or active whole-process RSS allowance is established here.

Lead decisions already closed the former rev1 Q1–Q6 policy choices: pinned ort/RVM provenance with
MODNet alternative, owned-input lease, early representation selection, one lead-owned format change
and typed resource, conservative five-landmark trial, and explicitly pending H-T. These are not new
questions for Riel. Remaining work is concrete verification and integration, all NOT RUN in this round.

## 16. Changes from rev1

No critic finding is rejected. The table records the chosen remedy, including the explicitly permitted
restricted face solver. It maps all seven findings to this complete standalone revision.

| Critic finding | Addressed in | Resolution |
| --- | --- | --- |
| 1. Camera RGB is not foreground colour | §§1, 7, 12–13 G2/G7/G8 | Required pre-grade bounded foreground estimation, explicit premultiplied filtering and CC5 mapping, post-fade replacement alpha, and an independent opaque-mixture oracle. Original RGB is a declared approximation that refuses failing replacements. Stage A acceptance remains open. |
| 2. Face edits need stage/idempotence semantics | §8; §13 G10/G11 | One restricted solver for both entrypoints, named isolated-host stage, absolute primary settings, pinned baseline/units/bounds, no-neutral policy and re-evaluation. Existing static correction and repeat-plan tests are explicit; disabled/masked/nonlinear stacks conflict unchanged. |
| 3. Exact ort RC and early packaging gate | §3; §12 A; §13 G1 | Select optional =2.0.0-rc.15 with defaults off and explicit dynamic/provider allowlist behind one boundary. Require an exact sealed native/API manifest before trial admission; unresolved numeric pins stay disclosed. tract is cost fallback, burn deferred, one shipping runtime. RVM weight provenance follows the lead. |
| 4. Representation/full-take capacity too late | §§4–5, 11–12; §13 G2/G9/G13 | Compare Gray8/Gray16 in A on actual outputs, calculate 20/40/60-minute overlap, prefer used ranges plus handles/replay, and gate production prefetch/pass admission and cached playback in C. |
| 5. Missing store owner and artifact address | §§4–6; §13 G4/G5/G6/G9 | Consume JA-1 host ownership, define reserve/write/seal/pin/retire, cross-project execution permit, digest artifacts separate from recipe lookup, crash/child recovery and root-last relocation. Same-recipe different outputs retain independent pixels/pins. |
| 6. Mapping/recurrence/temporal witnesses conflated | §12 A/C; §13 F1–F3, G2/G3a/G3b/G13 | Separate selector injection, stateful production-worker controls and actual RVM reset-quality trials. Known warps apply only to generated/transformed plates; recorded motion uses reference error changes. Fixture clearance is in A; real delayed reads and hold chronology gate C. |
| 7. Riel questions and Off behavior | §§2, 10, 13 G14, 15.1 | Show visual variants after inspecting the take, keep natural defaults, leave thresholds to the lead and keep public consent separate. Explicit Off may export; missing enabled analysis fails preflight. |

## 17. Changes from rev2

Only the focused round-3 decisions and their direct consequences change. Rev2's accepted model,
foreground, colour-solver, fixture and playback contracts remain; all acceptance gates remain NOT RUN.

| Round-3 decision | Addressed in | Change |
| --- | --- | --- |
| Adopt round-2 finding 1: immutable coverage extension | §§4–6 R11/R14–R18; §9 R25 | Flat immutable manifests directly share immutable chunks. Preserve old entries byte-for-byte; add only missing pictures and discard recomputed overlap. JA-1 reserves unique bytes plus temporary overlap, pins dependencies, seals C and commits a reviewed expected-revision rebind through its durable-root path. No generation chains, second owner, collector or ledger. |
| Retention, capacity and verification consequences | §§5, 11–13; G5/G6/G13 | Retain A/C and shared chunks through JA-1's complete root set/live pins. Apply feature and host quotas. Stage A includes retained-version/export extension capacity; B/C exercise pending trim, completion, bind, undo/redo/reopen, exact old pixels, complete new coverage, actual unique/temporary charges, stale refusal and seal-to-root crash recovery. |
| Pending coverage versus ready bind | §§4–6, 9–10; G6/G13 | Saving a trim may retain A with pending coverage. Initial/replacement ready binds require full dependency/coverage preflight. Readiness/export inspect the digest actually bound; sealed/job-ready C never silently attaches. Carry reviewed-plan/QA evidence through new pictures and coverage boundaries. |
| Defer optional fill and below-host wrap | §§1, 6–8, 10–15; G7/G9/G11/G14 | Defer controls, moving-window fitting and extra passes. Re-admit only for a benefit demonstrated on the real tight-end take, then require their independent gates. Default blur plus natural look must clear G14 without either. |
