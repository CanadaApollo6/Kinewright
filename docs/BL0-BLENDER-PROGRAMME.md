# BL0: agent-driven Blender programme design

> Committed copy of the approved design record (rev5, 2026-10-26). Round 4: Astra PROMOTE; round 5 narrow re-check: Muse Wonder Full PROMOTE. Riel's binding answers are folded in (§1, §5.1, §8, §11). Paths under `.review/` cite the lead's local review notes, which are not in the repository.

Status: **approved programme design, 2026-10-26**. Design only; every proposed gate is **NOT RUN**. BL1 builds after AW2 Part 2.
Source inspection uses main `cb4f6e2586974fbd7da1ba935e7c4e77390aeca0`.
The binding briefs are `.review/bl/task-bl0-design.md` and `task-bl0-rev2.md` through `task-bl0-rev5.md`, including Riel's answers.
Approval covers the design only, not implementation, sandbox certification or runtime acceptance.
Proposed symbols describe future contracts. Existing symbols are identified in §3.

## 1. Summary and goals

Riel's vision is an agent that creates an asset in Blender while editing a video, imports it, and continues the edit.
Keep the editable scene beside its rendered result. The person can watch, revise, undo, and move the project to another machine.

Build three slices, in the order Riel chose:

1. **BL1, background-render clips.** A Blender adapter on AW2 renders project-owned scenes into alpha-preserving frames.
   It carries duration, rational frame rate, dimensions, colour settings, and timing marks. Cheap stills support self-checks.
2. **BL2, live Blender.** The agent drives a visible, managed Blender window, inspects its viewport, and saves revisions.
   Its result enters through BL1. Saved manual changes make the linked clip stale until the corresponding revision renders.
3. **BL3, native 3D clips.** Blender exports a constrained glTF scene. Kinewright renders it on its existing wgpu device.
   The first version includes glossy metals and thin device glass under one project-owned, prefiltered HDRI, plus a screen
   playing a project media clip in timeline sync. It serves device mockups, camera moves with depth and mesh-based 3D type.
   Video textures reuse the existing decoder, frame cache and player. BL3 does not add another playback system.

Design now; start Linux BL1 immediately after accepted AW2 Part 2. The build order is Linux BL1, BL2 live,
BL1-W Windows background handoff, then BL3 ahead of job E. BL1-W is independently accepted and does not hold up Linux
BL1 or the both-OS live authoring/inspection checkpoint. Full Windows BL2 create-to-render acceptance waits for BL1-W.
BL3's technical dependency is BL1, not BL2 or BL1-W. Live Blender is an explicit full-access tier on both OSes,
off by default and granted per project by the person. Restricted Linux CPU rendering remains the default where certified.
A failed restricted launch never falls back to full access. The restricted profile is offered as the default only after
B1-mount proves the native handoff and the full profile is certified. Before that, the status is restricted unavailable.
**BL1 is not complete until automatic scene refresh works on Linux and B1-helper passes natively.**
A manual-preparation lane may land as an explicitly labelled checkpoint. It cannot satisfy BL1's exit.
Windows will still need manual scene-image preparation after saved changes; this design has no automatic Windows builder.
Every execution tier, including manual preparation and Windows live, requires the protected consent authority in §5.1.
Its installation is independent of the optional image helper. Unsupported consent configurations refuse execution.
Administrator setup installs the consent service once. For the first use of each restricted tier, Riel types the login
password on a separate text console on Linux or the Windows secure desktop and can choose a standing grant for own projects.
That secure prompt lists existing local projects whose native identities the broker can bind. Riel ticks the projects to
adopt as own projects in the same attended decision. Unticked projects stay outside standing coverage; later adoption
requires a per-project secure confirmation. Later locally minted projects use the grant without another prompt.
New and ordinary editing work when the broker is absent or not running. Origin goes unrecorded, so that project later
needs per-project confirmation or adoption before external execution. Imported, downloaded or foreign projects keep their
separate confirmation requirements. Full access, including live Blender and Windows background, always needs a per-project
confirmation. Active grants are visible and revocable.
**Expected monthly cadence: once at setup per enabled restricted tier, including ticked existing projects, then zero routine
prompts for covered own-project work. Add one confirmation per later adoption, unadopted or foreign project/tier scope,
or full-access project scope first enabled, and for scope widening, a new tier or re-grant after revocation.**
Ordinary edits and certified 5.2.x patch updates do not prompt. Certification records arrive without administrator action
per patch (§5.1). One prompt may combine the new decisions displayed in that scope. Authentication stays password-only.
Restricted Linux rendering also needs the installed certified mount component: declining that installation makes the tier
unavailable. The manual checkpoint uses the same installed builder/receipt/mount path and differs only in person-triggered preparation.
This adds a Blender lane to the job-driven order. It does not revive AW0's superseded AW3-before-AW2 proposal.
Measure agent tokens, tool calls, corrections, and time per accepted asset, plus Riel's interruptions and own minutes.
The product bar remains a publishable video within Riel's 30–60 minutes, with no invented facts. A rendered asset alone is not job completion.

Blender is always the person's installation. Kinewright bundles and auto-installs no Blender, Node, Python, or Chromium.
Blender's own embedded Python belongs to that external installation. Background processes exist only for their admitted job.
Pin the **Blender 5.2 LTS line**, with exact patch/build/profile certification. Recertify maintained patches promptly.
A Blender update pauses new renders until recertified. After a certified 5.2.x patch verifies within the same line/profile/scope,
the broker re-binds runtime identity without a password prompt. A new LTS line or wider profile needs confirmation.
Existing renditions keep playing, proofing and exporting without Blender.

## 2. Non-goals

No embedded `bpy`, Blender-as-a-library link, replacement modelling application, raster image model, or generated video model.
No character-animation product, physics engine, simulation playback, particles, cloth, fluid, hair, ray tracer, or full Blender material parity in BL3.
No general shader execution, arbitrary glTF extensions, universal asset downloader, remote control service, render farm, or cloud account.
No second project writer, history interpreter, render cache, player, agent orchestrator, or motion-curve language.
No promise that Kinewright Undo reverses Blender's in-memory undo stack or arbitrary Python's filesystem side effects.
Raster models remain deferred. Procedural geometry, scripts, materials, and code-generated assets are in scope.

## 3. Fit with AW, motion, colour, playback, and JA-1

Read with [AW0](../../docs/AW0-AGENT-WORKBENCH.md), especially amendment A1 and §15;
[AW2](../../docs/AW2-CODE-GENERATED-CLIPS.md) §§3–9; and [AW1](../../docs/AW1-HEADLESS-KINEWRIGHT.md).
[The product brief](../../docs/PRODUCT-BRIEF-2026-10.md) and [roadmap](../../docs/ROADMAP-AND-WORKFLOWS.md) govern job order.
The October job-driven order supersedes older fixed MO sequencing. BL does not make AW3, all of PF1-H, or HDR delivery a new SDR prerequisite.

| Dependency | Contract consumed and exact boundary |
| --- | --- |
| AW2 Part 2 | `ExternalClip`/`ExternalSpec`, adapter describe/validate/render, typed constant parameters, complete manifests, durable pins, stale states, one render queue, guardian, input leases, and native ingress limits. BL1 supplies another adapter. |
| AW1 | Authenticated HTTP and stdio, roots, consent, exact prepared operations, revision checks, lazy headless media, file-reference proofs, investigator restrictions. JA-1's host replaces recurring GUI/headless writer transfers. |
| MO1 | `AutomationCurve`, clip-local effect/enable keys, keep-outside trim/split semantics, ordinary 2D placement over rendered pixels. Blender animation remains in Blender. |
| MO2 | Identity-bearing resolved layers, straight source pixels, linear-light blending, adjustment/transition ordering, opaque final accumulator, invalid-value detection. BL3 depth exists inside one clip. |
| MO3/MO6 | Compound clips and title/template fields are consumers when available, not BL1 blockers. Mesh type is authored in Blender; BL adds no text/layout engine. |
| MO4 | Its accepted `ClipTimeMap` is required only for time-remapping stored frames/native animation. BL1 can re-render an explicitly changed timing request before MO4. No duplicate remap evaluator. |
| PF1 | Accepted scheduler/resource ownership, K-1/K-2 whole-set admission, K-6 synchronous proofs/export, stamps, final presentation, GPU retirement. Unrelated H lanes remain separate. |
| CC1/CC4 | Supported source profiles, f32 processing/f16 working storage, ordered input/correction/look stages, one final monitoring or delivery transform. Blender's view is not a creative LUT node. |
| CC8 | Accepted generated-SDR insertion for HDR projects; scene-linear output needs the additional source contract in A5. SDR BL1 can ship before CC8 HDR integration. |
| JA-1 | Accepted host/writer, branch histories, owned inputs, `HeadId`, digest-bound receipts, durable graph publication, source-retention handoff, Save As/recovery. Require composition acceptance, not an intermediate host commit. |

Current-source observations, rather than claims that these future dependencies have all landed:

| Existing symbol and source | Observation and design consequence |
| --- | --- |
| `ClipContent`, `model.rs` | Media, Title, Freeze, Adjustment, Solid exist. Code, External, and native 3D variants are absent on this main. |
| `PROJECT_FORMAT_VERSION`, `model.rs` | Maximum is already 3. AW2's historical statement that it is 2 is stale. Join the coordinated v3 union; do not allocate another version independently. |
| `RecordKind::External`, `RecordKind::supported`, `bundle_records.rs` | External is reserved but unsupported. A reserved name is not a usable renderer. |
| `ProjectStore::closure`, `ProjectStore::pin`, `StorePin`, `store.rs` | There is an existing complete-closure owner and pin mechanism. Extend its record edges and GC traversal. |
| `OwnedInputManifest`, `OwnedInputLease`, `owned_inputs.rs` | Owned bytes and retained access already have project types. A new Blender path string cannot replace those identities. |
| `BundleCommit`, `BundleCopy`, `SessionStore` | Bundle persistence, copies, and unsaved-source storage exist in `kinewright-project`. Blender never writes their authoritative roots. |
| `CommandEnvelope::digest`, `ReceiptLedger::lookup`, `receipts.rs` | Durable commands already bind payloads to identities. Live save/import retry must use that protocol. |
| `WorkingFrame::from_display_frame`, `frame.rs` | Generated RGBA8 enters through sRGB decode with independent alpha. Reuse this working conversion through AW2. |
| `WorkingFrame::from_rgba64_le`, `rgba64_normalization_max` | Managed video expects integer samples. Recognizing a float FFmpeg pixel format does not establish managed EXR support. |
| `probe_path`, `is_still_image_codec`, `decode.rs` | Ordinary still/video decoding exists. A complete, indexed Blender image sequence is an AW2 reader responsibility, not an existing generic sequence feature. |
| `FrameRenderer`, `Compositor`, `DecodedLayer` | Extend the existing preview/proof/export assembly and identity mapping. No parallel Blender compositor. |
| Workspace `wgpu` and licence | wgpu is pinned at 29.0.4; Kinewright is GPL-3.0-only. No glTF loader is currently declared. |

### Required AW2 amendments

These amendments are design obligations adopted with BL0, with implementation owners below. Retain Remotion's rendering
defaults and all acceptance gates except the consent authority explicitly replaced by A6.
**BL1a owns A6.** AW2 Part 2 lands as specified, including its original R21 path; that intervening state is **not yet
A6-compliant**. BL1a installs/certifies the broker, removes R21's headless-setting authority and migrates old grants,
then re-runs Remotion's consent/revoke/restart gates through the broker before exposing **any Blender tier**, probes or
inspection included, on that OS. There is no requirement to build this broker inside AW2 Part 2 and no weaker parallel
path after BL1a cutover. An OS awaiting cutover exposes no Blender execution. Stored renditions remain usable.
A6 strengthens execution-consent provenance programme-wide, including existing Remotion installations; it does not
retroactively relabel AW2's original acceptance as broker acceptance.

- **A1, R22/R23/R30, adapter dispatch.** Register `blender`, protocol 1, with a native data-only descriptor manifest.
  Reuse AW2 scalar descriptors and validation. Do not require Node, Zod, or an executing schema probe for Blender.
  Source creation/edit/seal and viewport inspection extend the adapter actions; pixel rendering retains the shared render contract.
  Amend R30's three service capabilities to ten at full BL3 scope: retain the three AW2 capabilities and add exactly
  seven Blender capabilities in §7. There are six before BL3. Ledger generated mutators and shared Operation definitions separately.
- **A2, R20/R25, timing context.** Add a versioned immutable timing attachment: authored clock, output clock, duration,
  half-open output indices, rational scene-evaluation map, marks/beats, and binding policy. Existing fixed External mapping stays unchanged.
  Add an explicit atomic compatible-spec replacement path for project-rate changes. MO4 sampling is opt-in after its acceptance.
  Retain authored fps, use frame/subframe evaluation, and require an executable timing binding or an explicit successor-scene rebuild
  for `timeline_locked`. A timing digest alone cannot authorize ready output.
- **A3, R24, source refresh.** Replace person-built per-edit images only on an OS that accepts §5.5's separately installed
  privileged helper and unprivileged builder. This explicitly adds a privileged service; it never grants mount/volume authority
  to the ordinary account or Blender. Captured, hashed blobs are built into images without privilege. The helper verifies a
  trusted builder receipt and the actual immutable digest before fixed-root attach/mount. No caller-supplied image is mountable.
  Linux's candidate is SquashFS/fs-verity. Enrollment covers only an image-service-launched submission client under its
  dedicated locked service UID, not Studio/the JA-1 host. Image supervision and receipt signing use a separate image-service key.
  §5.5 specifies builder activation, grant-bound project enrollment and one mount handoff; B1-mount precedes BL1a admission.
  Windows retains person-prepared NTFS VHDX because this design has no certified unprivileged NTFS/VHDX builder.
  Do not ship or install the Windows image service until that builder and its native gates exist. The separate consent broker remains mandatory.
  Declining the certified mount-component installation or failing its certification makes restricted Linux execution unavailable.
  A labelled manual checkpoint requires the installed certified builder/receipt/mount path and differs only in person-triggered
  preparation before automatic-refresh acceptance. No writable-directory fallback or BL1-complete claim follows.
  Retain R24 backing/root/executable leases, replacement/change-then-restore witnesses and the single input-lease owner.
- **A4, R14/R26/R27, renditions.** Add an immutable scene revision and a write-once render-binding identity per desired spec.
  Bind that identity to a content-hashed output manifest through a durable host receipt after verification/cleanup.
  Completion changes bundle-derived render state, not Core history. Equal request keys may dedupe, but different output bytes never overwrite
  a saved rendition. The first verified binding wins. Later completions remain unbound alternatives.
  Person-only `AcceptExternalRendition` creates a successor spec/binding through history; Undo pins the old closure.
- **A5, R19/R25/R26 and §9, colour/output profiles.** Keep `display_srgb_rgba8_png` as the BL1 default.
  Add `scene_linear_709_rgba16f_exr`, with explicit reference calibration, alpha association, bounded float decode, and CC integration below.
  Public EXR has a 96 MiB compressed-frame cap; PNG stays 40 MiB. Mandatory pre-PNG float32 evidence has its own
  192 MiB/frame bound and hash binding in §6.5. All outputs/evidence share AW2's 1,000-frame, 2 GiB/render and 8 GiB/substore limits.
  Multi-layer EXR is source/archival material, not a directly composited pass graph. No ProRes/FFV1 requirement is added.
  Both profiles require coverage-representable alpha. Additive zero-alpha emission refuses, even in EXR; no silent RGB discard.
- **A6, programme-wide R21/R27–R29 and JA-1 §6.3/§12, consent and lifetime.** Replace R21's headless-setting authority
  for **all external execution, Remotion included**, with §5.1's mandatory protected broker and authenticated person receipt.
  BL1a owns the cutover and Remotion re-gating described above. Editable policy files/flags cannot become grants.
  Old grants require one fresh confirmation under this standard; a new restricted standing grant can cover eligible own projects.
  AW1 destructive-edit consent remains separate and cannot authorize external code. Persist grants outside agent-writable roots.
  Add one explicit visible-session slot beside the one active external-job slot. Background jobs still exit after every action.
  Apply the same guardian handshake/cleanup to probes, scene edits and live Blender. A render cancel addresses only its tree.
  Amend JA-1 §12 and AW2 R28's separate caps to §5.3's combined maximum of seven transient work trees, including native SVG
  and the unprivileged image builder. The consent broker and image service have separate OS owners and bounded control/attachment records in §5.3.
  No new tree replaces an occupied cleanup slot. Guardian death also closes the managed visible Blender window.
  BL2 callback quiescence is required before acknowledged saves or person handoff (§5.4); arbitrary full-access Python
  also requires the process-replacement boundary there. A6 adds protected local-origin enrollment and separates confirmed
  line/profile scope from exact engineering-certified runtime bindings, so standing grants and patch updates do not self-widen.
  Blender-only frame/total watchdogs and optional child safety limits are §9's profile amendment; Remotion's limits do not change.
  A3 changes only submission-client enrollment and the blocked child mount handshake, not JA-1's host launch or writer authority.

## 4. Architecture per layer

### 4.1 BL1 uses the external-render adapter

The host owns intent and receipts. Its native adapter prepares immutable inputs, invokes the AW2 supervisor, and verifies outputs.
A small GPL Blender driver runs inside the person's Blender. Kinewright ships driver source, never a Python runtime.
The driver accepts a manifest path after `--`; source code and secrets never go in command-line expressions.

The command shape is an argument array equivalent to:
`blender -b --factory-startup --disable-autoexec --offline-mode --python-exit-code 70 -P driver.py -- request.json`.
The driver loads only the preflight-admitted closure with auto-execution disabled, applies typed parameters/context and controls output paths.
Creation from a script runs that explicit hashed script under a code-input grant. §5.4 inventories every supported executable route
before evaluation/render/export. Imported code-bearing scenes without that grant refuse. They never render an altered static substitute.
The trusted driver itself is pinned runtime code. Its presence does not grant permission to embedded or agent-written code.

CLI `-P` runs a file; `--python-expr` exists but is deliberately unused. `-f` renders selected frames, `-a` the inclusive animation range.
Blender evaluates options in order. If the certified adapter uses `-f`/`-a`, scene load and all overrides precede the render trigger.
The explicit driver normally loops over requested indices and emits bounded `render_write`/completion events on a separate control channel.
Human-readable `Fra:`, sample, memory, and timing logs are diagnostics, never the completion protocol. [R2, R3]

Admission freezes source revision, parameters, timing, colour, executable/runtime image, device profile, and resource reservations.
Render index `j` writes one fixed native-generated name. The host checks the complete index set, dimensions, decode limits, tags,
alpha, byte/pixel hashes, and native process exit. Exit 70 denotes a command-line Python exception in this profile.
Zero exit alone is insufficient; missing frames or a missing completion record fail. Signals/crashes/nonzero codes remain distinct diagnostics.
Cancel requests cooperative stop, then AW2's 500 ms grace and whole-tree termination. No partial output becomes ready.

Choose Cycles CPU for the reproducibility fixture and the default background profile. Offer certified EEVEE and Cycles GPU profiles explicitly.
Each still uses the same source/context at a selected time with a distinct preview-quality key, normally 512 px and 16 samples.
It is a labelled draft, never a final clip frame. A final-quality proof can render one frame before the full clip.
Cached playback/export/proofs of ready output do not discover, probe, or start Blender, even when execution permission is revoked.

### 4.2 BL2 is a visible working session with immutable saves

Kinewright launches a new Blender GUI on a disposable writable working copy. It does not take over an arbitrary existing unsaved window.
The window belongs to a host-issued session bound to project, branch, source revision, owner epoch, and one editing authority.
Closing Kinewright's viewer GUI need not close the host-owned Blender window. Project close stops the session after checkpoint/cleanup.
Host death invokes the guardian and may lose Blender's unsaved RAM state; the last acknowledged scene remains durable and recoverable.
Guardian death also kills the visible managed window. The periodic checkpoint contract in §5.3 bounds ordinary recoverable work,
but an unresponsive script can prevent a checkpoint. The UI shows the last acknowledged checkpoint time.

Launch with an argument array equivalent to `blender --disable-autoexec --offline-mode --python-exit-code 70 -P live_bootstrap.py -- session.json`.
Do not pass `-b`. Use a private managed config with the pinned bridge extension already installed from its source package,
clean startup data and explicit script/data/cache locations. Do not load the person's preferences, startup file or personal add-ons.
The bootstrap enables only the pinned bridge and admitted dependencies, then opens the admitted scene under §5.4.
Do not combine a factory-startup reset that discards the extension with a claim that the extension remains installed.
Private config and offline mode reduce ambient behaviour; live Python still has full file and internet access.

| Control option researched | Fit and decision |
| --- | --- |
| Community `blender-mcp` | MCP server plus JSON/TCP add-on, typically port 9876. The inspected upstream is MIT-labelled, uses an external Python/uv server, allows arbitrary execution, and documents an unauthenticated add-on socket. Optional AST safe mode is not OS containment. Useful precedent, not shipped or depended on. [R8] |
| Blender extension platform | Gives packaging, manifest/version bounds, installation, and permission declarations. It is not a command protocol or enforced Python sandbox. Use its install-from-disk format. No account or online repository is needed. [R9] |
| Kinewright Blender extension | Chosen. A small GPL-3.0-or-later extension connects to the native host over a local authenticated socket. It exposes scene/viewport/save actions and controlled script execution under the existing grant. No separate Python MCP server. |

Use an AF_UNIX socket in the private runtime directory on Linux, an ACL-restricted local named pipe on Windows.
The Blender extension initiates the connection. A per-session 256-bit secret arrives through a one-use inherited channel,
never argv, the `.blend`, project data, or logs. Bind authentication to session nonce, protocol version, executable profile, and host epoch.
Reject remote peers, wrong principal, replay, stale sequence, oversized frames, and unsupported versions before actions.
The socket authenticates requests; it cannot make malicious code in the same trusted Blender process safe.

Network I/O only enqueues bounded messages. Blender API actions execute on its main thread using a timer/operator queue.
One agent edit transaction runs at a time. Before it starts, save/seal any person edits, verify the expected base, and acquire agent control.
The extension visibly marks agent control. An authoring call returning does not end that control.
Before authoring code runs, install §5.4's timer/msgbus interception and handler snapshots. Before acknowledging a
checkpoint or person handoff, retire transaction callbacks and establish its quiescence barrier. Until then the inspector
says `Agent finishing changes` and person editing remains blocked. Unsupported asynchronous authoring refuses.
Interception only tracks cooperative code. Any transaction that ran arbitrary full-access Python also requires §5.4's
terminate/reopen boundary before acknowledgment or handoff, even if its tracked-callback barrier succeeded. If interception
cannot be certified, use that boundary after every Python authoring transaction. Preserve the last acknowledged checkpoint;
unverified provisional work is retained only as a proposal, with any loss made visible.
If a person changes the scene before acquisition, the command conflicts and the agent reinspects. No automatic overwrite or script retry.

