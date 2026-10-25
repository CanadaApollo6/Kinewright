# JA-1: Studio, versions, pointing and orchestration v1

Status: **design accepted 2026-10-20** (revision 3). Implementation has not started. Builds,
provider probes, renders, fault injections and measurements are NOT RUN. Windows durable writes
(G1-W) are a Windows release blocker; Linux ships first.

Part of job A in the [product brief](PRODUCT-BRIEF-2026-10.md#programme-mapping). This is
data-loss work: each implementation stage gets two reviews (Astra and Muse). JA-2
([person separation](JA2-PERSON-SEPARATION.md)) consumes this host's store ownership.

Review trail, under `target/review/ja/`:
- Astra critic: `ja1-studio-orchestration-critic.md` (revise; 2 Blocker, 8 Major, 2 Minor).
- Round 2 on rev2: `ja1-studio-orchestration-critic-r2.md` (revise; three new Major).
- Narrow round 3 on rev3: `ja1-studio-orchestration-critic-r3.md` (accept).
- Lead verification: `ja1-lead-verification.md`. Lead decisions: `task-ja1-rev2.md`, `task-ja1-rev3.md`.

Lead amendment at promotion: the loudness row in the reviewer coverage matrix reuses the
existing export meter and the shipped −14 LUFS streaming/social profile, not a new meter at
−16 LUFS (from the JA-3 scoping note, `ja3-editorial-scoping-rev1.md` §8).

The Songbird input contract is MockingBoard's shipped `mockingboard.songbird-answer` v1 export
(MockingBoard PR #309). Bare file names below refer to `target/review/ja/`.

## 1. Outcome, scope and ownership decision

Riel brings a C920 recording, QuadCast audio, script with optional bracket cues, Songbird data and reference material. Studio returns a complete cut,
independently reviewed before delivery. Riel watches, points at changes and receives a revised cut without learning a timeline. The target is 30–60 minutes of
Riel's own time; agent work may take hours. Nothing on screen may invent a fact. These are requirements, not measured outcomes: `docs/PRODUCT-BRIEF-2026-10.md`,
“What done means”, “How it works” and “The first video”.

**Choose the narrow run/commit host.** It owns agent sessions, candidate/specialist Cores and every project-file write. The sole editing GUI keeps its existing
manual Core under a fenced branch lease. It previews local valid edits immediately and sends ordered history/checkpoints to the host; those edits stay visibly
pending until durable acknowledgment. Other GUIs view and comment only. Closing a GUI transfers no agent authority. Rules and witnesses: §§4–7; G1–G6, G20.

This is smaller than moving every manual command behind IPC and preserves existing live-control behavior. A whole-editor host is a contingency only if **G5-L**,
the fenced local-Core/history-equivalence gate, demonstrates an irreducible failure to preserve validated ordered edits, undo and lease exclusion. G20 latency
failure first requires repairing the narrow path; it is not automatic permission for a bigger host. A change of ownership requires a lead ruling with the
failing witness. No gate currently rules the narrow option out; none was executed here.

Keep the critic's supported choices: conversation plus live picture, ordinary editable edits, immutable delivered versions, original feedback evidence,
sequential editing specialists, PF1 interactive playback separate from review renders, server-enforced permissions, durable receipts and no paid-provider
fallback. There is one user and machine; v1 admits one active Studio execution owner/run across that machine and refuses a second active run (§8.2).
No generic remote editor, multi-user service, concurrent timeline merge engine, OT or CRDT is needed.

JA-1 owns Studio, feedback, durable run/version records, orchestration, fact-binding admission and delivery predicates. It does not implement person separation,
face/look algorithms, take selection, chart construction or new creative DSP. JA-2 owns separation; lead-owned **JA-3** scopes the editorial work in §9. AW2
Part 1 and MO6 supply their accepted graphics/title contracts. No bundled Node/Python/browser runtime, uploads, AI video or AI images. Installed tools remain
external. GPL-3.0-only applies; no Tesseract plugin/skill material was read or used.

## 2. Existing foundation and the actual implementation boundary

| Foundation, inspected on this source | Design consequence for JA-1 (inferred) |
| --- | --- |
| `app.rs:3351–3433` in `crates/kinewright-app/src/` has bottom timeline/transcript/mixer, left rails, right preview/inspector and center chat; `docs/M24-CONVERSATION-FIRST.md`, “Layout” | Reuse those widgets; change the default layout, not the editing engine. October's brief permits hiding the timeline. |
| `chat_ui.rs:456–476,526–580` owns isolated branches, servers and sessions in GUI threads | Move run/session ownership to UI-free agent/project code. This is new lifecycle work. |
| `crates/kinewright-core/src/actor.rs:45–80,386–394` has coalesced commands and current undo-stack operations; `crates/kinewright-agent/src/branch.rs:206–280` merges at a runtime base | Reuse Core operations and atomic batches. Add durable history/base binding; M31 is not already persistent. |
| `crates/kinewright-project/src/lib.rs:1–37` exports shared IO/locks; `headless.rs:30–49` makes lock use transitional | Shared primitives have landed. Production CLI, authenticated attachment, GUI-wide writer exclusion and handover are not established by those exports. |
| `project.rs:235–249,406–412,478–503` has a document-only envelope/serializer and a rename-error in-place fallback | A bundle-aware strict writer is new work; generic save must refuse to strip Studio state. |
| `lockfile.rs:267–322` verifies spelling/lock-object identity; AW1 §0.1, S4-OBL/S5-OBL names remaining races | Reuse its never-unlinked lock/discovery design; close path-use and journal-writer gaps explicitly in §5. |
| `crates/kinewright-agent/src/server.rs:1206–1299,1714–1761` gates dispatcher/plans/destructive operations | Existing investigator restrictions are narrower than specialist field/track/bus scopes. Extend at these boundaries. |
| `crates/kinewright-core/src/qa.rs:58` accepts a document; M31 “Tradeoffs and future work” defers broader QA | Structural QA is not rendered-output QA or listening. Required new checks are in §10. |

Paths abbreviated in table rows retain the preceding crate directory. These are source observations, not fresh runtime proof. Older `docs/agent-harnesses.md`
language about a shared live Core does not override the isolated constructor above. AW1 §§4a–6 describes planned live/proxy/auth behavior, not inherited JA-1
acceptance. JA-1 retains its Bearer auth, roots, confirmation and lock requirements while replacing recurring GUI/headless authority handover with attachment.
AW0 §11's externally owned orchestration is superseded for this bounded workflow by the October product brief. Job A precedes B–E; H-T talk holds belong with A:
`docs/ROADMAP-AND-WORKFLOWS.md`, “Job-driven order”, at 892–910.

## 3. The person's workflow and app surface

1. **Drop files.** Import recordings, script, supplied exports, brand files and references. Show owned-storage estimate, material readiness and one short plan:
   select takes, tighten the talk, add charts/cards/captions, improve look/audio, review and export. Longer videos always get a plan. Prefer existing project
   reference/brand material.

2. **Let it work.** Show the last delivered cut; before one exists, show imported footage. Draft preview is available on request and labelled unfinished. Show
   current/completed/queued tasks, assumptions, waits and review progress.

3. **Watch a complete cut.** A ready card names the version, duration, changes, review coverage and preserved assumptions. Play the edit through PF1
   immediately; a background proxy is optional. No per-change export is needed to watch edits.

4. **Point and revise.** Pause/type, draw or record a voice note. Save comments without interrupting agents. **Revise cut** sends all saved selected comments as
   one batch, with an option to leave some for later. Return a new version and dispositions.

5. **Prepare publish kit.** Produce selected video files, optional 2–3 reels, title options, description, chapters and vector/code thumbnail candidates. Every
   derivative has its own applicable checks. Riel uploads outside Kinewright.

At desktop width, chat and its pinned composer sit left, a large player right, with versions and comments below it. At narrow width the two lists become
labelled tabs below the player. The top bar names project, selected cut/working copy, save state and run state, with **Open Editor**. Reuse preview, transport,
chat, confirmation cards and editor widgets; replace the default thread rail with versions. Specialist conversations and raw tool events remain in run details.
Editor reveals timeline, transcript, mixer, colour/inspector and media rail in one click; return preserves selection, frame and feedback draft. A delivered cut
opens with **Edit a copy**; during a run manual work stays separate by default. Use Cut Room tokens and quiet state indications (`docs/DESIGN.md`, “Direction”,
“Application frame”, “Agent panel”).

Distinguish Delivered, Working copy, Reviewing, Waiting for files, Rate limited, Paused, Interrupted and Unsaved. Stop chat auto-scroll while Riel reads earlier
material. Keyboard controls cover transport, frame steps, comments and A/B. Drawing has a text/object-selection alternative. Seek links always carry version
identity. Manual pending edits show their pending state next to the picture; **Saved** means the durable receipt, not a successful Core event.

Defaults, visible in the plan/settings: last finished cut in the player; comments batched on Revise; manual work separate; retain delivered versions; existing
references/brand first; configured subscriptions with independent reviewer. Use a conservative initial owned-media allowance of 20 GiB, also reserving space for
the next commit/review render; show actual free-space and copy estimates. Pause before exceeding it, never silently delete versions. Expansion is requested only
if it actually blocks. Account/login/limit controls live in settings, not an intake interview. At most **one batched intake prompt**, only for indispensable
missing files/reference material. Recovery and ambiguous feedback mapping remain agent work wherever evidence permits; unavailable prerequisites produce a clear
paused state.

## 4. Domain model, history and durable branch identities

### 4.1 Project records and one coordinated v3 migration

Core keeps ordinary `Document`/`Operation` editing semantics (`crates/kinewright-core/src/model.rs:1438–1479`). Studio metadata lives in the project envelope
and immutable objects; pixels, samples, credentials and chat transcripts do not become Document fields. Project IO owns persistence; agent owns
scheduling/sessions; app owns local interaction/PF1.

Studio joins **one coordinated format v3 migration** with AW2, JA-2 and MO6, owned by the lead. The landed maximum is 2 (`model.rs:1254–1263`); AW2 R13 already
specifies 3 and one minimum-required predicate. No separate Studio file-format number is allocated. Add these proposed envelope fields under `studio`:
`project_id`, `generation`, `creation_brief_ref`, `manual_branch_id`, `manual_switch_generation`, `candidate_branch_ids`, `branch_index_ref`,
`version_index_ref`, `feedback_index_ref`, `run_index_ref`, `claim_index_ref`, `input_manifest_ref`, `receipt_index_ref`, `issuer_state_ref`, and
`store_manifest_ref`. References include hash, size and record kind.

Referenced records hold script/cue revisions, immutable versions, feedback/batches/resolutions, plans/assumptions/usage, task scopes/attempts,
histories/lineage, review artifacts/coverage and claim bindings. Record-kind tags are validation metadata within v3, not a rival project-version predicate.
Nonempty Studio state, including parked/recoverable branches, requires v3; union that condition with AW2/JA-2/MO6 conditions in the shared bundle-aware
predicate. Otherwise preserve the current v1/v2 predicate and byte behavior (`crates/kinewright-project/src/project.rs:372–412`). The flattened ordinary
Document equals the acknowledged active manual branch checkpoint, verified at load/publication. Candidate delivery never replaces it. Adoption updates that
pointer and flattened document atomically in the next root. Load legacy projects with empty Studio defaults and no eager write. Activation durably preserves a
verified pre-migration copy before publishing v3. Migration is deterministic/restartable; it never changes the only original in place. Every Studio save,
recovery, headless save and Save As uses the bundle API. Document-only writers refuse Studio input. Unknown required fields/kinds disable writes and resume;
preserve original bytes. An explicitly labelled edit-only derivative may omit unsupported history, but generic Save As must never pretend to retain unknown
records.

### 4.2 History and merge base are separate durable contracts

Each branch checkpoint contains `branch_id`, `head_id`, head document hash, baseline document ref, ordered validated batches, cursor/redo order,
gesture/coalescing boundaries and pre-gesture baselines, applied-operations digest and lineage ref. It also identifies its ordered checkpoint generation,
history-floor generation and exact command-stream cut. Specialist branches also contain **target branch ID, exact base HeadId and base
document hash**, task/attempt/scope and the full merge delta. Runtime revision numbers are never persisted authority: M30 “Core contract” resets revisions on
reopen.

A narrow Core history export/restore API validates batches through existing operation semantics and preserves cursor, redo order, gesture grouping and
pre-gesture baselines. For manual recovery it preserves the recorded open-gesture state until the shared recovery barrier in §5.1 has reconciled the accepted
suffix; it never closes that gesture before replay. Use exact `JournalCommand` semantics, including coalesce keys and Undo/Redo boundaries
(`crates/kinewright-core/src/journal.rs:14–35`; `actor.rs:316–374`). After equivalence is established, closing prevents future continuation without changing
already-accepted batches or their undo target. Check document, history and applied-operation hashes, not just pixels. The API serializes no actor/channel
internals and creates no second operation interpreter. Asset checks occur before replay; replay cannot execute filesystem effects. Corrupt history quarantines
that branch without rewriting usable versions.

Restore the candidate first and bind `(branch_id, HeadId, document_hash)` to its fresh runtime revision. A specialist may obtain a new runtime base only when
its persisted target/base identities exactly match that candidate. Reconstruct its base/delta with the validated history API; never reuse the old numeric
revision or substitute the latest one. A new HeadId conflicts even if bytes or runtime counters match, including edit→undo returning the same document. Owner
validation of that relationship precedes M31's atomic `DoBatchIfRevision`. G2/G13 distinguish this from ordinary M31 stale checks.

Manual undo can advance its floor only through an **ordered history-floor checkpoint** shared by the GUI and validation shadow. Freeze all manual producers,
reconcile/drain the accepted chain (§5.1), and agree one command-stream cut. The checkpoint names the preceding checkpoint generation, next floor generation,
cut command ID, old/new baseline refs, retained batches/cursor/redo, lineage and gesture state, with a resulting history digest. Publish it through B1–B5; both
Cores install that exact checkpoint through the history API and confirm equivalence before any further Undo/Redo or new input. A lost reply reconciles that
same checkpoint, never independently chooses another floor. Recovery applies recorded checkpoints at their cuts before subsequent commands. Never cut away an
open gesture's pre-gesture baseline; retain it or close the fully reconciled gesture first. Reserve transition space; pause before admission if it cannot fit.
The earliest undo point is visible. An active specialist's base and full applied merge delta cannot be pruned with that floor; pause the task if its allowance
fills. Every acknowledged undo/redo/gesture or floor update gets a durable HeadId, even when document bytes are unchanged. Merge proposal
capture revokes the task, drains admitted commands, then reads head/history/lineage while quiescent (`branch.rs:211–245` currently queries head and operations
separately). An accepted full proposal becomes one candidate undo entry and one receipt, with lineage in the same durable commit.
A1 rescue-selection floors belong only to B's fresh actor and preserve this ordered equivalence and gesture baseline. Later reconciliation may settle only
that same attempt's already-selected ticket on the live, unsealed Core under A1 §5; it cannot impose another floor or restore/install history.

### 4.3 Versions, adoption and inputs

A delivered `VersionId` names immutable document/head, owned input manifest, script/data/style revisions, parent version, run provenance and exact
review/coverage receipt. Renaming/pinning is a separate annotation. Unreviewed checkpoints stay in run details; they are not Delivered. M31 variants are
aspect/reframe plans, not version IDs. Undo never removes a delivered cut or retargets comments (`docs/M31-BRANCHES-VARIANTS-VERIFICATION.md`, “Branch data
flow”, “Delivery variants”). **Use this version as my working edit** uses the fenced switch in §5.1: preserve the old branch/history and create a fresh working
branch from the version. Returning to a retained working branch uses the same switch without creating another branch. This avoids promising a nonexistent
whole-document undo command (`actor.rs:45–80`). The committed switch identity, restored GUI Core and fresh branch-bound lease must agree before editing resumes;
selection alone never changes writer authority.

Immutability includes decoder-bound owned recordings, generated assets, fonts, LUTs, styles, data and reference/proof bytes. Hash-at-intake alone is
insufficient. Copy/clone under the owner, verify full contents and bind the actual decoder opens to the sealed input manifest/owned handles. Missing or changed
inputs mark a cut damaged; relink creates successor evidence. This is a required owned-input integration dependency, not a claim that PF1's A-1 gap is closed:
`docs/PERFORMANCE.md`, “PF1-LH R97 checkpoint”. Do not wait for unrelated PF1-H work or call H-T passed.

## 5. Writer authority, manual GUI and Save As

C7 binds the legacy GUI and headless writer to one resolved project identity before
mutation. Its shared lock capability retains the native parent and remains alive in
journal recorders and queued sidecar jobs. Project, incident sidecar, LUT copy and
journal IO use admitted directory handles; caller aliases are never resolved again
after verification. Legacy permission/sharing fallback semantics remain confined to
legacy IO. Studio still uses its existing strict bundle publisher.

Every named GUI journal create, append, flush, checkpoint and retirement needs the
same real project authority. Startup restore/discard also takes that authority and
uses the journal file profile. Pending decisions retain their scanned native directory
and exact byte digest; changed bytes require a new decision. The active writer executes
post-restore retirement using that retained identity. Unreplayed data survives a headless save. The final recovery
probe includes allocator suffixes, and release checks the original claim plus lock
file identity before removing discovery. Missing discovery never permits takeover
of a held lock. C7 supplies local participants only; host activation and transport
remain C8. Native Windows Studio durability remains unsupported.

### C8 process ownership and retention

The originating GUI owns its temporary root, `SessionStore`, source `ProjectStore`,
`SourceCopyHold`s, complete-closure `StorePin`s, reader leases, and source reservations.
That physical source store has one `StorageUsage` owner in the GUI. These actual
objects remain there through focus changes, attachment changes, and host replacement.
Host source descriptors authorize bounded verified reads through the authenticated
attachment. They carry no cleanup, unpin, reservation-release, or Close authority.
Dropping a host source handle, disconnecting, orderly host exit, and panic unwinding
cannot delete or uncharge the GUI source. An accounting snapshot cannot retire it.

The host owns each named destination's `ProjectBundle`, namespace, lock, ancestor
claim, store, pins, and accounting. Each named physical store has one owner and ledger.
A named-source hold stays with that host store. Changing writer authority fences and
drains the previous writer, preserves the actual source holds, and validates the exact
checkpoint after the destination lock handover. No source ledger moves across IPC.
The GUI retains its manual and sealed actors. The host owns its agent sessions and
the one validation shadow. GUI detach closes only the attachment.
The private GUI bootstrap carries its exact frozen history snapshot. Before serving
sessions, the host validates that snapshot and its admitted source overlap, then seeds
the shadow from it, including unsaved history and Redo. This initialization publishes
nothing and cannot mark edits Saved; only the publisher's actual result does that.

When a replacement cannot re-establish exact retained ownership, every save and rescue
request refuses, including first Save and Save As to fresh destinations. A session ID,
remote handle, PID, or reacquired writer lock is insufficient evidence. The surviving
GUI freezes producers and preserves all histories, temporary bytes, pins, and charges.
Queries and history export remain available. The person sees: "The host could not
re-establish this session's retained data. Your edits remain here. Saving and rescue
are paused, including saving elsewhere. Close this editing context explicitly to end it."

Explicit context Close fences requests and drains readers. Only the owning process
retires its actual holds through the existing collector and namespace cleanup. Repeated
Close, a lost reply, or reader retirement releases each pin and reservation at most once.
Failed cleanup stays charged and retryable with that owner. Host Drop never means GUI
Close. A headless-only context has its originating source owner in the host and has no
surviving GUI after host death. C8 and C12 reports separate kill, orderly-exit, and caught
panic-unwind evidence from native Windows and physical-storage acceptance.

### 5.1 A fenced local manual Core

The host holds the project OS lock and pinned store/root handles. Acquire it on Studio activation before authorizing any writer. A legacy GUI checkpoints
unsaved state/history, stops journal/sidecar writers and pauses edits for this one-time activation. The host acquires and rereads the exact checkpoint after
release; on contention neither writes around the winner. Failed activation reacquires/revalidates before legacy editing can resume. Subsequent opens simply
attach.

One host-issued manual lease binds client, active branch, manual switch generation, owner epoch and lease generation. The GUI's manual Core remains the sole
live manual executor; the host retains a durable checkpoint plus **one validation shadow for that manual attachment only**, not another manual author or
shadows for parked branches. During a fenced transition that shadow is quiescent; replace/release it when attachment changes. Each Core-accepted command
produces an ordered history message with stable batch ID, predecessor, command kind, operations, gesture boundary, cursor, resulting hash and lineage. The host validates the expected durable/pending
chain using Core, then persists it. A received snapshot alone is not permission to replace history. Corrupt/skipped sequence refuses atomically.

Keep `DoBatchCoalesced` per UI frame and the pre-gesture undo target (`actor.rs:49–62`; `app.rs:2461–2490`). Local PF1 sees valid pending results immediately,
independent of disk latency. Pending command bytes/count are bounded (§12); when full, stop accepting more edits with visible Unsaved state. Never drop
intermediate accepted history messages or replace a gesture with just its last operation. Disk batching can acknowledge a contiguous prefix in one root commit.
After host failure enter the shared recovery barrier below. Only acknowledged history is guaranteed after a simultaneous GUI/host crash. Normal close drains
pending history before exit; forced close can lose unacknowledged edits and must never have displayed them as saved.

**One recovery barrier for the local Core and shadow.** Freeze every manual producer: live-control sends, shortcuts, Undo/Redo, queued actions and any authorized
live-editor endpoint. Settle already-submitted Core sends at an actor barrier, then drain accepted events to an identified final accepted command ID/cut;
no in-flight send may appear after that cut. Preserve pending command envelopes, IDs/digests, predecessors and gesture metadata unchanged until the barrier
completes. Fence the old lease. The successor host reconciles each ID through §6.2 and establishes
the exact durable prefix, including committed replies the GUI lost. A `retired_unresolved` or uncertain outcome keeps both sides frozen; document equality is
not proof of application.

Restore the acknowledged checkpoint with its original gesture grouping and pre-gesture baseline **still intact**. Replay only the proven-absent suffix,
including its original `JournalCommand::DoBatchCoalesced`, Undo and Redo sequence, under fresh host authority limited to that frozen chain. Reauthorization
references the old ID/digest without altering the preserved original command; its receipt durably links that original identity to the new result in the same
root, so another host crash reconciles the link instead of reauthorizing twice. No general editing lease is issued yet. Committed suffix commands are
reconciled, not reapplied. Restore/apply ordered floor checkpoints at their recorded cuts. Compare both sides' baseline refs, exact batches/grouping, cursor/redo order,
lineage, floor/checkpoint generations, gesture boundaries, resulting document and applied-operation digests. Only then record and durably publish the shared
recovery-close checkpoint, install it on both sides and grant a fresh lease for that exact branch/checkpoint/switch generation. Closure stops later input from
continuing an old gesture; it does not split or reinterpret already-accepted work. With no surviving GUI, reconcile the acknowledged prefix alone through the
same barrier. If the recovery-close checkpoint already committed but its reply was lost, resolve its recorded accepted cut and restore that same result;
do not close/replay an earlier prefix again. History-equivalence digests exclude fresh runtime counters/authority, not recorded gesture or floor semantics.
Any mismatch pauses with pending evidence intact; it never becomes a snapshot overwrite.

Lease transfer uses that same freeze, final-prefix and restore barrier. If the old GUI is gone, use its last acknowledged state; a surviving GUI's accepted
suffix is resolved before transfer. Old requests can reconcile but cannot mutate after fencing. The prior GUI becomes a viewer and cannot publish its local
Core. All other GUIs view/comment; each viewer's comments go through the host. Agents never write the manual branch. An explicitly authorized live-editor
endpoint, if provided for AW1, queues operations through the lease holder and the same history path; it pauses without that holder. Specialist endpoints cannot
reach it. Closing the GUI moves no agent Core or session.

**Fenced adoption/return switch.** Reuse that attachment barrier and the single-writer commit path, without a distributed transaction or whole-document Undo:

1. Freeze all old-Core producers, drain and reconcile an identified final accepted prefix, and fence its lease generation. Validate exact old branch, HeadId,
   checkpoint/floor generation and switch generation. A racing change conflicts; merely draining once without freezing is insufficient.
2. In one B1–B5 root publication retain the old branch/checkpoint/history, create the adopted version's fresh branch/head (or select the exact retained target
   branch/checkpoint), increment `manual_switch_generation`, and publish the active-manual pointer, matching flattened Document and digest-bound switch receipt.
   The receipt contains old and new branch IDs, HeadIds and checkpoint refs/hashes, target VersionId when adopting, and resulting switch/root generations.
   Branch creation belongs to this command's single result; retrying it cannot create another branch.
3. Restore the GUI Core/history, lineage and owned resource/input context from that committed result, check its checkpoint digest and current active pointer,
   then acquire fresh authority for that exact branch and switch generation. No old-Core execution or new edit is allowed during restore. Other GUI selection
   never grants authority. A late old-lease command refuses, including after a later return to the old branch.
4. A provable pre-publication failure leaves the old branch authoritative, but resuming it still requires restore/revalidation and a fresh lease. An uncertain
   publication freezes both sides until receipt/root reconciliation determines the committed switch. Lose the reply or crash either participant before/after
   GUI restore: retry the original envelope to recover its original result, not another adoption or revived lease. Lease acquisition is a separate current-
   authority step. If a later switch already superseded that receipt, attach to the current committed pointer; never replay the historical switch as selection.

Returning to the retained working branch follows all four steps and restores its exact undo/redo history. No branch is editable until its committed checkpoint,
GUI state and fresh lease agree.

### 5.2 AW1 obligations and identity-bound IO

Carry **S4-OBL** from `docs/AW1-HEADLESS-KINEWRIGHT.md:298–307` explicitly: every production save derives its project, sidecar and journal destination from the
lock's single resolved identity; no caller path is re-resolved after verify. Check spelling and lock-object ownership at admission, then use pinned
directory/root handles through publication. Release removes discovery only if both lock-object identity and claim still match. Probe allocator-suffixed
`-N.journal` names as well as base/legacy names. A missing discovery with a held lock means contention, never a second owner. Carry **S5-OBL** at AW1:293–297:
every GUI recovery/journal create, rename, flush and retire holds that identity's lock; port `defect_journal_appearing_after_the_scan_is_missed` to the real GUI
journal path. A journal cannot appear outside writer ownership after the scan. Unreplayed recovery journals survive headless saves; recovery itself runs under
the owner.

Linux uses opened directory-relative operations with no-follow admission and immutable-object no-replace publication. Windows pins verified directory ancestors
without delete sharing, rejects reparse points and uses their pinned namespace for replacement; never traverse the caller alias again. Do not deny the root
replacement's own required sharing while pinning its parents. Hard-linked ambiguous project roots are refused until a single canonical ownership identity can be
proved; aliases resolving to the same root attach to the same host. Path swapping may refuse or remain bound to the original claimed target; it may never
redirect writes into another project. G3/G4 assert referenced asset bytes too.
A1 rescue retains these identity-bound, pinned-IO obligations through admission, publication and cleanup.

### 5.3 Save As, portable paths and recovery

Freeze and pin source generation/history under its owner; pause new mutations. Resolve destination identity read-only. If it aliases this same project, return a
normal save/no-op, not a copy or second lock. Otherwise **try-acquire destination authority before touching destination directories/assets/sidecars**. Lock-file
creation is the authority claim itself; it is not permission to change an already-owned store. Contention returns immediately, with destination bytes unchanged.
Never block acquiring a destination while holding the source: use try-lock, release any partial destination claim on failure, retain the source; opposing Save
As requests cannot deadlock. Missing parent creation requires a claim anchored in the existing pinned parent before creation. Do not overwrite an unowned or
nonempty foreign asset directory implicitly.

With both authorities held, copy required objects exclusively to destination staging; verify and durably publish names and ancestors via §6. Preserve every
object reachable from the destination's prior root. Publish the new destination root last, with its own durable previous-root checkpoint. Interrupted copies
leave source/prior destination exact; leftovers are unreachable and reclaimed only under destination authority. Successful copies get a new ProjectId with
historical origin IDs; copied runs are paused, carry no live lease/endpoint/credentials, and never duplicate source tasks.

A1 rescue permits only its separately consented selected-history trim and unavailable-source-recovery omission, in A1 §4's order, with the complete closure
recomputed and verified. B must be separate by native path and pinned namespace identity from every retained destination and the temporary root, including
exclusion of an absent/changed/unproved mount-parent subtree before any creation. The originating `Ja1EditingSession` retains rescue sources, authorities,
pins, charges and temporary bytes until explicit Close/exit, across switches, reconciliation and reader retirement. Only immediate verified success and a
validated handoff attach editing to B's fresh Core/authority and seal A's actor. Unknown B resumes the original dirty edit; a late B commit never switches
editing. Ordinary same-project aliases remain normal-save/no-op; rescue aliases refuse. See S0 §4.3 and the A1 design record.

Store paths as validated relative components and native `PathBuf` at IO boundaries. Preserve case/Unicode; refuse traversal, separators/device/drive prefixes,
Windows reserved names and destination case collisions. Non-UTF-8 external names import under hash names with display labels. Absolute external locators are
hints, never portable identities. Verify a relink instead of substituting slash/drive strings. Corrupt roots/objects are preserved; recovery validates the
complete graph and selects a verified prior root without rewriting the damaged original. No recovery question when evidence determines the answer. Network
filesystems and concurrent machines are outside v1's supported writer profile.

### 5.4 Local recovery selection and rescue contracts (S2 revision 7)

This section supersedes the earlier P, exact pre-floor copy and editable paused-state
rulings. It is binding for the project API, local lifecycle, App and later host. C4
implements record, restore and already-saved selection operations. C5 composes the
ordinary Save, rescue lifecycle and retirement. C6 connects the App. Host receipts and
leases follow in C11. None of these contracts depends on C7+ to make local recovery work.

The Saved baseline is the exact history identity returned by the successful publisher,
including its ordinary floor result. First Save, ordinary Save, Save As and selection
must never recapture a live actor after IO to establish that baseline. An accepted
post-cut suffix remains dirty and Close warns until its own publication succeeds.
Unchanged Studio dirty polls compare an authoritative actor/history-change identity;
they must not export, serialize or hash retained history on every UI poll. Undo, Redo,
gestures, restores and floor commits invalidate the history cache.
Accepted empty Undo/Redo commands count as history changes even when the document revision
is unchanged.

Backend admission refuses reopening the manual branch as a durable session. Existing
working-branch selection refuses registered session/base dependencies both at prepare
and immediately before publication. A refused retirement preserves the complete original
branch/evidence vectors and releases any new pending pins before a later Save. Stage all
replacement rescue records before changing the evidence vector; a successful explicit
R_ack retirement changes only that edge and its required retirement reason.

Let O be the current actor, L its ordinary working branch and g the committed switch
generation. Prepare a checkpoint target as exactly one fresh working branch R. An
existing-branch target retains its BranchId and expected head. Freeze all producers
inside O, settle submitted sends and drain accepted events to one cut. Keep editing
read-only until the outcome permits resumption. Queries and history export remain
available. Keyboard, queued actions, Undo, Redo, live controls, alternate handles, agents
and the validation shadow cannot accept post-cut edits.

Before any preliminary Save, verify the chosen root/checkpoint ObjectRef, SHA, length,
kind, payload identities and complete content closure. Take an ordinary in-session
StorePin over that closure, with bounded verification and metadata admission. If it
cannot fit, refuse before either publication, preserving O, association, history and
Saved/dirty exactly. The pin is no byte copy and cannot recover unavailable bytes.
Resolve by that verified identity, never by automatic-list position or continued list
membership. Keep the same target, pin and original R ID through rotation, collection,
unknown outcomes, quota refusal and the offered retire-then-retry route. Duplicate
replies cannot create another R or increment g again.

If O is dirty, ordinary Save publishes O onto L first. Use the ordinary executor,
select_history, exact-ticket transition and normal history-limit disclosure. Only
that Save's legal oldest-first floor may remove Undo. Preserve current content,
complete Redo, the open gesture baseline/key, surviving batch/input identities and
independent roots. The bound owner may settle this exact frozen O/L/cut Save's own
ticket. That permission is not a direct-handle or general frozen-state bypass. A clean
O skips Save only after exact durable association/checkpoint validation. No extra
pre-recovery work point or durable initial-R pin is created.

After definite Save success, compare the accepted cut and its expected post-Save
history/floor identity. The validated floor may advance runtime revision without new
accepted input. Prepare all fallible target Core, inputs and resource attachment work
before publication or seal. A suffix, source-head/root/authority conflict or missing or
changed existing target refuses before selection, keeping every suffix edit intact
and ordinary saving usable. An existing target must still be indexed at its expected
head immediately before publication and on retry. Its pin cannot resurrect a removed
BranchId.

One B0-B5 selection transaction retains saved L as an ordinary branch, installs the
verified complete target history, batches, lineage and inputs, selects R, increments g
once with checked overflow and writes the matching flattened Document/input context.
Do not use document-only fork for checkpoint recovery. Refresh the writer's manual
pointer, generation, branch index, inputs and inventory on definite success and on
both committed and absent reconciliation. Seal O at its exact cut and activate the
prepared target before unfreezing it. Publication or protocol invariants stay frozen;
there is no editable durable-no-activation mode. A post-publication activation diagnostic
retains the committed phase and freeze for exact reconciliation; it is not a definite
selection refusal. Selection alone never fabricates a
Save acknowledgment. Return R to existing L uses the same freeze, ordinary Save if
dirty and one selection, without creating a replacement L.

Ordinary Save failure or definite refusal ends the selection attempt without a
selection publication. Release only its uncommitted exact ticket and resume O intact.
Save unknown keeps O frozen. Proved absence releases that ticket losslessly and resumes
unchanged. Only proved commit plus exact-ticket settlement permits selection. Selection
failure/refusal resumes O on the definitely saved L result, retaining the normal floor
disclosure. Selection unknown stays frozen. Proven absence refreshes L/g before resume;
proven commit refreshes R/g+1, seals O and activates the original preparation before
resume. Reconciliation grants no write authority. A genuine later selection gets a new
operation; retries use the original preparation and target.

All selection candidates inherit §6.1's fresh-open validation under the same limits as
an independent reader. Cache residency gives no authority. No next-save shadow allowance,
successor headroom, exact-retention selector bypass or additional ledger is introduced.
Every later ordinary Save can floor or refuse under its own current limits.

Shared history keeps the identical (branch_origin, creation_batch_id, result_ordinal),
batch IDs and complete split ancestry across L and R. Compare baseline, current, Undo
and Redo. Moving an occurrence into a floor baseline does not create it again. Seed the
actual current R checkpoint with the selected identities before R's first Save. L's
preliminary Save is its authoritative baseline for later saves. Match retained batches
by accepted identity and floor correspondence, never the position of an unrelated
redo-truncated batch. Mint once for genuinely new events, with their accepting branch's
origin. Ordinary branch and specialist saves follow the same rule.

Format 3 remains unchanged. Derive manual-switch@1 directly from raw nonzero
manual_switch_generation, and rescue-copy@1 from recognized rescue attachments and
records. Cross-check actual requirements against declarations and the reader's supported
subset before caller mutation. Generic OwnedEvidence support is insufficient. Reject
missing, mismatched, unknown role/schema/API declarations or invalid checkpoint edges.
Unchanged generation-zero v3 roots remain supported without either new requirement.
Keep all-nonmanual candidate_branch_ids, the two-predecessor validator/count/rotation,
and full RootCheckpoint content-edge validation.

RescueCopyV1 is a recognized OwnedEvidence role/schema in durable_pins. Its optional
acknowledged_root is a dedicated RootCheckpoint content edge outside RecoveryInventory,
with complete verified B-owned closure. Default selection preserves it and both selected
automatic predecessors. Record unsaved-source absence or exact separately consented
unavailable-root omissions. Available recovery roots are never omittable for quota.
Evaluate full-history omission alone first, and offer no unnecessary trim. Only otherwise
search legal oldest-first prefixes. Trim and omission have separate exact consents;
recompute blockers and the complete retained union after each choice. No archival source
manifest or ordinary copy may reintroduce an omitted root. Independent evidence/receipts,
current content, retained Redo, legal gesture baseline and required inputs stay exact.

Final L1 permits only inherited rescue-linkage edges to become provenance, with the
necessary linkage-container, manifest and root rebuilds. Preserve the source root SHA
and original identities as provenance and label the result "rebound recovery point".
Document, baseline, history, batches, cursor, Undo, Redo, gesture, inputs and their content
identities stay exact. Independent evidence and receipts remain reachable, even when an
independent path still pins an older object. New activation identity and authority belong
to B's wrapper/runtime, not rewritten payloads. An original input manifest may retain an
origin ProjectId only when B declares that origin. Prepare its verified leases from B's
owned namespace at B's current owner epoch. No A authority is restored. Validate the rebuilt graph normally.
The preservation oracle must reject a graph-valid, fully rehashed change to each
non-linkage Document, batch or input payload. Do not weaken checkpoint validation.

Provenance keeps at most 16 exact attempts and 64 KiB encoded data, including the one
recovery edge and exact current omissions. Up to four retained current-session attempts
remain exact. Flatten inherited provenance, keeping newest detail that fits. Older detail
becomes a fixed count, saturating-overflow flag, digest and unresolved warning. Summary
never proves settlement or reactivates an ID. Compaction cannot drop independent roots,
current omissions or the dedicated R_ack edge. Original pending command ID/digest lookup
returns UnresolvedOrigin, or conflict for a changed digest, never Absent or permission to
execute. Search exact receipts retained through R_ack beyond automatic predecessors.
Tentative command-effect exclusion and complete rescue publication are C5 obligations.

At either unknown publication, eligible A1 rescue captures the already-frozen O cut.
During selection unknown that cut is exactly saved L, with nothing newer. During Save
unknown its own exact ticket and unknown outcome remain. Rescue cancel, failure,
refusal or unknown B keeps O frozen and retains A's attempt, target pin, owners and
charges. Unknown B is a never-active capture copy. Immediate verified B handoff seals
O and irrevocably abandons A's selection. Late A proof settles/shows destination status
only, never activates abandoned R or changes B's actor, edits, history, association or
Saved/dirty. Outside this composition, A1's ordinary resume rule continues to apply.

A quota refusal resumes O before offering eligible non-active working-branch retirement.
Keep the chosen target's pin across retirement and retry, taking a fresh cut and ordinary
Save if O changed. Exclude the active branch, R_ack, the prepared existing-branch target,
all live attached Core/agent/specialist sessions and specialist base/target dependencies.
Direct removal of the prepared target refuses as well. A live session's unsaved suffix
must stay usable and persistable. Name blocking sessions/bases, offer Save this session
or Close this session, fence safely and recheck eligibility at publication. Saving alone
does not permit orphaning a still-writable session or required base. Durable agent and
specialist creation/reopen goes only through Ja1EditingSession and its registered
DurableBranchSessionGuard before accepting edits. Keep registration through save/close
and fencing, with no usable unregistered constructor or raw mutable-bundle escape.
C5 owns this registration witness; C6 has no production App durable-session credit.

The owner retains the registered Core, base, pin and charged registration after the caller
drops its guard or branch object. Dropping a guard never discards accepted edits. The agent
close route first persists the complete proposal, then freezes the exact saved history and
seals every Core handle before unregistering. A direct owner close refuses an unsaved suffix
and names the required persist step. A racing edit between persist and freeze also refuses.
Creation and reopen allocate a private actor inside the owner. Registration precedes
the first returned Core handle. These checks use the same
owner that lists and removes branches.

A guard keeps the epoch of its actual namespace owner across Save As and rescue. A live
specialist created at A continues to use A's retained writer after manual editing moves
to B. Its guard cannot write B or block removal of an independent B branch copy. A late
A proof may settle that specialist's exact original ticket while its actor remains live;
it never changes B or calls a sealed manual actor or a never-active capture actor.
Close names each registered session with accepted unsaved work or an unresolved save.

Registered persist freezes that specialist's accepted cut, computes the full proposal
with Core's existing floor and replay rules, and stages its SpecialistBaseRecord before
publishing the branch checkpoint. Both belong to one existing root transaction. The
owner keeps a charged capture of the last proposed cut so an unknown Save followed by
its exact floor settlement and a newer suffix preserves the complete original proposal.
The owner uses its canonical registration even when the caller holds an older guard
clone. This capture is session metadata for the specialist, not another recovery point.

Explicit retirement confirms exact branch/content/history loss and expected root, then
publishes removal of only that current index entry under the same fresh-open guard.
Use the existing collector after acknowledgment and reader safety. Independent evidence,
specialist bases, readers, selected-target and A1 source holds remain. Report current
history-residency relief separately from physically reclaimable bytes. Unknown retirement
follows ordinary reconciliation. Repeat cleanup never deletes or uncharges twice.
Forget the older recovery point from A is a separate explicit successor-record operation
removing only its dedicated R_ack edge. Branch removal cannot perform it silently.

Release the selection pin only after durable activation retains the content, final refusal
without retry, explicit abandonment/target change with no unresolved root publication,
or safe whole-context Close. Disclose whether the point remains durably listed first.
When only session-pinned, say: "This recovery point is no longer in the project's automatic
list. It is kept only for this selection in this session. Abandoning the selection lets it
go." Preview warns if preliminary Save will rotate it out. While prepared, refuse Save As,
opening another project and other ordinary destination changes with that explanation.
Only explicit abandonment when no root publication is unresolved permits the new action.
A1 rescue remains allowed with disclosed irrevocable abandonment at handoff. If B omits
the selected point, say so. During unknown selection add that A keeps it only if selection
completed. The pin is session-only: a crash can lose this chosen point. This is an accepted
limit, not a crash-survival guarantee or a new durable recovery slot.

For unavailable A, say work was saved at its current location only when proved. If storage
never returns, Save rescue copy is the way to keep the work. Close can discard the only
accessible in-session copy and must never be labelled safe. Keep reconcile/rescue choices
and the freeze after failed rescue. Close also discloses session-only target loss and,
while selection is unresolved, that A keeps it only if selection completed. Closing a
status view is not context Close, recovery or proof of absence.

C5's frozen Save permission belongs to the bound editing owner and exact accepted cut.
It calls the ordinary selector and exact-ticket publisher. `recovery_save_disclosure`
reports that selector's exact oldest-Undo loss before the action. Public Save and alternate
handles still refuse Frozen actors. Selection activation moves its already prepared
attachment, input context and history digest after sealing O.

A rescue preview walks the selected content closure without recursively importing an
archived root's automatic inventory. It then adds each retained recovery role explicitly.
This preserves R_ack and both automatic predecessors while allowing separately consented
unavailable-only omissions. The copier uses that same closure. An archival manifest cannot
reintroduce an omitted role. Independent evidence, receipts, Redo and required inputs
remain mandatory. Source adoption and the destination reservation occur at the actual
first namespace mutation, including a lock claim or missing-parent creation.

Whole-context Close fences active, registered and retained actors before ending source
reuse. Each copy owner keeps its own namespace, source reads, pins and charges until its
readers finish. Ending source reuse permits temporary-source cleanup. It does not resolve
any destination's unknown publication or permit a late copy to activate.

## 6. Durability, receipts and host-death recovery

### 6.1 Ordered persistence barriers

One writer loop publishes `.kinewright` as the authoritative root. Immutable records live in the project-derived
`<stem>.kinewright-assets/studio/objects/<sha256>`; owned media/voice/proof stores share the same authority. Use SHA-256 and measured lengths, not the
incident-pairing digest. A root includes generation, branch/record refs and receipts. Studio correctness never depends on the best-effort incident sidecar
(`headless.rs:22–27,57–74`).

For each command/contiguous batch, all barriers below are mandatory, including sources, drawings, voice recordings, proofs, histories, lineage, claim/review
evidence and newly created directories:

1. **B0 admission:** authenticate, reconcile duplicates (§6.2), check authority/expected state, validate complete operations, asset handles and quota; reserve
   transient, backup and output bytes. Apply host edits through Core or validate the local manual history. Resulting state is pending, never a durable success.

2. **B1 file data:** write each new object into an exclusive temp, finish/hash/verify it and sync its contents. Verify newly admitted bytes and any unverified
   dependency closure. Reuse previously verified sealed identities under the pinned store/owned-input contract; existing objects are never overwritten.
   A changed/unverified identity requires byte validation. Incomplete voice/proof writes cannot be referenced.

3. **B2 object names:** publish objects exclusively/no replacement, then sync each affected object/asset directory. Sync every newly created ancestor bottom-up
   through its already-durable parent. Complete this for the entire graph before any new authoritative root can name it; data sync without durable names is
   insufficient.

4. **B3 previous root:** validate the current root against its verified sealed graph and the new/unverified closure; prepare its immutable recovery checkpoint,
   sync data, publish its name, sync checkpoint directory and new ancestors. Preserve the old verified recovery root until this finishes. First migration has a
   verified legacy backup. A failed backup prevents the next root publication. This does not require rehashing all previously sealed media per metadata commit.

5. **B4 next root:** serialize the full next root, resulting heads and command receipts; exclusive adjacent temp, verify, sync data. No document-only
   serialization. There is no claim that backup and root replacements are one transaction.

6. **B5 commit point:** strictly replace the root inside its pinned filesystem namespace; sync the root directory/namespace barrier. Only after success
   acknowledge durable edits/comments, Saved or Delivered. Notify from a later durable event.

**Fresh-open publication invariant (JA-1 S2 C1).** Every root publication must admit its exact
candidate graph as an independent fresh reader under the same effective `StorageLimits`, before B5.
The shared reader/publisher traversal in `bundle_recovery.rs` enumerates all current BranchIndex
histories and charges each exact verified history hash once, including histories absent from the
writer cache. It also applies control, metadata, decoded scratch/replay peaks, archived/recovery
traversal, raw support, graph depth/object count, owned-byte and complete-closure checks. Encoded
length is not decoded cost. This corrects S1's ordinary multi-branch Save/Save As under-accounting;
cache residency is a performance choice. The operation-local empty-reader calculation is not added
to the writer's physical ledger; actual execution still reserves its own overlap. Save, branch save,
fork, first save/copy and evidence/receipt/floor publications share the root boundary. Refusal
preserves the authoritative root, live edits and exact floor-ticket semantics. Reconciliation checks
the observed graph under those same limits. No later-save headroom or persistent fit certificate is
created; every later candidate is admitted afresh. An in-session accounting memo may reuse costs
recorded from already decoded and verified objects, keyed by exact ObjectRef (SHA-256, kind,
length) and the entire effective limit profile. Misses take the same exact reader decode/preflight
path; encoded length never estimates an unseen history. Discard affected facts at explicit
re-verification and discard the memo when namespace continuity is not proved. The memo contains
no decoded histories, persistent accounting or reservation, and its presence never determines
which candidate branches count.

Linux's proposed local-filesystem adapter uses `fsync` files, no-replace object publication and `renameat` for the root, plus directory `fsync` at B2/B3/B5.
Storage and filesystem must honor flushes. Windows's named replacement primitive is **`MoveFileExW(MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)`**, same
volume, never COPY_ALLOWED or in-place write; flush file contents with `FlushFileBuffers`. The Windows namespace adapter must implement B2/B3/B5, not treat file
flush as directory persistence. A conservative supported native profile can use an already-authorized volume handle and `FlushFileBuffers(volume)` at those
barriers. No elevation prompt/service is introduced by this design.

**Platform release rule:** Linux ships first, as the product brief specifies. **G1-W remains a Windows release blocker. On any platform or account where
B2/B3/B5 cannot be certified, Studio refuses durable writes with `durability_unsupported` before any migration or store mutation; it never silently weakens
those barriers.** Read-only bundle inspection remains available; legacy behavior does not authorize a Studio write bypass.

This restriction is intentional: Microsoft's [MoveFileExW documentation](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw)
does not establish the whole multi-directory crash contract.
[FlushFileBuffers](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers) documents administrative privilege for volume flush.
[ReplaceFileW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew) explicitly does not support REPLACEFILE_WRITE_THROUGH; that
flag is not a substitute. These API facts were checked in documentation, not in native tests. Supported durability includes the certified storage profile, never
arbitrary devices.

Full graph integrity validation runs on open and recovery, establishing a bounded verified-identity set within §12's object cap. Subsequent commits validate
newly admitted bytes/dependencies and retain those verified identities; file names or timestamps alone do not certify bytes. Preserve every required namespace
barrier for new objects/ancestors and checkpoints. Actual input access still enforces the sealed, decoder-bound identity/integrity contract (§4.3); incremental
metadata verification cannot weaken it. Integrity loss blocks use/resume instead of silently trusting the cache. G15/G20 distinguish full open/recovery scans
from incremental metadata work by recording verified bytes and latency.

An error provably before replacement leaves the old root authoritative; retain pending state and stop later mutations. A replacement/barrier error that may have
published is **`durability_unknown`**, not failed/rolled-back. Reconcile observed root/receipt and complete durability before success; do not silently apply
again. Sharing violations pause without fallback. The current `project.rs:498–503` truncate-write fallback is forbidden for Studio. Acknowledged graph
corruption blocks resume; never silently fall back and call lost acknowledged work recovered. Recovery presents the prior valid root honestly. G1 distinguishes
process death from storage crash: SIGKILL cannot establish persistence of filesystem metadata.
A1 permits explicit independent rescue after eligible refused reconciliation. It acknowledges B only; A's pending command/effect is excluded and named as unresolved, never reapplied or acknowledged there. Later rescue-retained destination reconciliation updates that destination's receipt/pending state and may settle only its own exact ticket on the live, unsealed Core under A1 §5. It never restores/installs/replaces acknowledged history or imposes another floor on any Core; sealed A's ticket remains. Disclose observed contents while current edits remain unchanged. A subsequent explicit save is a separately admitted successor and must disclose omission of an agent effect known committed at A.

### 6.2 Receipt precedence and bounded reconciliation

Each command ID is `(random issuer_id, monotonic sequence)`. Its canonical digest binds protocol/action, project/target branch or record, original epoch/lease,
expected head/revision, exact normalized payload and referenced-object hashes. Transport tokens are excluded. Canonical serialization fixes typed field/key
order and option presence, rejects duplicate keys, and preserves text/stroke bytes without whitespace or Unicode normalization. Store digest, target, resulting durable
head/comment/version IDs, generation and outcome with the same root. For adoption/return, include the old/new branch, HeadId, checkpoint and switch identities
listed in §5.1; the receipt never contains a live lease grant. A retry resends the original envelope; replacing its old epoch changes its digest.

After authenticating current read access to this project, resolve ID/digest **before** new-command epoch, sequence, lease-lifetime or stale-head admission.
Return an existing matching committed receipt even after a lost reply/restart. Same ID/different digest returns `id_payload_conflict`, with no success and no
mutation. Only an unseen ID may reach fresh-authority admission. New commands require active issuer, next sequence, current lease/epoch and expected state.
Prior-epoch requests have read-only reconciliation authority; no reconciliation call revives mutation authority.

| Lookup state | Meaning and caller action |
| --- | --- |
| `pending` | The owner still has that exact admitted command; wait/reconcile it, never enqueue another application. |
| `committed` | Matching digest has a durable receipt and resulting identities; settle the local pending item once. |
| `absent` | Validated current/previous roots contain no commit and no live pending attempt. Old-epoch mutation still refuses; restore/replan under fresh authority. |
| `retired` | A durable issuer high-water/retirement record says this ID cannot execute again; detail may have been compacted. Resolve through the client's retained receipt or exact batch/comment/version evidence and digest. |

If a retired detail/digest is unavailable, return `retired_unresolved`, not success for supplied payload. The GUI preserves its pending item and performs
read-only evidence reconciliation; it does not mint a replacement edit automatically. If evidence cannot prove application, pause that operation. Recovery must
not guess from document equality, counts or text similarity. Unacknowledged manual commands proven absent may be reauthorized automatically only inside §5.1's
shared recovery barrier, with a new command referring to the preserved old ID/digest. Verify predecessor, batch, history checkpoint and gesture identity to
prevent duplication or changed undo meaning. Preserve the original pending chain until both sides agree and the recovery-close checkpoint is durable.
Switch reconciliation returns the committed branch/checkpoint/switch result; acquiring a current lease is separate and cannot revive the old generation.

Bound detailed receipts/tombstones to 4,096 live entries/project, issuers to 128 live/reconciling entries and one outstanding manual chain/client. Retain
unresolved-client evidence and retired digest bindings within that admission cap; on exhaustion pause new admission instead of evicting unresolved evidence.
Clients acknowledge their reconciled high-water, allowing compaction. Closed issuer IDs are never accepted for mutation, even after their high-water detail is
removed; unknown IDs must undergo fresh owner issuance, never self-register. Run/attempt counters persist across restarts. G1-R tests edit AND comment lost
replies, payload mismatch, expired issuers, retired receipts and bounded issuer retention.
For A1, resolve unresolved-origin lookup before fresh-command admission: the original ID/digest returns typed unresolved-origin status, never absent,
success or enqueue permission; a different digest conflicts. Preserve receipt precedence and these caps. Rescue-record compaction never turns an unknown
or retired ID into absent or grants execution authority; unavailable old detail remains unresolved/retired.

### 6.3 The named cleanup owner and resumption

Before any COMMIT releases provider execution, the host durably records the attempt reservation, nonce and guardian recovery identity. The guardian keeps bounded
private OS-local lifecycle/completion records, never project-root or asset-store writes; the host alone imports their verified results into the bundle. A startup
crash reconciles that same nonce/OS identity before releasing its reservation.

Reuse **AW2 R28/R29's native `kinewright-job-guardian` ownership mechanism**, not its renderer execution policy (`docs/AW2-CODE-GENERATED-CLIPS.md:760–845`).
For agent sessions, the **run host** owns intent and the sole non-inherited lifetime-pipe writer; a GUI has neither. Provider CLI
authentication/config/environment/tool restrictions remain connector specific; do not impose renderer read-only runtime images or pretend CLIs are hostile-code
sandboxes.

On Linux, an independent transient systemd user service owns the guardian outside the host's scope, with delegated cgroup-v2 leaves, verified kill/events
handles and subreaping. The guardian creates a blocked shim inside containment; it kills the tree on host lifetime EOF and confirms populated=0 plus
adopted/direct-child reaping. Guardian death triggers the user manager's KillMode=control-group/SendSIGKILL cleanup; the manager reaps the guardian. No
process-group fallback. On Windows the separate guardian exclusively owns a non-inheritable, non-breakaway kill-on-close Job. Create suspended children already
assigned via PROC_THREAD_ATTRIBUTE_JOB_LIST; verify containment before resume. Kernel last-handle-close cleans the tree on guardian loss. No observer duplicates
that lifetime handle. Missing prerequisites refuse before exec.

Use AW2's **OWNED → CONTAINED → COMMIT** pre-exec handshake with 5 s startup deadlines; only then release provider code. Lost ACK, cancel, timeout or host death
during startup cleans without direct-spawn fallback. Ordinary cancellation allows 500 ms grace then tree kill; abrupt host death skips grace. Within 2 s of kill
decision require OS emptiness/reaping or remain `cleanup_blocked`. Host crash leaves the guardian responsible for cleanup; guardian crash leaves the independent
OS owner responsible. A bounded recovery record names attempt/nonce, owner instance, service/cgroup or Job/process identities and phase. Never reconcile by bare
PID/name. Scope AW2's record/slot accounting to agent attempts (§12).

A successor host first acquires project authority, restores durable state and reconciles those cleanup records through independent OS identities. It retains
pins, charges and execution slots until emptiness is established. Lost observation is not zero processes. Guardian loss invalidates that attempt's unsaved
output. No new provider attempt until cleanup is proven; G6 includes a real descendant and a no-overlap execution log. Direct `Child::kill`/Drop in
`crates/kinewright-agent/src/drivers.rs:259–262,713–716` is insufficient for this guarantee.

Restore running tasks as interrupted at their last durable heads. Expire plans/tokens/leases; reinspect before edits. Existing Claude disables session
persistence (`drivers.rs:175–204`), so restart from a bounded brief containing accepted work, original feedback, assumptions, prior review findings/responses,
limits and exact durable state. Local edits are idempotent; provider computation/billing cannot be exactly once. Reconcile exports/artifact IDs before retry.
GUI closure alone keeps the same task/attempt/session alive. No automatic service at reboot: opening the app or explicit host start resumes authorized work
after integrity/cleanup checks. Graceful detach and abrupt host death are separate G6 cases.

## 7. Playback, comparison and feedback lineage

### 7.1 Selecting and presenting versions

Use existing `set_document`/seek/resource binding (`app.rs:942–956`; `crates/kinewright-media/src/engine.rs:1052`), with the selected frozen document **and its
LUT/asset/input context**. Background candidate/manual updates must not replace a selected version. Associate durable snapshot identity with the applied PF1
stamp, not the newest issued request. `crates/kinewright-app/src/presenter.rs:13–25` describes the displayed stamp/frame/stale state but has no VersionId today.
Extend that association at final presentation; retain old picture visibly stale until the correct frame is shown. PF1 R-1/R-2/R-5 remain binding
(`docs/PF1-PLAYBACK-PERFORMANCE.md`, §7).

A/B uses one active playback/audio pipeline per GUI, two cached paused stills and a change list. Compare unique script beat/source moments where known;
otherwise show both explicit timecodes. A/B changes neither working branch. Proxies are optional, low-priority artifacts keyed to version/input/settings, never
evidence of delivery readiness. Additional viewers may each own a local player; the one-player rule is per GUI, not a machine-wide memory claim.

### 7.2 Original feedback packet and voice notes

Capture the **displayed** version/head, document/input hashes, project-frame integer/rational fps, display raster/orientation, original text/strokes, optional
snapshot-local clip/layer hit, source/local frame and script-beat anchor. Normalize geometry after letterbox/display scaling; retain its raster basis. Pending
seeks use the actual displayed version/frame or wait; never label old pixels with the selected new version. On a pending manual working snapshot, preserve a
local draft until its head is acknowledged before saving an authoritative anchor. Hit-test can expose an ordered active-layer selector; overlap/alpha/blur is
not proof of pixel ownership. Return unresolved rather than guess.

Agent packet example (illustrative IDs; no invented football data): `{comment_id, revision, batch_id, version_id, head_id, document_hash, input_manifest_hash,
frame, fps, region:{basis,raster_size,orientation,strokes}, target:{local_clip_id,occurrence_id,local_frame}, script_beat_id, text, frame_proof_ref, voice_ref,
transcript_revision}`. Optional absent values are not zero. Save edits as comment revisions; preserve original packet and proof. A submitted batch is immutable
and may contain several versions. Drafts stay private until Revise; status/tombstones do not erase evidence.

Microphone press anchors the note and starts explicit recording; save owned audio before transcription. Reuse CPU Whisper small/source-and-model cache
(`docs/TRANSCRIPTION.md:3–20,33–37`); no new speech service. Show editable ASR text and save the chosen transcript revision with the batch. Cancellation during
inference suppresses late publication by note/job ID at the next safe boundary. Missing model retains audio and typed feedback; silence-only notes cannot
auto-accept ASR text.

### 7.3 Occurrence identity and two independent mappings

Core IDs are snapshot-local: `crates/kinewright-core/src/operation.rs:4425–4434` allocates max-current-plus-one and split uses it at 2326. Add a project record
of **occurrence lineage**, without rewriting Core IDs globally. An occurrence ID contains branch-origin identity, creation batch ID and result ordinal.
Head-scoped maps bind local IDs to occurrences. Split descendants carry parent occurrence plus exact range ancestry; moves/trims retain identity with changed
mappings; delete/recreate always creates a new occurrence even for the same source/ID. Sibling branches cannot collide. Merges import only accepted lineage;
fresh adoption derives from version ancestry. Undo/redo selects the corresponding recorded head map, so an undone split cannot remain an active descendant.
Abandoned branch maps never enter the candidate. Commit head, history and lineage together under B1–B5. Unknown operation lineage refuses automatic mapping.

`CommentResolution` names original packet and exact candidate HeadId and returns separate **temporal** and **spatial** results with evidence. Temporal statuses:
exact, mapped, ambiguous, removed, unresolved. Use verified occurrence ancestry, source-frame mappings and then unique script beat; repeated ranges/takes
produce multiple candidates. Spatial statuses: exact, transformed, unresolved, removed. Recheck transforms/crop/composition/aspect **at the selected mapped
frame**. Only transfer a region through a verified mapping with correct bounds/visibility. A temporally exact retained chart can have unresolved spatial
geometry after crop/transform/vertical reframe. Keep its original drawing; do not copy the box. Take replacement maps meaning, not pixels. No optical-flow
tracking in v1.

The lead inspects preserved old/new frames for ambiguity/removal and records a disposition; it does not routinely ask Riel to recover machine bookkeeping. Close
feedback only with exact resulting evidence or a reasoned removed-target disposition, reviewed against the original request. G10 covers ID reuse, branch
collision, undone splits and spatial changes.

## 8. Agent surface, scoped tasks and run behavior

### 8.1 Models, connectors and roles

**`claude-opus-5-5` is verified**, accepted by installed Claude Code 2.1.304 and Riel's login: `target/review/ja/ja1-lead-verification.md:3–9`. The catalog fix
is landed at source `660845e`, inspected in `crates/kinewright-agent/src/models.rs:71–84`. Do not reopen availability or include a catalog repair in JA-1.
Same-login parallel sessions are observed practice, not documented entitlement; numeric limits remain unknown (`ja1-lead-verification.md:10–13`). Wait/serialize
on reported limits; never fall back to paid API billing.

| Role | Default responsibility and permitted assignment |
| --- | --- |
| Lead | Short plan, assumptions, serial task creation, candidate merge and request for independent review |
| Editor | Script/take/structural edit on assigned tracks with explicit sync/ripple closure |
| Motion designer | Assigned graphic/title fields/assets with validated data/cue bindings |
| Audio engineer | Assigned audio clips/nodes with explicit routing, buses and sidechain closure |
| Colourist | Assigned image-effect fields/adjustment layers; no timing or unassigned global colour changes |
| Reviewer | Frozen candidate/artifact/input/reference evidence only; no mutation or self-delivery |

Configure role connector, full model ID/options and existing login once. Record requested/resolved values and CLI version per attempt
(`crates/kinewright-core/src/agent.rs:35–50` already has model/effort/tier). Reviewer must use a different model from lead/authors and a separate session;
prefer another provider. Actual visual modality certification remains required. Reuse `AgentDriver`/`AgentSession` and `pump_session`
(`core/src/agent.rs:94–113`; `agent/src/session.rs:1–14,47–75`). Do not spawn provider-native subagents: Kinewright owns TaskIds and scopes. Keep ACP shared
runtime/config leases (`acp_session.rs:226–242`). Driver allowlists are supplementary; server authority is essential (`docs/agent-harnesses.md:14–21`). Current
ACP notices retain built-in shell/file/web tools (`crates/kinewright-agent/src/acp_drivers.rs:35–39`); declining ACP client filesystem requests alone does not
disable them. Only a connector/profile certified to prevent direct project/store writes and unrelated terminal/filesystem actions may join a Studio run. Until
certified, that ACP profile is unavailable for this workflow; do not silently claim MCP scope enforces its ambient tools. G12 must exercise the real profile as
well as server requests. This is a named connector dependency, not a new OS sandbox design or permission to modify the person's global CLI configuration.

### 8.2 Enforcement and minimal merge

Preserve the compact seven-tool runtime (`docs/M36-AGENT-RUNTIME-EFFICIENCY.md`, “Outcome”). New Studio actions/inspectors use registry capabilities behind it;
no general task HTTP API becomes a model filesystem bypass. Internal host messages are `checkpoint_manual`, `reconcile_command`, `start_run`, `assign_task`,
`freeze_task`, `merge_task`, `save_comment`, `validate_claims`, `request_review`, `seal_version` and paged `observe`. Each mutation uses a command envelope
(§6.2).

Bind task authority at its server endpoint to project/run/task/attempt, branch, durable base, epoch, allowed operations, object/field sets, asset roots,
lifetime and recorded consent grant. A prompt/argument cannot widen it. At prepare AND actual commit expand planners, classify all effects and validate indirect
closure: sync/ripple tracks/order/duration, media import/relink, title/graphic addresses, nested effects, shared buses/routes/sidechains, global colour and
export/file side effects. Enum-exhaustive classification refuses unclassified operations. Recheck at replay and final merge admission. Reviewer actions cannot
import, mutate, cancel exports, create tasks or overwrite files. Untrusted scripts/prose/tool results are data. Direct capability invocation cannot bypass
compact-runtime or scope checks (`server.rs:1206–1299`). Preserve the existing destructive-confirmation broker (`server.rs:1714–1761`). A recorded run grant
covers specified reversible edits on owned tracks only, never blanket filesystem authority or the person's branch. Outside-grant actions refuse or wait for a
batched decision; time passing is not consent.

Allow **one editing specialist per candidate**. For v1 there is **one active Studio execution owner/run on this machine**, with up to three admitted provider
attempts. A second run in the same or another project refuses with `studio_run_active`; the existing host remains attachable for viewing/comments. Other GUIs
can view/comment through their project writer without gaining execution authority. Use one OS-local execution claim and its bounded recovery identity, reusing
§6.3's guardian/cleanup ownership. Acquire it before starting a run; retain its reservations until all descendants/cleanup are settled. After owner death,
reconcile the previous identity before admitting execution; a free claim alone is not proof of emptiness. No daemon, multi-project queue or scheduling service
is introduced. Structural editing precedes dependent look/audio/graphics work; the lead may reorder independent domains. Read-only inspection or immutable
asset preparation may overlap under the host admission limits. Freeze/drain before capturing a proposal; compare durable base (§4.2), then merge its complete currently applied operation sequence once via M31 atomic batch. Undo removes operations from
that delta. Changed candidate means conflict and a new task on the new head; never reset an old base number or auto-cherry-pick. Manual reconciliation is a
fresh candidate from preserved snapshots. Delivery never overwrites the manual branch.

### 8.3 Visibility, limits and continuation

Run states: queued, running, waiting_for_files, waiting_for_provider, reviewing, ready, paused, interrupted, failed, cancelled, cleanup_blocked. Persist
transitions and counts before acknowledgment. Show concrete progress, model, waits, assumptions, missing evidence, review findings and limits; raw logs stay
paged. T3-inspired concepts map to video RunId, specialist TaskId, branch isolation, connector/model/options, durable task status and merge receipt; no T3 code
dependency. Each review round is a new task carrying original brief, previous findings/responses and unresolved objections.

Pause stops new scheduling and reaches durable/cleanup boundaries; Stop retains completed work while cancelling queued and active work safely. UI acknowledges
promptly while native work winds down. Two repeats of the same failure trigger replanning; at most two automatic repair rounds/delivery attempt. Default run
limit: six elapsed hours including provider waits and 80 sends/task, persisted across restart. Waiting does not reset the deadline. No automatic reset or model/cost escalation.
Provider token stops can overshoot by one turn (`session.rs:66–75`). If tokens are unavailable, enforce time/turn limits.

Persist provider-reported input/cache creation/cache read/output/reasoning/cost categories by attempt/turn/event identity. Normalize cumulative counters; do not
add reasoning twice when it is already output (`core/src/agent.rs:53–75`). Missing values show unavailable; unknown cost is not $0 and subscription telemetry is
not an incremental invoice. Deduplicate replay and preserve consumed allowances. Numeric token ceilings await dry-run calibration. A monetary hard stop is
offered only where a connector can enforce it. Credentials remain in connector/OS storage, never bundle/evidence/export/notification.

## 9. Fact binding and the named Job A dependencies

JA-1 defines the shared **ClaimBinding**, its validator admission and delivery predicate. Lead-owned JA-3 implements the small Songbird v1 adapter and
editorial consumers; it is a separate scoping task, not silently included here. The JSON export **has shipped**, per `task-ja1-rev3.md`, decision 1:
MockingBoard main `dcaa369b`, PR #309. This is the lead-supplied status, not a new cross-repo acceptance claim. The binding contract is
`format: "mockingboard.songbird-answer"`, `version: 1`, as specified in `mb/task-songbird-json-export.md`, “Shape (version 1)”, extended by
`mb/task-songbird-json-export-r2.md`, “Required change”. There is no four-field fallback.

Preserve exact immutable export bytes/hash, format/version and adapter version. V1 carries `source.app`, `source.traceId`, `exportedAt`, nullable
`dataGeneratedAt`, question, kind, nullable headline/prose/caveats/sql/chartSpec, columns/rows/rowCount/rowsComplete, sections, and nullable
`chartData: { rows, reflines }`. Each section has title/prose/sql/columns/rows/rowCount/rowsComplete. Preserve all supplied provenance, caveats and section
context as evidence: export time is not data-generation time, nullable `dataGeneratedAt` is not a fabricated date, and trace ID is not content identity.

Every operand/finite selection carries a typed **dataset selector**, validated container address, row key/index and adapter-known typed field address:

| Dataset selector | Only valid v1 container | Evidence scope/completeness |
| --- | --- | --- |
| `answer_rows` | `/rows` | This answer's own `rowCount` and `rowsComplete`, with its SQL/question/caveat context. |
| `section_rows { section_index }` | `/sections/<index>/rows` | That section's own `rowCount` and `rowsComplete`, SQL/prose/title context. Index is within this exact export; duplicate titles do not identify a section. |
| `chart_rows` | `/chartData/rows` | Resolved plotted rows, interpreted with this export's chartSpec fields/series/units. Completeness is `resolved_plot_only`; v1 has no chartData rowsComplete flag. |
| `chart_reflines` | `/chartData/reflines` | Resolved reference-line values, interpreted with chartSpec. Also `resolved_plot_only`, without an inferred query/population completeness claim. |

Row keys must resolve uniquely within that selected container or use its exact export-local index. No arbitrary path expression, SQL execution or chart query
engine is added. These are input addresses; output addresses remain registered typed MO6 properties or declared AW2 slots.

A ClaimBinding contains: binding ID/revision; export identity/format/version and adapter version; dataset-qualified operands/selected cells and field types;
each operand/selection's evidence scope, supplied count/completeness (or resolved-plot scope), context and caveats; bounded typed derivation and resulting claim
scope; source units/conversion; precision/rounding mode; expected rendered value AND text; addressed output field (occurrence plus registered title/caption
property or AW2 declared label/data/geometry parameter); output head/definition hash and validation evidence. Derivations allow identity, sum/count/ratio/
difference over explicit finite selections, rational/decimal arithmetic and stated zero/missing-value policy. No arbitrary SQL/eval/code, unbounded queries or
model-authored constants masquerading as data. Prose/SQL provide semantic context, not substitute result cells.

For each operand and derivation distinguish a present-cell claim, explicitly labelled subset, complete selected query result, and any broader population
claim. Preserve/validate the selected answer's or section's supplied `rowCount`/`rowsComplete`; do not synthesize missing rows, borrow another dataset's flag or
turn the count into invented values. A value actually present in incomplete rows may support a correctly scoped point claim. Explicitly labelled finite-subset
derivations may proceed. A total, ranking, proportion or other whole-result claim **requiring omitted rows refuses when `rowsComplete=false`**. A derivation
cannot gain scope from a complete but unrelated operand. Even `rowsComplete=true` certifies only the supplied result within its SQL/filter/limit/time scope,
not a broader population assertion. Semantic review must check that scope and preserve caveats; all visible scope qualifiers are addressed output fields too.

To reconstruct the exported plot, **only `chartData.rows` supplies plotted data and `chartData.reflines` supplies reference lines**, with chartSpec's
field/series/unit interpretation. Never silently substitute answer/section SQL rows or re-run MockingBoard's data pipeline. `chartData=null` means resolved plot
evidence is unavailable, including unresolved/failed charts: pause affected chart work/show placeholders and refuse affected render starts. Other separately
supported claims can proceed. Chart data certifies the resolved plot, not completeness of an underlying query or population; broader aggregates require their
own sufficient, dataset-qualified evidence. Absence of a chartData completeness flag must remain explicit.

Generate **both labels and addressed data/geometry parameters** from those validated bindings. Independently resolve operands/derivations, scope/completeness,
chartSpec interpretation, units and rounding, and compare **actual addressed output fields** to expected values/text. A correctly hashed JSON paired with an
altered SVG label or geometry parameter must fail. For the one-off chart, use bounded editable AW2 SVG nodes/declared parameters with an inventory of all
visible factual labels and data/geometry fields; unbound factual output outside that inventory blocks. Pixel review checks presentation; an asset hash alone
is not a statistic validator. MO6 typed bindings provide field addresses, not statistical correctness (`mo6-design-rev3.md`, §7); AW2 R14 is render provenance,
not fact checking.

**Pre-render block:** any visible factual field without valid binding/supplied-source evidence, wrong dataset, unavailable resolved plot, insufficient scope/
completeness, missing row/field, invalid units/rounding, derivation error, altered output field or unsupported script claim blocks affected preview-proof, review
and final renders of that candidate. Ordinary playback of already validated delivered snapshots is allowed; unvalidated working graphics show placeholders.
The validation key binds export hash/format/version, adapter version, typed dataset/container/selected cells, scope/completeness and supplied counts, relevant
context/caveats, chartSpec interpretation, derivation, units/rounding, script/style output, output head/definition and addressed fields including scope labels.
Changing any of these invalidates validation. Seal rechecks the frozen key; no stale pass. Semantic script claims need source-linked evidence review by the
lead and independent reviewer before render admission; the system is not a universal deterministic natural-language truth detector. Unsupported claims stay
flagged, never filled in.

| Named dependency and owner | Contract JA-1 consumes; prerequisite for |
| --- | --- |
| Shared owned-input work, lead coordinates with AW2 | Sealed, durable manifest and actual decoder-open identity, including fonts/LUT/generated inputs; required before immutable playback/review. |
| JA-2 person separation | Matte outputs through the host store seam below; ordinary editable separation settings and input-bound results; needed for Job A blur/replacement. |
| JA-3 Songbird import, lead-owned | Exact shipped Songbird v1 bytes/hash and versioned adapter; typed dataset/container/cell selectors, per-answer/section counts/completeness, resolved-plot-only chart scope, provenance/caveats, units and missing-value handling as above; before claim validation. |
| JA-3 one-off data-bound AW2 SVG chart | Labels and data/geometry inventory generated/checked from chartData rows/reflines plus chartSpec interpretation; dataset/scope/completeness-bound ClaimBindings and ordinary editable clip before chart render. No recurring-template or Remotion dependency for this first video. |
| JA-3 script/take mapping | Script/cue/beat revisions, source ranges/retake choices and intentional pauses; before structural edit, feedback beat mapping and speech QA. |
| JA-3 look/audio edit scope | Face-aware look, sync, conservative clean-audio operations and configured delivery profile; before whole-cut review. JA-1's deterministic output checkers do not replace these edits. |
| JA-3 cards and captions | Source-linked card copy/assets, caption word/timing inventory, factual field addresses and safe-region expectations; before preflight/review. |
| AW2 Part 1; MO6 relevant title behavior | Owned generated bytes, typed fields, shared preview/export behavior; use only the subset the cut needs, not all later template work. |
| JA-1 output-review integration with media | Whole-output checker reports plus certified reviewer evidence transport in §10; before any Delivered seal. |
| PF1-H H-T owner | Independent talk-hold evidence for Job A playback quality; retain its pending/report-only status until measured. |

JA-2's store seam is deliberately short: **reserve → write → seal → pin → retire**. The host grants bounded staging authority for an exact job/input key, checks
measured bytes, verifies sealed hashes, publishes through §6, pins references from heads/history/recovery/exports and retires only after verified
cleanup/unreachability. JA-2 gets no independent durable writer or cache-clear escape. Matte formats, scheduling and its store internals remain JA-2's design.

## 10. Required review coverage and delivery

### 10.1 Frozen evidence and actual modalities

Freeze candidate HeadId/document/input/claim keys. The shared export path makes an exact settings-bound review artifact; decode that artifact for output
evidence, rather than passing only source frames or live monitor proofs. Bind artifact byte hash, decoder identity, output PTS→timeline manifest and checker
configuration to each result. Use the existing media compositor/export architecture (`docs/M4-COMPOSITING-EXPORT.md`, “Export and cancellation”).

The review task invokes a read-only bounded evidence capability through the compact runtime. It returns **decoded output images** as actual connector-supported
image content, plus reference images and exact IDs/frames; local paths in text are not proof the model saw pixels. Return short artifact clips only to a
connector certified to consume them. The present AgentSession text API and Claude text content block (`core/src/agent.rs:105–113`; `agent/src/drivers.rs:683`)
do not prove image/video support. Certify tool-result modalities through the real permitted connector. With still-only support use timed strips, state sampled
temporal coverage, and retain whole-output deterministic checks; never claim model video/audio perception from strips/transcripts. If an assessment requires
short-clip perception, select a capable connector or block it.

### 10.2 Pinned Job A coverage matrix

All rows marked Required must return pass for the exact output; `unsupported`, `not_run`, missing coverage or fail blocks Delivered. A blocker cannot be
downgraded to an optional omission by the lead. Defaults below are proposed profile values to freeze with fixtures before implementation acceptance, not
measured quality guarantees.

| Check / required status | Checker and consumed modality | Coverage / expected result / consequence |
| --- | --- | --- |
| Structure and delivery, Required | Existing `qa_document` plus authoritative document/delivery validators | Whole document/settings, no invalid/offline assets or blocking structural errors. Ordinary QA warnings are separately reviewed. |
| Facts, Required | §9 validator plus source-linked semantic claim review | All visible factual fields and script claims, before render and at seal; no unsupported claim. |
| Artifact integrity/A-V conformance, Required | New media post-render scanner, actual decoded frames/PCM and stream metadata | Entire output, expected duration/frame count, audio presence and bounded A/V sync against authored mapping; decode errors/gaps fail. |
| Loudness/true peak, Required | The existing export loudness/true-peak measurement of final decoded PCM (`crates/kinewright-agent/src/server.rs:13866-13880`), made a seal gate; no duplicate meter (lead amendment) | Whole output, against the shipped streaming/social delivery profile: integrated −14 LUFS ±1 LU and true peak ≤−1 dBTP (`crates/kinewright-core/src/delivery.rs:194-199,233-237`). The same profile drives planning, export normalization and acceptance. Missing measurements fail. No model listening needed. |
| Clicks, dropouts, silence gaps, Required | New deterministic bounded PCM/window scanner, JA-3 speech/intent map | Whole output plus all cut boundaries: flag impulse/discontinuity, unplanned near-zero speech spans and gaps. Pin thresholds and fades to clean/seeded fixtures; each unresolved flag blocks. |
| Single-frame flashes/black/frozen gaps, Required | New decoded-frame streaming scanner with authored-intent/cut map | Every output frame, including outside model samples; unplanned defects block. Legitimate intentional cards/cuts remain positive controls. |
| Captions/cards, Required | Existing layout validation plus new output-frame/expected-overlay checks | Every declared visible cue/transition boundary and extrema: no hidden/truncated/unsafe text or missed requested card. JA-3 supplies expectations; sampled stills alone are insufficient. |
| Independent visual judgment, Required | Configured independent model, certified output images/reference stills; certified short clips when available/needed | Sample opening/middle/end, every cut/chart/card/caption transition and every repaired/commented region; assess look/readability/pacing against references. Findings or unavailable imagery block. State sampled scope explicitly. |
| Model listening, Optional | Only an explicitly audio-capable certified connector | Disclose unsupported/not_run without claiming listening. This never waives deterministic audio rows. |
| Riel's creative judgment, Final human gate G19 | One finished practice-video watch | Publishable without opening timeline; automated completion cannot assert this result. |

For the simple audio scanner, initial test profile flags a normalized-sample step above 0.5 with short-window impulse energy, and near-silence below −60 dBFS
for at least 80 ms inside declared continuous-speech spans; also check unplanned gaps against the script/take map. Calibrate/pin windows and thresholds using
clean QuadCast/cut controls before release. These checks detect named defect classes, not all audible problems. Intentional pauses/effects must have prior
authored windows; a model cannot add a waiver after detection simply to pass. A scanner not implemented/calibrated is not_run.

G16 seeds flash, click, dropout, silence gap, hidden caption and wrong label **independently**, with clean controls. Include a one-frame defect away from
ordinary model samples and audio-only defects with identical video. Output checks must catch their own seeds; a model's supported-modality subset cannot
redefine away failures. Model visual judgment is still required, and is separate from these objective defect witnesses. Stills/transcript do not establish
listening/full-video QA. G17 injects wrong authored fields before render. G16 can inject a wrong label downstream of that preflight to exercise output visual
review; the matching artifact manifest records the injected bytes. A checksum mismatch alone is not the defect-detection oracle for that case or for the
audio-only seeds.

### 10.3 Delivery predicate

Only the host may seal. Require: frozen candidate/input integrity; every required fact/coverage row passed; independent review identity; no unresolved blocking
feedback/review finding; exact candidate/input/render/settings/claim keys; completed required cleanup; and durable publication of all version/review objects and
root. Any relevant edit invalidates the old receipt. Missing review, self-review, changed output bytes or required unsupported/not_run means no Delivered. After
at most two repair rounds, pause with a concise explanation; a labelled draft may still be inspected. Publish-kit derivatives/reels bind to their own output
manifest/checks, not just the master review. Delivery sealing is later than early snapshot storage/selection gates; G7-S can pass without claiming G7-D.

## 11. Headless completion, attachment and notifications

Authenticate loopback attachment with private runtime Bearer tokens, protocol/project identity and forwarded roots as in AW1 §§6/9. Never store secrets in
project/discovery/notifications or expose them in argv/logs. A held lock with stale or missing discovery stays contention; no PID-based lock stealing.
StdIO/external clients must proxy scoped endpoints if that AW1 surface is implemented; shared IO alone is not a shipping CLI. Required CLI/auth/roots work is
assigned in S2.

Closing every window keeps the same host/run/attempt/branches alive. On completion commit the version and completion-event ID, then notify best-effort:
freedesktop notification in the Hyprland session; Windows toast with registered application identity/activation. Use a small adapter in existing packaging, no
startup daemon. Activation contains only validated project/version IDs and opens the saved cut idempotently, never a model-supplied shell command. Lost
service/logout may prevent notification; retain that state. Notification replay may duplicate after crash but cannot duplicate work or delivery. Next open shows
the saved ready cut. With no clients/work/cleanup pending, checkpoint and exit after 60 s idle grace, releasing decoders/GPU/model children. No
power/GPU/desktop-setting changes or keep-awake policy is introduced.

## 12. Performance, memory and bounded storage

These are **proposed admission limits/targets**, not measured allowances. Retain existing PF1 contracts separately. `docs/PERFORMANCE.md`, “Measurement
boundaries” distinguishes callbacks from presentation; “R97 checkpoint” retains G14 pending/report-only and no active whole-process RSS allowance. JA-1 supplies
neither a PF1 pass nor a causal speed claim.

| Resource / scope | Proposed bound or target | At the bound |
| --- | --- | --- |
| Studio execution / machine | One active owner/run, 3 live provider attempts including lead/reviewer; 1 editing specialist | Refuse another active run; queue this run's tasks. Cleanup retains owner/slots until emptiness; no daemon. |
| Core states / editing project | Local manual + one validation shadow for its leased attachment + candidate + active specialist | No shadows for retained branches; share immutable documents, page completed histories, measure transient copies. |
| Host control metadata / project | 64 MiB, excluding Documents/media/children | Pause admission; this is not total RSS. |
| Command ingress / project | 64 commands, 256 KiB each; pending manual chain within same cap | Stop new edits/admission visibly; no dropping accepted commands. |
| Driver/event ingress / attempt | 32 events, 64 KiB each; 64 KiB pipe rings, 64 MiB durable event quota/run | Bound at actual readers before allocation, backpressure/spill then interrupt safely. |
| GUI event cache / GUI | 8 MiB, 200 rendered rows | Virtualize/page; sequence gaps request a snapshot. |
| Histories / project | 32 MiB serialized/live branch, 128 MiB including manual shadow/pending copies | Ordered shared floor checkpoint (§4.2) or task pause; retain merge bases, gesture baselines and redo semantics. |
| Comments/voice / project | 2,000 comments, 16 KiB text and 512 points each; 2 min/note, 256 MiB audio | Typed refusal/stop capture; retain originals. |
| Records / project | 100 versions, 10,000 Studio objects, graph depth 32, decoded record ≤64 MiB, metadata ≤512 MiB | Pause new sealing; no silent version eviction. |
| Automatic recovery roots / project | Two verified predecessor checkpoints, at most one reserved B3 staging checkpoint; one separate pre-migration backup | Retire superseded automatic roots after safe publication as below; pause if retention cannot fit. |
| Run records / run | 128 tasks, 256 assumptions/questions/artifacts each; two repair rounds | Replan/pause; retain terminal summary. Receipt/issuer bounds are §6.2. |
| Guardian / project | One owner, 3 active attempts + 3 cleanup records, ≤1 KiB recovery identity each | Refuse new execution until verified retirement; charge guardian RSS too. |
| Review imagery / request | 12 frames, ≤8 MiB encoded; bounded clip bytes/duration certified per connector | Split requests, stream and retain exact coverage; no whole-film frame dump. |
| Playback/background / machine | One player/GUI, one background proof/export, proxy lowest priority | Prioritize gestures/playback; additional viewers consume separately measured resources. |
| UI / editing GUI | p95 callback ≤16.7 ms; gesture→correct presented frame p95 ≤100 ms warm | Instrument input, Core accept, PF1 identity and presentation separately. |
| Selection/save/control / local project | Correct version presentation p95 ≤500 ms warm/2 s cold; durable comment ≤250 ms; Pause/Stop UI ≤100 ms | Honest loading/pending states; safe-boundary completion measured separately. |

The current Claude driver uses `unbounded()` at `crates/kinewright-agent/src/drivers.rs:237`; a downstream queue alone does not meet the ingress bound. Count
transient serialization/decode and unsent accepted history, not just stored sizes. Reserve media/next root/previous root/review output bytes before admission;
owned media defaults to §3's visible allowance. GC is owner-only after a durable root commit;
roots/backups/versions/undo/redo/branches/recovery/comments/claims/in-flight jobs pin their full reachable graph. Uncertain reachability retains bytes. Asset
leases/charges retire exactly once.

Retain the current root plus the **two most recent verified automatic predecessor checkpoints**; B3 has one reserved staging slot, reused/reconciled across
failed attempts rather than accumulated. After B5 succeeds and the new root is validated, durably update that bounded recovery inventory before retiring the
oldest automatic checkpoint and its otherwise-unreachable history floor. A recovery checkpoint does not recursively pin earlier recovery inventories: validate
its project-content graph, then rebuild the bounded inventory on restore. Keep the one verified pre-migration backup until a successful v3 reopen and explicit
legacy-backup retirement. Delivered/pinned versions, retained working branches, unresolved receipt/recovery evidence, active readers/jobs and their reachable
histories remain independent pins; never discard those to meet the automatic-root count. Unsettled publication or reader/pin/space conflicts pause retirement/
admission. The object/byte caps still apply, so manual floors do not silently retain every old automatic checkpoint forever.
A1 keeps this two-predecessor limit, validator, ordinary rotation and nonrecursive checkpoint rule unchanged, with no v3 format-counter change. Its R_ack
is one dedicated optional recovery reference in the durably pinned `rescue_copy_v1` record, an independent pin with complete B-owned content closure.
B open/recovery validates that reference, its declared content edge and complete closure. Ordinary rotation/compaction cannot retire it; S0 §4.3's binding
one-level retention and explicit, disclosed person-initiated retirement ruling governs later rescue generations and release through the ordinary collector.

G20 measures GUI, host, guardian and each real provider/Whisper child RSS separately and together; record per-process peaks and sampled simultaneous aggregate
without calling the sum of nonconcurrent peaks a simultaneous peak. Include VRAM, owned-input copy/verification, restore, transient history clones, repeated
switches, manual gestures, detach and teardown. Record newly verified bytes per metadata commit versus full open/recovery scans, and retained recovery-root
bytes after repeated floor advancement. One execution owner's admission counters enforce the run limits; viewers' resources remain measured separately.
Unrelated projects are annotated, never stopped. Use paired runs on the workstation as it is, no builds/tests during timing, per PITFALLS. Record p50/p95,
displayed identities/hold chronology and bytes/tokens. Windows has independent native lanes; unrun lanes stay NOT RUN.

## 13. Ordered stages, runtime witnesses and regression pins

### 13.1 Baseline-compatible evidence contract

Every new gate needs a runnable compatible harness **before** production types. Use a small native test adapter around existing Core/project/server/media/App
seams, a two-process controller and a recording `ScriptedDriver` as appropriate. It accepts stable fixture/action JSON and emits observed document bytes,
history entries, process identities, presentation marks and side-effect counters. Baseline mode invokes current APIs; revised mode invokes the new host through
the same fixture protocol. Harness records/opaque fields must not require future production types to compile. It may not implement the missing semantics itself.
Missing binary/endpoint, compile error, disabled lane, unavailable provider/OS/device and timeout during setup are prerequisites/NOT RUN, **never the desired
runtime red**.

For absent surfaces (such as receipts/delivery), a compatible observer first drives a successful existing edit/save/export and records the available artifacts;
then it asserts the missing durability/authority/evidence property. An unsupported operation response alone is not proof. Positive controls establish the
exercised edit/render actually happened. Where that observer cannot reach a behavior on today's base, retain a **pending baseline witness**, build only the
minimal observational seam first, then demonstrate the runtime red; do not claim all gates are already reproducible failures. Each row below gives entry/harness
(E), positive control (P), expected baseline deficiency (B) and targeted mutation (M). All B statements are source-derived expectations, **not observed reds**
in this task.

### 13.2 Dependency order

| Stage | Required predecessors and result | Gates |
| --- | --- | --- |
| S0 contracts/harness | Lead coordinates v3 union, owned-input interface, JA-3 handoffs and coverage/profile fixtures; pin current source and compatible observers | R-M31/R-PF1/R-lock; baseline preparation for all new gates |
| S1 durable bundle | S0; graph publication/receipts, history/base/lineage, identity-bound Save As/recovery and caps | G1, G2, G3, G15-storage; no scheduler/delivery dependency |
| S2 writer/manual attachment | S1; run/commit host, GUI fencing/history stream, auth/roots, AW1 S4/S5 obligations | G4, G5-L, G5-A; no running specialist required |
| S3 Studio selection/feedback | S2 plus sealed owned-input fixtures; layouts, read-only snapshot selection, comparison, typed/drawn/voice comments | G7-S, G8–G11; no delivered cut is promised |
| S4 scheduler/containment | S2; role settings, scope, serial tasks/merge, independent guardian, limits/usage, resume | G6, G12–G15; uses S1 histories and S2 authority |
| S5 facts/output review | S3/S4, relevant AW2/MO6/JA-2 and JA-3 §9 contracts; fact admission, required QA and certified reviewer evidence | G17 before review render; G16; then G7-D sealing |
| S6 delivery integration | S5; notifications/publish kit; all applicable native platform contracts | G18, G20; Windows G1-W required for durable-write parity |
| S7 practice video | Prior stages and real material; complete lead dry run, model review, one Riel watch | G19; human judgment separate from automated acceptance |

### 13.3 Exit gates

**G1 graph durability (S1).** E: adapter around `serialize_project_document`, `write_project_bytes`, `load_document`, with an injected persistence-operation
recorder/crash filesystem. P: save a real changed Document and independently reopen/compare exact bytes and a valid graph. B: current writer lacks required
object/ancestor/backup barriers and may truncate on rename refusal. M: skip each B1/B2/B3/B4/B5 barrier separately; enable in-place fallback. Cut after each
barrier, assert acknowledged edit/comment/voice/history/proof names and bytes survive, and unacknowledged commits are wholly old or new. Keep SIGKILL
process-crash results separate from modeled dropped-unflushed-write results. **G1-W** additionally uses native Windows replacement/sharing faults and a
disposable VM/storage-crash lane, never this workstation's power. Native process-kill alone does not discharge storage durability. An unsupported-profile
fixture must refuse with zero migration/store mutations. S1 uses opaque comment/voice/proof object fixtures through the store API; capture/transcription/render
production flows are S3/S5 integration repeats. G1/G1-R do not depend on those later stages or on the scheduler.

**G1-R receipt reconciliation (S1).** E: same edit/save adapter, reply-drop hook and host command fixture protocol; baseline observer counts real undo
entries/comment bytes. P: one edit and saved feedback with reply retained. B: current Core/save path has no command-digest durable lookup; repeating an accepted
edit can produce a second undo entry. Comment receipt baseline needs the minimal comment observer. M: check stale head before receipt, omit digest, evict
unresolved issuer or reapply retired IDs. Drop edit/comment replies, restart/change epoch/expire issuer, retry identical and changed payloads, retire details;
assert exact text, one batch, correct identities and no false acknowledgment/reapplication. S2 extends the same reply-drop/reconciliation observer to branch
switch receipts: lose the reply after B5 and retry the original adoption before/after GUI restore or host restart. Require the original old/new branch/checkpoint
identities and switch generation, exactly one adopted branch, and no lease grant from receipt lookup; fresh attachment uses the current committed pointer.

**G2 history/base restoration (S1).** E: real Core commands/queries, `TimelineBranch` and project save/reopen adapter. P: uninterrupted nonzero-revision branch
edit→undo→redo→merge. B: saving only current Document loses cursor/batches/base relationship. M: restore old numeric base, bind merely equal hashes/counters,
save append-only operations or drop redo. Replay with different runtime counters; exact durable base still merges once; equal counter/different head and equal
bytes/new HeadId conflict. Compare merged bytes, undo/redo order, batch boundaries and applied-op digest; corrupt history quarantines without rewrite. Lost
merge reply is reconciled through G1-R, not another undo entry. Extend the Core/history adapter with drag A→B→C under one key, then Undo/Redo: checkpoint after B,
restore/replay C and the pending history moves with the original baseline A, and compare uninterrupted bytes, one original gesture entry, undo target and redo
order. M: close the gesture before suffix replay or restore a floor on only one side. S1 tests export/restore equivalence; S2's G5-L adds real host death and
shared checkpoint/floor handshakes, without making S1 depend on a later host.

**G3 migration/Save As (S1).** E: existing load/serialize/save plus destination lock/store adapter and copy barrier hooks. P: legacy v1/v2 byte-preserving save
and unowned new destination copy. B: current document-only save cannot preserve a Studio bundle, and `write_project_bytes` publishes project before LUT copy
(`project.rs:520–549`). M: acquire destination lock after copying, skip graph preservation or allow generic downgrade. During an active destination owner
attempt Save As; assert exact source/destination roots AND every referenced asset. Fail copies, swap alias after verify, test same-project alias, opposing Save
As, future format, migration twice, collision/traversal/quota and corrupt backup; no partial overwrite.

**G4 writer identity/AW1 obligations (S2).** E: two real lock/save processes, existing `REV2_HOOK` seam in project lib, plus real GUI journal observer. P: known
alias lock contention (R-lock) and one successful locked save. B: current verify-then-path-use can follow a repointed alias; GUI journal exclusion remains
S5-OBL. M: re-resolve caller path, omit GUI journal lock, skip lock-fd identity on release or ignore suffixed journals. Barrier-swap aliases after verify before
root/sidecar/object use; race post-scan journal creation and replaced-lock release. Assert both project/asset graphs exact, one writer, successor discovery
retained and pending recovery refused. No missing-discovery takeover of a held lock.

**G5-L fenced manual Core (S2).** E: existing coalesced App/Core path plus history-bridge adapter, two GUI clients and host restart hooks. P: live slider
changes pixels every frame while collapsing one gesture to one undo entry. B: no fenced local-Core-to-durable-history bridge today. M: accept old lease/skip
sequence, drop an intermediate coalesced batch, ack before durability or wait for disk before preview. Transfer/disconnect/crash with pending edit, undo/redo
and host failure; verify exact durable prefix, local pending state, one editor and fresh restore. Add a durable first drag batch A→B, an absent second B→C
under the same key and pending Undo/Redo. Crash the host and require uninterrupted final bytes, one original gesture entry, undo target A and identical redo
order. Repeat with C committed but its reply lost; never replay it twice. Advance the floor with an accepted pending suffix, including Undo/Redo; drain/reconcile
the cut and install the same ordered checkpoint on both sides, retaining the same earliest undo point. Lose the floor checkpoint reply and restart. Mutations:
close before suffix reconciliation, skip any producer freeze, or change/install a floor on only one side. Assert no new input until full history equivalence and
shared recovery closure. R-PF1 alone cannot pass this gate. This is the named ownership-choice gate in §1; inability to implement it needs an evidenced lead ruling.

**G5-A adoption/isolation (S2).** E: two ordinary Core branches through the host adapter; stored snapshot fixtures need no scheduler or delivery. P: manual and
candidate diverge independently. B: no durable switch-and-retain branch adoption. M: assign a snapshot over live history or ignore expected head/lease. Race
pending manual edit with adoption; either refuse or drain/checkpoint and create a fresh branch, preserving exact old branch/history. No claim of one undoable
replacement. Extend with reply loss after B5, GUI/host death immediately before/after restore, original-envelope retries and delayed old-lease commands. Require
exactly one adopted branch, one active writer, exact flattened/active-branch document, unchanged retained history, restored resource context, successful first
edit/Undo on the adopted branch, and exact undo/redo on switching back. Fresh acquisition must use the committed switch/checkpoint identity. Additional M:
omit fencing, grant before restore, trust GUI selection, revive a receipt's old lease or recreate a branch on retry; each must fail its respective assertion.

**G6 containment/headless resume (S4).** E: production guardian launcher with a native fixture CLI speaking recorded connector protocol, creating a real
grandchild (Linux also setsid), plus real installed-connector integration lane. P: close every GUI, same session/attempt completes durably and reattach starts
no new process. B: GUI-owned sessions/direct Child kill do not supply host-independent whole-tree cleanup. M: inherit lifetime writer, execute before
CONTAINED/COMMIT, duplicate Job handle, treat EOF as emptiness or release slot early. Kill host, lose startup ACK at each handshake barrier, kill guardian, lose
cleanup observation; independently prove live descendants existed, then empty/reaped identities before retry. Assert no overlapping attempts, preserved
usage/heads/undo, expired old plans and lost-reply reconciliation. Provider unavailability is NOT RUN for the live lane; it cannot stand in for
descendant-cleanup failure.

**G7-S snapshot storage/selection (S3).** E: real save/load/PF1 adapter selecting immutable unreviewed fixture snapshots. P: two different documents/owned
sources display distinguishable landmarks. B: current project roundtrip lacks the Studio snapshot index/frozen selection context. M: bind mutable path/latest
head, strip index or allow background head to replace selection. Reopen/edit copy, undo/redo and switch; original hashes unchanged. Tampered owned input marks
damaged. This gate does not create Delivered. **G7-D (S5)** uses those snapshots plus G16/G17 receipts; a clean reviewed cut seals once;
missing/changed/unsupported required coverage refuses. M: bypass any conjunct of §10.3 independently.

**G8 displayed A/B identity (S3).** E: existing presenter finalization/PF1 path with delayed completions and version binding observer. P: current-frame R-PF1
control and two clearly different snapshots. B: DisplayCell has no durable version binding today. M: label issued identity as applied, reuse wrong LUT context
or permit second audio owner. Switch/seek A/B with out-of-order frames and changed timings; assert actual displayed frame/input/version, original still identity
and one audio owner. Measure presentation, not receipt/GPU submission; latency result is recorded in G20.

**G9 typed/drawn batches (S3).** E: minimal App input/presenter/comment-store harness around displayed stamp and geometry. P: independent landmark through
letterboxing yields correct normalized rectangle. B: current UI lacks persisted version-bound comment batches. M: anchor selected request instead of displayed
cell or omit letterbox offset. Capture during delayed seek, reopen and submit a mixed-version batch; assert exact proof/frame/version/text, retained draft and
saved acknowledgment.

**G10 old-feedback lineage (S3).** E: Core operation fixtures plus head-scoped lineage/resolution observer; baseline demonstrates ID reuse through current
allocator before asserting occurrence-safe mapping. P: unique move/trim maps exact source moment. B: local ID/content alone aliases delete/recreate and sibling
branches. M: map by Core ID, retain undone split ancestry or transfer region on temporal match only. Test same-ID/same-source recreation, sibling collision,
split undo/redo, repeated chart, take replacement/removal, animated transform/crop and vertical reframe. Assert temporal and spatial statuses independently and
original drawing bytes unchanged, with no routine recovery question.

**G11 voice feedback (S3).** E: existing Whisper path plus microphone fixture and note-ID publication adapter. P: a real short spoken note yields editable
transcript attached to its pinned frame. B: no version-bound voice-note workflow today. M: accept late cancelled result by newest selection or auto-accept
silence ASR. Cancel during inference, change version, inject late result; no wrong attachment. Missing model retains audio/honest state; that case alone is not
the baseline red.

**G12 specialist scope (S4).** E: current seven-tool server/ScriptedDriver plus bound task-policy adapter. P: one permitted audio batch actually changes the
intended samples/settings. B: investigator denylist is not per-role footprint authority. M: skip planner/indirect/commit/merge checks separately. Attempt video
edit, shared bus/sidechain, ripple/sync collateral, global colour, relink/import-root escape, export cancel, direct capability and reviewer mutation; refuse
entire batch with exact pre-state. Late expired capability refuses. Existing destructive confirmation remains a separate regression pin.

**G13 serial merge/resume (S4).** E: `TimelineBranch::compare/merge_into` through durable owner freeze/drain adapter. P: editor then audio tasks from
consecutive durable candidate heads; each merge one undo entry, manual branch exact. B: current branches lack durable-base binding/task-quiescence ownership. M:
snapshot before drain, substitute numeric revision, include undone operations or repeat merge after lost reply. Barrier-race in-flight commit with completion,
restart at nonzero original revision with changed replay counts and equal-counter/different-head case; assert atomic proposal, undo/redo and exactly-once merge.
Basic M31 stale rejection alone is R-M31, not this new gate.

**G14 usage/limits (S4).** E: existing ScriptedDriver/pump with recorded cumulative/partial costs and provider-limit events. P: known categories sum exactly
once over two attempts. B: no durable multi-attempt scheduler accounting survives restart. M: add cumulative totals, count reasoning twice, reset allowance,
silently switch provider or treat missing cost as zero. Disconnect/replay, rate/time/turn exhaustion and restart; assert exact normalized totals, unavailable
fields, bounded repair rounds and no paid fallback. Actual numeric provider limits remain unknown, not fabricated acceptance targets.

**G15 bounded admission (storage S1; full S4).** E: existing IO/session paths with bounded allocator/queue/lease counters at real ingress, then host/guardian
adapter. P: valid work fills/retires a slot once. B: driver `unbounded()` and no Studio graph/pending-history accounting. M: bound only downstream, omit
transient bytes/pins or retire on EOF. Exhaust each §12 cap, receipt issuer retention and disk reserve; assert typed backpressure, no accepted data loss, intact
reachable objects, no premature cleanup-slot reuse and exact charge retirement after cancel/failure/teardown. Repeat metadata commits and floor advances:
retain only the bounded automatic recovery inventory, reclaim otherwise-unpinned old floors, and keep independently pinned versions/branches/unresolved evidence.
Crash during inventory retirement; never lose the required verified predecessors. Record byte-verification calls: new/unverified bytes are checked, sealed media
is not wholly rehashed per comment, and open/recovery fully validates the graph. Tamper an owned input and require refusal at integrity/access checks. M:
retain every recovery predecessor, recursively pin old inventories, evict a live pin, skip full recovery validation or weaken decoder-bound opens. Attempt a
second active project/run while the execution owner lives or cleanup is unresolved; require refusal and no extra provider process, then allow a run after
verified retirement. M: admit without the owner/cleanup check. Assert at most one manual validation shadow, without clones for parked branches.

**G16 required review (S5).** E: real shared export→decode→checker path and evidence capability through the selected actual reviewer connector. P: clean
chart/caption/speech artifact with independently verified imagery/PCM and passed matrix. B: structural QA alone misses seeded output-only defects and lacks
required coverage sealing. M: skip each required row, replace output evidence with source frames, permit self-review/stale keys or mark not_run as pass. Seed
flash outside normal samples, click, dropout, silence gap, hidden caption and altered output label separately. Require each relevant checker red/clean control
green, certified model visual receipt and changed-candidate seal refusal. No model listening requirement.

**G17 fact admission (S5, before any review render).** E: minimal JSON-export/typed-binding/output-field adapter around real AW2/MO6 consumers plus render-spy
counter; every fixture goes through the declared `mockingboard.songbird-answer` v1 adapter. P: rendered labels AND data/geometry derive from dataset-qualified
cells/allowed derivations with units/rounding and explicit scope. B: source hashes and typed output fields alone do not validate dataset identity/completeness.
Use intentionally different answer rows and chartData rows: the plot must consume chartData rows and an exported reference line. Include duplicate section
titles addressed by index, complete answer/incomplete section and the reverse, and `chartData=null`. A truncated row set's subtotal must differ from a known
complete fixture; a whole-result claim requiring missing rows yields **zero affected render starts**, while a present-cell claim and visibly labelled subset
remain positive controls. Complete-but-SQL-limited results cannot certify a broader population claim; chartData has resolved-plot scope, no borrowed flag.
Keep the export correctly hashed while altering visible SVG/title/caption values or addressed geometry; also omit a field, change units/rounding, remove a
subset qualifier or supply an unsupported script claim. M: substitute SQL rows for chartData, borrow answer/section completeness, invent chartData completeness,
trust the export hash, omit actual-field comparison or defer the guard until seal. Each affected invalid case must start zero renders. Restore binding and
verify exact dataset, output and validation-key/receipt identity; changing selector, scope, completeness, chartSpec interpretation or derivation invalidates it.

**G18 completion/publish kit (S6).** E: native notification adapter/activation harness on each OS plus headless scheduler. P: saved reviewed cut notifies and
opens at exact version once. B: no durable completion-event-to-version integration. M: notify before root durability, activate newest head or replay task on
notification retry. Remove notification service, repeat/crash activation and export optional reels; saved cut survives, activation idempotent, derivatives have
their own checks and no upload occurs. Missing Windows identity/session is NOT RUN, not a passing or failing behavioral witness.

**G19 real practice video (S7).** E: lead's complete Studio workflow with actual practice take/retakes, script, supplied Songbird export, AW2 chart,
cards/captions, JA-2/look/audio and old-version feedback. P: required technical/model review passes before the single finished-video watch. B: that complete
workflow is not established today. M: deterministic omission of required evidence must fail G7-D; do not manufacture a mutation of Riel's taste. Record actual
minutes and unanswered blockers. Riel alone judges publishable-without-timeline; automated quality does not discharge this gate.

**G20 performance/parity (S6).** E: current App/presenter/PF1 instrumentation plus input timestamps/process sampler and real host/children, using practice
project and controlled camera/chart landmarks. P: existing direct coalesced gesture and warm seek measurement establishes the same presentation boundary. B: no
end-to-end narrow-host manual/history/version measurement exists, not an asserted existing latency miss. M: introduce controlled durable-write delay/on-UI IO or
hidden second pipeline; witness must expose latency/residency increase. Measure §12 targets, transient copies, steady/peak RSS, VRAM and repeated
close/open/lease transfer on Linux/3090; Windows replacement/lock/notification lanes are independent. Retain exact source/binary identities and raw summaries.
Any unrun/unsupported lane stays named and open.

### 13.4 Regression pins and acceptance discipline

**R-M31:** existing branch divergence, atomic stale merge, applied-operation undo/redo, one-entry merge/cherry-pick; entry `branch.rs` tests and
`docs/M31-BRANCHES-VARIANTS-VERIFICATION.md`, “Acceptance gates”. Positive valid branch merge; mutation remove Core revision guard must fail the stale-control
case. These properties can already pass today. **R-PF1:** existing stamp/final-consumer/paint-marker, resource/teardown and shared preview/export checks;
entries `presenter.rs` tests, PF1 §13 I8/I18 and media engine tests. Correct frame positive, delayed old-stamp mutation negative. Preserve evidence classes and
pending H-T/G14; old PF1 passes do not establish Studio identity/latency. **R-lock:** existing `lockfile.rs` alias contention/release tests, project
headless/sidecar/recovery tests. One uncontended save positive, bypass lock negative. These do not discharge AW1 S4/S5 integration. No regressions-only gate
substitutes for the new contracts. M31/lock/PF1 pins are rerun during implementation as appropriate, not during this design task.

For implementation, record actual baseline behavior, targeted mutation red and restored green with exact identities/raw summaries; run only witnesses affected
by each mutation. Follow repository environment/nice/serialization rules and required CI fast/slow checks. No gate above ran here. The lead orders Astra and
Muse persistence/concurrency reviews; design acceptance is not runtime/native/human acceptance.

## 14. Risks, reductions, deferrals and numbered open questions

Largest risks are incomplete writer migration, unflushed graph names, path-use races, lost history/base bindings, uncontained subscription processes and falsely
complete QA. Each has a specific gate above; no general remote-editor framework or generic conflict graph is added. Platform release/durability policy is fixed
in §6.1. Decoder-bound owned inputs and JA-3 consumer inventories are necessary for immutable truthful output; mere hashes cannot replace them.

Deferrals: concurrent timeline-writing specialists, semantic rebase, multi-person/machine editing, two active A/B players, optical-flow region transfer, reboot
autostart, universal billing enforcement, required model listening, recurring chart templates/Remotion for the first cut, and unrelated PF1-H/CC8 work. Optional
proxies follow live snapshot correctness. No resource target is a measured result. Required unsupported/not_run checks block delivery rather than becoming
deferrals.

### 14.1 For Riel

1. **No design-decision interview remains.** Only if indispensable source/reference files are missing at intake, show one batched prompt listing those files and
   why they block the cut. Defaults and provider/storage settings are in §3.

### 14.2 For the lead and implementation owners

1. Which ordinary-account Windows namespace-durability adapter can be certified by G1-W without elevated volume flush? The platform decision is already fixed
   in §6.1; only adapter implementation and native evidence remain open.

2. Pin the single v3 field/asset union and decoder-bound input owner at S0 with AW2/JA-2/MO6. The lead already chose v3; schema integration details and native
   evidence remain work, not a new version-number question.

3. Scope JA-3's adapter for the shipped Songbird v1 export, dataset/scope/completeness handoff, one-off AW2 chart inventory, script/take map,
   look/audio/cards/captions and evidence handoffs before S5. The export is available; Kinewright's adapter/editorial implementations are named dependencies, not JA-1 scope.

4. Certify the independent reviewer connector's actual image/short-clip tool-result support and pin required checker thresholds/intent fixtures. Model listening
   is optional; deterministic audio QA and visual review are mandatory.

5. Pin the actual AW1 production gaps, finite Job A operation-footprint set, history-restore API and native guardian packaging. Prove G5-L before considering
   whole-editor ownership; auth/roots/S4/S5 cannot be assumed landed.

6. Select notification adapters/application registration and replace provisional §12 targets with practice measurements, including total RSS/VRAM and
   gesture-to-presented-frame latency. Keep pending PF1/H-T lanes explicitly separate.

## 15. Changes from rev1

| Critic finding | Disposition and concrete revision | Addressing sections / witnesses |
| --- | --- | --- |
| 1 — Blocker: referenced graph durability | Accepted. File data, exclusive names, all affected directories/new ancestors and verified previous root precede root acknowledgment; named Windows primitive and fail-closed namespace barrier; storage vs process crash separated. | §6.1; G1/G1-W |
| 2 — Blocker: destination authority/path use | Accepted. Claim destination before any store mutation; pinned identity through writes; alias/same-project/nonblocking Save As; explicit AW1 S4/S5 GUI recovery obligations. | §§5.2–5.3; G3/G4 |
| 3 — Major: oversized host/manual latency | Accepted. Narrow run/commit host; local manual Core with fenced history bridge, immediate coalesced preview, explicit pending state/lease transfer. Whole-editor contingency only through named G5-L ruling. | §§1, 5.1, 12; G5-L/G5-A/G6/G20 |
| 4 — Major: retry lookup/payload binding | Accepted. Auth then digest-bound receipt lookup before stale-head/lifetime checks; prior-epoch read-only reconciliation; pending/committed/absent/retired rules and bounded issuer retention. | §6.2; G1-R/G6/G15 |
| 5 — Major: durable merge base/adoption | Accepted. Persist target branch/base HeadId/hash, bind fresh runtime counter only to that identity; retain cursor/redo/coalescing; adoption switches fresh branch and preserves old one. | §§4.2–4.3, 8.2; G2/G5-A/G13 |
| 6 — Major: feedback occurrence/spatial validity | Accepted. Head-scoped occurrence lineage uses creation batch/ancestry, not Core-ID equality; temporal and spatial outcomes are independent, with frame-specific crop/transform/reframe validation. | §§7.2–7.3; G9/G10 |
| 7 — Major: host-death cleanup owner | Accepted. Run-host lifetime writer, independent AW2 R28/R29 guardian/service/Job, pre-exec handshake, OS emptiness/reaping and recovery identity before retry; no direct-child fallback. | §6.3; G6/G15 |
| 8 — Major: mandatory review coverage | Accepted. Pinned whole-output vs sampled matrix, required loudness/peak/audio defects and independent visual review; unsupported/not_run blocks delivery; model listening optional. | §10; G16/G7-D |
| 9 — Major: first-video facts/dependencies | Accepted. JA-1 ClaimBinding, actual output-field validation and pre-render guard; lead-owned JA-3 dependencies named; shipped Songbird v1 dataset/scope/completeness contract replaces the superseded export assumption; one-off AW2 chart sufficient. | §§4.3, 9, 10.3; G17/G16/G19 |
| 10 — Major: baseline witnesses/stage order | Accepted. Existing entrypoints/minimal compatible observers, positive controls, source-derived expected baseline deficiency and targeted mutations for every gate; regression pins separate; scheduler before resume and review before Delivered. | §13; all G1–G20 and R-M31/R-PF1/R-lock |
| 11 — Minor: excessive Riel questions | Accepted. Adopt playback/comment/manual defaults, existing references, conservative visible storage and saved role subscriptions; only one indispensable-file intake batch. | §§3, 14.1; G19 records actual interruptions |
| 12 — Minor: verified Opus fact | Accepted. `claude-opus-5-5`/CLI 2.1.304/login verified by lead; catalog landed at 660845e; observed concurrency distinguished from unknown numeric limits, wait/serialize and no paid fallback retained. | §8.1; G14 |

## 16. Changes from rev2

| Round-2 finding / lead decision | Focused revision | Sections / discriminating witnesses |
| --- | --- | --- |
| 1 — Manual recovery and ordered history floor | Freeze every producer; reconcile the accepted prefix/pending IDs; retain pre-gesture baselines and replay exact JournalCommands before shared closure/fresh lease. Floor generation and agreed stream cut are installed on both sides before input/Undo/Redo. | §§4.2, 5.1, 6.2; G2/G5-L: durable first drag batch, absent or committed/lost-reply second batch, pending Undo/Redo, floor advance/reply loss; one-sided closure/floor mutations. |
| 2 — Fenced adoption and recoverable switch | Fence old authority, atomically retain/create/select branches with active pointer/flattened Document and identity-rich switch receipt; restore GUI/history/lineage/resources before fresh authority. Retry returns the original result; returning to a retained branch uses the same path. | §§4.1, 4.3, 5.1, 6.2; G5-A/G1-R S2 extension: B5 reply loss, both participant crash points, delayed old lease, one adopted branch, first edit/Undo and exact switch-back history. |
| 3 — Shipped Songbird v1 dataset/scope/completeness | Replace every superseded four-field assumption with format/version 1, provenance and typed dataset selectors. Preserve per-answer/section completeness and query scope; chartData rows/reflines plus chartSpec exclusively drive the plot. Bind labels and geometry, actual output comparison and the full validation key; update JA-3 handoff. | §9, §14.2 and corrected §15 handoff summary; G17: differing SQL/plot rows, reference line, duplicate section titles, mixed completeness, null chartData, truncated subtotal, scoped positive controls and correctly-hashed/wrong-output mutations. |
| 4 — Scope reductions | Incremental admission verifies new/unverified bytes while open/recovery validates the full graph; retain two automatic predecessors plus bounded staging/legacy backup; one execution owner refuses another run without a daemon; one manual attachment validation shadow. | §§1, 5.1, 6.1, 8.2, 12; G15/G20: verified bytes/latency, repeated floor/root retirement and crash, independent pins, second-run refusal through cleanup and no parked-branch shadows. |
| Lead decision 3 — Platform durability | Fix the release/refusal policy in one statement in §6.1; remove repeated policy uncertainty elsewhere, preserve all barriers and keep native evidence unclaimed. | §6.1; G1/G1-W unsupported-profile zero-mutation witness; §14.2 asks only for implementation/evidence. |

### S2-R local App integration (C6)

The App's `ProjectSession` carries one local `Ja1Attachment` and moves that same
`Ja1EditingSession` into each finite background operation and back. Closing a status
window or focusing another project does not end its originating editing context.
Modal save/selection/rescue choices disable App producers; the existing actor enforces
the frozen/sealed cut for retained and alternate handles. Subscribe and prepare the
new view before selection publication or rescue's first mutation/seal. Activating a
new actor fences application of old in-memory agent branches; their work remains
available for inspection or an explicit new base.

Studio activation is explicit. Unsupported inspection stays read-only and cannot be
converted into a writable local session by generic Save As. Local headless and agent
open/save/resume use the same admission and lifecycle; no process host or manual stream
is implied. Explicit loss, omission, retirement, abandonment and completed-capture
release choices retain the rules above. Source-bound C6 evidence and its exact gate
are recorded in `docs/reports/ja1-s2/c6.md` and `c6-gate.sh`; native Windows/hosted slow
and physical storage lanes require their separately recorded results.

### C9 accepted manual history

A named Studio attachment starts its manual stream from an exact acknowledged
checkpoint. An unsaved or dirty bootstrap remains read-only until an explicit
foreground Save establishes that identity. The attachment, read-only modal and
manual-refusal notices say "Save once to keep editing here; until then this
timeline is read-only." The modal offers Save (named project) or Save As (new
project). Status/retry replies retain this requirement; the first successful Save
grants the manual gate and clears the read-only presentation. The App's `ManualProducerGate` is the
only manual mutation capability; discrete actions, frame-coalesced controls,
keyboard Undo/Redo, revision-gated recovery tools, deferred actions and
import/relink installation all use it. Specialist/session MCP endpoints have no
manual permit, and the host MCP surface cannot acquire one or install foreground
snapshots. Other attachments can read and operate their isolated sessions.