Save creates a new immutable scene proposal. `save_post` is a hint; native hashing of captured bytes determines revision identity.
Unsupervised working-file edits are noticed by directory notification plus a bounded periodic hash check, including linked dependencies.
Unsaved Blender changes show `Unsaved in Blender`; they are not claimed as captured just because an event fired.
On a stable save, the host publishes one source-replacement operation through the correct branch/manual-history owner (§6.3).
The clip becomes stale immediately on verified change; no old render is relabelled. Matching re-render completion clears stale.

The two-way handoff sends timing/colour context into Blender and returns scene revision, supported markers, and render/export proposals.
Returned marker edits are proposals for ordinary Kinewright operations, not authority to change its timeline.
Viewport snapshots identify session, scene generation, camera/view, time, and quality. They are cheap perception, not colour-managed final proofs.
The agent imports through BL1 after inspecting a final-quality still over the actual edit.
Live permission never authorizes a background child. On Windows, live authoring/inspection can be available while background
rendering is unavailable or ungranted. The source stays captured/pending until that separately accepted background lane and grant
can produce its rendition. Do not claim complete Windows BL2 create-to-render acceptance before that handoff passes.

### 4.3 BL3 renders a constrained glTF scene on the existing device

Propose `ClipContent::Scene3d(Scene3dClip)` with an immutable `Scene3dAsset` and a finite animation/camera selection.
It is one video-track clip. Its internal node hierarchy is the imported asset, not a second project-wide layer model.
Depth is shared among objects inside that clip; separate clips composite in track order. Cross-clip occlusion is out of scope.
`Scene3dClip` stores an asset ref, validated scene/camera/animation indices, fixed-point reference-white control, finite timing tuple,
and optional owned environment/video-binding references. The project graph traverses these references through history, versions and Save As.
Propose `AddScene3dClip`, `SetScene3dSource`, and `SetScene3dView` as ordinary atomic operations. Camera motion stays in glTF animation;
placement/enable/effects reuse existing owners. Asset floats remain in the verified glTF blob; Core adds no floating motion evaluator.

Export evaluated Blender meshes as glTF 2.0, preferably a self-contained GLB. Convert procedural geometry and text to meshes.
Bake constraints into supported transform animation. Keep the original `.blend` and export settings as the editable source.
Blender's exporter supports cameras, PBR materials, punctual lights, transform animation, morphs, and skins. That exceeds BL3's accepted subset. [R12]

BL3 accepts triangle meshes; indexed positions/normals/tangents/UVs; static PNG/JPEG textures; metallic-roughness PBR;
normal, occlusion, emissive and unlit materials; OPAQUE/MASK alpha; double-sided flags; perspective/orthographic cameras;
point/spot/directional lights; and STEP/LINEAR/CUBICSPLINE node TRS animation with normalized quaternion evaluation.
Select one glTF animation per clip; no implicit mixing. Camera-node motion is supported; animated camera intrinsics are not.
Add the bounded thin-glass subset of `KHR_materials_transmission` and `KHR_materials_ior` below, plus an explicit native video binding.
Reject skins, morph targets, arbitrary BLEND transparency, general material/light animation, volume/clearcoat, compressed meshes,
external URLs, and unsupported required extensions. Report unsupported optional visual features rather than silently dropping them.
Use BL1 for those scenes. Area lights, Blender World lighting, arbitrary shader graphs, and simulations do not automatically survive export.

Use `gltf` 1.4.1 as the candidate parser, with default `import` disabled and explicit `utils`, `names`, `extensions`,
`KHR_lights_punctual`, `KHR_materials_unlit`, `KHR_materials_transmission` and `KHR_materials_ior` features.
Validate the original extension inventory before a parser can discard unsupported data.
Resolve all buffers/images through owned input handles; never call an unrestricted convenience filesystem importer.
Cap JSON at 8 MiB before parsing, then check counts, checked accessor offset/stride/span arithmetic, indices, cycles and decoded sizes before allocation/upload.
Allow only embedded GLB data or manifest-relative package files. Reject URL/file/data URI schemes and decoded traversal/drive/reparse escapes.
Use the existing image dependency with a deliberately enabled JPEG decoder and bounded allocations. Pin/audit exact dependencies at BL3 entry.
`rend3` is archived; adopting it is not a maintained-renderer shortcut. Importing a full Bevy runtime would add another scene/resource scheduler.
Choose a small Kinewright-owned forward PBR pass on pinned wgpu, with GGX/Smith/Schlick lighting and a CPU scalar lighting reference. [R13–R15]

Its actual cost includes hostile-input validation, texture colour roles/mips, animation, camera/projection, normals/tangents,
depth/culling, lighting, antialiasing, targets, GPU resource lifetime, shader validation, and GUI/agent controls.
Use up to four punctual lights plus one owned environment. A flat ambient term is insufficient for the chosen metal/glass device fixture.
No shadows, SSAO, temporal AA or separate tone mapper. Environment and glass are mandatory BL3 v1 work, with their costs in §9.

**Environment lighting.** `EnvironmentAsset` owns one bounded scene-linear Rec.709/D65 lat-long HDRI and its provenance.
Use an RGB half-float EXR under the bounded reader contract, or a certified Radiance RGBE decoder. Unknown primaries refuse.
The environment EXR profile permits exactly RGB, treats it as opaque radiance and retains the same scanline/window/compression bounds.
It is distinct from the RGBA clip profile. It does not widen generic media float acceptance.
Cap the input at 2,048×1,024 and 16 MiB compressed. Offline native preprocessing produces a 256-side specular cube with
roughness mips, a 32-side diffuse irradiance cube and a 256-side two-channel BRDF LUT. Total GPU environment residency is ≤8 MiB.
Use split-sum GGX or a numerically equivalent certified method. Native preprocessing is asynchronous, bounded and never per frame.
Its derived key includes original hash, conversion/calibration, orientation, filter algorithm/build, sample schedule, formats and resolutions.
Store derived blobs through the existing project store/cache owners. Changing the environment creates ordinary history and invalidates its dependents.
One active project-owned HDRI may be shared by layers. History may retain older HDRIs under disk/pin quotas.
Each clip selects its owned reference and bounded orientation/intensity controls. The complete required set of any presented
frame permits **one distinct environment identity**, including source/calibration/prefilter, orientation and intensity.
Layers using that identity share the ≤8 MiB allocation. A second distinct identity refuses the native frame before upload,
with a choice to use the same environment or render through BL1. Never silently substitute one clip's environment.
Charge that unique allocation once in PF1's GPU ledger through the last reference and fence retirement.

**Glass.** V1 supports a thin dielectric cover above the device's own opaque screen/interior, with finite IOR 1..2.5,
roughness and transmission from the two admitted extensions. Use Fresnel-weighted reflection from the environment, conserve
reflected/transmitted energy and sample the in-scene opaque colour/depth in a bounded transmission pass. Cap glass draws at eight.
Glass may not sample other timeline clips. After the opaque colour/depth pass, create one per-clip full mip chain
of the resolved RGBA16Float opaque-colour transmission buffer down to 1×1 using native compute downsampling.
Roughness selects bounded colour mips with depth-checked samples. V1 uses full-resolution opaque depth, with no depth mip chain.
Reserve the complete colour chain before work. At 1080p it is ≤22.2 MiB with admitted allocation alignment, base included.
Mips above the base use ≤5.6 MiB. Both are inside the ≤128 MiB target cap, never added allowances.
The base may alias the resolved opaque target when safe; count each actual allocation once by identity.
Generation, transitions and sampling count in the all-pass GPU budget. Retain targets through the last glass draw and fence.
Require an in-scene opaque backing for transmitted screen pixels at every evaluated pose.
An unsupported thickness/volume, unresolved transmitted sample or required cross-clip refraction refuses with a BL1 route.
This covers the chosen phone cover-glass fixture. It does not promise arbitrary glass sculpture or Blender path-tracer parity.
Retain OPAQUE/MASK coverage around the device silhouette. Optical transmission is not a substitute for alpha coverage.

**Moving screens.** `SceneVideoBinding` maps one material's emissive screen texture to a project-owned media asset and a
finite source window, rational clip-time offset and an accepted time map. It is a Kinewright attachment, not a claimed standard glTF video extension.
V1 allows one distinct video source per 3D clip, screen raster ≤1,920×1,080, no implicit loop and no separate audio player.
Audio remains an ordinary timeline media clip when desired. An out-of-range screen time refuses rather than holds the last frame silently.
Core operations `SetScene3dVideoBinding` and `SetScene3dEnvironment` atomically publish references and bounded controls with the scene.

At each requested project time, resolve the screen's exact source time through existing media selection and, when available, MO4 `ClipTimeMap`.
Extend `reader_demand` with these hidden-in-the-mesh media requirements before PF1 admits the whole frame set. Reuse its
reader regions, decoder permits, frame-cache identity, cancellation, working conversion and `DecoderGauge`. The same asset/time/raster
shares its decoded allocation with an ordinary timeline consumer. Distinct times use existing reader rules; no BL3 decoder or playback clock.
Sample the managed working-linear video texture as emission at the declared screen reference white. Do not apply sRGB decode twice.
Base-colour video substitution, procedural video shaders and recursive use of the scene's own rendered output are outside v1.
Proof/export request the same exact screen frame under K-6. A missing/late screen prevents a current proof and follows existing counted preview policy.
BL3 cannot label the previous video frame as current merely because the mesh camera is current.

glTF uses metres, a right-handed Y-up frame, and cameras looking down local -Z. Apply Blender's export conversion once.
Convert glTF projection depth to wgpu's 0..1 convention once; distinguish mirroring, negative scales, winding, and normal matrices.
Evaluate animation at the rational clip time, then cross the numeric boundary into f32. No stateful simulation or incremental time stepping.
This makes seek and reverse random access possible. It does not promise cross-device bit-identical PBR shading.

Render to a transparent f16 colour target with depth on the existing device/queue. Resolve MSAA, then convert associated coverage
to the compositor's straight-source contract and preserve its clip/frame identity.
At zero alpha require RGB already zero under §6.5; emission with no coverage refuses before any conversion discards it.
Feed that texture through the existing colour/effect/transform/composite path, with no full-frame CPU readback for preview.
The final MO2 accumulator stays opaque. Proof/export use the same scene pass and quality at the requested raster.
GPU-only source bytes count in the existing ledger, not a fictional zero-byte `WorkingFrame`. K-2 admission gains their actual residency.
Native shading emits scene-linear Rec.709, using §6.5's explicit reference-white mapping into the selected working space, with no AgX.
For already-managed screen pixels, convert the working primaries/reference to that scene domain explicitly before shading,
then use the common scene-to-working boundary once. Include both conversions in the screen binding's working-cache key.
Decode base-colour/emissive textures from sRGB; normal/metallic/roughness/occlusion channels and factors are linear data. Alpha stays independent.
The native frame key/stamp binds the applied scene/animation/camera/time, environment and prefilter hashes, actual media
asset/frame/raster/colour identity, source window/time map, renderer build, device/quality and working/output context.
Cache hits and presentation must carry those applied identities. A newer requested playhead cannot relabel an older screen frame.

## 5. Security model

### 5.1 A protected consent authority serves every execution tier

Threats include destructive or prompt-injected scripts, hostile `.blend` parsers, malicious add-ons, path swaps,
credential reads, exfiltration, uncontrolled descendants, and forged consent/output. Arbitrary `bpy` Python has OS capabilities.
Autoexec, factory startup, extension declarations, and offline mode are controls, not a Python sandbox. [R2, R9]

Separate the OS tier from action/code permission. The following named grants are off until the person confirms them.
The Linux restricted profile becomes the background default only after B1-mount and profile certification.

| Displayed grant | Scope and enforcement |
| --- | --- |
| Person inspection, restricted CPU | Passive closure/version inspection only, under the certified Linux confined profile. No imported code admission, evaluation, authoring or render follows from inspection. |
| Person inspection, full access | Passive inspection on Linux or Windows with the person's file/internet privileges. Native parser risk exists before code admission. No background render or live authoring permission follows. |
| Background Blender, restricted CPU | Linux supervised render/data actions. Hashed agent/embedded programs additionally require the code-input classes below. A failed restricted launch never retries full access. |
| Background Blender, full access | Optional Linux tier. Windows is separately delivered/granted as BL1-W. No enforced filesystem/network confinement is claimed. |
| Live Blender, full access | Both OSes, one visible managed session. Permits its displayed inspection/authoring actions, never a background child by implication. |
| Code-input permission | Additional exact classes for authoring, imported embedded hooks, approved add-ons or render-time programs. Bound to an OS tier and action set. Imported embedded code needs explicit hash admission. |

Windows Jobs enforce lifecycle/resource policy, not filesystem/network isolation. BL1/BL2 claim no AppContainer implementation.
Linux GUI/display/GPU access also has full-access status. Unsupported or ungranted tiers return `blender_execution_tier_unavailable`.
Stored frames and validated native glTF remain usable without execution permission or a running broker.

Full-access consent says: "Allow Blender code with the same file and internet access as you for this project?
This permission covers later agent-written revisions in the displayed scope. Code can read, change or delete your files
and contact the internet. Undo restores your Kinewright edit and captured scene. It cannot reverse file or internet actions."
Displayed roots describe admitted inputs/outputs, not an enforced limit on full-access Python.
Inspection consent also says: "Opening this scene runs Blender's file parsers before any embedded code is approved.
A damaged or hostile file can exploit a parser. In full-access inspection, that risk has your file and internet privileges."
Show this copy for downloaded and locally created scenes alike. Do not infer inspection consent from choosing a filename.

#### Authority, installation and custody

`kinewright-consent-broker` is a small native OS service, independent of the optional image helper. It parses bounded
consent/origin/certification records, never Blender/project code, image files, preparation manifests, builder/validator
results or renderer plug-ins. It has no project writer, builder-launch, builder-signing or mount API. The optional image
service supervises its own client/builders and owns an independent receipt key (§5.5). Its only calls to the broker are
bounded grant-generation queries; installing it never changes the broker's executable or parser surface.
Administrator installation is mandatory on both OSes for exposed external execution, including helper-absent/manual
preparation, probes and Windows live. An ordinary same-user GUI process is never the signer.

| OS | Protected principal, files and confirmation path |
| --- | --- |
| Linux | Root-owned system unit `kinewright-consent-broker.service`, root-owned executable/policy/PAM files, no user-writable module search. Key in `/var/lib/kinewright-consent/authority.key`, mode 0600 root; store in `/var/lib/kinewright-consent/grants`, directory 0700 root. Socket `/run/kinewright-consent/control` admits bounded requests from the enrolled user but cannot grant by caller UID. A root-owned confirmation worker uses a dedicated real kernel VT and PAM as specified below. |
| Windows | SCM service `KinewrightConsentBroker`, LocalSystem with a restricted service SID/policy and no volume-management privilege. Binary under administrator-protected Program Files. A nonexportable machine CNG signing key grants use only to SYSTEM/the service SID, never the user or ordinary machine-key readers. `%ProgramData%\Kinewright\Consent` and its store have SYSTEM/service-SID-only write ACLs and protected ancestors. A fixed SYSTEM-owned session confirmation worker uses Windows secure CredUI and LSA authentication below. |

The consent installer registers the authority identity, eligible UID/SID, fixed confirmation binaries and policy hashes.
It explains the trusted confirmation UI and removal before elevation. Installation never creates a project grant.
Broker binary/policy updates require administrator installation, preserve protected revocations, and invalidate certification
when the trusted path changes. Same-scope runtime certification records are checked through the separate patch-binding path below.
Removing the broker disables new external execution and cancels its active authorizations, while keeping project media.
Reinstallation rotates the installation ID/key unless a protected administrator migration explicitly preserves them.
No execution grant survives a missing/reset store by import from a project, a backup in the user's home or a policy file.

#### Person confirmation binds exactly what was displayed

An ordinary GUI/headless invocation can request confirmation. It cannot supply a successful confirmation or authentication result.
For adoption, the protected worker first collects the person's ticks from the broker's fixed, native-bound candidate list
below. The broker then creates one immutable final pending request, random 256-bit nonce and 120 s expiry.
It computes a canonical digest over
installation ID, intended UID/SID, grant kind (own-project standing or per-project), native project identity and displayed
path/title for a per-project grant or the protected local-origin rule for a standing grant, the exact selected adoption
identities with their displayed paths/titles and native roots, tier, action/code classes, fixed roots or the displayed
per-local-project root-derivation policy, certified LTS line/profile, network claim,
resource limits and standing-revision policy. Exact patch/runtime hashes are verified execution bindings, not new person decisions. Display bounded full scope without truncation.
Escape project-controlled labels. If the complete security scope cannot fit the protected prompt, refuse and narrow the request.
Only one prompt exists per machine. Repeated external requests are rate-limited and cannot change the displayed request.

On Linux the broker opens a reserved `/dev/ttyN` itself, verifies its root ownership, obtains exclusive access, flushes old
input and switches with `VT_ACTIVATE`/`VT_WAITACTIVE`. The fixed worker displays the complete scope, request digest and nonce
on that VT, then authenticates the named local person with `pam_start`, `pam_authenticate` and `pam_acct_mgmt` against a
root-owned `kinewright-consent` PAM policy. Require fresh authentication and a separate explicit confirm of that displayed request.
No cached sudo/polkit approval, desktop key event, caller PTY, stdin bytes or environment answers can satisfy this flow.
The supported console profile requires legacy TIOCSTI injection disabled, no unprivileged access to that VT, no same-user
input-injection device access, and verified exclusive input/display ownership during confirmation. Restore the prior VT on cancel/timeout.
If the compositor/seat/remote session cannot supply that trusted path, refuse `external_consent_authority_unavailable`.
A GUI dialog or ordinary X11/Wayland prompt is not a fallback. Native negative injection controls are B1-grant prerequisites. [R23]

On Windows the service launches only its fixed administrator-owned confirmation worker in the intended active local session,
using `CreateProcessAsUserW` with its fixed service-owned session token and process/thread ACLs that deny same-user
injection and handle duplication. The broker derives the session from OS active-console identity, never caller-supplied session IDs. Credentials remain in that worker
and broker, never Studio. The worker calls `CredUIPromptForWindowsCredentialsW` with `CREDUIWIN_SECURE_PROMPT`, a broker-built
`CREDUI_INFO` message containing the full scope/digest/nonce, no supplied credentials and no save checkbox.
It validates the returned credential buffer with the selected LSA package through `LsaLogonUser`, checks the resulting SID
against the intended person, and erases the buffer. CredUI success alone is not authentication.
The authenticated Submit confirms only that worker's fixed pending request. Cancel, a different SID, an expired nonce,
secure-desktop downgrade, remote/caller-supplied prompt or an unsupported credential provider refuses.
Use the OS trusted-path credential policy and test scope visibility/input isolation on each certified Windows profile. [R24]

Only the protected worker reports success to the broker over its inherited one-use private channel, bound to worker OS identity,
request digest and nonce. No public `confirm=true` or `issue_grant` endpoint exists. Consuming that nonce and durably
publishing the grant and any confirmed adoptions is one broker transaction. Only a new decision prompts: scope widening,
a new tier, adoption of an existing project, a project without standing coverage, or re-granting revoked permission.
A narrower request already covered by an enabled grant needs no confirmation.
A new LTS line/profile outside the confirmed scope is a widening; a certified same-line patch re-binding is not.

#### Standing scope, project origin and runtime certification

Restricted inspection and restricted background/code-input scopes may each have a **standing own-project grant**.
The protected confirmation says: "Allow this tier for your own projects created in Kinewright on this machine and the
existing projects you tick below, including later agent-written revisions in the displayed code scope? Imported projects
and full access still ask separately."
The active-grant list shows the tier, 5.2 LTS profile, own-project rule, code classes, enforcement, creation time and Revoke,
plus the bounded list/count of covered project identities. No standing grant covers **any full-access tier**. Live,
Windows background, optional Linux full-access background and full-access inspection all remain per project.
Foreign projects may receive a per-project restricted grant; confirming one never adds it to the standing own-project set.
Imported embedded code still needs its displayed exact hash admission, combined with the project/tier decision when possible.

"Own" means a project newly created here in Kinewright for the enrolled person, or an existing local project the person
explicitly adopts, bound to the broker's protected native project enrollment. A field or signature copied inside the
project cannot establish eligibility. BL1a adds a bounded `LocalProjectOrigin` record to that enrollment: installation/machine
ID, UID/SID, broker-minted enrollment identity, native root/store identity, creation or adoption kind and origin generation.
Adopted records also reference the actual protected confirmation event and selected-set digest. A protected local-origin
record establishes eligibility under the standing grant; automatic New enrollment is not a new person confirmation.

At standing-grant setup, the secure prompt lists the person's existing local projects by title and path, with unchecked
adoption choices. Ordinary callers may suggest candidates, but the broker derives and pins each native project/root/store
identity itself using no-follow access. Adoption is never offered for an identity it cannot bind natively. The protected
worker collects the person's ticks, then displays that exact selection and scope in the final immutable request before
password authentication and confirmation. The broker rechecks the pinned identities before committing only the ticked
adoptions with the standing grant. Unticked projects gain no eligibility. If the list exceeds §9's prompt/store caps,
narrow the displayed batch rather than omit or silently adopt a project. No extra authentication is needed for projects
adopted in that one standing-grant decision. Later adoption requires a per-project secure confirmation displaying its
native identity, path and applicable standing tiers/scopes. Adoption binds the existing identity; it never invokes New
for it. A same-user request, supplied checkbox state, document flag or forged confirmation cannot adopt a project.

With the broker running, the Kinewright New path registers a fresh empty project/root with no-follow, exclusive creation
before any loaded/imported project can be attached. The broker derives and retains native identities itself; it never
accepts caller-supplied IDs or a `local=true` claim as origin. New projects enroll dynamically within §9's quotas, without
admin work or another password prompt. New and ordinary editing also work if the broker is absent or not running. Origin
goes unrecorded in that case; installing/restarting the broker does not retroactively mint it. That project later needs
per-project confirmation or explicit adoption before external execution.

Opening/copying/downloading/restoring a project cannot invoke the New path or adopt its existing identity automatically.
Without protected eligibility or an explicit adoption confirmation, unknown origin, foreign machine/person, native-root
mismatch or a copied project is **foreign**, even with a copied local-origin receipt. Adoption never admits imported
embedded code or widens roots/code classes by implication; those separate confirmations still apply.
A normal Save As creates a new native identity, never copies grants; it may register an own-project successor without
prompt only when its predecessor has verified local origin and the host's receipt-bound copy creates the destination
under the already displayed root policy. Save As of a foreign project remains foreign. Copying files outside this
protocol needs its own per-project confirmation. A root replacement cannot inherit the predecessor's origin record.
Importing a project/scene or executable foreign closure into a local project preserves its foreign classification and
requires the relevant per-project/hash confirmation; creating an empty local wrapper is not an import-consent bypass.
Local eligibility derives only the project's own captured store and disposable working/output roots by native identity.
It does not authorize arbitrary external library/home roots. New external roots, code classes, add-ons or limits outside
the confirmed policy are scope widenings. New instance roots produced by that exact own-project policy are already covered.