The owning actor checks its attachment and permit generation and reserves the
64-command/256-KiB pending ingress plus bounded history/control overlap before
acceptance. A refusal leaves history unchanged and is visible. Each accepted
message retains its original CommandId/digest, BatchId/predecessor, JournalCommand,
coalescing key, resulting history/cursor identities, identity effects and bound
lineage/input context. Closing or sealing ingress preserves its drainable queue.
Only the bound owner can release an exact acknowledged contiguous prefix.
The input manifest is transferred once at lease acquisition, checked against its
ObjectRef bytes/hash and retained as bounded actor control metadata. New media,
LUT or font references absent from this admitted context refuse before acceptance.
The person sees an input-admission refusal; it cannot become an unsaveable pending
command. This reuses the existing owned-input rules and grants no decoder certificate.

One host shadow validates the original contiguous messages through Core and the
distinct admitted-history BundleCommit route. The S1 command route still requires
`lease=None`. Publication preserves each intermediate batch, actor-assigned BatchId,
receipt and input context. A foreground snapshot cannot replace this stream. Local
preview and paint do not wait for publication. Pending manual work is not Saved;
a durable acknowledgement uses the publisher's result, and Close warns about any
remaining local suffix. Unknown publication or lost retention fences the actor.
C9 does not authorize recovery replay, a shared floor, recovery-close or switching;
those remain later stages. A required floor or unsupported input context refuses
and retains the accepted pending history for explicit resolution.

### C10 shared manual recovery and ordered floors

C10 implements §5.1's shared barrier around C9's accepted envelopes. On transport
failure or transfer, the GUI freezes the actor before joining submitted stream IO.
The original pending envelopes remain in the GUI until the shared result is
confirmed. The successor acquires the ordinary named writer, authenticates a fresh
callback to the same actual GUI source admission, and reads the acknowledged root.
It creates no source owner, source ledger or cleanup right. Retained destinations
or a prepared selection whose ownership cannot be re-established still refuse.

Each original ID/digest is resolved before fresh admission. Only proven absence
permits recovery-only reauthorization. Its new receipt contains the original
ID/digest in that same publication. The bounded absent suffix reserves its control
and history overlap before replay, then publishes every original-to-new receipt
and preserved batch ID in one ordinary root transaction. The S1 selector may
choose a legal floor for this full accepted cut; neither actor resumes until both
install that exact selected result and then the recovery-close result. This uses
the existing limits, without special recovery headroom. Committed commands are never reapplied;
unknown, payload conflict and retired-unresolved outcomes remain frozen. Recovery
compares the complete accepted history, including the original open gesture,
with the shadow. With no surviving GUI, the barrier uses the acknowledged prefix.

A runtime RecoveryBarrier binds the switch generation, original transition envelope,
checkpoint reference, and exact before/after histories. The reference identifies
the predecessor for a new transition, or the committed batch result for an
automatically selected floor. Ordered floors
use S1's validated HistoryTransition and existing transactional publisher. After
both actors install and confirm the same floor, the host prepares recovery-close.
Close likewise requires its durable receipt before confirmation can grant a fresh
lease. A lost publication reply retries the same envelope and recorded result.
Later metadata-only publications may change runtime head wrappers; every payload
reference, command cut, lineage and floor identity must still match that result.

The GUI endpoint installs only an owner-authorized exact frozen cut's legal floor
or recovery-close result. Installation keeps input frozen and invalidates old
permits. Resume and binding the fresh permit are one actor turn, conditional on
the originating context still being open. Rescue's exact own-ticket settlement
remains separate and gives no recovery-install permission. C9's explicit Save
requirement for an unsaved or dirty bootstrap remains unchanged. Adoption, return,
C11 switch receipts and the sealed-A rescue-tail repair remain later work.
An acknowledged prefix with no accepted command has no cut CommandId. Its
recovery-close record uses an absent cut, checked against the predecessor's absent
cut, rather than inventing a command. Existing nonempty cut encodings stay exact.
Manual leases also bind the admitted native parent identity and logical namespace
name. The name identifies the slot within that native parent; native identity,
rather than name or path equality, proves continuity. An identical copied
root at a replacement parent is insufficient proof of an original command's
absence. Recovery refuses before reauthorization when that identity differs or
was never recorded; this comparison uses native IDs rather than path spelling.