This is a workflow provenance boundary, not a way to prove the intentions of arbitrary same-user shell. A compromised
ordinary host can misuse already-granted authority or copy data into allowed work, as discussed below; it cannot mint
protected origin/store records for an existing or copied native identity without a real adoption confirmation, or falsely
record a fresh person decision. Test the public New/Open/Import/Save As/adoption paths, native-root swaps and protected-store
custody, rather than trusting a document origin label.

The **confirmed scope** names Blender **5.2 LTS**, the stable certified profile ID, tier, code/action/root policy and limits.
An engineering-signed certification record maps that line/profile to exact patch/build,
executable/runtime/driver/extension/OCIO/launcher/device hashes and an enforcement-policy digest. The broker accepts bounded
records over its control socket or from a shipped file and verifies them against its pinned engineering public key.
An ordinary process may deliver signed bytes; delivery is not certification authority and requires no administrator
action per patch. The broker persists the monotonic certification-generation high-water mark and revocation state in its
own protected store, atomically and durably before ACK or runtime re-binding. It refuses any older generation regardless
of a file's contents or valid signature. An equal-generation replay is idempotent only for the identical accepted record
digest; conflicting contents refuse. Restart restores the high-water mark and revocations before considering delivered
records, so a replayed old signed file cannot revive a revoked runtime. §9 bounds these records without rotating away
security state. Broker executable/key/policy changes retain the administrator-update rule above.
The broker verifies the certification signature, generation/revocation and actual pinned runtime tuple before issuing a
`RuntimeBindingReceipt` tied to the existing grant ID/generation and its current boot epoch. It records which engineering
certification re-bound which runtime. This receipt is not a new person-authentication event. A user-writable delivery
file or version string cannot establish certification without the pinned signature and protected generation/revocation checks.
A record with a different tier, wider roots/code classes, weaker enforcement or
larger limits cannot re-bind under the old scope. A profile change outside the confirmed policy needs a new decision.

A certified **5.2.x patch** within the same profile/scope re-binds automatically for standing and per-project grants,
including already-confirmed full access. No password prompt follows. Until the new exact runtime verifies, **refuse** new
execution as `Blender update awaiting certification`; do not ask the person to override certification. A new LTS line,
new tier or widened scope prompts. Patch changes still create new pixel/cache keys and cannot relabel old renditions.
Certification removal invalidates runtime tickets without granting a broader profile. Engineering recertification is
an engineering gate; it neither grants permission nor resets a revoked grant.

#### Durable grants, restart and revocation

The protected store is the sole authority. A signed `ExecutionGrantReceipt` binds installation ID, UID/SID, grant kind,
confirmed scope digest, nonce, durable grant ID/generation and actual person-authentication event. A per-project receipt
also binds that native project identity. A standing grant creates no automatic per-project authentication events.
Adopted project records reference the actual setup or later adoption confirmation; New enrollment does not invent one.
Dispatch binds the protected eligible project-origin record, the parent standing grant generation and runtime binding.
These are references to live store state, never transferable offline bearer permissions. Public verification keys and
receipts may be copied, never signing keys. Every dispatched code hash is recorded under its actual grant and runtime.
Host/guardian validate the broker's current boot epoch, grant/origin generations and certified runtime at queue admission,
immediately before COMMIT and before ready/source publication. Broker loss/disconnection cancels execution admission,
invalidates dispatch tickets and blocks late publication. Full-access work outside the managed process remains uncontained.

A flag, TTY claim, provider environment/argv/config, MCP elicitation, restored session, document field or generic destructive
permission cannot issue/broaden a grant, adopt an existing project without protected confirmation or turn an uncertified
patch into a certified one. An unattended action with no covering grant refuses `blender_permission_needed`.
A covered own project can run unattended. Apply the same protected provenance standard to Remotion and every adapter at BL1a's A6 cutover;
Remotion full access remains per project. Never retain R21's weaker setting path after that cutover.

Revoke durably increments the parent store generation and marks the grant disabled **before replying**, then cancels
all covered queued/active execution and submission authority. Revoking a standing grant blocks **every covered project**,
including after broker/host restart. Covered-project entries are derived coverage, never independent enabled grants.
They cannot survive by cached tickets, fall back to a new grant silently or auto-reenroll after patch certification.
A separate previously confirmed per-project scope is displayed distinctly; it cannot silently replace revoked standing
coverage for the same request. Re-grant requires a new secure password confirmation. Revoking/removing one project may
instead disable its origin/enrollment without revoking unrelated projects; removal never reuses its identity.

Revoke needs no privilege escalation: the enrolled person may always reduce permission. Same-user processes may cause
revocation/denial of service, but cannot create consent. The inspector reads current broker state and exposes Revoke;
project data cannot create an active-grant row. Atomic store replacement plus file/directory sync on Linux, and protected
transactional replace/flush on Windows, precede ACK. Crash before durable ACK returns unknown and reconciles the same
request ID. Restart reads grant/origin/certification generations first, rejects old boot tickets/nonces, reconciles cleanup,
and only then reissues tickets for still-enabled coverage. Corruption, rollback detection, missing keys or unavailable
protected storage refuses execution until repaired and reconfirmed. §9 bounds standing grants, enrollment and audit history.

Honest boundary: arbitrary same-user shell already holds the person's privileges. Full-access Python can damage ordinary
files, impersonate ordinary user processes or misuse an already-granted user-level permission. This broker does not contain
that shell. It must prevent false attribution of new consent and acquisition of protected helper authority without a real
confirmation. Root, an elevated administrator, a compromised kernel/credential provider or stolen person authentication
is outside this boundary. Tests cannot turn that boundary into a promise against already-compromised privileged components.
Investigators/reviewers still cannot execute renderers. Specialist action/root scopes remain additional server-side restrictions.

### 5.2 Enforced roots and network policy

The intended read set is the sealed scene/dependency image plus certified runtime/OS libraries. Never bind the project store or home directory.
The write set is an expendable job output/temp root; live sessions also have their one expendable workspace.
The host imports selected files after verification. Blender never receives the root writer's credentials or writable project handles.
Clear inherited provider/cloud/proxy credentials, `PYTHONPATH`, `PYTHONHOME`, ambient script/config paths, and arbitrary environment overrides.
Use a private profile/cache, explicit runtime paths, no user startup/add-ons, and a pinned OCIO configuration.

The Linux implementation is **system-installed bubblewrap 0.13.1**, non-setuid, under a Kinewright-owned native launcher policy.
Pin its executable and policy hashes in the certification tuple. Do not accept an arbitrary `bwrap` from PATH or all newer versions automatically.
The tuple also includes kernel, systemd, cgroup delegation, seccomp architecture, Landlock ABI, SquashFS/fs-verity support and Blender/runtime hashes.
The fail-closed prerequisite check verifies nonprivileged user namespaces, nested user/mount/PID/IPC/network namespaces,
private mount propagation, cgroup kill/reaping, the seccomp policy and actual allowed/denied operations before any source code.
An executable version string alone is not certification. Recertify changed prerequisites and retain their negative/positive controls.

The guardian creates a blocked bubblewrap child in its user/mount/PID/IPC/network namespace profile, with fixed empty
`/scene` and `/runtime` mount points. The helper transfers the pinned roots into **that child's existing mount namespace**
using §5.5's single privileged fd-based handoff. The ordinary host/guardian never enters the helper's private namespace
or performs an unprivileged rebind of an init-user-namespace-owned mount. Verify the resulting mount IDs and roots before COMMIT.
The fixed child gate drops remaining capabilities, applies `no_new_privs`/Landlock and the final seccomp policy denying
namespace/mount/ptrace escape, then execs the pinned runtime. Create private `/proc`, minimal devices and bounded scratch.
B1-mount proves this exact handoff natively before BL1a entry. Its failure is restricted unavailable, never a default-profile claim.
No display, D-Bus, SSH agent, host abstract sockets, home, host `/tmp`, cgroup control files, or GPU devices are exposed.
Use an isolated network namespace with no external interface; a preconnected inherited control channel needs no network listener.
Apply Landlock filesystem rights as defence in depth with runtime-queried ABI and tested rename/truncate handling.
Landlock alone is not the all-protocol network policy. Fail closed if any required namespace, syscall, mount, or guardian protection is unavailable. [R10]
Set `blender_restricted_unavailable` and show "Restricted rendering is unavailable on this system" with the missing prerequisite.
Cached work remains available. Do not steer the person into full access or offer an automatic retry at a different tier.

Full-access profiles request offline behaviour and disable all shipped download integrations, but network blocking is **not enforced**.
Do not silently replace a failed restricted launch with full access. The person must select that distinct tier.
Linux live GUI access to Wayland/GPU sockets broadens trust; X11 has additional shared-display exposure. Neither is labelled restricted.
Windows full access uses exact executable paths, ACL-private staging, retained input leases, and the non-breakaway Job.
Those controls protect Kinewright's workflow and cleanup; they do not contain hostile code running with the person's rights.

Third-party asset downloads happen in the existing host/harness acquisition workflow, outside Blender, then enter as local owned bytes.
No Blendermarket/Sketchfab/Poly Haven login, network helper, package installer, telemetry, or raster-generation service runs in the extension.

### 5.3 Lifetime, cancellation, and audit

Every execution uses AW2 `OWNED → CONTAINED → COMMIT`, with the JA-1 host holding the sole lifetime writer.
The guardian writes no project roots. Linux requires independent systemd/cgroup-v2 cleanup and reaping; Windows the sole kill-on-close Job handle.
Success, invalid output, failure, cancel, timeout, supersession, revoke, close, and host death all retain reservations until OS cleanup proof.
Guardian death invalidates that job's output even if all frames arrived. Lost cleanup observation is `cleanup-blocked`, never zero processes.
Restricted descendants inherit confinement. Full-access code deliberately launching work through outside services is not contained by that profile.
Show that limitation; no claim of hostile whole-machine containment follows from a Job/cgroup test.

A live agent action first checkpoints the current scene. Cancellation may terminate its managed window if Python is unresponsive.
The pre-action checkpoint survives; partial/unacknowledged scene edits are not called saved. Never kill the person's unrelated Blender process.
On ordinary close, retire agent callbacks and save/seal recoverable person work before stopping. Revoke first durably
disables permission, then attempts only safe recovery capture and cleanup. If no quiescent checkpoint is possible, retain
the last acknowledged checkpoint and workspace, show the unsaved state, and terminate the managed process.

The inspector exposes what ran: source/script hashes and viewable source, normalized executable/arguments, version/build,
effective roots, enforcement tier, network status, add-on list, device, timings, exits, cancellation, output hashes, and receipts.
Full script text and asset contents live in bounded source blobs, not logs. Secrets and ambient environment never enter provenance.
This is an execution record, not a syscall audit or proof that trusted Python had no side effects outside the admitted contract.

The following table replaces the independent caps in **JA-1 §12 Guardian** and **AW2 R28/R29** for their composition with BL.
It applies to the machine's one admitted Studio run, including headless ownership. A tree's recovery record stays in its slot
through cleaning or observation loss. Active and cleaning instances cannot each consume the same slot.

| Kind | Maximum transient work trees | Recovery records / total bytes | Cleanup owner |
| --- | ---: | ---: | --- |
| JA-1 provider attempts, including lead/reviewer | 3 | 3 × 1 KiB = 3 KiB | Shared ordinary guardian; host owns reservations |
| AW2 native SVG worker | 1 | 1 × 1 KiB = 1 KiB | Same ordinary guardian |
| AW2 external or BL background action, including probes/edits/stills/export | 1 shared | 1 × 1 KiB = 1 KiB | Same ordinary guardian; never one slot per adapter |
| BL2 visible Blender session and its in-process agent action | 1 | 1 × 1 KiB = 1 KiB | Same ordinary guardian; action does not add a tree |
| Unprivileged source-image builder/validator, serialized | 1 | 1 × 1 KiB = 1 KiB | Independent OS service owner in §5.5 |
| **Combined transient maximum** | **7** | **7 records / 7 KiB** | Six ordinary slots plus one image-builder slot |
| Installed image-helper control service | 0 renderer trees; ≤1 service process | 1 fixed recovery header / 1 KiB | System service manager, independently of host/guardian |
| Image attachments, mounted but not processes | ≤4 total source/runtime image leases | 4 × 1 KiB = 4 KiB | Privileged helper journal plus OS reconciliation |
| Protected consent broker | 0 source-execution trees; ≤1 native service | 1 fixed header / 1 KiB | Independent system service manager |
| Enrolled image submission client | 0 source-execution trees; ≤1 fixed native process | 1 fixed header / 1 KiB | Image service/system manager; dedicated service UID, no helper credentials in the host |
| Protected confirmation worker | 0 source-execution trees; ≤1 fixed native process | 1 fixed header / 1 KiB | Consent broker/system manager; 120 s expiry |
| **All transient recovery metadata** | 7 work trees plus at most 5 native control processes | **≤15 KiB** | No second project writer |

There is at most one ordinary guardian, consent broker, submission client, confirmation worker and image service while needed.
The consent broker is socket/SCM activated and may exit at idle after persisting state and retiring dispatch obligations.
Its protected grants store is a separate bounded durable store (§9), not part of project or transient recovery metadata.
The unprivileged builder is the seventh tree, not an uncounted helper child. No builder and validator overlap in that slot.
The helper's image leases may be shared by verified identity across ordinary jobs. Four means two source generations
and two runtime generations at most; refresh queues until a slot retires. Each ≤1 KiB image record includes bounded
consumer/namespace-clone identities for at most six ordinary slots. Manual Linux leases use the same four-record allowance
under the certified mount service/input-lease owner; helper-absent restricted Linux execution is unavailable. Windows manual
R24 leases use that allowance under their ordinary input-lease/OS owner, without an image service. There is no hidden
per-clone record allocation or unbounded per-edit mount accumulation.
Consent broker, confirmation worker, submission client, image service, builder and guardian memory, IPC, locks and pins all count in native §9 costs. Provider/Blender process
RSS remains separately reported. These record bytes are metadata caps, not a process-memory allowance.

Before COMMIT the host reserves the kind slot, recovery record and all owned bytes. One ordinary guardian dispatches
distinct cgroup leaves or distinct Windows Jobs; it never gives Blender a provider's handles or credentials.
Killing the shared guardian invalidates output from all six ordinary slots and triggers independent OS cleanup of all six,
including the visible Blender window. A dead guardian cannot certify any buffered READY, even after the last frame arrived.
The privileged helper's independent service survives to detach after actual consumer emptiness, not merely guardian EOF.
If a tree, mount or OS identity cannot be reconciled, keep its record, pins, byte charge and slot as cleanup-blocked.
Host idle exit waits for those obligations. Helper idle exit is allowed only after no mounted lease or pending preparation remains.

BL2 checkpoints before each agent transaction, after a quiescent agent save, on person save and before graceful close.
Revoke first durably disables admission, then attempts safe recovery checkpoint/cleanup without allowing more agent mutation.
Every acknowledged checkpoint and person handoff requires §5.4's callback barrier and, after arbitrary Python, its process-replacement boundary. During ordinary manual work, request a checkpoint after 2 s without edits and at least every 30 s while dirty,
at the next main-thread safe point. Do not interrupt a modal edit or blocking Python and claim it was saved.
Use two rotating workspace recovery checkpoints within disk reservations; promotion still needs the stable capture in §6.1.
The inspector shows missed checkpoint deadlines and the last durable time. Death can lose work since that time.

Stage the composition witnesses without requiring BL2 to exist during BL1. **B1-accounting** uses the five production
BL1 trees (three provider attempts, SVG worker, shared external/Blender action) plus the named native
`VisibleSessionLifecycleFixture` and `ImageBuildLifecycleFixture` in the two reserved slots.
Both fixtures use the production admission/guardian/service cleanup paths, create independently visible native descendants,
and hold real reservations. Label them lifecycle fixtures, never live-window or builder acceptance.
**B1-helper** replaces the builder fixture with the real activated builder/validator and retains the visible-slot fixture.
It is part of Linux BL1 exit. **B2-accounting** replaces the visible fixture with the real managed Blender window and repeats
the combined load with the accepted real builder. Keep the same seven-tree ceiling, cleanup-blocked/eighth-tree refusal,
four image leases and all death/retirement cases at every applicable stage. Report the three integrations separately.

### 5.4 Code inventory, admission and refusal

Autoexec is one control. It is not the admission policy. For the exact certified Blender build, the passive closure probe
must inventory the following routes across **all** data blocks, scenes, view layers, scene strips, linked libraries,
overrides, node groups and referenced resources, including disabled/unused content. Do not scan only the active camera scene.

| Route | Render-only treatment | Treatment with code-input grant |
| --- | --- | --- |
| Driver expressions, variables, targets and driver-namespace registrations | Refuse any driver-bearing closure in v1, including expressions Blender could optimize as simple | Admit exact expression/data/code hashes, declared namespace and supported evaluation; verify the actual motion |
| Embedded text blocks, registered scripts and external Python/module references | Treat every text block as code in v1, including dormant blocks; refuse and show names/hashes | Admit explicit hashed blocks/modules and their closed dependencies |
| Freestyle Python style modules and render-triggered line-style code | Refuse Freestyle in v1 render-only, in every referenced scene | Admit only inventoried hashed style code under the matching class of grant |
| Load/save, frame/depsgraph, render, bake and export handlers; timers; message-bus callbacks | Only the pinned driver/bridge baseline is allowed; no foreign registration | Record module/code hashes, callback identities, owner, phase and retirement rule below. A hash alone does not admit an indefinite callback lifetime |
| Add-ons, custom operators/property callbacks, Python nodes, custom render engines and exporters | Only pinned trusted runtime components; no user discovery | Explicit add-on/module/build hashes and declared hooks; unsupported routes still refuse |
| OSL script nodes or external shader source, dynamic library/module loading and subprocess hooks | Refuse | Require a separately certified code class and complete hashed inputs; initial BL1 still refuses uncertified classes |
| Command-line Python, authoring/rebuild programs, render-time timing programs and live console execution | Only the pinned driver/bootstrap; no arbitrary expression | Explicit source blobs in the admitted execution manifest, never an unrecorded console command |
| Native Blender data evaluation, including keyframes and built-in nodes | Allow only the certified data-only subset and captured caches | Same data validation; a code grant does not make unknown features supported |

The closed supported-route inventory is part of each patch certification. An unknown hook, unresolved executable reference,
unexplained registry difference or unsupported node/engine refuses `blender_executable_surface_unsupported`.
Do not claim a future Blender plug-in architecture can be exhaustively sandboxed by this table. Its admission rule is closed-world refusal.

Inspection itself is supervised execution of the pinned passive driver under **Person inspection, restricted CPU** or
**Person inspection, full access** from §5.1, before source-code admission. Both require the protected authority and the
parser-risk consent copy. On Windows the full-access inspection grant explicitly precedes loading a downloaded scene.
It loads with autoexec off, no personal add-ons, no Freestyle/render/export/frame evaluation and no imported code registration.
Snapshot enumerable handler lists and driver-namespace/add-on state before opening; install timer/msgbus wrappers and
establish their tracked baseline before any authoring code. Public timers/msgbus APIs have no complete registration
enumeration; do not report a count of tracked registrations as proof that hidden ones do not exist. Enumerate stored references without evaluating them.
If the certified load path cannot inspect a feature without invoking its code, refuse that file before load and require an explicit
code-input admission for inspection. Native parser exploits remain part of the OS-tier threat model, not a granted Python feature.
The implementation must demonstrate this passive path with real marker hooks before the profile is certified.

The probe returns a bounded `ExecutableInputManifest` of kind, scene/library origin, owning data block, source hash,
dependency hashes, phase and runtime registry identity. Pin that inventory with the normalized closure.
Render-only absence must be established, not inferred from autoexec's disabled flag. Never remove a driver and render ready.
The code-input grant names permitted classes and trust tier. Each execution binds actual hashes to that grant generation.
A confirmed authoring scope may cover later agent-authored programs: restricted standing coverage or a full-access
per-project grant, as displayed. It is permission
for arbitrary code, not an assertion that the host can prove an agent did not copy a downloaded script.
Imported embedded code requires explicit hash admission rather than implicit permission from a project's render grant.

After admission, start a **fresh** process from the pinned runtime with only the admitted closure. Apply only the approved
autoexec/hook registrations needed by that exact manifest. Check registries after load and before each evaluation/render/export.
Unknown registrations fail the job. Python introspection is an integrity check on cooperative admitted code, not containment
of hostile Python. It cannot prove no native side effect occurred once arbitrary code was granted.
No grant, incomplete inventory or unsupported motion means a visible refusal, never ready output or a misleading static rendition.

#### Callback ownership and quiescence

An executable inventory is not a lifetime rule. Assign every supported registration one owner and phase. [R26]

| Callback class | Owner, permitted phase and lifetime |
| --- | --- |
| Pinned bridge queue, save observer and checkpoint timer | Managed session/host epoch. Only the fixed bridge dispatcher runs on the main thread. It cannot perform an agent mutation after that transaction generation closes. Session end removes the baseline registrations too. |
| Scene-evaluation hooks, including drivers, Freestyle and approved frame/depsgraph/render handlers | Exact scene revision and executable manifest. Active only inside a labelled evaluation/render/export phase for that revision. Hooks can compute declared evaluated outputs, not schedule later authoring or persist unrelated changes. Background lifetime ends with process cleanup. Live viewport evaluation is an explicit bridge-owned phase; render-only hooks stay disabled there. Person control may keep approved scene evaluation but no agent authoring queue. |
| Authoring timers, authoring handlers, msgbus subscriptions and queued/modal work | One `BlenderAuthoringTransaction` and control generation. Run only during visible agent control. Bridge APIs and direct public timer/msgbus calls pass through the preinstalled interception below, with private owners and cancellation handles. `persistent=True` grants no session lifetime. |
| Untracked/bypassing asynchronous registrations, Python threads or unmanaged subprocess/modal tasks | Unsupported authoring; no checkpoint/handoff ACK. Keep agent control while cancelling/cleaning and replace the process. Direct public calls caught by interception are tracked; hostile bypasses require process death, not introspection. |

Before loading authoring modules, running an agent script or admitting any authoring callback, the bootstrap wraps
`bpy.app.timers.register`/`unregister` and preserves `is_registered` behaviour through an original-function→wrapper map.
Each registration creates a bridge dispatcher tagged with transaction ID/control generation; direct calls to Blender's
public API receive the same treatment as calls to bridge helpers. Repeating/persistent timers keep that owner.
Wrap `bpy.msgbus.subscribe_rna` and `clear_by_owner`: replace caller owners with bridge-minted private transaction owners,
retain a bounded caller-owner mapping and generation-check the notify wrapper. A transaction cannot clear session owners.
Snapshot every enumerable `bpy.app.handlers` list before authoring and diff identities after the synchronous call and at
barriers before the event loop runs again. New supported handlers get the same owner/phase and dispatch guard; unsupported
ones refuse. Keep session/evaluation registrations separate and compare wrapper identities at every cooperative barrier.
Cap combined tracked registrations/queued callbacks at 1,024 per session and 256 per transaction, with ≤256 KiB tracking
metadata inside native/control RSS. Refuse excess before registration; never evict a live cancellation handle.

B2-callbacks must demonstrate that these wrappers can actually be installed and cover direct public API calls on the
certified build. No assumption that Blender exposes a global timer/msgbus registry is allowed. If interception cannot be
shown, select the terminate/reopen route after **every** Python authoring transaction before any checkpoint or handoff.
Even successful interception is **cooperative**: hostile Python can replace wrappers, retain raw functions or bypass Python
introspection. It is not a sandbox. Before dispatching arbitrary full-access Python, the native host marks that control
generation process-tainted; Blender cannot clear the mark or attest that it ran no Python. The transaction must use the
process-replacement boundary below, regardless of apparently empty tracked tables. No heuristic "looks safe"
classification can waive that requirement. Certified fixed bridge data actions alone may use an in-process barrier.

V1 refuses general asynchronous authoring. Only the bridge's tracked bounded callbacks may extend an authoring transaction.
A deferred authoring result is `running`, not success, even after the initial Python call returns. A foreign callback that cannot
be inventoried/retired makes that transaction unsupported. Full-access introspection cannot contain deliberately hostile Python.

To finish a supported transaction, stop new queue admission, advance its generation and cancel future timers.
Unregister owned timers, remove owned handlers, call `bpy.msgbus.clear_by_owner` for its subscriptions, and cancel queued/modal
work through the declared API. Drain already-running callbacks at main-thread safe points. Require zero running/queued
transaction callbacks, no pending writer and the exact admitted session/evaluation registration baseline.
Pump the pinned event loop through an acknowledged dispatcher barrier. A late queued callback must check the retired generation
before any side effect and be discarded, even if its due time is hours later. Waiting two seconds alone proves no such thing.
Suspend evaluation hooks while taking the save barrier, flush writes and perform native stable capture. Acknowledging a checkpoint
requires this quiescence evidence plus the durable scene receipt, not just `save_post` or an unchanged callback count.
For a fixed certified data action with no arbitrary Python, only then acknowledge and transfer control or switch branch.
Ordinary person saves/checkpoints use the same barrier; any arbitrary-Python-tainted process still requires replacement.

Cancel, revoke and branch switch close the transaction generation first and run this same retirement path.
If the barrier fails, interception is uncertified, or the transaction ran arbitrary full-access Python, retain the
**last acknowledged checkpoint**, terminate and independently reap the managed process/tree, then reopen from that
checkpoint in a fresh pinned process before person handoff or branch switch. Process death is the guarantee that old
in-process timers/msgbus/handlers cannot resume; interception alone is not that guarantee for hostile code.
A stable provisional save may be retained under its own hash but stays **unacknowledged** while the old process lives.
After proven cleanup and reopening the acknowledged checkpoint, the host may pass that captured candidate through fresh
passive inventory/closure verification and load its admitted data as a successor proposal, without rerunning its arbitrary
authoring program. Acknowledge it only after clean-process validation, the new save barrier and a durable host receipt.
If that cannot succeed without arbitrary code at the barrier, keep the acknowledged checkpoint and show the proposal/lost
unacknowledged work; never silently promote it. No attempted checkpoint can precede the required cleanup proof.
Reopening starts with autoexec off and only its newly admitted evaluation hooks; unknown persistent content refuses.
This prevents old-process callbacks crossing handoff, not arbitrary filesystem effects, malicious saved data or external
services already launched under full access. Those remain in the stated full-access threat boundary.
If reopening is ungranted/unsupported, leave the session closed with the preserved checkpoint available.
The pre-action checkpoint and 2 s/30 s cadence in §5.3 remain recovery limits, not claims of arbitrary Python containment.

### 5.5 Automatic image preparation has a small enrolled client

Riel chose Linux automatic refresh. Its trust chain is captured blobs → unprivileged builder/validator → authenticated
receipt and sealed digest → privileged read-only mount → ordinary input lease. The image helper never creates, populates,
repairs or writes a filesystem. A hash of arbitrary caller-supplied filesystem bytes is insufficient admission.
Consent always comes from §5.1, including when the image helper is absent.
A Linux restricted manual checkpoint still needs the certified fixed mount component and B1-mount. It can use a
person-triggered build of captured blobs through the same builder/receipt/mount path before automatic refresh is accepted.
It cannot admit arbitrary person-built containers through the automatic service or substitute a weaker mount mechanism.
With that mount component absent, this design exposes no restricted Linux execution. Windows manual intake/live is
independent of the image component, but never independent of the consent broker. Unsupported combinations refuse.

#### Principals, activation and enrollment

An administrator separately installs root-owned `kinewright-image-helper.socket`/`.service`, the fixed client binary,
writer/validator units/tools and locked **distinct** `kinewright-image-submit`, `kinewright-image-build` and
`kinewright-image-validate` accounts. Its socket activates one single-threaded native image service, so namespace transfer
cannot race another request. The root image service is a separate protected service security domain from the
consent broker, with private mount/staging roots, no grant-store/key handles and no access through its protocol to consent
mutations. Its mount path needs `CAP_SYS_ADMIN` and `CAP_SYS_CHROOT` for namespace entry; exact process-observation rights
are pinned by certification. This is privileged kernel filesystem-parser exposure, never authority given to Blender.
The fixed roots are `/var/lib/kinewright-image-helper/{incoming,images,mounts,journal}` and `/run/kinewright-image-helper`.
The image service owns **its own builder-receipt signing key**, `/var/lib/kinewright-image-helper/receipt.key`, root 0600
under root-only ancestors. Its verification key/installation ID are pinned by the separate image-component installation.
It never uses the consent authority key. The consent broker exposes to it only bounded `CheckGrantGeneration` queries
(project/origin ID, grant generation, tier/scope and runtime binding); it never parses a preparation manifest/result,
launches a builder/client, receives image bytes or signs a builder receipt. The image service cannot issue consent.
The units isolate its namespace/files/API from the broker; compromise of a privileged service remains a security-review
risk, not a claimed guarantee against compromised root. Protected receipt types/keys cannot validate as person grants.

Studio and the JA-1 host launch normally. **Only `kinewright-image-submit` is enrolled**, not Studio, its GUI or a provider.
The image service checks live grant coverage, then asks the system manager to start the fixed
`kinewright-image-submit.service`. From process creation, credential setup, dynamic-loader startup and exec onward it runs
under the dedicated locked **unprivileged service UID**, distinct from the person, full-access Blender, writer and validator.
All real/effective/saved IDs and supplementary groups are fixed before exec. No other program may run under that UID;
no login shell, home, user-writable executable/library/config search, ambient environment or supplied plug-ins are allowed.
The system manager and image service own its cgroup and IPC. The person cannot ptrace, change its memory, duplicate its
fds or inject signals as that UID at **any startup phase**, even on a certified system permitting ordinary same-UID tracing.
Post-exec `PR_SET_DUMPABLE` is defence in depth only; it is never the startup protection. [R28]

Keep helper credentials unavailable until the image service verifies the manager-created pidfd/start identity, fixed UID,
cgroup, executable/support-file hashes, cleared environment, descriptor allowlist and authenticated READY on a
service-owned private startup channel. Only then send the one-use session credential; bind it to project/coverage grant
generation, boot nonce and monotone sequence. A failed/startup-stalled client gets no credential and is cleaned up.
Root ownership of an executable, its pathname or caller UID alone does not enroll a process. No client secret goes to Studio.
The host-facing socket is created/owned by the service, with only the enrolled person's connect ACL; internal helper/control
sockets and job channels are not user accessible. At most one active host submission, four queued ≤4 KiB requests and
≤128 KiB streaming buffers are allowed, within §9's limits. Other same-user callers can submit only bounded data within
already-granted coverage; their requests never become authenticated person decisions or raw helper commands.
The client accepts typed manifest references and digest-checked blob streams, not Python, filesystem images, arbitrary
paths/fds/namespaces or executable/unit options. It cannot add a project writer. No JA-1 host-launch change is required.

On first use, the image service allocates an opaque enrollment ID against broker-verified project/native root identity,
UID/SID, capture scope and either a standing grant plus local-origin generation or an explicit per-project grant.
It obtains coverage by a bounded query, not a broker builder/enrollment push. Locally minted projects covered by an enabled
standing grant enroll without another prompt, as do projects explicitly adopted under §5.1. Unadopted or foreign
projects require their own confirmation, and full access remains per project. New identities, adoption and Save As use
§5.1's origin rules. Source revisions inside scope do not re-confirm.
The service rechecks grant/origin/runtime generations and quotas on every submission and before attach. Revoke blocks
new submissions immediately; active jobs cancel and existing mounts drain safely. No per-project administrator action.
At most 64 enrolled projects/machine, one active Studio project, one build/validation slot, four mounted source/runtime
generations, 320 MiB/source image and the shared 8 GiB/project cap apply. Excess enrollment refuses; removal never reuses IDs.

**Activation and fd passing.** The client sends bounded `PrepareCapturedManifest` to the **image service's** separate
preparation endpoint. The image service reserves scope/quota/slot and generates a private job ID. It activates only the
administrator-owned `kinewright-image-build@<job-id>.socket` template through the system manager, with
`ListenSequentialPacket=/run/kinewright-image-helper/jobs/%i/build.sock`, `Accept=no` and the matching fixed service.
The root-only job-directory ancestor excludes user connections. The service's first connect activates the writer unit;
the writer receives the listener via `sd_listen_fds`, accepts one peer, checks `SO_PEERCRED` against the image service
and verifies its job nonce. On that AF_UNIX connection, `sendmsg`/`recvmsg` with **`SCM_RIGHTS`** passes at most three
image-service-created descriptors: read-only bounded manifest, bounded ordered blob stream, and writable reserved output
spool. Reject unexpected/truncated ancillary data, descriptor type/access/length or job mismatch. Only fixed arguments,
private service-generated IDs and this channel reach the unit; no promised inheritance across an unexplained manager launch. [R29]

The fixed native materializer/pinned `mksquashfs` run in `kinewright-image-build@<job-id>.service` as the locked writer
account, without capabilities, external network, login/user-manager IPC, home/project-store access, user environment or
writable runtime dependencies. System-manager ownership prevents cgroup migration. **Writer and validator are separate
units and separate UIDs.** After writer exit the image service stops/disarms its socket and unit, requires independent
`cgroup.events populated=0`, manager reap/exit evidence and no remaining writable mapping/handle. Main-PID exit alone
never suffices. The image service then reopens the same verified output inode read-only, closes its own writable spool
descriptors and exposes no writer-accessible path to the validator's input. Until these steps complete retain the single
builder slot as cleaning/cleanup-blocked and do not start validation.
Only after that proof activate `kinewright-image-validate@<job-id>.socket`/`.service` through the same private
socket-activation/SCM_RIGHTS mechanism, with read-only manifest and result-image descriptors and a bounded report channel.
The validator has no output-image write permission. Require its own empty/reaped cgroup before receipt issuance.
Both phases use the same one transient slot **serially**, not two concurrent trees; manager failure retains its reservation.
The image service supervises all phases, authenticates fixed units/results and signs the receipt below with its own key.
The broker is absent from that data path. No caller can start arbitrary units or change executable/options through either endpoint.
B1-helper must prove this activation/fd/closed-writer composition natively before automatic refresh is accepted. [R29, R30]

#### Fixed protocol, construction and receipt

Protocol 1 exposes only `AttachVerifiedDigest`, `ReleaseLease`, `Status`, and `Reconcile` to the enrolled client.
Messages are ≤4 KiB, with opaque project/job/slot/guardian-ticket IDs, digest/length, builder-receipt ID, generation and nonce.
No command, arbitrary path/fd, filesystem choice, executable, mount flag, device/drive target or numeric namespace PID is accepted.
Unknown fields refuse. The service derives names from fixed private roots with no-follow/open-by-handle ancestor checks.
It can detach only its journaled native devices/mounts. Runtime slots are separately person/administrator-provisioned and
certified; source refresh can never replace them. Reconfiguration requires administrator reprovisioning with no live leases.

The client accepts an already enabled grant, not proof that the ordinary caller is the person. A same-user process can
request work within existing permission, just as it can use the person's shell. It cannot enroll itself as the trusted
client, broaden the grant, mint a receipt or select a privileged operation outside this fixed contract.
The ordinary capture owner seals §6.1's manifest and reserves all overlapping copies before submission. The native builder
accepts actual hashed blob streams, never a caller's image. It materializes safe generated names, regular files only,
fixed metadata, no links/device nodes/xattrs/setuid bits, and the retained logical-path mapping.
Pin **mksquashfs 4.7.5**, exact executable/build/options hashes, as the initial candidate. Bound compression/memory/threads,
deterministic ordering and no-append options. After the writer unit is proven empty, the separate unprivileged validator fully reads the inventory and file contents,
compares paths/types/sizes/hashes with the captured manifest and refuses unsupported filesystem features. The image
service obtains bounded results through its private channels, checks both unit identities/exit/reaping and signs a
builder receipt with **its own independent receipt key**, never the consent key. The distinct verification key and record
type cannot validate as a person grant. No signing key is supplied to the submit client, packer or validator.
The receipt binds image SHA-256/length, captured manifest, trusted tool/profile hashes, job/slot/grant generation and nonce.
The caller cannot fabricate or replay it for another image. No writer or writable mapping survives receipt issuance.

The image service copies the exact opened result to its reserved private slot, rechecks receipt and full digest/length,
fsyncs, enables fs-verity, and retains its inode/backing lease. Only then it mounts read-only SquashFS in its private namespace
with `nodev,nosuid,noexec` for source images. Runtime images retain only their separately certified executable permission.
No Blender or caller-supplied command runs inside the privileged service.

#### One mount handoff, before any source execution

Choose **privileged fd-based mount transfer**, performed entirely by the image service. This replaces both inconsistent
rev2 descriptions. Do not add an unprivileged directory-fd rebind or a host `setns` alternative.

1. The production guardian creates bubblewrap's blocked child inside its reserved job leaf. A pinned native gate is the
   first command, not Blender. The gate has already pivoted to the final sandbox root, creates no namespaces thereafter,
   and sends its root and fixed `/scene`/`/runtime` destination identities on the inherited startup channel.
2. The enrolled submission client obtains a guardian startup ticket containing the independently checked child pidfd,
   cgroup membership, start identity, mount/user namespace IDs and root identity. The image service verifies these against
   the registered guardian job and broker dispatch ticket, querying only its live grant generation from the broker. Independently open the actual child's root/namespace handles,
   check the fixed gate executable and final private-root layout, and require its non-dumpable/no-user-module bootstrap.
   Destinations must be empty fixed mount points in that root, never aliases into home, project or host mount trees.
   An arbitrary caller-supplied namespace fd/PID never suffices.
   Open the actual `/proc/<verified-child>/ns/mnt` handle while the child is blocked and alive. Pin and compare nsfs identities.
3. In the service's initial user namespace, call `open_tree(source_root_fd, "", AT_EMPTY_PATH | OPEN_TREE_CLONE | OPEN_TREE_CLOEXEC)`
   for the already verified source and runtime roots. Apply the fixed recursive read-only/nodev/nosuid attributes with
   `mount_setattr`; source also has noexec. Save the service mount-namespace fd, then `setns(child_mnt_fd, CLONE_NEWNS)`.
   The service stays privileged in the initial user namespace, with the capabilities needed for both owning namespaces.
4. Call `move_mount(detached_fd, "", pinned_child_root_fd, "scene", MOVE_MOUNT_F_EMPTY_PATH)` and the corresponding fixed
   `runtime` target. Only these fixed destination names exist in the private blocked root. Reopen them and verify actual
   `statx` mount IDs, root/backing identities, flags and manifest. Return to the saved service namespace before another request.
5. The child verifies the lease/root identities, applies final confinement, closes all service/namespace/mount-control fds,
   and reports CONTAINED. The host and broker recheck grant/intent. Only guardian COMMIT releases the fixed exec gate.
   Ordinary processes receive an input lease and verification data, never mount capability or a reusable helper credential.

The guardian remains the parent/reaper of its job. The helper neither launches Blender nor receives a lifetime-pipe writer.
A transferred clone belongs to the same source/runtime generation lease, not an uncounted image generation. Charge every
backing/clone/root reference until all consumers and namespaces retire. Roll back partial transfers only after the blocked
child is dead; never release a slot because an ACK or fd was lost.

**B1-mount is a BL1a entry gate.** On the target Linux kernel, real system-installed bubblewrap and production guardian,
prove this exact capability/namespace/clone transfer, final path visibility and read-only restrictions with a native fixture,
then a minimal real Blender load. Exercise source/runtime replacement, namespace/pid swap, changed roots, helper/guardian death,
partial mount and cleanup. Pin the exact tools/kernel/options/syscalls. Failure keeps restricted unavailable and BL1 incomplete.
Documentation and a passed unprivileged user-namespace demo cannot substitute for this particular cross-namespace handoff. [R25]

#### Windows scope, parser risk and recovery

This design has **no certified unprivileged NTFS/VHDX builder**. Therefore the Windows image-helper service, volume token,
attach pipe and activation units are **not shipped or installed**. Windows manual R24 intake and Windows live still require
§5.1's separately installed consent authority. No dormant privileged image service exists for zero usable operations.
A future Windows helper would need a dedicated service SID/restricted token enabling `SeManageVolumePrivilege` only around
fixed `AttachVirtualDisk`, protected ProgramData roots, authenticated fixed submission client, unprivileged build receipt,
read-only/no-drive-letter attach, retained non-write/non-delete leases and volume-GUID identity. Administrator install/removal
would be separate. That is a future certification obligation, not an available lane or a new choice for Riel. [R18]

A digest proves identity, not kernel parser safety. A trusted packer plus separate unprivileged full-content validator bounds
metadata/features while agent influence remains regular-file payloads. Use one maintained kernel/filesystem profile,
bounded mount time/memory, private propagation and read-only mounts. This reduces but does not eliminate kernel-parser risk.
Independent security review and native controls must accept that exact boundary. Until that boundary is accepted, restricted Linux execution is unavailable. A manual checkpoint uses only the same
installed certified path and cannot bypass this review or mount certification.
Never accept downloaded filesystem images or a full-access bypass for privileged mounting.

Persist the fixed lease record before mount and actual native device/mount/consumer identities before ACK. After service
failure the system manager restarts into reconciliation, not attach admission. Kill/reap failed builds before deleting partial
images. At boot reconcile four bounded image records before new work. Namespace clones must also be gone before lease retirement.
After host/guardian death, inspect actual consumer emptiness independently. Busy/unverifiable detach retains the record,
slot, pins and bytes as cleanup-blocked. No force-detach under a live renderer, no publication during unresolved required cleanup.
Removal stops admission, cancels/reaps consumers, detaches only owned images and removes units/client credentials/staging.
Keep captured blobs/renditions and the independently installed consent authority. Blocked detach reports its exact pending lease.
The service cannot erase source folders or run an arbitrary cleanup command. Same-user full-access Python cannot reconfigure it.

Setup copy: "Restricted Linux rendering needs an administrator to install the certified image component once, including
its mount service and isolated submission, build and validation accounts. It mounts verified read-only scene images;
Blender does not get administrator power. Mounting still uses your system's filesystem parser. Without this certified
component, restricted rendering is unavailable. The manual checkpoint uses this same installed path when you choose
Prepare scene image; automatic refresh later triggers it after accepted saves. Removing it stops its jobs and unmounts
its images while keeping projects. Windows always needs manual preparation under this design. The separate consent
service is required for any external execution. Creating another covered own project needs no administrator or password prompt."
B1-helper supplies authorized/unauthorized/interrupted witnesses against these real units, client, builder and mount path.

## 6. Data model, provenance, and cache identity

### 6.1 Immutable scene closure and editable workspace

Propose `BlenderSceneRevision` containing `.blend` ref, authoring-script refs, typed-descriptor/parameter refs, dependency manifest,
authored clock, scene/camera name, colour contract, required add-ons, and bounded declared/verified provenance.
Scripts create/edit a scene; the **saved `.blend` closure is the render authority** after capture. Rendering does not rerun the authoring script implicitly.
An explicit rebuild from script creates a successor scene revision. Render-time scripts, when allowed, are separate hashed execution inputs.

Import copies bounded bytes into JA-1 owned storage before any operation references them. The external locator is a display/provenance hint only.
The host publishes the complete graph through JA-1 B0–B5, using `ObjectRef`, owned input records, `ProjectStore`, `StorePin`, and `StorageUsage`.
Extend AW2's asset/spec records and the single bundle reachability walker. Do not add an independent `blender-assets` GC or root writer.
The exact physical subdirectory follows the merged AW2/JA-1 store mapping, not a second spelling frozen by this draft.

Inspect dependencies only under execution consent/supervision. Opening a downloaded `.blend` for inspection can exercise native parsers.
The driver enumerates images, UDIM tiles, sequences, fonts, linked libraries, caches, sounds, compositor/VSE inputs, and script modules.
Recursively capture supported linked libraries with cycle detection. Pack eligible data or rewrite a **copy** to manifest-relative paths.
Packing is a convenience, not proof of completeness; Blender cannot pack every external dependency. [R16]
Missing, escaping, unresolved, cyclic, undeclared dynamic, or unsupported dependencies refuse closure admission.
Simulations must be baked into supported owned inputs for BL1; uncaptured bake caches refuse. Blender libraries outside allowed roots require a new grant.

Never open an absolute path embedded in a `.blend` as authority. Resolve dependencies at intake against the granted roots using no-follow native identity checks.
Preserve the original scene blob, plus the normalized rendering copy and a mapping report. Both are content-hashed.
Probe the normalized copy in an environment containing only its captured closure, and require no missing-resource diagnostics.
Arbitrary Python can invent dependencies; reproducible supported scripts must declare them. Full-access execution cannot prove absence of hidden inputs.
Mark such output `unverified-inputs` when known; keep its exact rendered bytes but do not certify reproducibility or automatic cache regeneration.

Native snapshot building consumes the actual copied bytes, not a directory listing. R24 sealed source/runtime images pin file and namespace identity through use.
Working edits require fresh source images. Never render directly from the mutable live workspace or original install paths.
Capture a managed save only after §5.4's callback quiescence barrier, with no pending Blender writes. Validate each captured byte hash and full dependency map.
This names the captured snapshot; it is not a claim of an atomic snapshot of an independently changing external tree.
Refuse an unstable capture, retain the old revision, and retry only read/capture after a later save event.
For every watched `.blend`, texture, library and other dependency, require **two identical full-length SHA-256 hashes**
separated by at least **2 s of quiescence**, with unchanged size, native file identity and directory membership.
The first pass streams an owned candidate copy while hashing. The second rereads the entire file, not metadata or a prefix.
Require matching full-manifest vectors across both passes. Restart the interval after a write/rename notification or observed change.
Bound an attempt at 15 s; if files keep changing, show `Source still being saved`, keep the prior revision/rendition and await another event.
Managed saves also pass this native check after the main-thread barrier. Neither `save_post`, mtime, notification count nor Blender's
usual save implementation is proof of stable bytes. This defines a captured revision, not a universal atomic snapshot of unrelated external edits.
Passive executable inventory from §5.4 is part of the closure; an unsafe/unstable referenced scene blocks the whole capture.

For downloaded models, textures, HDRIs, and fonts, retain original URL/provider/asset ID, fetch time, byte hashes,
licence identifier/text, author, attribution requirements, and evidence of any person-supplied permission.
Licence claims are declared until supported by retained evidence. Unknown or incompatible rights block publish-ready delivery,
not local inspection. Include required notices in the publish kit. Do not infer CC0 from a provider name or from Blender's GPL.
Facts visible in 3D labels or device screens still require JA-1 claim binding before review renders.

### 6.2 Requests and immutable renditions

`BlenderRenderSpec` extends `ExternalSpec`; it is not a new clip execution system.
Its request key is a domain-separated SHA-256 of length-prefixed canonical fields:

- adapter/driver protocol and build; normalized `.blend`, render scripts, complete dependency/descriptor/parameter hashes;
- scene/camera/view-layer selection; full timing context, frame range, dimensions, pixel aspect, and rational rate;
- engine; seeds and animated-seed rule; samples, adaptive thresholds, bounce/clamp settings, shutter/motion blur,
  denoiser implementation/version/device, compositor settings, simulation/bake inputs, and every effective render flag;
- Blender exact version/build hash and executable/runtime-image hashes, including its Python/OCIO support closure;
- enabled add-on code/version hashes and exporter version/settings when exporting glTF;
- OS/architecture, CPU feature/thread profile or selected GPU/backend/device/driver identity, and quality profile;
- colour primaries/transfer/view/look/exposure/white-balance/OCIO dependency hashes, calibration, format, depth, alpha association,
  and effective PNG dither setting, fixed off as `scene.render.dither_intensity = 0.0` for the certified PNG path (§6.5).

Do not hash path spellings, mtimes, attribution, receipt IDs, or cosmetic labels as pixel determinants.
Source provenance still retains those details separately. Marker labels used by a script are inputs and do belong in its timing digest.
An unknown active render setting refuses certification rather than disappearing from the key.

Cycles with fixed seeds/samples is a repeatability candidate, not a cross-CPU/GPU byte-identity promise.
EEVEE is a distinct engine/profile; GPU drivers, temporal settings, shader compilation, and denoisers can change output.
Disable adaptive sampling and denoising for the initial CPU identity fixture. Other profiles have separate certification. [R5, R6]
Store each output file hash and raw-pixel hash in a content-hashed rendition manifest. A write-once render binding selects that exact manifest.
Same-profile byte equality is tested on selected fixtures. Cross-profile comparison may be visually equivalent under predeclared tolerances,
but never substitutes for byte identity, cache reuse, or saved-version evidence. Stored verified frames carry exact results across machines.
Binding uses an atomic compare-and-publish at the expected intent/generation. **First verified binding wins**, including simultaneous completions.
A second completion with different bytes is stored **unbound**, linked to its job report, and surfaced as `Alternative rendition available`.
It cannot replace ready pixels, saved-version evidence or an Undo target. An identical completion deduplicates by manifest identity.
Keep at most eight unbound candidates project-wide within the existing store/record quotas. A full candidate allowance refuses retention
with a visible result; it never overwrites the selected rendition or evicts pinned work.

The proposed **`AcceptExternalRendition`** operation accepts `{clip, expected_spec, expected_binding, candidate_manifest}`.
Only a person action in the source inspector, labelled **Use this rendition**, may authorize it. Server admission derives
the person principal from the GUI broker, never a caller-supplied actor field. Agent plans, specialists, investigators and generic MCP calls refuse it.
The operation verifies the full candidate closure/profile and creates a successor spec with a fresh write-once binding identity
already selecting that candidate. Its pixel-request digest may equal the old one; the selected-result identity differs.
Accepting is one attributed undoable edit. Undo restores the old binding and exact bytes. Damaged/missing old output never
causes silent regeneration acceptance. Show the diagnosis and offer a separately verified candidate for person acceptance.

### 6.3 Undo, branches, receipts, and saves

One agent commit changes the scene ref, typed parameters, and desired External spec together as one ordinary attributed Core batch.
One Undo restores the old scene/spec/render binding and finite timing tuple. Render completion adds no second Undo entry.
A completion receipt advances only bundle-derived metadata/root generation, never the branch's Core revision, history cursor, or `HeadId`.
`AcceptExternalRendition` is the explicit person-authorized exception to completion-only metadata: it advances ordinary
Core history because the person chose different output. The renderer completion itself cannot perform that operation.
A render-only request does not alter history. A rejected/cancelled source proposal does not publish an operation.
If source replacement was already accepted and rendering fails, the new source remains pending/failed until retry or Undo.
Late output cannot satisfy an undone, superseded, switched, or differently leased intent.

For manual Blender saves, the session was explicitly opened for one linked clip and branch. Each new stable save proposes one source replacement.
On the manual branch, route it through the fenced GUI lease holder and JA-1's ordered local-Core/history bridge.
On a specialist branch, use that task's expected head and scope. An agent never gains manual-branch authority from the Blender socket.
Compare expected old scene revision as well as branch/head. A concurrent timeline change conflicts; retain the captured save as unattached work.
Retrying the same save ID/digest returns its receipt before stale-lease admission, including after host restart. It never creates another edit.

Kinewright Undo leaves the Blender working copy untouched and marks it `Different from linked revision`.
Reapplying that save requires an explicit action against the new expected base; periodic watching cannot undo the person's Undo.
Opening the restored source creates a fresh working copy or replaces one only after preserving its current work.
Blender's own Undo affects only its workspace; a later saved result becomes a new scene proposal.

Reachability includes live/saved Documents, retained undo/redo, branches, versions, recovery, unresolved commands, live sessions, and in-flight render/export.
Save As pins the complete closure, acquires destination authority before mutation, copies/verifies objects, then publishes the new root.
No live endpoints, tokens, per-project grants, or tasks transfer. Standing coverage of a Save As successor from a locally
minted or explicitly adopted own project is re-derived only through §5.1's protected local-origin protocol; a foreign/copy
destination cannot inherit it. Session-owned unsaved storage retains GUI ownership/charges across borrowed host access.
Failed source re-establishment fails closed; a Blender child or host disconnect never cleans up another owner's retained bytes.
Retain JA-1 durability-unknown/rescue rules and receipt precedence. A render report cannot settle a project save ticket.

Join the accepted v3 minimum-required predicate. Empty BL fields omit; old v1/v2/v3 projects re-save unchanged when they use no new content.
Old v3 readers must refuse unknown adapter/API/required record/clip kinds, including disabled and parked content, before writing.
Do not rely on the numeric v3 envelope alone. Gate actual prior-reader refusal and graph-copy preservation.
BL3 adds required typed scene records/operations to that same union. Core stores references and bounded controls, never `.blend` or GPU state.

### 6.4 Two-way time and edit semantics

All Kinewright intervals are half-open. Blender's requested inclusive range is translated explicitly; output index 0 normally corresponds to Blender frame 1.
Record authored Blender frame origin `B0` and reduced rate `Fs`, output rate `Fo`, and exact rational source time `t(j)`.
For ordinary playback `t(j)=j/Fo`; evaluate Blender at `x=B0+Fs*t(j)`.
Retain the authored `scene.render.fps`/`fps_base`, whose ratio is `Fs`. Do not overwrite them with `Fo` for an imported scene.
Compute `k=floor(x)` with checked rational arithmetic and call `scene.frame_set(k, subframe=float(x-k))`.
Only this last API boundary converts the fractional part to a float. Pin frame bounds and motion-blur shutter units in the profile.
Existing keyed scenes preserve their authored clock. New procedural scenes receive the output clock and timing attachment at authoring time.
Never silently rewrite arbitrary F-curves, drivers, or simulation timing when the project rate changes.
When `Fs≠Fo`, certify only keyframed motion and specifically supported **interpolable baked content**.
The bake manifest records its authored rate, sample domain, interpolation rule and complete sampled shutter interval.
Unbaked simulations, integer-only/non-interpolable caches, sequential state and unknown interpolation refuse `blender_subframe_unsupported`.
Neither changing scene fps, rounding every sample to an integer nor repeating cached frames is a substitute.
The 24→30 fixture in B1-time checks actual baked landmarks at fractional source frames, including a nonzero shutter sample.

AW2's existing selection is unchanged: at timeline frame `p`, select `j=O+a+(p-T)` while `T<=p<T+(b-a)`.
Head/tail trim, split, roll, slide, move, and ripple preserve that finite source identity exactly under R20.
Trim-in-then-out restores the original pixels/keys. Extending beyond rendered coverage refuses until an explicit expanded spec is accepted.
Moving a clip does not restart Blender animation. 2D effect/enable keys follow MO1's local keep-outside policy.

Choose one timing policy in the source spec: `asset_local` by default, or explicit `timeline_locked` for beat-synced work.
Marks carry stable IDs, kind, exact project seconds, exact clip-relative seconds, and rational source positions, with range/length caps.
Allow at most 512 marks/beat events, labels at most 128 UTF-8 bytes, and the whole timing attachment at most 128 KiB.
Rational components use checked i64 values with positive denominators; intermediate products use checked i128 arithmetic and overflow refuses.
Send clip in/out, duration, beats, frame rate, and current colour context. Raw audio is a separately declared asset, not implicit microphone/file access.
For timeline-locked content, changes to placement, marks, trim boundary, or rate create a new timing digest and stale intent,
but a changed digest alone cannot make the replacement ready. `BlenderTimingBinding` must name one of these mechanisms:

| Mechanism | Render authority and conflict rule |
| --- | --- |
| Declared data bindings | Stable scene/object/action IDs and whitelisted numeric RNA targets map mark IDs to key time/value or supported node inputs. The pinned driver applies a bounded binding program to an ephemeral copy before evaluation. Record baseline values/hashes, checked units and the binding digest. A changed target/manual edit conflicts; do not overwrite it silently. |
| Admitted render-time program | Explicit hashed program and declared inputs consume the new immutable timing attachment under §5.4's code grant. Its code/context hashes enter the request key. It is separate from the original authoring script. |
| Explicit rebuild | Re-run a named admitted authoring program only after accepting a proposal against the current scene/head. The result is a successor `BlenderSceneRevision`, with the timing digest and preservation/conflict report. Keep the manually edited predecessor and its rendition. No implicit rebuild during playback. |

V1 declared bindings allow at most 512 targets, no arbitrary Python expressions/path evaluation, and no index-based identity
that silently points at a different object after manual edits. A binding may affect only its declared target set.
Imported keyed scenes with no such contract remain `asset_local`; requesting `timeline_locked` refuses `blender_timing_binding_missing`.
The renderer records the applied binding/program/rebuild identity and effective scene digest. Behavioural fixture checks still
matter: a program may be buggy even when every hash matches. B1-time must move the visible event from 2 s to 3 s at 30 fps,
check pixels around frames 60 and 90, then Undo and recover the original scene, binding and frame-60 event.
For asset-local content, ordinary trim/move only selects existing frames; explicitly retiming the authored content changes the spec.

With accepted MO4, Nearest/Blend remap of stored frames changes playback mapping without invoking Blender. Preserve its finite bounds,
keep-outside keys, reverse/hold semantics, and exact split adjacency. Blended frames are not a new Blender simulation.
An explicit `re-render for timing` samples Blender at the new rational times and rebuilds motion/shutter-dependent imagery.
Unbaked stateful effects that cannot support those samples refuse; do not turn a bad simulation into a successful duplicate-last-frame sequence.
Before MO4, refuse stored-frame speed/remap setters and offer that explicit re-render path.

A project-rate change supplies compatible replacement specs/mappings for every affected External clip in one revision-gated batch.
Map each shared timeline endpoint once with `Q(x)=sign(x)*floor(abs(x)+1/2)` at the new/old rate ratio, preserving shared cuts.
For each replaced Blender clip use `O=a=0`, `b=N=Q((T+D)*ratio)-Q(T*ratio)`, and render from its old selected source time at the new rate.
Retain the original scene/clock and old rendition; that replacement has a new finite window, and extending it requires explicit re-render.
Reject a collapsed interval, arithmetic overflow, or colliding time-converted keys rather than dropping/coalescing them.
This preserves marker seconds exactly and timeline endpoints within half a new frame; report the resulting duration error, which can approach one frame.
The rate/spec plan has one atomic Core commit. Before commit, cancellation discards the plan and leaves the old rate/document/history intact.
After commit, rendering is asynchronous. Playback may show labelled stale images; proofs/final export refuse until replacements finish.
Cancellation **after commit** stops the render only. It preserves the accepted new rate/specs as pending/cancelled until retry or ordinary Undo.
Do not roll back the document in a job-cancellation callback. An intervening edit remains in history and must be undone before the rate batch.
Staging partial output never commits or reverses a plan. B1-cancel checks both sides of this boundary with exact history identities.
Different Remotion/Blender clips are validated together; Blender cannot bypass another adapter's rate constraint.

### 6.5 Colour and alpha contracts

| Profile | Blender output and Kinewright interpretation |
| --- | --- |
| Display SDR, default | RGBA8 PNG; explicitly sRGB/D65 output with selected AgX, Standard, or legacy Filmic view baked once. Pin view/look/exposure/OCIO hashes. Feed AW2's straight display-sRGB conversion into CC1. |
| Display SDR on HDR edit | Same PNG source, inserted through CC8's named `AppearancePreserving` default or explicitly selected `SceneMatching`. White maps to the project reference white; Kinewright applies no second AgX/Filmic view. |
| Scene linear | RGBA half-float EXR Combined beauty, explicitly exported as Linear Rec.709/D65 using the pinned Blender OCIO configuration. No display view baked. Kinewright applies the new scene-linear source adapter, not sRGB decode or an SDR insertion intent. |

Standard still encodes to the selected display; it does not mean the file is linear. Filmic is deprecated upstream.
AgX is a view transform, not the scene working primaries. New Blender versions can use different scene working spaces. [R4]
Do not relabel ACEScg, Linear Rec.2020, or a custom OCIO scene as Rec.709. Use an explicit certified Blender colour-space export conversion,
record its configuration/dependencies, or refuse. An unknown custom config cannot be inferred from a familiar view name.

For scene-linear input define `reference_linear_millionths`, positive and default 1,000,000. It declares the source value representing diffuse white.
For decoded linear RGB `r` and reference `v`, SDR working RGB is `r/v`; HDR working RGB is `M709_to_2020*(r/v)`.
In CC8 terms this declares normalized scene `s=s_white*(r/v)`, then uses its existing `working=s/s_white` contract.
This is a new scene source interpretation, not a shortcut for CC8 R36's display-SDR insertion equations.
Preserve finite negative/over-white values through grading/compositing; refuse nonfinite values and f16 overflow.
Use CC8's one output rendering/tone/gamut/HLG chain. No input OOTF or second Blender view transform is applied.
Scene EXR brightness is therefore intentional and may differ from Blender's AgX viewport; a Kinewright final proof is authoritative.

CC1's current integer-only managed decoder does not satisfy this contract. BL1's float substage adds a bounded native EXR reader,
candidate `exr` 1.74.2 with parallel decoding disabled, and an explicit working-linear entry point. It never passes through RGBA64 integer quantization.
Propose a closed `GeneratedPixelDomain::SceneLinear709` plus `WorkingFrame::from_scene_linear` at the generated-source seam.
This is a narrow CC1/CC8 amendment for verified generated inputs; it does not silently widen generic `classify_source_with_assumption` float-media acceptance.
Accept one bounded scanline beauty image with R/G/B/A, equal data/display windows and lossless ZIP; reject deep, tiled,
arbitrary multi-part/multi-layer output and unknown channels in the playback profile. Preserve optional original multi-layer files as owned source artifacts.
HDR exposure/calibration, output-intent changes, and their Undo are in working-cache identity; source PNG bytes need not re-render for a Kinewright-only grade.

Set transparent film, RGBA channels and the encoder's alpha conversion explicitly. The driver sets and verifies
`Image.alpha_mode` for admitted source images and intermediate save images, rather than trusting defaults or a filename extension.
The certified PNG path has **dither off**. The driver sets and verifies `scene.render.dither_intensity = 0.0` before each
PNG save and disables dither in any intermediate float-to-byte conversion. It never inherits a scene's nonzero dither.
The effective zero setting enters §6.2's request key and the output profile. The certified save path produces straight PNG
file alpha or associated scene-linear EXR alpha as selected. Verify the actual decoded files.

Both interchange profiles are limited to **coverage-representable alpha**. For associated `(c,a)`, require finite RGB,
`0<=a<=1`, and, if `a=0`, **every RGB component must already equal zero**. Nonzero RGB at zero alpha represents an additive
contribution that MO2's straight-source coverage contract cannot preserve. Refuse `blender_alpha_not_coverage_representable`,
identify clip/frame/pixel and explain: "This glow emits light without covering the background. Render it over a chosen background."
Never zero that contribution, clamp it into coverage or report the scene ready. A person's chosen opaque-background bake
creates a successor source/spec and records the background. A general emission-plus-coverage compositor is outside this programme.

For `a>0`, divide `c/a` before straight-source compositing and validate finite division and the working f16 range.
Only a proven `(0,0,0,0)` may return zero straight RGB. Recheck after quantization so an underflowed alpha cannot discard retained RGB.
Alpha is never gamma transformed. Apply appearance/view transforms to straight RGB, never to associated dark edges.
The pre-PNG check uses a concrete mandatory float-buffer route, not a driver success flag.
For each PNG frame, before any view/alpha conversion, the pinned driver writes the **Combined associated render result**
as a single scanline RGBA **float32 lossless ZIP EXR**, with identical data/display windows and a recorded scene-linear
colour domain. It uses the certified `Render Result`/`Image.save_render` EXR path with display-view baking disabled.
Call this `pre_png_float` evidence. The native verifier streams its full channels and checks the coverage rule before admitting
that frame's straight PNG. It also rejects malformed/nonfinite data, wrong windows/channels and evidence/profile mismatch.
It compares the PNG's dimensions with the evidence dimensions and **every decoded alpha byte exactly** against the
corresponding float evidence sample projected by the profile's pinned quantizer, `A8=floor(255*a+0.5)` after checking
finite `0<=a<=1`. With dither explicitly off, no view transform applies to alpha; a certified save path must implement
this q8 rounding or refuse. B1-colour must prove it on a semi-transparent gradient through the real build/save path.
A one-byte mismatch or swapped distinguishable alpha mask refuses; no tolerance or dimension-only comparison is allowed.
The alpha-underflow coverage check still applies when a positive float alpha quantizes to zero. This binds the checked
coverage to this PNG's alpha; it does not prove RGB derivation for two frames with identical alpha, nor honest output from
hostile full-access code. Tests use different spatial alpha masks and inspect known RGB landmarks independently.
Hash the actual float file and retain it as owned evidence in the rendition closure. The rendition manifest binds each
PNG index/hash to `pre_png_float_sha256`, length, dimensions, alpha-quantizer/comparison version, dither-off setting,
colour/association profile, verifier build and validation result. Reverification after restart checks that same byte-exact
alpha correspondence.
Missing or changed evidence prevents ready binding, including on restart. Zeroing RGB before this write is forbidden and
B1-colour checks the float pixels against a real emission fixture, not an asserted validation flag.
This diagnostic EXR subset is mandatory in BL1a even when public half-float EXR playback is not yet accepted.
It requires a bounded native scanline verifier, not `WorkingFrame` float playback or CC8. Limit evidence to 192 MiB/frame,
stream with ≤8 MiB scratch, and charge retained evidence plus PNG inside the same 2 GiB/render and 8 GiB/store caps.
Transparent flame/glow scenes require the public EXR profile or refusal; PNG's straight 8-bit output may clip colour after division by small alpha.
EXR preserves range but does not bypass the coverage restriction: zero-alpha emission still refuses. If the float substage is unavailable,
offer refusal or the explicit opaque bake, never a silent PNG fallback. These checks do not impose a 0..1 clamp on valid linear RGB.
Verify partially transparent saturated edges over black and white through Normal, a non-Normal blend, and export.
Kinewright's final opaque accumulator/export is separate from the transparent source clip. No transparent-project-output feature is implied.

PNG is the mandatory interchange because AW2 already owns its reader. ProRes 4444 and FFV1 can carry alpha through FFmpeg,
but their codec/pixel-format capability does not prove Kinewright managed colour/alpha acceptance. Current FFV1 fixtures establish narrower paths.
An optional transcode must certify exact tags, depth, alpha association and decoded pixels; no opaque H.264 fallback. [R7]
BL1 acceptance includes SDR PNG. Float EXR acceptance is separately gated; HDR use requires accepted CC8 integration rather than a false SDR label.

## 7. Agent verbs

Keep `COMPACT_TOOL_NAMES` and all served input schemas byte-identical to the accepted AW1/AW2 predecessor.
Reach the following actions through registry capabilities and `invoke_capability`; use ordinary prepared operations for timeline mutations.
CLI/headless calls and GUI actions share the host admission path. No permission-grant capability exists.

| Minimal action | Inputs and bounded result |
| --- | --- |
| `blender_scene`, create/edit | Hashed script or `.blend`, declared inputs/params, expected scene/head, timing/colour context. Returns job ID, then immutable scene proposal/operations. No raw Python in a tool schema/result. |
| `blender_render_still` | Scene/spec, rational time, draft/final quality. Returns job ID, then image/evidence file refs and exact profile. |
| `blender_render_clip` | Existing External spec/render-binding ID. Returns deduped/queued job ID, never inline frames. |
| `blender_status` | Project/job/session ID. Returns availability, effective enforcement, progress, pending source, hashes, bounded errors, and next valid actions. |
| `blender_cancel` | Job/session action ID. Returns requested/cleaning/cleanup-blocked/terminal state. It cannot report cancelled before cleanup. |
| `blender_live`, open/inspect/snapshot/close | Expected source/head and control generation. Returns session/window state, paged scene/object summaries, or viewport file refs. Agent edits reuse `blender_scene`. |
| `blender_import_gltf` | Owned GLB/glTF closure or supervised export proposal, expected head, camera/animation selection. Returns compatibility report and exact add/replace operations; BL3 only. |

`AcceptExternalRendition` has the person verb **Use this rendition** in the source inspector. It is deliberately absent
from these seven agent capabilities. The admission context of any prepared plan must reject it for an agent actor.
The BL3 generated mutators are `AddScene3dClip`, `SetScene3dSource`, `SetScene3dView`, `SetScene3dEnvironment`
and `SetScene3dVideoBinding`. They expose references and bounded controls, not renderer code or another source player.

### Registry growth ledger against AW2 R30

The following are proposed byte ceilings, not measured schema sizes. The slice owner captures actual serialized definitions,
description bytes, operation-definition bytes and token counts before/after each landing and records the accepted predecessor hash.

| Registry group | Added entries | Definition growth ceiling | Stage |
| --- | ---: | ---: | --- |
| BL1 `blender_scene`, still, clip, status and cancel | 5 capabilities | 16 KiB aggregate | BL1 |
| BL2 `blender_live` | 1 capability | 6 KiB | BL2 |
| BL3 `blender_import_gltf` | 1 capability | 6 KiB | BL3 |
| Person-only `AcceptExternalRendition` operation definition | 1 operation; zero agent capability | 2 KiB shared definition | BL1 |
| BL3 operations named above | 5 generated mutators | 10 KiB aggregate | BL3 |
| Shared Operation/schema reference propagation | No independent entry | 8 KiB aggregate across affected registry definitions | Each stage |
| **Full programme registry delta** | **7 service capabilities + 6 operations/mutators** | **≤48 KiB** | Separate from served definitions |

AW2 R30 retains its six generated mutators and three service capabilities. BL adds six service capabilities before BL3
and seven in total, making nine and ten respectively. Ledger each newly propagated operation reference once per real serialized
definition. If the chosen schema layout exceeds a ceiling, simplify it or revise the measured budget explicitly before acceptance.
Every capability description remains **<1,024 UTF-8 bytes**, with its admission rule in sentence one.
The served quad stays 7 tools / 5,660 definition bytes / 3,510 input-schema bytes / 998 description bytes, with byte identity,
not merely length checks, on HTTP and stdio. No growth allowance applies to `prepare_edit_plan`'s frozen input schema.

Availability is metadata-only until a person grants execution. A version probe is code execution and uses the supervisor/grant.
Progress is at most four updates/second, status ≤4 KiB, paged object inspection ≤16 KiB/128 objects, text errors bounded/escaped.
Each proof request returns at most eight still refs, individually ≤2 MiB, under owned evidence quotas. No inline/base64 images or whole-frame arrays.
Store scripts as ≤1 MiB blobs; manifests refer to their hashes. Page larger source views outside the compact tool result.
Cancellation, retries, and source publication use stable IDs and JA-1 digest-bound receipts; Python code is never automatically rerun after an uncertain edit.

The agent checks a cheap still first, inspects a final-quality still in context, then renders the clip and requests a normal Kinewright proof.
Extend AW3 `explain_frame` when present to report the actual selected scene revision, spec/rendition/frame, mapping, camera, colour/alpha, and composite contributors.
AW3 is not a release dependency: BL1 supplies the same bounded source/render attribution in `blender_status` and proof sidecars.
Neither inspector invents object/pixel attribution absent renderer evidence. BL3 can expose mesh/material IDs from an explicit diagnostic pass when requested.

## 8. Person workflow

Use Studio's existing clip/source inspector and conversation progress card, with [DESIGN](../../docs/DESIGN.md) tokens and immediate input.
No new task dashboard or embedded Blender viewport is needed. Hashing/decoding/capture never runs on the UI thread.

The source inspector shows `Blender not configured`, `Permission needed`, `Unsupported version`, `Preparing`, `Rendering 12/90`,
`Checking output`, `Ready`, `Source changed`, `Unsaved in Blender`, `Failed`, or `Cleanup blocked`, as applicable.
It also shows `Restricted unavailable on this system`, `Blender update awaiting certification`, `Source still being saved`,
`Code permission needed`, `Unsupported alpha`, `Alternative rendition available` and `Manual image preparation needed` when applicable.
Use status colours only for those outcomes. Preserve the person's path spelling in labels; private canonical/native identities govern IO.

Select an existing install or choose its executable. Explain the detected version and certified profile; link manual installation guidance if missing.
Never download/install automatically. Missing Blender still permits old rendered clips to play/export and BL3 scenes to scrub.
Use only the **5.2 LTS line** for new Blender execution. Each patch/build is certified with driver, extension, OS, launcher,
runtime image, OCIO and device tuple. A changed executable/runtime hash pauses new probes of project scenes, edits and renders
until the new tuple is recertified. A safe supervised version probe after permission may identify the changed install.
Preserved old renditions continue to play/proof/export. Do not silently migrate a scene, reuse a cache key or overwrite old output.
Engineering owns timely patch recertification, prioritizes security fixes and publishes its status. A verified certification
record for a 5.2.x patch arrives over the broker's control socket or as a shipped file and re-binds the runtime under the
existing line/profile grant, with no password prompt or administrator action per patch. The broker verifies the pinned
engineering key and its protected generation/revocation state, refusing older records (§5.1). Uncertified runtimes refuse;
asking Riel to approve a patch cannot replace certification. A new LTS line or wider profile is a new decision.
5.2.3 is the current research candidate, not a runtime certified by this design. No parallel 4.5 support commitment remains.
System setup explains the mandatory one-time administrator-installed consent service and its removal, then the separate
Linux image component. Confirmation uses Riel's **login password** on a separate real text console on Linux or the secure
desktop on Windows. No PIN/fingerprint path is added. The first confirmation for an enabled restricted tier can say
"For my own projects created in Kinewright on this machine and the existing projects I tick below." The secure prompt lists
the person's existing local projects by title/path. Riel ticks the ones to adopt as own projects in that one attended
confirmation; an unticked project stays outside standing coverage. A project whose identity the broker cannot bind natively
is never offered for adoption. The broker checks protected native origin/adoption records, and new covered projects enroll
automatically. Later adoption of an existing project requires a per-project secure confirmation. Unadopted, imported,
downloaded or foreign projects ask for their own project/tier scope. Imported code keeps its separate hash admission.
New and ordinary editing work without the broker installed or running. Origin goes unrecorded, and that project later
needs per-project confirmation or adoption before external execution. Installing or restarting the broker does not silently
adopt it. All full access, including a live window and Windows background, asks per project; standing permission never covers it.

**Expected monthly cadence: once at setup per enabled restricted tier, including ticked existing projects, then zero routine
prompts for covered own-project work. Add one confirmation per later adoption, unadopted or foreign project/tier scope,
or full-access project scope first enabled, and for scope widening, a new tier or re-grant after revocation.**
Already confirmed project/full-access scopes do not ask every month or session. Combine new code/hash/action decisions
in one displayed scope when possible. Ordinary source edits, new covered own projects and certified 5.2.x patches do not
prompt. Failed certification simply pauses execution.

First confirmation displays tier, roots/root policy, network enforcement, code classes/hashes and the 5.2 LTS profile.
It includes: "Undo restores your Kinewright edit and captured scene. It cannot reverse a full-access script's file or
internet actions." Standing copy explains that later agent-written revisions are covered only in the displayed scope.
The active-grant list shows own-project standing coverage and every per-project scope separately, plus **Revoke** and
covered projects, including adopted ones and the recorded adoption decision. Revoking standing permission blocks all of
its covered projects after restart; re-grant asks again.
An agent cannot operate confirmation. Requests for reduced permission remain possible without privilege escalation.

Normal Studio launch stays normal. The image service starts its isolated service-UID submission client on demand after
checking existing coverage; the consent broker does not launch it or process image preparation. New projects enroll within
quotas without per-project administrator work. No protected helper credential reaches Studio or Blender.
Setup copy says: "Restricted Linux rendering needs the installed certified image component, system bubblewrap **0.13.1**,
mksquashfs from squashfs-tools **4.7.5**, and the certified kernel/systemd/cgroup/fs-verity profile. The manual checkpoint
needs that same installed path; you choose when to prepare each changed image. Without the certified mount component,
restricted execution is unavailable. Kinewright does not install these tools during a render. A version number alone is
not certification. A missing or changed prerequisite shows Restricted unavailable; it never switches to full access."
The inspector lists detected exact tool/build hashes and the failing prerequisite. Windows manual preparation still
requires the separate consent service; refusing it disables external execution while old rendered clips remain usable.

Show rendering progress, selected device/quality, estimated completion only when grounded in observed frames, and Cancel.

`Open in Blender` opens a managed copy and a visible Blender window. Before launch, setup and the inspector say:
"This managed Blender window uses a private Kinewright configuration. None of your own add-ons, startup files or preferences
are loaded. It includes only the certified Kinewright bridge and the dependencies you approve. Your usual Blender setup is separate."
The source inspector shows person control, agent control or `Agent finishing changes`. A call returning is not person handoff.
After arbitrary full-access Python, or if callback interception/barriers cannot be certified, show that the managed window
will close/reopen from the last acknowledged checkpoint before handoff. A provisional save is not yet acknowledged.
Never label a callback-active scene as saved or person-controlled.
The person saves a change, sees `Source changed`, and chooses Re-render, or uses their saved auto-render preference with quota/admission checks.
Default manual saves do not launch expensive renders. Agent workflow requests them explicitly after its still check.
The old image remains visibly stale while rendering. Final export names every unresolved clip and refuses; ordinary cutting/2D grading over ready frames stays available.
Undo restores the prior source/render without deleting the newer Blender work. `Use this saved scene` explicitly reconciles a divergent workspace.
`Use this rendition` is a separate person choice for an unbound verified completion. It creates the one undoable acceptance in §6.2.
When a managed window closes after host/guardian failure, show its last acknowledged checkpoint time and any unsaved-work warning.
Recover a preserved workspace as a proposal, never as a silently accepted source change.
Errors offer a concrete recovery action: choose a supported install, grant missing scope, fix missing inputs, free unpinned space, or use the last good rendition.

## 9. Budgets

All new ceilings are **proposed acceptance targets**, not measurements. Use stricter accepted AW1/AW2/PF1/JA-1 caps where they overlap.
Report Linux and Windows separately, with exact source/binary/fixture/runtime/device/driver hashes. No line-count budget applies.

| Resource | Proposed bound and overflow behaviour |
| --- | --- |
| Native BL1/BL2 control | ≤32 MiB incremental settled RSS, ≤192 MiB incremental peak during capture/decode, including guardian, native helpers, leases, IPC, and transient copies; also meet AW1's 400 MB idle/1 GB proof and 120 MB proxy targets where applicable. |
| Source capture | ≤256 MiB complete source closure, ≤10,000 files, individual `.blend` ≤128 MiB, linked-library depth ≤8. Reject before growth; never omit a dependency to fit. |
| Disk | AW2 substore ≤8 GiB and ≤2 GiB/render, charged inside JA-1's existing allowance. Reserve workspace, new source image, overlapping old image, output and backup bytes separately. Pinned assets are not eviction candidates. |
| Records/queues | Share AW2's 256 asset/spec records and 256 KiB project metadata allowance; manifests are blobs under JA-1 graph caps. One render, one live session, ≤16 queued jobs/256 KiB, ≤64 summaries. Supersede unstarted work only. |
| Recovery/image helper | §5.3's seven tree records/7 KiB, four control headers/4 KiB and four image records/4 KiB total ≤15 KiB. At most five native control processes including the ordinary guardian. One build/serial validation at a time; ≤320 MiB/image, ≤60 s build, ≤5 s mount handoff. Reserve captured blobs, staging, helper copies, overlapping images, mount references and checkpoints in the same 8 GiB/project allowance. Runtime-image disk is separately reported. |
| Protected consent/enrollment | ≤64 enrolled projects/origin records per machine, ≤8 per-project scoped grants/project and ≤8 standing scoped grants/machine; grant/origin record ≤2 KiB. Protected store ≤2 MiB including enrollment/generations, bounded runtime-certification records, the persistent monotonic certification-generation high-water mark/revocation state and a rotating ≤512 KiB audit history. One pending ≤4 KiB scope/prompt, 120 s expiry, one untrusted prompt request/minute. Refuse full stores; never evict active grants, the certification high-water mark or revocation state when rotating audit history. Unknown/removed IDs refuse. Only fresh authenticated confirmation mints/re-grants a permission or adopts an existing project. Adoption lists/selections share the prompt and project caps; refuse unbindable identities. Eligible own-project origin/enrollment and same-scope certified patch bindings derive from that live permission without another prompt. |
| Output | ≤1,000 frames, ≤3,840 per side and ≤8,294,400 pixels; PNG ≤40 MiB/frame, public half-float EXR ≤96 MiB/frame, mandatory float32 pre-PNG evidence ≤192 MiB/frame with ≤8 MiB streaming scratch. All retained output/evidence together ≤2 GiB/render. One frame verified/decoded at a time; no timeline-sized RAM buffer. |
| Native CPU | Hash/copy/decode on ≤2 bounded workers, off UI/Core; idle live-session polling ≤1% of one CPU core over 60 s. Source-image tools use ≤2 threads and low priority within those native costs. |
| Warm decoded frame | 600 verified 1080p frame hits already in the working cache: lookup/identity check/handoff p95 ≤2 ms on each release performance lane. No hidden decode in this row. |
| Cold PNG frame | 600 distinct 1080p frames absent from the decoded cache: compressed read/hash/PNG decode/working conversion p95 ≤25 ms on each named release performance lane. Record filesystem-cache state separately. Measure cold storage and OS-cache-warm reads independently; neither is called a decoded hit. A cold frame missing its due time follows PF1's counted policy and never masquerades as current. |
| External compute | One Blender renderer; default four CPU threads, low priority. Record external peak RSS and CPU/device time separately; initial job safety ceiling 8 GiB process-tree memory and 30 min/job, explicitly changeable by the person. This is a resource policy, not a render-performance acceptance allowance. No external RSS exemption for native Kinewright helpers. |
| Timeouts | Probe 5 s, handshake 5 s, describe 15 s, edit/still action 60 s, callback retirement 5 s. A frame has a 120 s **inactivity** watchdog, reset only by validated increasing sample/tile/phase progress within the same frame, never heartbeats/repeated logs. Overall render including preparation defaults to 30 min and never resets. The person can set finite inactivity/action/total limits before dispatch, including slower Cycles stills/frames; scripts cannot change them. Persist them in the resource profile and show elapsed/last-progress time. Capture/build/mount retain their phase deadlines. Remotion limits remain unchanged. Cleanup retains AW2's 500 ms grace/2 s kill-decision deadline. |
| BL3 content | ≤100,000 triangles, 256 nodes, 128 primitives/draws including ≤8 glass draws, 32 materials, 16 static textures, static texture side ≤2,048, ≤32 MiB decoded geometry, ≤48 MiB static textures including mips, four lights, one animation ≤32,768 keys. Reject/route to BL1, never silently decimate. |
| BL3 environment | One active HDRI/project and **one distinct effective environment identity per presented frame**, shared across clips. Source ≤2,048×1,024 and ≤16 MiB compressed; unique GPU derived allocation ≤8 MiB within the assets cap, charged through last use/fence. A second effective identity refuses atomic frame admission. Prefilter scratch ≤32 MiB and CPU ≤5 s on LH, off UI; no per-frame prefilter. Historical HDRIs stay under store/pin quotas. |
| BL3 glass transmission | Full RGBA16Float opaque-colour chain to 1×1 per 3D clip, ≤22.2 MiB at 1080p including ≤5.6 MiB above the base, with full-resolution depth and no depth mips. Charge real padded bytes for every simultaneously retained chain inside ≤128 MiB targets. Compute mip generation GPU p95 ≤0.5 ms on LH/3090, included inside the all-pass ≤4 ms budget. Refuse if raster/alignment/overlap exceeds target headroom; retain through last glass use/fence. |
| BL3 video screen | One distinct media source/3D clip, ≤1,920×1,080; actual decoded frames/reservations join PF1's 224 MiB required-set budget, existing reader limits and media texture accounting. ≤16 MiB incremental resident screen GPU storage within the BL3 assets cap; shared allocations count once by identity. No second decoder/player. |
| BL3 memory | Native decoded scene/control ≤96 MiB incremental RSS; GPU assets ≤112 MiB including geometry/static/environment/screen allocations, colour/depth/MSAA/resolve/transmission targets ≤128 MiB. All stay within accepted total GPU ceilings, initially 384 MiB ordinary and 1,536 MiB heavy-4K. These are subcaps, never additive allowances. |
| BL3 frame | At 1080p LH/3090: animation/control CPU p95 ≤1 ms, all 3D passes including IBL/glass/video sampling GPU p95 ≤4 ms/p99 ≤6 ms. Warm geometry/static/environment reuploads = 0; non-video dynamic uploads ≤64 KiB/frame. A changed video frame uses the existing media upload path and reports its actual bytes, capped at the admitted screen raster. Zero full-frame CPU readback in preview. |
| Interaction | UI callback p95 ≤16.7 ms; warm gesture→correct presented frame p95 ≤100 ms. Asynchronous bounded glTF parse/upload target ≤2 s with an already prepared environment. A new HDRI has a separate ≤5 s preprocessing target on LH. Record end-to-end scene/environment/first-screen readiness too; show each loading phase honestly. |

PF1's 224 MiB preview WorkingFrame cap remains hard. Admit the entire media/generated/native-3D required set before work starts.
Count each actual CPU allocation once by shared identity through last owner, and each GPU allocation through fence retirement.
BL3 GPU-only targets use the existing GPU ledger and required-set admission, not the CPU frame-byte counter as if they were CPU memory.
Proof/export retain K-6 accounting; no claim that synchronous fallback meets I12. Unsupported/budget-heavy 4K native scenes refuse and offer BL1.
No quality/sample/raster reduction can masquerade as a full-quality final frame.

GPU contention policy is explicit: default Cycles CPU leaves the preview GPU available. A selected separate render GPU may run concurrently.
On a shared GPU, queue EEVEE/Cycles GPU work until playback/dragging stops; do not promise reliable fine-grained GPU preemption.
On new playback demand, cancel a background job and retain completed scratch frames only as non-final work, then release its device/tree.
GPU-driver stalls can exceed cancellation targets; that fails the gate and blocks the profile rather than causing silent device changes.
For a visible live session, request solid/material viewport pause during Kinewright playback. Full-access person overrides are labelled contention mode,
with no simultaneous-preview performance promise. Never alter global driver settings, clocks, desktop settings, or other projects' processes. [R6]

Benchmark the same accepted PF1 workloads with a rendered overlay and, for BL3, a rotating device over video plus mesh type/camera motion.
The required BL3 fixture is a metal phone with a glass-covered screen playing a numbered screen recording under a high-contrast HDRI.
It includes highlights, static roughness wedges and readable moving screen landmarks. Time all effects together with background video,
both warm and after seek; testing an unlit still screen does not discharge this gate. Record the decode work as well as 3D pass time.
Use PF1 P-play/P-seek/P-rss, three individual passing runs, due-frame denominators, presentation acknowledgements, and existing lane floors.
BL3 LH must meet 60 displayed fps on its 60-fps bounded scene fixture and PF1's ≤1% late/held/dropped, ≤100 ms hold/stall targets on added 30-fps workloads.
LL retains PF1's 24 fps typical/explainer and 15 fps blend/reel floors; propose the same 24/15 fps floors for added native-3D workloads.
The local Windows WARP native-3D target is at least 20 displayed fps on the bounded fixture, measured on a pinned VM profile.
These new floors need measured acceptance; neither the pending historical WARP floor nor an inherited software pass counts as BL3 evidence.
Measure with a running CPU Blender render and shared-GPU cancellation too. Existing no-BL workloads keep their unchanged regression pins.
Annotate other workstation load, use paired runs without concurrent builds, and record per-process peaks plus sampled simultaneous aggregate.

## 10. Per-OS differences

| Concern | Linux x86-64/glibc | Windows x64/MSVC |
| --- | --- | --- |
| Discovery | Explicit configured path first; enumerate PATH, common official archive locations and desktop metadata as candidates only. Snap/Flatpak launchers are unsupported unless their nested confinement/profile is certified. | Explicit `blender.exe` first; enumerate installer registration/common Program Files and person-selected portable installs. Do not execute registry command strings or arbitrary file associations. Store/MSIX installs require an explicitly certified launch/image profile. |
| Runtime identity | R24 verified executable fd and read-only runtime image; include libraries/Python/OCIO. No mutable PATH launch. | R24 retained executable/backing/root leases, volume-GUID namespace and suspended image verification. Unicode/long paths are native paths, not shell text. |
| Source images | Captured blobs, separate locked writer/validator units with socket-activated fd handoff and verified emptiness, protected image service and independent receipt key, fs-verity and privileged fd mount transfer. Installed certified mount path required even for the person-triggered manual checkpoint; without it restricted is unavailable. B1-mount precedes restricted default; B1-helper is required for BL1 completion. | Person-prepared NTFS VHDX under R24. No automatic Windows builder or shipped/installed image service. A future service needs a certified unprivileged builder before installation. |
| Supervision | Independent user service/cgroup-v2/subreaper; exact native identities, no process-group fallback. | Independent guardian, creation-time Job assignment, non-breakaway kill-on-close; no duplicate lifetime handle and no `taskkill` by name. |
| Confinement | Restricted CPU uses exact certified bubblewrap 0.13.1/policy/kernel tuple, namespaces and negative controls. GPU/live full access is a distinct grant. Missing prerequisites show restricted unavailable, with no fallback. | Both-OS BL2 live full access is explicit; Windows background full access comes later under a separate grant. Job Objects are not AppContainer. No enforced-root/network claim. |
| Consent authority | Mandatory administrator-installed root consent broker, protected key/store and authenticated real-VT/PAM confirmation, even with manual images. If the trusted input/display path is unsupported, external execution refuses. | Mandatory administrator-installed consent service, service-only CNG key/store and secure-CredUI/LSA confirmation, including Windows live/manual images. No ordinary same-user GUI signer. |
| Image-helper setup | Separate administrator-installed root image service and locked builder account. Only the image-service-launched fixed service-UID client enrolls. Ordinary Studio launch is unchanged. Covered local-project enrollment needs no administrator or prompt; foreign/full-access scopes confirm per project; removal drains owned images and retains projects. | The image service is not shipped or installed until a certified unprivileged builder exists. Future `SeManageVolumePrivilege` belongs only to that service token. The separate consent service has no such privilege. |
| Local control | Private AF_UNIX socket, restrictive permissions and peer credentials plus session secret. | Named pipe with current-user/logon ACL and remote clients rejected, principal check plus session secret. |
| GPU | Cycles CPU/CUDA/OptiX/HIP/oneAPI as certified; EEVEE supported backend/context without assuming a display is available. | Same manufacturer-specific choices as certified. Kinewright's DX12 adapter and Blender's render device must be correlated by physical device identity. WARP does not imply Cycles software compatibility. |
| GUI/files | Wayland/X11 display access only for explicit live mode. No notification-only watcher proof of byte identity. | Visible Blender window; sharing violations/antivirus retry within bounded policy. Native volume/file IDs handle aliases/8.3 paths; preserve supplied spelling for display. |

Both OSes need native process-death, input replacement, filesystem/receipt, colour/alpha and missing-runtime lanes for their exposed modes.
Linux evidence, a cross-compile, or a software renderer does not discharge Windows execution/GPU acceptance.
No macOS support or timing claim is introduced.

## 11. Slice plan BL1–BL3

Use separate later slice designs and landable parts. Security/data-loss stages need the product brief's two independent reviews.
This draft starts no review round. Every gate below is a future obligation and **NOT RUN**.
No symbol-not-found/compiler failure, mocked renderer, rejected valid input, or missing optional install counts as a runtime-red witness.

Each gate has a feature-opening baseline and an invariant witness. The baseline uses today's public `invoke_capability`/plan API,
ordinary media controls, and a minimal external observer; it can build without future Rust types.
Today the Blender action/scene kind is unsupported. Record that actual runtime refusal before implementation; do not claim a measured baseline here.
After integration, mutate one production guard at a time and collect targeted runtime red plus restored green.
Old passing lock/PF1/CC1 tests are regression pins, not evidence that Blender already works.

### 11.1 BL1: background-render clips

Dependencies: accepted AW2 P0 and complete Part 2, accepted PF1 integration, AW1 auth/roots/hosted handlers,
settled MO1/MO2, and accepted JA-1 host/project composition including source retention and receipts.
**BL1a owns programme-wide A6 implementation and cutover.** AW2 Part 2 lands unchanged and remains **not yet
A6-compliant** until BL1a removes R21's headless-setting path, migrates old grants through genuine confirmation and
re-runs Remotion's consent/revoke/restart gates with the broker. Complete this on each OS before exposing any Blender
tier there, including inspection/probes/Windows live. Broker failure cannot select the legacy path. This is BL1a work,
not a new AW2 Part 2 dependency; all non-consent Remotion gates remain regression requirements.
Before **BL1a entry**, run B1-mount's native feasibility spike on the intended Linux profile. Until it passes and the
profile is certified, report restricted unavailable. B1-grant must certify the real independent consent authority before
exposing any execution tier on that OS, including helper-absent Remotion/Windows live and supported Windows manual intake. Without the certified Linux
mount component there is no restricted execution to certify; the broker remains independently required for exposed tiers.

BL1a lands A6 and the shared adapter/model/PNG/stills, mandatory pre-PNG float evidence/alpha verifier, restricted CPU
profile, code admission, ownership, guardian and usable source-image intake. Its restricted manual lane requires §5.5's
installed certified image service, service-UID client and person-triggered writer/validator/receipt/mount path. Without
that component it refuses restricted execution. It may land only as **BL1 manual-preparation checkpoint, automatic
refresh pending**. BL1-auto then accepts automatic activation after saves and the complete native B1-helper suite for
create→still→adjust→render without per-edit person preparation; basic path installation alone is not automatic acceptance. A failed helper remains unavailable, never elevated Blender or a waived exit gate.
BL1b adds public half-float EXR playback/scene-linear entry; HDR separately waits for accepted CC8 integration.
State each accepted output profile explicitly rather than claiming PNG is float/HDR acceptance.

**BL1 is not complete until automatic scene refresh works on Linux and B1-helper passes natively.**
No manual checkpoint, document review, reserved-slot fixture or Windows refusal satisfies that requirement.
Linux-only acceptance does not imply Windows acceptance. Do not expose loadable unrenderable clip kinds between parts.
CC4 ordering remains a regression dependency.

The programme build order is Linux BL1, BL2 live, **BL1-W**, then BL3 ahead of job E.
BL1-W is the later separately granted Windows full-access background profile. It uses person-prepared R24 images,
requires the Windows consent authority/native guardian/identity/colour lanes, and does not gate Linux BL1 or the
both-OS BL2 live authoring/inspection checkpoint. It completes Windows BL2's separately pending create-to-render handoff.
Windows still needs manual scene-image preparation after saved changes. No automatic Windows builder/service ships here.

| Gate | Runtime witness, expected baseline deficiency, and discriminating mutation |
| --- | --- |
| B1-clip | Real stdio client creates a procedural rotating card, checks a still, renders an alpha clip and overlays two backgrounds. Baseline has no adapter. Valid installed-Blender render is the positive control; mutate index/alpha validation and assert changed pixels/refusal. |
| B1-owned | Move project+owned store, remove original scene/assets and Blender, then play/proof/export exact stored frames. Undo/redo, branch, recovery and Save As retain every source/output hash. Baseline cannot create that clip. Delete one graph edge or release an undo pin: actual later read/copy must fail. |
| B1-security | Real malicious fixture tries outside-root read/write/delete, symlink swap, credential read, network TCP/UDP/IPC, and descendant escape in restricted Linux. Assert OS denials and unchanged sentinel bytes, plus successful allowed output. Full-access lanes assert honest unenforced labels and person-only grant, not fictional denials. Mutate launch confinement/auth/grant checks separately. |
| B1-grant | Exercise the production installed broker/key/store/OS password path on each exposed OS, with image helper absent and present; Windows live/manual intake still use it, while helper-absent Linux restricted refuses. From real same-user provider/full-access Blender processes try forged env/argv/config/TTY, fake GUI replies, store/key writes, injection/handle duplication, nonce replay, project/root/scope swaps and broadened code/tier requests: no false confirmation/helper authority. Confirm one restricted own-project standing tier, ticking two existing native-bound local projects in the setup adoption list; both run with zero extra prompts, while an unticked existing project still prompts. Offer no adoption for an identity the broker cannot bind natively. Create two projects through real Kinewright New and run both with zero extra prompts. With the broker absent or stopped, create a project and edit/save normally; after installation/restart it still lacks origin and prompts for its own scope or adoption before execution. Confirm later adoption through the per-project secure path and verify standing coverage without another routine prompt. From a real same-user process submit a forged adoption request/selection/confirmation: no adoption record or false person event results. Copied/downloaded/foreign projects (including a copied origin receipt, foreign Save As and a local wrapper import) and full-access requests still prompt before execution. Proper per-project confirmations permit those scopes; no standing full access exists. Deliver a certified same-profile 5.2.x patch record by control socket and shipped file; actual runtime hashes re-bind with zero prompts, no administrator action and no invented authentication event. Accept a higher-generation revocation, restart, then replay the older validly signed record by both delivery paths: the protected high-water mark and revocation persist, the record is refused and the revoked runtime remains blocked. Uncertified, revoked-cert and wider-profile runtimes refuse or request the genuinely new scope. Revoke the standing grant, restart broker/host and show every covered project blocked, old tickets/cached bindings unable to dispatch/publish, and no automatic re-enrollment/re-grant. Crash before/after durable ACK and replay old boot nonces/forged store copies. A real secure confirmation re-grants only its displayed scope. At BL1a cutover legacy Remotion headless flags cease authorizing execution, while a genuine broker Remotion grant succeeds; repeat revoke/restart. Repeat client startup attack controls from B1-helper and successful ordinary Studio submission. Mutate origin/adoption eligibility, certificate-generation anti-rollback/scope binding, parent-generation/restart and protected-confirmation guards separately. |
| B1-mount | **BL1a entry spike**, not a paper assertion: use the real root image service, guardian and pinned bubblewrap with an unprivileged fixture child blocked in its final sandbox namespace. Prove §5.5's `open_tree`/`mount_setattr`/`setns`/`move_mount` sequence under the exact owning-user-namespace capabilities. Check source/runtime mount/root identities, immutable A reads and executable launch, with outside roots/network denied. Then load a minimal real Blender scene. Forged child/ns/root targets, host alias swaps, helper death between two transfers and guardian death before COMMIT produce zero source execution or ready output and retain cleanup charges until retirement. Mutate target identity, flags, startup barrier and lease release separately. Record the native syscall/capability trace; unsupported kernels mean restricted unavailable. |
| B1-code | A downloaded scene and linked-library scene each contain a Freestyle sentinel script, a text-block hook and a driver-animated landmark. Render-only permission refuses every code-bearing input before a sentinel/action/render; a plain keyed control renders. With explicit matching code hashes/grant, the supported hook executes and the driver landmark moves in actual frames. Hidden referenced code, mismatched hashes and foreign handlers refuse. Mutate recursion, Freestyle/driver refusal and hash admission separately. |
| B1-helper | Native Linux ordinary Studio obtains covered enrollment and the image service starts the fixed dedicated-service-UID submit client; the broker receives only bounded grant-generation queries, no build manifest/results, and its key never signs image receipts. Barrier the client before/through exec and before READY/credential delivery on a profile allowing ordinary same-UID tracing: real person-UID provider/Blender attackers attempt ptrace/process-memory/fd/IPC injection and get no control/credential; an ordinary same-UID fixture proves the attack harness works, and Studio's normal submission succeeds. Observe distinct locked submit/writer/validator UIDs, root-protected image receipt key, fixed socket activation, actual sd_listen_fds/SCM_RIGHTS transfer and the no-credential-before-verification order. Wrong peers, extra/truncated fds and wrong job nonces refuse. Leave a real writer descendant alive: validator activation/receipt must wait for independently proven writer-cgroup emptiness and reaping; restore cleanup and validation succeeds in the same one-tree slot. This activation/fd/closed-writer spike is mandatory feasibility scope beside B1-mount. Then create→still→adjust→new-image→render automatically with no per-edit preparation. Keep five BL1 production trees, the real serialized builder/validator and named visible lifecycle fixture at seven. Unrelated UID, full-access Blender and agent-spawned host cannot enroll as the protected client or select privileged paths/devices/units/commands/namespaces. Reject forged/wrong-key receipts, stale nonces, downloaded images, digest swaps and change-restore. Saturate quotas, revoke standing/project grant during preparation, interrupt capture/build/validation/digest/mount-ACK/detach and kill host/client/helper/writer/validator separately. Preserve old root, owned-only mounts, charges and independent reap/detach/boot/removal reconciliation. Mutate startup UID/credential ordering, fd/peer checks, empty-cgroup barrier, separate-key supervision, receipt/root/digest/lease and recovery independently. Windows verifies absent unshipped image service plus supported manual intake under its broker; that refusal is not helper acceptance. |
| B1-identity | Pause before exec/after first read; replace source, add-on/runtime/executable/backing paths and attempt write-then-restore. Execute admitted A or refuse, never B under A's identity. Refresh B deliberately and get a new key. Remove each R24 lease/seal and make its specific witness fail. |
| B1-lifetime | Use real Blender plus native descendant fixtures through the production guardian. Observe descendants first, then success/cancel/timeout/host death/guardian death/startup-ACK loss. Verify empty/reaped trees and retained charges before release; mutate lifetime-writer inheritance, kill boundary, EOF release, and buffered-ready publication separately. |
| B1-accounting | Admit the five BL1 production trees: three provider attempts, native SVG worker and one shared external/Blender job. Fill reserved slots with **VisibleSessionLifecycleFixture** and **ImageBuildLifecycleFixture**, using production slot/cleanup paths and real native descendants/reservations. Independently observe seven tree identities, ≤7 tree records/7 KiB, ≤15 KiB transient recovery metadata and ≤5 native control processes. Exercise four image leases, eighth-tree and occupied/cleanup-blocked-kind refusal, guardian/broker/helper/client death and busy detach. Retain actual slots/bytes until independent retirement, release once. Fixtures prove reserved lifecycle/accounting only. Actual builder integration is B1-helper; actual managed window is B2-accounting. Mutate slot composition, record growth and early retirement separately. |
| B1-history | Source A→B+render is one undo; lost reply/restart returns one receipt; delayed B cannot attach after Undo. Complete one request twice with distinct manifests: first binding stays selected, second is visibly unbound, and agent acceptance refuses without history change. Person `AcceptExternalRendition` selects the second in one edit; Undo restores the first exact manifest, including after save/restart. Mutate binding comparison, actor, expected-head, receipt and completion-generation guards separately. |
| B1-time | Numbered eight-frame fixture covers O/a/b/T edits and finite bounds. Preserve 30000/1001→24 declared seconds. A 24 fps interpolable baked-motion fixture sampled at 30 fps shows independently calculated subframe landmarks and shutter samples; its integer-only twin refuses. Move a bound beat from 2 s to 3 s: actual event moves from frame 60 to 90 at 30 fps, checking surrounding frames too. Undo restores original scene/binding/rendition and frame-60 event. Asset-local control stays fixed; an unbound keyed scene refuses timeline lock. Mutate subframe/fps, binding application, origin/rounding/end inclusion and mixed-adapter admission separately. |
| B1-cancel | Cancel the rate/spec plan before Core commit and assert identical document/history. Commit it, make an unrelated title edit, then cancel its actual render. New rate/specs and title remain, with no rollback entry or ready partial output. Undo title then rate batch; compare exact old scene/spec/binding/history. Mutate phase handling or callback rollback independently. |
| B1-colour | Real Blender PNG over black/white through preview/proof/decoded export checks AgX/Standard, straight alpha, tags and no double view. BL1a verifies actual Combined float32 pre-PNG pixels and binds hashes/profile/result, then compares dimensions and every decoded PNG alpha byte against q8(evidence alpha) exactly as §6.5 specifies. On the real pinned build/save path, render a semi-transparent gradient with alpha samples across q8 step boundaries, set/verify dither off and compare every decoded alpha byte to q8 of the actual float evidence. A setting readback alone cannot satisfy this case. Use different spatial alpha masks on successive frames: swap evidence indices, change one alpha byte or dimensions, omit/change evidence or fake driver success and require refusal before ready/restart binding. An identical-alpha control is explicitly not proof of RGB derivation; inspect known RGB landmarks separately. A zero-alpha emissive flame/glow refuses before PNG readiness and at public EXR/native ingress, preserving the last binding. Opaque-background successor and representable partial glow succeed. Tiny positive/quantized-zero alpha tests division and underflow. Public float subgate preserves -0.1, 0.18, 1, 4 and partial coverage; HDR checks white/HLG/intent. Mutate alpha correspondence/evidence binding, emission refusal, quantizer/dither/setup/association, EXR sRGB decode and extra view/OOTF separately. |
| B1-bounds | Overlong logs, forged progress, incomplete frames, decode bombs, full disk/pinned store, and cancel during capture each refuse safely with old root intact. Real source/render succeeds within §9. Remove a byte/reservation/decode bound, measure the resulting runtime breach. |
| B1-agent/person | Missing/unsupported install stays readable; person grants once, agent runs bounded verbs, inspector shows source/stale/cancel, prior project versions refuse new kinds safely. Capture exact served-tool bytes and measure real headless footprint. Baseline feature refusal is separate from existing protocol/format pins. |

Linux BL1 exits only with native **automatic** restricted-CPU scene refresh, B1-helper's independent security review
and passing native production gate, B1-mount, B1-grant, staged B1-accounting and all applicable PNG/ownership/history/
cleanup gates. Retain source-bound reports, raw summaries, restored targeted mutations, notices and a practice-edit asset.
The manual lane is useful but remains the labelled checkpoint above. Public EXR/HDR and Windows have separate dispositions;
NOT RUN lanes remain open and never become full-platform/HDR acceptance. BL1-W exits separately with native Windows
full-access/manual-intake and its genuine consent/revoke/identity/lifetime witnesses, at its named slot after BL2 live.

### 11.2 BL2: live session

Depends on accepted BL1 and A6 host/session ownership. No new model provider or AW3 requirement.
BL2a supplies extension/package, local auth, visible session/control transfer, save proposals and recovery.
BL2b adds viewport perception, timing/colour handoff and person workflow instrumentation on that same session owner.

| Gate | Runtime witness and mutation |
| --- | --- |
| B2-watch | Real Blender window is visible while the agent builds/inspects/adjusts a device. Saved result imports via BL1. Baseline has no live-session action. Positive manual Blender save remains possible; removing main-thread/control serialization must expose a scripted person/agent race. |
| B2-auth | A separate local process sends wrong secret/principal, replayed sequence, wrong project/epoch and oversized messages. Zero actions occur; valid paired control works. Mutate authentication, binding, framing, and grant checks separately. |
| B2-manual | The lead performs person save A→B with texture/library edits and a lost watcher event. Interrupt an external multi-write save after a same-length first fragment. Barrier the first hash, then resume the write before the second full read; unequal hashes must retain A and show saving. A truncated partial also refuses closure validation. Stable B yields one source undo only after two equal full hash/size/identity vectors. Undo leaves the workspace and never auto-reapplies B. Mutate full-read/quiescence/size checks, save-ID dedupe and undo suppression separately. |
| B2-time/colour | Push rational beats, trim, fps and colour settings; inspect actual Blender marks/settings, not just sent JSON. Save-returned marks require normal revision-gated operations. Delayed viewport/saves after branch switch cannot attach. Mutate generation/context binding with distinguishable A/B imagery. |
| B2-loss | Observe the real managed window/dirty state, then kill host, guardian and Blender separately during checkpoint, authoring or save receipt. Guardian loss closes the managed window; preserve last acknowledged checkpoint/time and charges through independent cleanup. Check 2 s quiet/30 s dirty requests only acknowledge after callback quiescence and durable receipt; blocked main-thread deadlines remain overdue. Restart never duplicates an edit or kills unrelated Blender. Mutate callback/checkpoint ordering, death cleanup, early retirement and receipt precedence separately. |
| B2-callbacks | On the certified real Blender build, verify interception is installed before authoring. Use both a bridge-helper control and an authoring script that directly calls bpy.app.timers.register (including repeating/persistent), unregister and bpy.msgbus.subscribe_rna with its own owner; add direct handler-list registration. The initial call returns before the callback is due; at a native/event-loop barrier make it due afterward. Independently exercise person handoff, checkpoint, cancel and branch switch. During active agent control each positive callback changes a visible landmark. Wrappers must mint transaction owners, retire/drain them and reject late generations; inspect saved scene hashes/landmark pixels/control after the due time, not counts. Deliberately bypass/replace interception using a real reachable route on that build: arbitrary full-access Python must force terminate/reap and fresh reopen from the last acknowledged checkpoint even when tracked tables look empty. No attempted checkpoint ACK/person handoff/branch publication precedes cleanup. Test provisional-candidate promotion only after cleanup and fresh passive validation, or preserve the prior checkpoint with visible unacknowledged-work loss. An uncertified-interception build always takes the reset route after Python; fixed certified data actions show the ordinary cooperative barrier control. Inspect actual old/new process identities, late timers/msgbus effects and exact checkpoint hashes. Mutate wrapper installation/direct-call ownership, retirement/generation, taint→replacement and cleanup-before-ACK guards separately. |
| B2-accounting | Repeat §5.3's composition with the **real managed Blender window**, five ordinary production BL1 trees and the accepted **real** Linux builder/validator in the seventh slot. Independently observe window/tree identities, grants, all reservations/records, four image-generation leases and seven-tree ceiling under guardian/helper/broker loss and busy cleanup. Replacements/eighth trees refuse until actual retirement. Windows tests its exposed production slots plus explicitly named reserved lifecycle fixtures; it claims no absent builder acceptance. Mutate visible-slot admission, lost-window cleanup and early byte release separately. |
| B2-budgets | Live idle CPU/RSS, snapshot byte caps, UI presentation, revoke/cancel and shared-GPU contention meet §9. Baseline availability refusal is not a performance failure. Inject controlled blocking/UI work or unbounded snapshot allocation to fail the measured gates. |

Exit includes a **lead-run practice session** exercising both the agent and person paths: build, inspect, manual save,
stale state, re-render, Undo, revoke and recovery. Retain the evidence and finished result for Riel to watch once.
No dedicated Riel rehearsal is required. Observe friction during Riel's real video work later, as the product brief requires.
Account Linux and Windows live lanes separately. Full Windows create-to-render acceptance also needs its separately granted background lane.

### 11.3 BL3: native 3D layer

Depends on accepted BL1 and merged PF1 resource/presentation seams, MO1/MO2, JA-1 graph/history and AW1 tools.
There is no technical BL2 dependency: supervised BL1 can export glTF without a live GUI. The build order keeps the three main layers BL1, BL2, BL3, with BL1-W between live authoring and BL3 as specified in §11.1.
MO4 is required for its remap controls; without it native animation plays on fixed finite time. CC8 is required for HDR output only.
BL3a supplies bounded glTF/owned HDRI/media bindings; BL3b the shared-device PBR/animation/environment/glass pass;
BL3c shared media-frame sampling, controls and final colour/resource/playback gates. All are v1 acceptance work.
Keep model-only work off main until usable rendering. Static screens or black reflective metals do not complete BL3 v1.

| Gate | Runtime witness and mutation |
| --- | --- |
| B3-job-E | Import an original metal/glass phone, project-owned high-contrast HDRI and numbered playing screen recording, with mesh type, depth occlusion and animated camera. Scrub nonsequential/reverse/seek frames headlessly and in GUI without Blender. Compare screen landmarks to an ordinary media clip at the exact source time. Baseline lacks scene kind. Mutate coordinates/projection/depth, screen time and animation selection separately. |
| B3-subset | Valid PBR/unlit/mask/camera/light/animation and thin-glass controls render. Skins, morphs, arbitrary alpha BLEND, unsupported volume/refraction, unresolved glass backing, missing textures/HDRI/media, unsupported required/visual extensions and invalid accessor spans refuse visibly, with BL1 route. Mutate guards with their valid controls. |
| B3-environment | Metal/roughness wedge and glass-covered phone show reference highlights, Fresnel/energy response and transmitted numbered screen landmarks. Rotate the shared environment and check prefilter/LUT hashes and ≤8 MiB unique residency. Two simultaneous layers with the same effective identity share one charge; distinct HDRI/orientation identities refuse before native frame work. Verify every transmission mip and its dimensions/roughness selection, ≤22.2 MiB at 1080p including the base, and correct per-frame/fence retirement. Mutate environment-set admission, dedupe, mip generation/selection and reflection/transmission/double-view guards separately. Pin numeric tolerances before runs and retain reference stills. |
| B3-video | Production `DecoderGauge`, reader/cache and allocation identities show screen and ordinary media at equal time share a decode; different times follow existing reader rules. Trim, split, MO4 reverse/remap when accepted, rate change, seek, Undo and export select exact numbered source frames. No second playback clock/audio path. Late/missing frames never publish current proofs. Mutate media-demand admission, time mapping, colour-domain conversion and stamp validation separately. |
| B3-colour | Compare actual offscreen working pixels to independent BRDF/unlit/alpha vectors and final CC paths, with negative/overrange/nonfinite controls. Mutate texture sRGB-vs-data roles, association or extra tone mapping. Cross-backend tolerance is pinned before the run; no invented Blender parity claim. |
| B3-history | Replace camera/animation/scene/environment/video binding via exact operations, trim/split/remap, Undo/Redo, save/replay/branch/Save As and restart. All geometry/HDRI/prefilter/media hashes, history and source provenance stay exact. Mutate graph references, expected-head and time-origin guards separately. |
| B3-PF1 | Measure metal/glass/moving-screen full presented frames, cold seek/decode, environment preparation and every §9 cost/residency row. Include ordinary video and screen frames in atomic required-set pressure; retain CPU/GPU identities through cache clear, cancellation and fences. Measure contention, device loss and teardown. Assert zero warm static/environment uploads/full-frame readback and report changing-video upload bytes. Mutate reservation, final stamp and retirement, then inject pass/decode delay separately. |

BL3 ends with a native-scrubbed job-E practice sequence and explicit complexity/device envelope.
Record CPU-reference, CI-L/lavapipe, CI-W/WARP, LH/3090 and native Windows hardware dispositions independently.
No timing result is claimed from hosted CI. A failed or NOT RUN gate cannot become a full product/platform acceptance statement.

## 12. Risks

- Full-access Blender can destroy/exfiltrate data. Person-only grants, revoke and isolated copies reduce mistakes but do not contain hostile Python or reverse its side effects.
- Automatic images add privileged kernel-parser exposure. The unprivileged builder, protected submission client and fixed fd mount transfer require independent security review and B1-mount/B1-helper. A manual Linux checkpoint cannot complete BL1. No automatic Windows builder/image service ships here.
- Standing grants enlarge the already-confirmed restricted scope to eligible own projects; protected native-origin records and parent-generation revoke must prevent copied/imported projects inheriting it. Full access stays per project; exact patch certification re-binds only inside the confirmed line/profile.
- The mandatory consent authority adds one-time administrator setup and a protected password confirmation path on each OS. If real-VT/PAM or secure-CredUI custody/input isolation fails native certification, refuse external execution even with manual images. Same-user shell privileges remain outside containment.
- A delayed authoring callback can cross save/handoff. Pre-authoring interception only tracks cooperative calls; arbitrary full-access Python requires process replacement before acknowledgment/handoff. B2-callbacks checks direct registrations, bypass and restart, not a fictional global registry.
- Per-edit immutable images and the two-pass capture can slow iteration. Measure sealing latency and refusal burden, including manual Windows preparation.
- Blender/add-on/OCIO churn changes outputs or API behaviour. Certify exact tuples; preserve old output and require explicit profile/source refresh.
- Dependency discovery cannot prove arbitrary Python purity. Restrict supported authoring, preserve outputs, and avoid universal determinism claims.
- Live saves, timeline Undo, receipt loss and branch switches can race. Scene generations, expected old refs, single editing authority and immutable save proposals are mandatory.
- Shared GPUs can starve the player or hang in a driver. Conservative admission/cancel policy is part of acceptance, not a promise of OS GPU preemption.
- BL3 includes environment filtering, device glass and video textures, not only a glTF parser. Keep the stated subset and measured envelope; route unsupported scenes through BL1 without dropping these mandatory v1 features.
- PNG sacrifices scene range; EXR requires new float-domain handling. Separate profiles and numeric wrong-transform controls prevent silent clipping/double transforms.
- New v3 records can be lost by older writers. Actual prior-reader/write-refusal, Save As closure and recovery gates block release.
- Large source/output closures fill pinned storage. Bound and reserve before execution; never evict the only saved or undoable scene/rendition.

## 13. Open questions for Riel

None.

## 14. Research appendix with sources

Primary sources distinguish upstream facts from proposed Kinewright contracts. On 2026-10-26 rev4 rechecked the release/LTS
pages, exec/ptrace startup rules, systemd socket/cleanup and cgroup/fd APIs, and the exact 5.2.3 timer/msgbus/Freestyle
source. R28–R30 support the new startup and plumbing mechanisms. R1–R27 retain rev3's research with the refinements below;
other carried-forward links were not all reopened in full. Direct 5.2 timer/msgbus documentation opens returned HTTP 402;
accessible exact upstream source supplied those API facts instead. Latest/main links are mutable: implementation must
pin release/build/source hashes and native profiles. Protected local-origin records, standing scope and automatic patch
re-binding are Kinewright design requirements, not Blender authentication features. Rev5 carries that research forward and
checks the exact 5.2.3 dither setting source cited in R27 for this narrow PNG change.
The trust-chain composition is an engineering inference from these APIs, not an upstream guarantee that it works.
B1-mount/B1-helper/B1-grant and profile certification remain mandatory. No build, cargo command, Blender execution,
sandbox probe, benchmark, runtime witness or new independent review round ran for this revision. All gates are **NOT RUN**.

- **R1, release/API policy.** [Blender releases](https://www.blender.org/releases/) lists current stable 5.2 LTS at **5.2.3** and active older LTS 4.5 at **4.5.15**,
  both updated 2026-10-22; 5.3 is beta. [5.2 LTS](https://www.blender.org/releases/5-2/) is supported until July 2028.
  [LTS policy](https://developer.blender.org/docs/handbook/release_process/lts/) aims to preserve Python API/output/file compatibility, with exceptional fixes possible.
  [Release cycle](https://developer.blender.org/docs/handbook/release_process/release_cycle/) freezes API during beta, not across arbitrary feature releases.
  Pin the 5.2 LTS line only. Initial research candidate is 5.2.3; certify exact build tuples before execution.
  Patch updates pause new renders until recertified, while stored renditions keep playing. Same-scope certified 5.2.x
  re-binding needs no prompt (§5.1); neither an LTS compatibility policy nor person confirmation certifies output identity.
- **R2, CLI.** [Blender 5.2 arguments](https://docs.blender.org/manual/en/5.2/advanced/command_line/arguments.html) documents `-b`, `-P`, `--python-expr`,
  `--python-exit-code`, autoexec/offline flags, inclusive `-a`/`-f` ranges, `--` argument separation, and Cycles device arguments.
  [Rendering](https://docs.blender.org/manual/en/5.2/advanced/command_line/render.html) requires settings before render triggers.
  The chosen exit 70, driver protocol, cancellation deadlines and manifest checks are Kinewright design choices, not Blender completion guarantees.
- **R3, progress/save hooks.** [5.2 application handlers](https://docs.blender.org/api/5.2/bpy.app.handlers.html) provides `render_write`, `render_stats`,
  `render_complete`, `render_cancel`, and save/undo/depsgraph hooks. They enable observation, not native file integrity or a universal stdout schema.
- **R4, colour.** [5.2 colour spaces](https://docs.blender.org/manual/en/5.2/render/color_management/color_spaces.html) describes scene-linear working spaces
  and linear EXR intermediates. [Displays/views](https://docs.blender.org/manual/en/5.2/render/color_management/displays_views.html) distinguishes Standard,
  AgX, deprecated Filmic, and Save as Render. The CC1/CC8 mappings in §6.5 are proposed Kinewright integration, not upstream equivalence claims.
- **R5, stochastic settings.** [Cycles sampling](https://docs.blender.org/manual/en/5.2/render/cycles/render_settings/sampling.html) documents seed,
  animated seed, adaptive sampling, samples and denoising. Therefore fixed seed alone is insufficient cache identity; CPU/GPU equivalence remains a test question.
- **R6, devices/contention.** [Cycles GPU rendering](https://docs.blender.org/manual/en/5.2/render/cycles/gpu_rendering.html) documents CUDA/OptiX/HIP/oneAPI
  platform/device differences and UI responsiveness problems on shared GPUs. The CPU default/shared-GPU queue policy is this draft's decision.
- **R7, formats.** [Image formats](https://docs.blender.org/manual/en/5.2/files/media/image_formats.html) lists PNG alpha at 8/16 bits and EXR alpha at half/full float;
  [output settings](https://docs.blender.org/manual/en/5.2/render/output/properties/output.html) distinguishes image and multi-layer EXR outputs.
  [FFmpeg codecs](https://ffmpeg.org/ffmpeg-codecs.html#ProRes) documents ProRes 4444 and `alpha_bits`;
  [FFmpeg n8.0 FFV1 encoder](https://github.com/FFmpeg/FFmpeg/blob/n8.0/libavcodec/ffv1enc.c) includes alpha-capable RGB/RGBA formats.
  Existing Kinewright source inspection is in §3; no new codec round trip was run.
- **R8, community bridge.** [blender-mcp](https://github.com/ahujasid/blender-mcp), now presented as mcp-for-blender, documents the Python MCP-to-JSON/TCP bridge,
  arbitrary code execution and unauthenticated add-on socket. Its [licence](https://github.com/ahujasid/mcp-for-blender/blob/main/LICENSE) is MIT-labelled.
  [Safe mode source](https://github.com/ahujasid/mcp-for-blender/blob/main/src/blender_mcp/safe_mode.py) is an optional AST filter.
  No upstream code is copied; an MIT label on the repository does not settle the distribution obligations of `bpy`-using code.
- **R9, extensions/security.** [Creating extensions](https://docs.blender.org/manual/en/5.2/advanced/extensions/getting_started.html) specifies package/version/permission manifests.
  [Add-on guidelines](https://docs.blender.org/manual/en/5.2/advanced/extensions/addons.html) explicitly says Blender cannot prevent third-party add-ons violating internet preferences.
  [Extension licences](https://docs.blender.org/manual/en/5.2/advanced/extensions/licenses.html) requires GPL-3.0-or-later for platform add-ons.
- **R10, Linux enforcement.** [bubblewrap](https://github.com/containers/bubblewrap) and its [launcher source](https://github.com/containers/bubblewrap/blob/main/bubblewrap.c)
  describe namespace construction; security depends on the selected policy. [Landlock](https://docs.kernel.org/userspace-api/landlock.html) documents ABI-specific filesystem/network/IPC restrictions and limits.
  [cgroup-v2](https://docs.kernel.org/admin-guide/cgroup-v2.html) supplies resource/tree control, not a Python sandbox or automatic parent-death cleanup.
  The selected launcher candidate is [bubblewrap 0.13.1](https://github.com/containers/bubblewrap/releases/tag/v0.13.1).
  The fixed policy, syscall filter and kernel tuple remain Kinewright certification work, including absence of user namespaces.
- **R11, Windows enforcement.** [Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects) supplies lifetime/resource control;
  [AppContainer isolation](https://learn.microsoft.com/en-us/windows/win32/secauthz/appcontainer-isolation) supplies distinct file/network/process isolation.
  [Launching AppContainers](https://learn.microsoft.com/en-us/windows/win32/secauthz/implementing-an-appcontainer) describes capability-based setup.
  None establishes that a particular Blender install/GPU/UI works inside one; BL does not claim that result.
- **R12, interchange.** [Blender 5.2 glTF exporter](https://docs.blender.org/manual/en/5.2/addons/scene_gltf2.html) covers cameras, punctual lights, PBR,
  transform/morph/skin animation and export limitations. [glTF 2.0 specification](https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html)
  defines coordinates, metres, texture colour roles and animation. BL3's smaller subset and unsupported-feature refusals are design choices.
- **R13, Rust loader.** [gltf-rs](https://github.com/gltf-rs/gltf) is MIT/Apache-2.0 and loads glTF 2.0;
  [1.4.1 features](https://docs.rs/crate/gltf/1.4.1/features) shows default filesystem/image import dependencies. Disable those defaults for controlled owned-input resolution.
- **R14, renderer alternatives.** [rend3](https://github.com/BVE-Reborn/rend3) is archived/read-only since 2025-06-07.
  Its PBR/graph/asset machinery illustrates that glTF parsing is only one part of rendering. No wgpu-29 compatibility or measured cost is inferred from it.
- **R15, float reader.** [exr 1.74.2 manifest](https://docs.rs/crate/exr/1.74.2/source/Cargo.toml.orig) identifies a native Rust OpenEXR reader/writer,
  BSD-3-Clause licensing and optional parallelism. Candidate selection is not parser-bomb, profile, memory, or colour certification.
- **R16, source closure.** [Packed data](https://docs.blender.org/manual/en/5.2/files/blend/packed_data.html) covers packing resources/linked libraries
  and warns that not every external file can be packed. The complete dependency manifest and owned normalized copy remain necessary.
- **R17, GPL boundary.** [Blender licence](https://www.blender.org/about/license/) and [FAQ](https://www.blender.org/support/faq/) distinguish program output,
  distributed Python API scripts, and external software that exchanges data/runs Blender. Ship the driver and extension as separately labelled
  **GPL-3.0-or-later source packages**, with notices/corresponding source and no bundled runtime. New bridge code is authored under that licence;
  do not relicense copied GPL-3.0-only Kinewright code. Native Kinewright stays **GPL-3.0-only**, communicating through files/local protocol,
  with no `bpy` linking/import. Rendered media/plain scene data do not automatically become GPL; embedded/distributed API scripts carry their own obligations.
  Distributed user-authored API scripts retain the appropriate GPL notice/source; private project use alone is not public distribution.
  Retain third-party model/texture/font licences separately. This follows upstream's stated distribution model, not a claim that all IPC eliminates GPL obligations.

- **R18, privileged attachment and immutable backing.** Microsoft documents that
  [`AttachVirtualDisk`](https://learn.microsoft.com/en-us/windows/win32/api/virtdisk/nf-virtdisk-attachvirtualdisk)
  requires `SE_MANAGE_VOLUME_PRIVILEGE`. Its
  [attachment flags](https://learn.microsoft.com/en-us/windows/win32/api/virtdisk/ne-virtdisk-attach_virtual_disk_flag)
  include read-only/no-drive-letter controls. Neither API supplies an unprivileged NTFS image builder or safe caller authentication.
  Linux [SquashFS](https://www.kernel.org/doc/html/latest/filesystems/squashfs.html) freezes a filesystem tree;
  [fs-verity](https://docs.kernel.org/filesystems/fsverity.html) protects backing bytes, not path identity or parser safety.
  §5.5's signed builder receipt, protected small submission client, fixed roots and recovery protocol are new design requirements.
  No Windows image service is shipped/installed before a certified unprivileged builder exists. The independent consent broker is §5.1.
- **R19, code and alpha.** Blender's [security manual](https://docs.blender.org/manual/en/5.0/advanced/scripting/security.html)
  identifies executable routes beyond the autoexec setting; the [5.2 security reference](https://docs.blender.org/manual/nl/5.2/advanced/scripting/security.html)
  remains a patch-certification input. Exact [5.2.3 Freestyle source](https://raw.githubusercontent.com/blender/blender/v5.2.3/source/blender/freestyle/intern/blender_interface/FRS_freestyle.cpp)
  skips embedded script-mode modules with autoexec off, while editor-mode line sets construct internal Python handlers.
  Do not claim an older bypass still executes on this patch. Keep explicit route inventory/refusal, and make B1-code
  discriminate admission and actual admitted motion rather than relying on a superseded bypass alone.
  [Image alpha settings](https://docs.blender.org/manual/id/4.0/editors/image/image_settings.html) distinguish straight alpha
  from associated alpha's emissive capacity. Coverage-only admission is Kinewright's limitation, not invalid Blender output.

- **R20, subframes.** [`Scene`](https://docs.blender.org/api/5.2/bpy.types.Scene.html) exposes frame/subframe evaluation and
  authored frame settings. This supports §6.4's chosen clock boundary; it does not promise that arbitrary bake formats interpolate.
  Each admitted bake type needs its own actual rendered 24→30 witness.
- **R21, device materials.** Khronos specifies [transmission](https://raw.githubusercontent.com/KhronosGroup/glTF/main/extensions/2.0/Khronos/KHR_materials_transmission/README.md)
  and [IOR](https://raw.githubusercontent.com/KhronosGroup/glTF/main/extensions/2.0/Khronos/KHR_materials_ior/README.md)
  separately from ordinary alpha coverage. §4.3's thin, internally backed glass and split-sum environment are a deliberately
  bounded renderer profile. The external clip-time video binding is Kinewright data, not a glTF core feature.
- **R22, unprivileged builder candidate.** [SquashFS tools releases](https://github.com/plougher/squashfs-tools/releases)
  list 4.7.5. Pin the exact installed `mksquashfs` binary/options and independently validate its output before admission.
  A tool release is not a privileged-parser safety guarantee. No certified unprivileged Windows NTFS/VHDX builder was
  established for this design; manual Windows preparation is consequently retained.

- **R23, Linux person authentication.** Linux-PAM's primary [`pam_authenticate` contract](https://github.com/linux-pam/linux-pam/blob/master/doc/man/pam_authenticate.3.xml)
  and [`pam_acct_mgmt`](https://github.com/linux-pam/linux-pam/blob/master/doc/man/pam_acct_mgmt.3.xml) separate authentication
  from account authorization. Linux's [VT ioctls](https://man7.org/linux/man-pages/man2/ioctl_vt.2.html) provide real-console activation.
  The kernel's [TTY configuration](https://github.com/torvalds/linux/blob/master/drivers/tty/Kconfig) documents legacy TIOCSTI policy.
  Those interfaces alone do not make a trusted prompt. §5.1's root-owned VT, exclusive input/display, fresh PAM authentication,
  nonce/scope binding and injection-negative certification are additional mandatory design requirements.
- **R24, Windows authority.** [`CredUIPromptForWindowsCredentialsW`](https://learn.microsoft.com/en-us/windows/win32/api/wincred/nf-wincred-creduipromptforwindowscredentialsw)
  supports `CREDUIWIN_SECURE_PROMPT`; [`LsaLogonUser`](https://learn.microsoft.com/en-us/windows/win32/api/ntsecapi/nf-ntsecapi-lsalogonuser)
  authenticates credentials and returns a token. The [trusted-path credential policy](https://learn.microsoft.com/en-us/windows/client-management/mdm/policy-csp-admx-credui)
  distinguishes secure-desktop entry from the ordinary session. [CNG key properties](https://github.com/MicrosoftDocs/win32/blob/docs/desktop-src/SecCNG/key-storage-property-identifiers.md)
  include export policy and persistent-key security descriptors. §5.1 combines them with a SYSTEM-owned fixed worker,
  service-only key ACL, protected store and exact pending-request binding. It does not treat CredUI return success or a machine-key label as sufficient authority.
- **R25, one Linux mount transfer.** [`open_tree`](https://man7.org/linux/man-pages/man2/open_tree.2.html) creates detached mount clones;
  [`move_mount`](https://man7.org/linux/man-pages/man2/move_mount.2.html) attaches them. [`setns`](https://man7.org/linux/man-pages/man2/setns.2.html)
  documents mount-namespace entry capability requirements. [bubblewrap's upstream options](https://github.com/containers/bubblewrap/blob/main/bwrap.xml)
  describe namespace/startup synchronization. No claim follows that an unprivileged recipient can rebind the helper's root.
  §5.5 instead keeps the actual transfer privileged and fixes its target to the production blocked child. B1-mount must
  demonstrate the exact ordering, permissions, fds and cleanup on the chosen kernel before this becomes a supported default.
- **R26, callback lifetimes.** Exact 5.2.3 [timer source](https://raw.githubusercontent.com/blender/blender/v5.2.3/source/blender/python/intern/bpy_app_timers.cc)
  exposes register/unregister/is_registered, not global enumeration. Exact [msgbus source](https://raw.githubusercontent.com/blender/blender/v5.2.3/source/blender/python/intern/bpy_msgbus.cc)
  exposes subscribe/publish/clear-by-owner, not a subscription inventory. [Handlers](https://docs.blender.org/api/5.2/bpy.app.handlers.html)
  include persistent hooks. Returning from authoring does not establish quiescence. §5.4's pre-authoring wrappers, minted
  owners and list snapshots are cooperative integrity mechanisms; process termination/reopen provides the old-process
  callback boundary after hostile arbitrary Python. B2-callbacks must demonstrate both paths, never infer them from API reading.

- **R27, float evidence before PNG.** [`Image.save_render`](https://docs.blender.org/api/5.2/bpy.types.Image.html) uses scene render settings.
  Blender's [image-format documentation](https://docs.blender.org/manual/id/latest/files/media/image_formats.html) lists 32-bit float RGBA EXR
  and distinguishes linear intermediates from display formats. These facts permit the proposed diagnostic route; they do not prove
  that a particular configuration preserves every emission value. B1-colour must inspect the actual pre-PNG float pixels,
  certify Combined/association/colour settings, dimensions, the explicit alpha quantizer and byte-exact PNG/evidence
  alpha comparison, and verify the native per-index hash binding before ready publication. Alpha correspondence alone
  cannot establish RGB derivation when two frames have identical masks. Exact 5.2.3
  [`RenderSettings` source](https://raw.githubusercontent.com/blender/blender/v5.2.3/source/blender/makesrna/intern/rna_scene.cc)
  exposes `dither_intensity` with zero in its range. §§6.2/6.5 require zero for the certified PNG path and its request key;
  that setting alone does not prove alpha rounding. B1-colour's real semi-transparent gradient must establish q8.

- **R28, startup protection.** Linux [`execve`](https://man7.org/linux/man-pages/man2/execve.2.html) can reset dumpability;
  [`ptrace` access checks](https://man7.org/linux/man-pages/man2/ptrace.2.html) include real/effective/saved UID/GID and capability
  checks. Thus post-exec non-dumpability is not continuous same-UID startup protection. The dedicated locked service-UID
  client with verified launch before credential delivery is this design's choice, subject to B1-helper's startup controls.
- **R29, manager-started fd delivery.** [`systemd.socket`](https://www.freedesktop.org/software/systemd/man/latest/systemd.socket.html)
  describes AF_UNIX sequential-packet activation and passing listening sockets to matched services;
  [`sd_listen_fds`](https://www.freedesktop.org/software/systemd/man/255/sd_listen_fds.html) describes the listener-fd protocol.
  Linux [`unix`](https://man7.org/linux/man-pages/man7/unix.7.html) specifies `SCM_RIGHTS` descriptor passing and peer credentials.
  These support the chosen private socket-activation plus fixed-fd handoff; they do not authenticate a caller or prove
  the complete writer/validator composition. B1-helper must exercise the actual units, peer/nonce/fd checks and ordinary Studio flow.
- **R30, phase separation.** [`systemd.kill`](https://www.freedesktop.org/software/systemd/man/latest/systemd.kill.html)
  defines whole-cgroup termination. The kernel's [`cgroup.events populated`](https://docs.kernel.org/admin-guide/cgroup-v2.html#un-populated-notification)
  covers live processes in the subtree. Separate UIDs/units, verified writer emptiness/reaping and retained descriptors
  before validator start are additional Kinewright obligations. A clean main-process exit alone establishes neither no
  lingering writers nor permission to issue a receipt. B1-helper includes a lingering-descendant negative control.

Required local reading also includes `.review/PITFALLS.md`, `.review/task-design-critic-common.md`,
[MO0](../../docs/MO0-MOTION-PROGRAMME.md), [MO1](../../docs/MO1-KEYFRAMES-TRANSFORM-STILLS.md),
[MO2](../../docs/MO2-BLEND-ADJUSTMENT-TRANSITIONS-SOLIDS.md), [PF1](../../docs/PF1-PLAYBACK-PERFORMANCE.md),
[CC1](../../docs/CC1-MANAGED-SDR-PRIMARY.md), [CC4](../../docs/CC4-LOOK-MANAGEMENT.md),
[CC8](../../docs/CC8-FLEXIBLE-COLOUR-AND-YOUTUBE-HDR.md), [DESIGN](../../docs/DESIGN.md),
and [JA-1](../../docs/JA1-STUDIO-ORCHESTRATION.md). Their older status/ordering statements are historical where later decisions or inspected symbols supersede them.
