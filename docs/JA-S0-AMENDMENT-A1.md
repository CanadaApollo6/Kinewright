# S0 amendment A1 rev5: independent rescue copy

> Committed copy of the approved design record (rev5, 2026-10-23; Astra and Opus APPROVE). Paths under `.review/` cite the lead's local review notes, which are not in the repository.

Design amendment at main `134267bd4c9c86a83824206a40d9923f2501834f`, 2026-10-22.
The two rev5 lead rulings govern; earlier rulings remain binding where unchanged.
This revises rev4 and appends round-4 dispositions in §13. §§10-12 retain earlier dispositions
with their rev5 qualifications. R_ack has one dedicated recovery reference in the rescue
record, outside the unchanged two-predecessor inventory. Reconciliation may settle only its
own attempt's exact floor ticket on the live, unsealed Core under §5.
Implementation and witnesses remain S2 work. This document
changes no code or other contract file.

**Scope.** Studio writes require btrfs, ext4, or XFS
(`crates/kinewright-project/src/namespace/linux.rs:885-889`). Windows durable writes remain
refused (`namespace/windows.rs:57-70` below). A can be an internal disk or a supported
external drive. An exFAT/FAT camera card cannot host a writable Studio A. Network writers
remain unsupported. A1 fully covers the owned-byte source for first save of an unsaved
session: its graph is in the retained local temporary store. Destination admission,
capacity, and durability can still refuse. For a project opened from A, rescue works only
when every selected required owned byte remains readable through retained handles or local
state and passes its hash/length check. A visible picture or a `StorePin` is not a byte copy.
Missing objects produce the refusal in §3 unless the person separately consents to a legal
B-only history trim and/or omission of exactly unavailable source recovery roots (§4). The
final selected graph must be complete and verified; no locator substitutes or partial B.
External camera/source-media availability is separate; rescue is not a camera-media backup.

Source basenames below are under `crates/kinewright-project/src/`, except explicitly
prefixed `core/` paths under `crates/kinewright-core/src/`. Proposed S2 type/result names
describe obligations, not existing APIs.

## 1. Exact amendment to S0 §4.3

Current `docs/JA-S0-CONTRACTS.md:403-406`, starting with its second sentence:

> If replacement or its barrier has an uncertain outcome, classify it
> as `durability_unknown`, retain both authorities/pins and reconcile the exact root/receipt
> before success or another attempt. An uncertain publication is not a reported failed save
> or proof of rollback (`docs/JA1-STUDIO-ORCHESTRATION.md:334`).

Replace with the following text. §§2-7 of this amendment are its normative A1 rules;
references to "A1" below mean this amendment, not S0's existing section numbering.

> If replacement or its barrier has an uncertain outcome at A, classify it as
> `durability_unknown` and retain both authorities/pins. Reconcile A's exact root/receipt
> and required durability before acknowledging A, writing there again, or another ordinary
> save/copy attempt from that unresolved owner. An uncertain publication is not a reported
> failed save or proof of rollback (`docs/JA1-STUDIO-ORCHESTRATION.md:334`).
>
> Exception A1: after an eligible failed reconciliation and a fresh selection-time probe,
> the person may explicitly choose **Save rescue copy** to fresh, independent B. Writes require
> btrfs, ext4, or XFS; Windows writes stay refused. First-save sources remain in the local
> temporary store. Opened-A rescue requires every selected owned byte to remain readable
> through retained handles/local state and hash-verified. Otherwise return A1 §3's missing-object
> refusal. B must be separate by native path
> and pinned namespace identity from every retained destination and the session temporary
> store. B receives a fresh ProjectId, fresh destination authority, and a genuinely separate
> Core actor. Historical origins remain; A's authority never transfers to B.
>
> Freeze the accepted live edit and all required history/input/lineage/evidence roots at one
> cut. Full history, a checkpoint of A's last acknowledged root when one exists, and all
> selected source recovery roots are the default. A1 §4 first evaluates whether omission
> alone makes B fit and be self-contained; if so, offer it without an Undo-trim preview.
> Only otherwise offer an eligible oldest-first history trim, alone or with separately
> consented omission of exactly unavailable source recovery roots, recorded by identity,
> root SHA and cause as provenance. Available recovery roots and all objects still needed by current state, selected
> history or independent roots remain required. Recompute, copy and verify the complete
> closure of the final selected roots, including LUTs, before B5-last publication. R_ack is
> one dedicated recovery reference in the durably pinned rescue record, with its complete
> content closure in B; it never occupies a third automatic-predecessor entry. B validates
> and offers this recoverable root on reopen (§4). A durable, root-reachable rescue record binds
> the full capture, B's selected result and disclosed loss, and A's unknown publication and
> pending-command identities. B has no graph edge to A or to temporary-store paths.
>
> Only a rescue verified durably successful at the time of its initial outcome can switch
> editing to B's fresh authority/Core. Seal A's actor against all editing handles at that
> handoff; editing and ordinary saves then continue in B. If B returns unknown, resume the
> original dirty edit associated with A. A later B commit only records/shows a rescue copy as
> of its capture; it never switches the active edit, even if histories happen to match.
> The originating editing session's JA-1 owner retains A's original owner/Core, attempt/tag
> and settlement status, authorities, source leases, pins, physical charges, and all source
> temporary-store bytes until that session ends, even after reader retirement or independent
> A reconciliation. Only §5 may settle the live attempt's own ticket/barrier; sealed A's
> ticket/barrier stays until session end.
> A stays independently reconcilable; B's success never settles A. Reconciliation of any
> rescue-retained destination settles its own receipt/pending state. Its sole permitted Core
> operation is S1 exact-ticket settlement of that same attempt's ticket on the live, unsealed
> Core, after its own destination proof (§5). Commit replays current edits and preserves
> content, post-floor Undo and Redo. Sealed A's ticket stays until session end. No other floor,
> restore, install or history replacement is allowed on any Core. Report the destination
> contents while preserving current edits, dirty/Saved baseline and writer association.
> Cancel, failure, or an uncertain B changes none of
> A's Core, ticket, floor, histories, or holds.
>
> An unknown publication at B gets its own tag, Core state, and independent holds, with no
> Saved message or destination writer exposed. Explicit rescue to C may follow under the
> same rules. Four rescue-retained uncertain-publication slots belong to the originating
> editing session (§5). Previews and admission use provisional reservations only. After the
> second probe and all required explicit consent, atomically adopt A's unknown attempt into
> rescue retention and consume its source slot when the first B mutation begins. Preview,
> re-probe, cancel and any abort before mutation consume nothing. Reserve the destination
> slot at that boundary; it is consumed only if the rescue-destination outcome is unknown.
> Verified rescue success and definite refusal return the destination reservation.
> Ordinary saves never reserve or consume a rescue slot; a later unknown ordinary attempt
> counts only when adopted as the source of a new explicit rescue. Consumed slots remain
> consumed until session end, even after settlement. Existing byte/object/metadata and
> cleanup quotas still apply. No hold is evicted to admit another attempt.

## 2. Eligibility, consent, and source capture

The structured `RescueProbe` carries the publication tag/attempt identity, whether a Core
ticket exists, failed reconcile phase, failing operation, native error, and normalized
cause. Phases distinguish namespace/parent identity, root read, graph/receipt validation,
durability barrier, and cleanup. Preserve this evidence instead of erasing every failure
to `DurabilityUnknownAt` (`bundle.rs:1384-1412`). Eligibility requires an existing unknown
root publication whose exact reconciliation failed for destination unavailability, missing
or damaged required graph/receipt bytes, or a genuine persistence failure.

| Normalized cause | Treatment |
| --- | --- |
| Inaccessible parent/device, denied read/search, I/O or stale-device failure | Eligible if A's publication remains unknown. Illustrative Linux errors include `ENOENT`, `ENODEV`, `ENXIO`, `EACCES`, `EPERM`, `EIO`, `ESTALE`, and `ETIMEDOUT`. A readable, identity-proved parent with an absent/older root follows independent settlement instead. |
| Damaged/missing required graph or receipt; failed persistence barrier | Eligible only if phase/cause establish a real inability to verify/complete A. Examples to classify include `EROFS`, `EUCLEAN`, `ESHUTDOWN` or XFS shutdown reported as `EIO`, and barrier `ENOSPC`/`EDQUOT`. An errno alone never authorizes rescue or a trim. |
| Interruption, contention, invalid handle/invariant, unsupported capability | Ineligible. `EINTR`, `EAGAIN`, `EBUSY`, `EBADF`, and unsupported-operation `EINVAL` must not become an unavailable-device offer. Preserve the native cause and return the appropriate retry/internal/capability result. |
| Cleanup-only uncertainty or definite root refusal | Ineligible for A1. Keep charged cleanup retryable. Deferred Minor #7 supplies the ordinary named-owner copy path in S2 (§8). An error at candidate B cannot establish A's eligibility. |
| Future Windows mapping | Design notes only. Missing file/path/device-not-ready, access denial, and I/O must be separate from `ERROR_SHARING_VIOLATION`, `ERROR_LOCK_VIOLATION`, and unsupported operations. Sharing violations pause without fallback under JA-1 `:336`. Today's Windows profile cannot produce a writable tagged A. |

Offer the action after one failed foreground probe. On explicit selection, serialize a
second probe with the same pending attempt and consent. If A settles, return ordinary save
choices. A changed tag, changed selected capture, or changed disclosed loss invalidates the
consent. One re-probe establishes current eligibility, not permanent failure. Timers,
background retries, ordinary Save As, and agent retries never supply consent. The second
probe, capture/choice previews and cancel neither consume slots nor adopt a new source
attempt into session-long rescue retention. Keep the existing ordinary owner and its holds
alive for these reads; any preview reservations remain provisional under ordinary accounting.
Only after that probe, final admission and all required consent does §5's atomic first-mutation
boundary adopt the source. If A settles before that boundary, use ordinary settlement and
consume nothing. If A returns after the boundary, rescue still only reads its frozen retained
sources and writes B; §5's destination receipt/pending settlement and sole own-ticket
exception apply.

An unresolved, receipt-bearing agent command is eligible once its publication attempt has
finished with an unknown outcome. It is not an actively executing save. Freeze/drain GUI,
agent, validation-shadow, and other producers at the accepted cut. Capture the live Core
with `export_history`, current input contexts, retained branches/versions, recovery roots,
receipts, evidence, and lineage together. Do not assemble them from separately changing
queries or copy only acknowledged `branches` (`bundle_copy.rs:382-505`). Keep producers
paused through the initial publication outcome and, on immediate verified success, atomic
handoff. On cancel, definite failure or unknown B, resume producers on the original Core;
unknown B leaves it associated with A and dirty, with B retained as a tagged destination.
A later B settlement never repeats this handoff. An edit arriving during capture must
conflict or be drained and recaptured before consent to a changed result. No task/grant
continues in B by relabeling.

## 3. Owned bytes and actionable refusal

Read retained local stores/state first, then already-retained A file or pinned directory
handles under their original admitted identity. Never re-resolve a caller path or follow an
external hint to manufacture a source. Re-encoding retained decoded state can restore an
existing immutable object only when the exact bytes match its required SHA-256 and length.
New B-specific history/manifests receive new hashes and explicit origin bindings. Verify
every selected immutable object and dependency; cached metadata alone is insufficient.

For opened A, histories may be cached and receipt/issuer/root structs retained
(`bundle.rs:639-681,700-725`). That does not retain all LUTs, fonts, generated outputs,
evidence, old manifests, noncurrent history, or recovery payloads. `StorePin` holds ownership
and accounting (`store.rs:606-625`); input files are not a local mirror. `begin_copy` and
`open_verified` presently verify/reopen the source namespace (`store.rs:2254-2277,630-637`).
S2 needs an explicitly admitted retained-source read path without weakening ordinary guards.

Before B mutation, full-closure admission reports `rescue_missing_owned_bytes` if required
data cannot be verified. §4 alone can turn eligible failures into separate explicit selection
previews; until all choices and final admission succeed, there is no publishable B. Otherwise
return the refusal below. Its bounded entries name object kind, display name when known,
hash, length, retaining uses, failing operation, and native/validation cause. Group current
edit, Undo/Redo with cut/branch, other branch/version, recovery, and evidence dependencies.
Use existing object/diagnostic quotas, report omitted-entry counts, and expose a bounded
continuation over the frozen inventory. A missing manifest or unknown children sets
`dependencies_complete=false`; never imply that a diagnostic prefix is the whole graph.

Person-visible result:

> Rescue copy was not saved. Required project data is unavailable from A: [items and uses].
> [The dependency list is incomplete, if applicable.] Your edits remain in this session.
> Reconnect A or restore these exact objects, then retry. A's save outcome is still unknown.

Keep the original edit open and dirty. If a source becomes unreadable during copying,
publish no root and clean only safe B staging under B's authority; uncertain cleanup stays
charged. Never report publication failure after a possibly completed B5. External source
media gets a separate availability diagnostic. Saving missing current-edit objects as
"unavailable" is outside A1 and needs its own S0 amendment.

## 4. B-only choices and complete closure

Full frozen history, A's last acknowledged-root checkpoint when one exists, and all selected
source recovery roots are the default. History includes its current Document, baseline,
batch/gesture identities, undo, redo, inputs and lineage.
Full admission and every preview precede all B mutation, including missing-parent creation.
The history trim and recovery omission below require **separate explicit choices**; choosing
one never authorizes the other. A noncapacity error at B authorizes neither loss.

A history trim is eligible only when omission alone cannot make B fit and be self-contained.
It still requires a typed capacity/quota refusal or a complete retaining-use analysis proving
unavailable nonrecovery bytes belong only to a removable old prefix of the active history.
The search below must find a legal prefix that addresses the remaining blocker. Recovery-root
blockers are considered separately under item 4 and the choice order below. Missing
current-state, retained Redo, branch/version, receipt or independent evidence bytes cannot be
excused by either choice. An incomplete inventory of the roots that
will remain cannot establish eligibility or self-containment.

Use `bundle_commit::select_history` and its existing oldest-first legal-prefix search
(`:899-1044`, `core/actor.rs:810-887`), not a cut at the current state. Select the earliest
legal cut satisfying B's effective history allowance, full graph/overlap admission and
readability of the final retained graph. For total-byte quota or history-only missing-byte
relief, apply the same ordered search with those additional predicates. A later cut is
considered only when the earlier candidate fails the disclosed bound or still requires a
missing object. This extends admission, not replay/equivalence rules. A lower serialization
size alone proves neither a smaller nor a readable graph.

The result has these exact semantics:

1. B's current Document, content/input object identities, retained batch IDs and lineage
   equal the accepted capture, except for declared destination ownership/head rebinding.
   Runtime revisions and ProjectId/epoch are fresh. Removed prefix commands and their
   oldest Undo entries become B's earlier baseline/floor. Surviving commands replay to
   the exact current content; retain the corresponding per-history input contexts.
2. Preserve the complete redo stack. A legal prefix has empty redo at its cut, and replay
   must produce the original final redo exactly (`core/actor.rs:821,856-879`). A1 authorizes
   no redo loss. Preserve an open gesture's pre-gesture baseline and key; do not close or
   discard it merely to fit. If no legal cut fits, refuse. These rules can limit relief.
3. Preserve independently retained branches/versions, receipts, original durable evidence,
   specialist bases/deltas and their closures. Rebuild B's active history checkpoint,
   floor, input manifests and lineage consistently. Recompute the union from all final
   selected roots. An old LUT is omitted only when no retained root needs it. An original
   manifest retained independently as evidence remains required. Do not automatically add
   the full source manifest as new archival evidence merely to describe the rescue; bind
   its identity as provenance instead. Existing independent evidence cannot be downgraded.
4. Include a checkpoint of A's last acknowledged root, R_ack, by default when A has one,
   whether or not that checkpoint already appears in A's recovery selection. Bind its exact
   root identity/SHA and verify its complete content closure. Use ordinary `save_as`'s exact
   root-byte checkpoint primitive (`bundle_copy.rs:438`), with the following A1 representation.
   `rescue_copy_v1` has one dedicated optional `acknowledged_root` recovery reference, an exact
   `RootCheckpoint` ObjectRef for R_ack. The record is durably pinned in B's manifest, and its
   declared content dependencies include this reference. Copy and verify R_ack's entire
   project-content closure into B before B5. The reference is a live B-owned graph edge,
   distinct from the source root's historical identity/SHA values and omitted-root provenance.
   It carries no A authority, path dependency or temporary-store dependency.

   B's `RecoveryInventory` keeps at most two automatic predecessors, plus its existing staging
   and pre-migration roles. Preserve A's selected recovery roots and complete closures in
   their applicable existing roles, including in a history-trimmed selection. Do not insert
   R_ack as a third predecessor or reuse `save_as`'s `.take(2)` (`bundle_copy.rs:439-442`) to
   discard a selected predecessor. `RecoveryInventory::validate` (`store.rs:1902-1905`),
   JA-1 `:708`, and the v3 format counter stay unchanged. If R_ack already appears in the
   selected inventory, keep the dedicated reference and that inventory role, with ordinary
   hash-based physical deduplication and exact accounting. First save of an unsaved session
   has no acknowledged A root, so the slot is empty with that reason recorded.

   Enumerate R_ack and the bounded source inventory before admission. Use existing root-pin
   and nonrecursive checkpoint traversal (JA-1 `:708-714`, `store.rs:757-758`,
   `bundle_recovery.rs:322-363`), never recursively following an old recovery inventory.
   Metadata, graph-depth, object, byte and overlap limits still apply. Three readable roots,
   R_ack plus two predecessors, are a valid positive selection within those limits.

   On B open, recognize the rescue record's role/schema, cross-check `acknowledged_root`
   against its declared dependency edge, verify its kind, exact bytes/hash/length and complete
   content closure, and reject a missing or conflicting required edge. Use the checkpoint
   content-edge validation at `bundle_recovery.rs:555-577`. B's recovery view enumerates this
   as the source's last acknowledged recovery point, separately from its automatic
   predecessors. It is a recoverable root of B's graph, reached through the record. Recovery
   reads only B-owned verified bytes and uses ordinary validated recovery with B's activation
   identity/fresh authority and rebuilt bounded inventory, never A's writer association.
   Damaged/unavailable checkpoint bytes produce the normal damaged/unavailable recovery
   result, never a silent omission, fallback to A, or an extra predecessor.

   The dedicated reference and its closure stay durably pinned while the rescue record is
   required by B or another retained root. Ordinary B predecessor rotation
   (`bundle_commit.rs:683-688`) cannot retire this independent recovery edge. Ordinary saves
   and later copies preserve it; warning-provenance compaction cannot delete it or turn it
   into a digest-only value. Do not accumulate extra R_ack slots in one record. Any older
   record required by a selected checkpoint's content closure remains independent required
   content under the existing graph/byte/object caps, not new warning-only ancestry.
   Retirement requires the existing owner/pin rules and no remaining required reference;
   no automatic recovery-evidence retirement is added by A1.

   R_ack follows the same unavailable-only omission rule below as every other source recovery
   root. An available R_ack and its closure stay required even if that limits Undo-trim relief.
   If a trimmed prefix used a LUT still needed by R_ack, trimming cannot remove that LUT.
   Independently retained references always remain required. Never omit an available root.

   If the candidate would refuse **only because** some of these source recovery roots or
   objects in their closures are unavailable, offer a separate omission choice. A root
   is unavailable when its own bytes or a required closure object cannot be read and
   hash/length-verified through the admitted sources (§3). Omit **exactly** that identified
   unavailable subset; never omit an available recovery root, omit extra roots for quota
   relief, or omit a current-state/selected-history object by calling it recovery. An
   independently retained reference still requires the same object, even if a recovery root
   needing it is omitted. An unavailable root's unknown children need not be invented:
   its known identity/root SHA and failed verification cause suffice to identify the
   omitted root, but the final retained graph must be completely enumerated and verified.
   In the composed plan below, this test applies to the candidate after the legal history
   trim; omission alone is offered only if the full-history omission candidate passes.

   Record every omitted root's identity/role, root SHA (and ObjectRef length/kind where
   available), failed operation and native/validation cause in B's rescue record as
   **provenance values, not graph edges**. Say: "Older recovery points from A are not in the
   rescue copy" and show the exact roots/reasons. If R_ack is omitted, also say: "The recovery
   point for A's last acknowledged save is not in the rescue copy." Never infer that A lost
   them: A's actual recovery inventory, pins, charges and holds remain unchanged. Recompute B's complete
   closure after omission and verify it; any remaining missing object refuses. This is an
   explicit S0 `:349-354` exception, not a reinterpretation of the old whole-graph rule.

   Do not reinsert an omitted root later through ordinary `save_as` backup construction or
   an archival full-source manifest, including an omitted R_ack checkpoint. All available
   selected source recovery roots, including R_ack in its dedicated record slot, stay.
   B's own prior-destination graph/B3 protection is mandatory and cannot be omitted; if B
   is new, it has no prior destination root. Subsequent ordinary B saves use ordinary B3.
5. Bind the full source capture digest and root/input/lineage/recovery-inventory identities,
   selected-result digest, old/new floor offsets, exact cut/batch identity, removed Undo
   range/count, unchanged redo/open-gesture disposition and each recovery omission in B's
   record. Full-capture identifiers and omitted-root SHAs are historical values, not edges
   to omitted objects or claims that unavailable payloads were verified. The bounded source
   recovery-root list is recorded exactly within existing metadata limits and §7's encoded
   provenance bound; admission reserves it before mutation, never silently summarizes a
   current selection's omissions to make room.

**Choice order and composition.** Analyze the default selection first, without mutation.
Recent Undo history takes priority over unavailable older recovery points.

- If the default fits and is self-contained, offer the full selection with no loss. Otherwise
  identify exactly the unavailable recovery subset, including R_ack if unavailable, under
  item 4. Before offering any Undo trim, provisionally exclude only that subset and evaluate
  the **full-history, zero-cut candidate** with its complete closure, exact rescue record and
  physical overlap. This is read-only planning and supplies no omission consent.
- If that zero-cut candidate fits and is self-contained, offer **omit unavailable recovery
  points** alone, preserving all Undo/Redo. Explain and record each omission. Do not show or
  require a history-trim choice, even when the default also reported a capacity/quota failure
  relieved by the same omission. After explicit omission consent, recompute and authoritatively
  admit the exact result. Capacity alone never makes a recovery root eligible for omission.
- Only if omission alone does not suffice may an eligible **history trim** be offered, alone
  or with omission. Use the existing oldest-first search, provisionally excluding only the
  identified unavailable recovery subset when needed to find the earliest legal fitting,
  self-contained result. Show both losses in advance for a combined plan. Keep every available
  recovery root, including R_ack, and all independent roots in the search. If a current-state,
  retained Redo, branch/evidence or other independent reference still requires missing bytes,
  neither choice permits their removal; refuse if no legal candidate passes.
- For a combined plan, obtain the exact trim consent first, form that trimmed candidate with
  all source recovery roots still selected, and recompute its blockers. Only when its remaining
  refusal is solely the unavailable recovery subset may the separate omission choice be offered
  second. A trim alone publishes only if its own complete selected graph passes; consent to
  trimming never authorizes omission. The shared old-Undo-LUT/unavailable-recovery case remains
  reachable through both choices, with no independently required dependency pruned.
- After all required consents, recompute and admit the final full closure, record and overlap.
  Any changed capture, cut, omission set or disclosed loss invalidates the affected consent;
  reevaluate omission alone before offering a newly needed trim. Do not iterate silently.
  Cancel either choice leaves no B root, consumes no slot and preserves A exactly. If no legal
  selection fits or verifies, return the typed refusal. A conditional preview promises no success.

History preview: "Save a rescue copy with older Undo history removed in B. Undo before
[cut] will be unavailable there. Current edits, Redo, and the open gesture are kept. A is
unchanged." Bind each consent to its preview/capture/result and exact omission-set digests.
Failure or unknown B leaves A's Core, ticket, floor, histories, holds and charges unchanged,
apart from the separately charged B overlap/cleanup. If the final closure, record, retained
available recovery roots or simultaneous copies cannot fit, return a typed capacity/missing-
object refusal. A conditional preview is never a guaranteed-success claim.


### Binding S2 revision 7 qualification: local recovery and L1

[JA-1 §5.4](JA1-STUDIO-ORCHESTRATION.md#54-local-recovery-selection-and-rescue-contracts-s2-revision-7)
is the binding local selection, session-pin, frozen-rescue, retirement, registered-session
and availability contract. It supersedes earlier P/exact pre-floor copy and editable
paused-state rulings. Ordinary Save on L precedes one frozen selection of R at g+1;
normal select_history and its disclosed floor apply. There is no retained-manual role,
read-only working branch, exact-retention bypass or editable durable-no-activation state.

Final L1 supersedes this document's earlier exact-root retention wording only for inherited
rescue linkage. Convert only that linkage to provenance and rebuild its required containers,
manifests and root. Keep the source root SHA as provenance and label "rebound recovery
point". Recovered Document, baseline, batches/history/cursor/Undo/Redo/gesture, input payloads
and identities remain exact. Independent evidence and receipts remain required even when
they still retain an older object. New B activation authority belongs to the wrapper/runtime.
Full checkpoint-edge validation still applies. A graph-valid, fully rehashed non-linkage
Document, batch or input change must fail the exact preservation oracle.

Pin the verified selected root/checkpoint's complete closure with ordinary StorePin at
prepare, before ordinary Save. Resolve by SHA, never automatic-list position. Keep the same
pin/target through rotation, collection, unknown outcomes and quota retirement/retry.
Existing-target retry requires current index membership and expected head; never resurrect
an ID. Exclude that target from offers and direct retirement. Also exclude live attached
sessions, unsaved edits and specialist base/target dependencies until save/close, fencing
and eligibility recheck. Ja1EditingSession registers every durable session create/reopen
with DurableBranchSessionGuard before edits; no usable unregistered route may bypass it.

During either unresolved local-recovery publication, rescue uses the existing frozen O cut.
Cancel/failure/refusal/unknown B leaves O frozen. Immediate verified rescue seals O,
irrevocably abandons R and activates fresh B; late A is destination-only. This qualifies
only A1's ordinary resume rule. It never changes newer B edits or Saved/dirty/association.

Before preview, rescue handoff, Close and every destination change, disclose whether the
chosen point is still durably listed. If held only in-session, abandoning lets it go;
warn in advance if preliminary Save will rotate it out. Refuse Save As/open-another-project
while prepared. Require explicit disclosed abandonment first, only with no unresolved root
publication. Rescue may proceed with its disclosed irreversible handoff. If B lacks the
point, say so; during selection unknown say A keeps it only if selection completed. A crash
can lose a session-only selected point: this is an accepted limit, not durable retention.
Unavailable storage also needs the warning that Rescue copy preserves work if A never
returns and Close may discard its only accessible in-session copy. Never label Close safe.

Format 3 and the two-predecessor validator/rotation remain. Raw nonzero switch generation
requires manual-switch@1; actual rescue role/schema requires rescue-copy@1. Cross-check
actual requirements and declarations as a supported subset before mutation. Generic
OwnedEvidence is insufficient. Generation-zero ordinary v3 remains a positive control.
Shared L/R occurrence tuples, batch IDs and split ancestry survive first and later ordinary
saves and floors. Only genuinely new events get the accepting branch's origin.

C5 selects and copies the content closure separately from automatic recovery inventories.
Every retained recovery role is added explicitly, with R_ack outside the unchanged pair
of automatic predecessors. Only unavailable recovery roles may be omitted. Full-history
omission is tried before the ordinary oldest-first floor. Trim and omission require
separate consent bound to the capture, selected history, omitted roots and target limits.
The rebuilt source manifests never restore an omitted recovery edge. Independent roots
and all inputs needed by current content, Redo and the retained floor remain mandatory.

The mutation boundary is the first destination lock claim or missing-parent creation.
Read-only preview, inspection and cancelled consent consume no source or destination slot.
Every unknown destination retains its own copy owner. After immediate B activation, a
later unknown save on that same retained copy is reconciled as its new attempt, rather
than returning the original copy's earlier committed status. On whole-context Close,
actors are fenced before source reuse ends. Actual readers defer source cleanup; this
teardown does not prove any unknown destination absent or change its captured outcome.

Registered specialists keep their original namespace owner when the manual actor moves
to B. A late proof can settle only that original specialist's own ticket while it remains
live. It cannot change B, activate a capture copy or call the sealed manual A. The full
proposal is retained with its branch checkpoint in the same publication, independently
of the bounded Undo floor. Context Close names any such session with unresolved or
unsaved work before ending its retention.

## 5. Session retention and editing after success

C8's process split is binding in JA-1's "C8 process ownership and retention" and
S0 §4.3. GUI-origin temporary bytes, holds, pins, and their one source ledger stay
in the GUI. Host source access is borrowed and has no cleanup or release authority.
Unproved replacement retention refuses every save and rescue, including fresh paths,
while preserving the GUI's actual source objects and histories until explicit Close.

**Originating session and owner.** The originating session is one app editing context from
New or Open (including a project opened from A) until its explicit Close or app exit. A
rescue switch A→B and ordinary focus/project switches do not start or end this context or
reset its slots. A separately opened project has a separate context; it cannot steal a
retained owner's locks or holds. Opening a late-committed B while this session retains B's
owner is read-only. Editing B in a separate context requires this session to release B's
completed destination writer/namespace owner, only by the person's explicit choice after B
is proven committed and ordinary reader/cleanup safety permits release. This never releases
A's or any rescue source's retained holds/pins/charges/temporary bytes, refunds a consumed
slot, or switches the active edit. This narrow release of B's completed destination owner
qualifies its retained lock lifetime; rescue source retention remains session-long. Explicit
Close performs whole-context teardown, including any active B and retained A states, after warning about unknown destinations and dirty edits.
Closing only an A status view never closes the context; exit is no proof that A never saved.

S2 extends the existing app `ProjectSession` (`crates/kinewright-app/src/project.rs:109`,
owned by `KinewrightApp.projects`, `app.rs:347`) with a JA-1 lifecycle owner field,
`ProjectSession.ja1: Ja1EditingSession` (proposed). That object owns `RescueSessionState`,
active bundle/Core association and all retained owner objects below. It survives replacing
the active association with B, even if the UI replaces a `ProjectSession` wrapper: move the
same owner, never recreate/drop it. Headless/agent-only entry points own the same lifecycle
object from open/new to close. This is added integration, not a claim that today's app already
holds `ProjectBundle`/`SessionStore`; it currently holds `source_admission`. It introduces no
second store, collector, ledger or background service. Only whole-context teardown retires
rescue holds after ordinary reader/cleanup safety work.

Preview/capture reads borrow or provisionally pin through A's existing ordinary owner;
that owner cannot be dropped while the preview uses it. Do not convert it to session-long
rescue retention at preview or selection-time re-probe. After the second probe, all required
consent and final admission, source adoption and slot consumption occur atomically at the
first B mutation boundary specified below. Before that mutation starts, `RescueSessionState`
must own A's actual retained objects; no result may escape or owner be replaced/dropped in
between. Ordinary unknown saves use ordinary S0 ownership until that boundary; a transient
recovery or cancel before mutation leaves those rules and the slot count unchanged. The
adopted objects are:

- A's `ProjectBundle` or first-save `BundleCopy`, its destination namespace/lock and
  `PreparedDestination` ancestor claim, source `SessionStore`/`ProjectStore` and leases,
  original Core and exact `PendingSave`/floor ticket when present, retained until only
  §5's own-ticket settlement or session teardown permits its disposition;
- A's `publication_tag`/`unsettled` and `pending_command`/result, exact
  `PendingCopySource`/`SourceCopyHold`, pins, overlap reservations, charges and temporary
  bytes. Bundle uncertainty still exists without a Core floor ticket.

Each rescue-retained unknown **attempt**, including a later source attempt at the same
active destination, gets an independent entry/hold. Reuse the destination's one namespace
owner, not a second lock. Shared physical objects may share their accounting owner, never
their settleable uncertainty flag. Do not overwrite the one `pending_copy_source` slot
(`bundle_copy.rs:508,515,616`), share A/B's hold, or settle B's failure on A's Core (`:322`).
Neither returned copy handles nor UI views own these holds. Ordinary `settle_copy_source`,
`SourceCopyHold::reconciled` and `Drop` (`bundle_copy.rs:50-68`; `store.rs:2365-2400`) cannot
retire them early.

**Immediate verified success.** Build B with `Core::spawn` and validated `restore_history`
of the selected result and eagerly bind its fresh project persistence proof. `Core::clone`
shares A's actor (`core/actor.rs:201-206`) and cannot be B's Core. B's pending save/floor
belongs to B alone. After B5/history validation, with producers still paused and the capture
unchanged, perform a serialized handoff: an authority-gated actor message verifies A's exact
cut and seals A, then an already-prepared, infallible active-owner attachment binds all GUI,
agent and shadow state to B and issues fresh authority. No fallible rebind follows the seal.
If validation/seal fails, preserve the original edit; classify any durable B as a capture
copy, never as a failed publication or permission to overwrite a newer suffix.

The seal is enforced **inside A's actor**, for every clone, queued message and alternate
entry point. After the seal, refuse edits/DoBatch, Undo, Redo, restore, checkpoint/transition
installation and other user-visible mutations with a typed noneditable result. Queries,
`export_history` and `pending_history_save` remain allowed. Rescue-retained destination
reconciliation is receipt/pending-only for sealed A: it invokes no Core settlement, restore,
install or floor, needs no seal exception and cannot reopen editing. Sealed A's original
ticket/barrier remains session-owned until session end, even if A proves committed. The
own-ticket exception below applies only to the live, unsealed Core. No public
`install_checkpoint` or `install_history_transition` may bypass the seal. Eager proof binding and install-API gating
coordinate with deferred Minor #1 (`s1-deferred-minors.md:7-11`). An accepted suffix before
the seal must prevent the stale-cut handoff; no handle can acknowledge an edit on A afterward.
B's Undo and ordinary Save use B alone. Copied issuers retire; paused tasks need fresh authority.

**Unknown B and late settlement.** Resume producers on the original Core associated with A,
still dirty; do not seal A or attach B. Retain B's tag, captured B Core and actual independent
owners/holds in `RescueSessionState`. B cannot expose a writer or Saved message while unknown.
Edits after this outcome continue in the original actor, subject to A's existing floor/ticket.
Later settlement updates B's destination receipt/pending state without touching its captured,
never-active Core or another attempt's ticket on the active Core. If it proves commit, mark/show
"B holds a rescue copy as of [capture]"; never replace, restore, install history, impose a new
floor or seal the active edit, clear its dirty state, advance its Saved baseline, or move its
writer association. This rule holds
even if histories compare equal or the active edit has since moved to C. The person may
open B separately read-only while this session holds B. A separate editable open requires
explicit release of B's completed destination owner after proven commit, as above; it must
preserve the active edit and does not complete A's pending chain.

**Any rescue-retained destination's reconciliation** settles that destination's own
receipt/pending state against its own proof. Its sole permitted Core operation is the
following S1 exact-ticket settlement on the live, unsealed Core. Every other floor,
`restore_history`, `install_checkpoint`, `install_history_transition` or replacement by
acknowledged destination history is forbidden on every Core, including live or sealed A,
B's captured/active Core and C's. In particular, do not reuse
`receipts.rs:508-523`'s `reconcile_command` sequence: it calls `reconcile_core_save`, then bundle
reconciliation, then unconditionally restores acknowledged manual-branch history without
an expected-before guard. Separate receipt/pending settlement from that restore and gate all
Core calls, including incidental ordinary save/copy/reconcile entry points, by this rule.

The exact-ticket exception has these conditions and effects:

- Bind the retained attempt, destination identity/tag/root SHA, original Core actor and exact
  pending ticket together. Before a Core call, prove that the live, unsealed Core's pending
  ticket belongs to this same attempt. A numeric ticket ID alone is insufficient across
  actors. Do not fetch and settle whichever ticket happens to be current. Another
  attempt/destination's proof never settles it, even on the same active destination. A sealed
  Core and an unknown rescue's captured, never-active Core receive no settlement call.
- When that destination proves this attempt's exact tagged root and required durability,
  commit only its own existing ticket through S1's project-proof path
  (`history_save.rs:31-37`, `core/history_transition.rs:55-62,91-107`,
  `core/actor.rs:258-263,703-766`). This settles a floor already selected by this attempt.
  It does not create a new floor, restore, install or replace acknowledged history. S1 replays
  the current accepted suffix, including edits made during IO, through the ordinary executor
  (`core/actor.rs:724-735`) and refuses unless current content, post-floor Undo and complete
  Redo match (`:736-740`). Only the previously selected, now-proven-durable prefix leaves
  Undo and the baseline/floor advances accordingly. Retained batch/gesture identities and
  post-floor edits survive. The runtime revision may advance as in ordinary S1 settlement.
- If the same attempt instead proves an older/absent root under §6's namespace-continuity
  rules, use only S1's lossless release of its exact ticket (`core/actor.rs:758-759`), without
  advancing the floor or removing any Undo/Redo. An unknown, damaged or unproved namespace
  cannot authorize commit or release. A proof mismatch refuses with the live edit and its
  current ticket unchanged. Replay/equality failure uses S1's lossless failure path and
  `HistoryCommitInvariant` (`:763-765`), preserving the known publication phase and all
  rescue holds; no restore/install fallback or failed-save claim is allowed.
- Settle the exact ticket once. Repeating the same destination outcome is idempotent and
  cannot consume a newer ticket. Keep rescue-attempt status/identity, locks, pins, charges,
  temporary bytes and consumed slots for the session even after the live ticket settles.
  Do not retain an obsolete live barrier that would make the next `begin_floor` fail with
  `BarrierInvariant` (`core/actor.rs:668-676`). Later ordinary saves at reconciled A or active
  B must remain admissible under their ordinary limits, including over-allowance history.

Current content, the accepted suffix, Redo, dirty/Saved baseline and writer association stay
unchanged. Undo/floor changes are limited to that own-ticket commit or release; with no such
live ticket they stay exact. A pending agent command staged on its separate candidate grants
no ticket on the live Core and never authorizes restoring its effect there. Queries and
receipt/pending settlement still work when A is sealed, without any seal exception or ticket
consumption. Tell the person what A actually contains and that current edits are unchanged
(§7). Retain its locks, pins, charges and temporary bytes even afterward. This can block later
Save As back to A and
increase retained-byte/import-quota pressure; S2 measures those costs rather than weakening
the approved session-long retention.

**Four slots, rescue only.** Count A's adopted unknown attempt and each rescue-destination
attempt whose publication outcome becomes unknown. A verified-successful rescue consumes
no destination slot. An ordinary save on active B, including a transient unknown that later
settles, never reserves or consumes a rescue slot or creates a new rescue-attempt record
solely for that save. Preserve any existing rescue record/warning and follow ordinary S0
reconciliation/retirement rules for its own new holds. Previously rescue-retained
holds remain retained. At four consumed slots, healthy ordinary B saves still proceed.

If active B's own destination later becomes tagged/unreadable and the person chooses rescue
from it, that **new source unknown attempt** becomes rescue-retained and counts only when
the next rescue destination's first mutation begins. It is not counted twice if that same
attempt is already retained. During preview, reserve capacity provisionally under existing
accounting; that is neither slot consumption nor session-long source adoption. After the
second probe and all required explicit consent, revalidate the same tag/capture/selection
and exact metadata/record/physical overlap admission. When the first B mutation begins,
atomically convert the provisional source entry to rescue retention, consume its source slot,
and reserve the destination slot. There is no cancellable gap between adoption and mutation
start. This includes a writing authority/lock
claim, missing-parent creation, staging or store writes, whichever occurs first. If the
operation cancels, re-probes to settlement, or aborts before mutation begins, return every
provisional reservation: no new source adoption or slot consumption. Once mutation begins,
a source slot stays consumed even if B later definitely refuses; success/definite refusal
returns only the destination reservation, and an unknown rescue consumes it. A consumed slot
never returns until context end, even after settlement. Count by attempt/tag identity, not distinct destination identity.
At the bound refuse the next rescue before even parent creation; never evict holds or gate
an ordinary save on this bound. Existing byte/object/metadata/cleanup quotas can refuse sooner.
A new rescue target is still fresh/disjoint under §6; repeated source attempts at one active
destination do not authorize overwriting an earlier rescue target.

## 6. Separation and A's independent return

Before `prepare_destination` can create anything (`namespace/linux.rs:148-156`), reject B
at, inside, or containing A's recorded native locator or asset-tree path, every retained
destination's corresponding paths in this chain, or the session temporary root. Apply
component-aware native path comparisons to both claimed spellings and resolved ancestry,
alongside pinned device/inode/root/asset identities. Reject shared asset trees, aliases,
hard-link ambiguity, and unprovable separation. Whenever a retained destination's recorded
parent path does not resolve to its pinned identity, exclude that entire recorded parent
subtree: an absent/removed mount directory, changed device/inode, or unprovable identity all
qualify. Do not recreate a missing mount directory as B's parent on the underlying filesystem;
a remount could shadow B. The exclusion applies before missing-parent creation. An unrelated
sibling on the same proved parent is not excluded solely because that parent is shared.

Try-acquire B's authority without waiting on A. Carry the admitted pinned parents through
every create, object write, B5, and cleanup under JA-1 `:244-256`. A check followed by a
fresh path open is insufficient. Parent/alias swaps after the final check must refuse or
stay bound to the originally claimed disjoint namespace; no write may follow the swap.

For rescue-retained A, a changed parent `st_dev`/inode or a root belonging to B/another
rescue descendant is an identity conflict, not evidence that A's publication failed.
Do not use `bundle.rs:1391-1409`'s current changed-parent release to settle this case.
Require independently proved continuity with A's pinned namespace for absence/older-root
release, or A's exact tagged root/receipt, complete graph, and barrier for commit. If a
remount changes the device identity and continuity cannot be proved, keep A unknown.
Never say "A was not saved" from a changed device alone. This conservative change needs
no new remount authority mechanism. A can remain unknown while editing/saving B continues.
No B record, byte match, new authority, or shared-hold settlement is an A proof.

## 7. Pending agent command and durable rescue record

An agent command pending at A is deliberately **excluded** from B's effect and committed
receipt. In the unknown-publication path, the candidate effect is staged privately and
`install_checkpoint` follows successful publication (`bundle_commit.rs:175-241`). B uses the
accepted live Core cut and previously verified committed receipts. Exclude the tentative
command checkpoint/result from B's selected roots even if staging changed owner caches;
retain all those tentative bytes at A. Preserve accepted live edits without guessing
whether A committed the pending command.
Select independent evidence from the acknowledged manifest, never the mutable staged
evidence vector. This excludes a pending-only comment result and its text/strokes even
when A's tentative root actually landed. Independently committed references still select
their full closure; pending envelope/result holds stay at A. After a failed ordinary or
first-save Retry, the App still runs the phase-aware probe and retains any eligible offer,
while preserving the reconciliation error and pending-command semantics. Rescue handoff
must not leave a Live B actor retained if A's exact seal fails.
A post-B5 internal/tail failure must still be classified by its publication phase; a mixed
or unverified live/receipt state refuses capture rather than fabricating an acknowledged
history. JA-1 `:334-338,348-364` requires reconciliation, never speculative reapplication.

In `rescue_copy_v1`, record the pending command's original project/branch, command ID,
canonical digest, expected head/capture identity, and unknown A publication/root identity.
Keep its pending envelope/result owned at A. B's record says its effect and receipt are
excluded and A may nevertheless contain them. B returns a typed unresolved-origin result
for the original ID/digest, never `Absent`, success, or enqueue permission. Same ID with a
different digest conflicts. Do not mint a replacement command automatically. Retire copied
issuers and require fresh B authority for new work; neither a reconnect nor B's success
acknowledges the original command. If A later proves its receipt or the older root, settle
A's pending item/result and destination receipt lookup exactly once, and report that outcome
in the session's rescue status. Never restore, install or replace acknowledged destination
history on any Core or apply that effect/receipt to the live edit or B. Only §5's settlement
of the same attempt's own live ticket is permitted, never a floor for this staged command
on a different Core. A sealed Core requires no exception: receipt/pending settlement must
complete without calling its mutation APIs.
B's durable capture record remains truthful historical provenance; showing A's later outcome
never invents an acknowledgment in B or silently rewrites the capture record.

If A proves the command committed while the current edit excludes its effect, say: "A contains
the agent command's effect as of [time]; your current edits are unchanged." If A proves the
older root, report that exact observed outcome and the same unchanged-edit assurance. Do not
silently replay the command or the post-capture suffix. A later explicit save of the current
edit back to A is a separate successor publication under ordinary authority, expected-head,
barrier and admission checks; disclose when that successor omits the agent effect now known
to be in A. Receipt reconciliation itself grants no history override or plain-Saved claim;
if those ordinary checks cannot admit the new save, preserve the edit and refuse it.

Store the record as existing `OwnedEvidence` in B's manifest `durable_pins`, with B's
ProjectId, full/selected capture bindings (§4), originating session/project identities,
and each session-retained unknown destination's native locator, pinned parent/root
identity, attempt/tag identity, optional attempted root SHA, command status, and each omitted
source recovery root's exact identity/root SHA/unavailability cause (§4.4). In addition,
`acknowledged_root` holds at most one selected R_ack `RootCheckpoint` reference and its
required content dependency edge (§4.4). Omission/first-save absence is explicit; an omitted
R_ack has only its provenance values and no live reference. Historical identifiers carry
no authority, credentials or graph edges to A. The selected checkpoint edge reaches only
B-owned objects. Finish record data/name durability before B5, validate it on reopen, and
show its warning. Give `rescue_copy_v1` a recognized schema/role
at its evidence attachment and declare required API `rescue-copy` version 1 in the existing
required field/kind/API inventory. `OwnedEvidence` support alone does not declare these
semantics. The reader must recognize/validate the record and its role, then enforce warning,
unresolved-origin lookup, dedicated-checkpoint recovery/retention and later-copy behavior.
S0 §3.3 support admission derives required semantics from actual raw attachments/records
and cross-checks declarations; a missing,
mismatched or unsupported declaration refuses writes/resume before any caller mutates
session/sidecar/store state. Read-only inspection may report unsupported semantics. This is
a v3 support requirement, not another format counter, version predicate or store. Large
capture inventories bind by digest, not an unbounded inline list. Later copies preserve
the warning's meaning under this bounded representation, rather than recursively pinning
prior warning records solely for provenance. Compaction preserves the dedicated recovery
edge and any older record independently required by a retained content closure (§4.4).

The representation keeps at most 16 exact destination-attempt entries and 64 KiB encoded
provenance (including the dedicated recovery reference and exact current-capture recovery
omissions), with at most four rescue-retained attempt entries from the current originating
session. Repeated destination
identities do not merge attempt entries. Current-session entries are never summarized while
retained; reserve their exact bounded encodings before mutation under ordinary metadata/path
admission. Flatten inherited entries and keep the
newest that fit the remaining count/bytes. Fold older entries into one fixed-size summary
with a saturating count, a count-overflow flag, a rolling digest, and an explicit "earlier
copies may remain unresolved; details unavailable" warning. The summary cannot prove
settlement. Inherited-provenance overflow alone never refuses a save or drops an independent
evidence/receipt root or the dedicated R_ack recovery edge/closure; ordinary complete-graph
quotas still apply. The single dedicated recovery slot is outside the 16 attempt-entry count
and is not another `RecoveryInventory` entry. Compact unavailable old command detail to
unresolved/retired status, never new execution authority or inferred success.
Retain unresolved current-session command detail within existing receipt/metadata limits.

On immediate verified success and handoff say: "Rescue copy saved to B. Editing continues
in B. A may also contain a copy from this session; its save outcome is unknown." Include
the exact Undo loss, "Older recovery points from A are not in the rescue copy" with exact
omissions, and excluded pending command when applicable. After unknown B say that editing
continues in the original session and B's outcome is unknown. On later commit say: "B holds
a rescue copy as of [capture]. Editing continues in the current session; you can open B
separately read-only." Explain that editing B separately requires the person's explicit
release of its destination owner after proven commit (§5). Preserve the warning about A
and disclosed losses. Unknown B gets no Saved message or escaping writer (`bundle_copy.rs:117-124`). A destination outcome of
`NeverAttempted`/`DefinitelyRefused` creates no publication tag or consumed destination slot, even if staging cleanup is uncertain. Preserve monotonic post-B5 outcomes.

## 8. Consequential edits and S2 integration

These are proposed replacements/qualifications, not edits made by this document. Search
covered S0, JA-1, the S1 plan, rulings, and reports at the source SHA above.

| Source file:line | Required disposition |
| --- | --- |
| `docs/JA-S0-CONTRACTS.md:100-120` (§3.2) | Add persisted-inventory member `rescue_copy_v1`: recognized schema/role on root-reachable `OwnedEvidence` in `durable_pins`, B/session/origin and full/selected capture bindings, one dedicated optional R_ack RootCheckpoint/content edge with complete B-owned closure and recovery semantics, bounded exact omitted-recovery identities/SHAs/causes, unresolved destination-attempt/command identities and bounded warning provenance; JA-1 owner, S2 first writer. |
| `docs/JA-S0-CONTRACTS.md:178-184` (§3.3) | Apply existing raw-field/graph-derived required field/kind/API admission to the record, its declared role/schema and `rescue-copy@1` API. Cross-check the dedicated checkpoint reference against actual content edges and validate its complete closure. Generic opaque evidence support is insufficient. Missing/mismatched declarations or unsupported semantics refuse writes/resume at callers before mutation; no second version predicate. |
| `docs/JA-S0-CONTRACTS.md:274-276` | Append: "A1's separately consented B-only history trim and omission of unavailable source recovery roots may remove only edges exclusive to those selections; all remaining independent evidence edges stay required. Its bounded rescue-provenance transformation may replace older warning records with a counted unresolved summary, never independent receipt/evidence roots or current-selection omissions. Quota exhaustion alone authorizes no silent loss." |
| `docs/JA-S0-CONTRACTS.md:349-354` | Append: "A1 rescue preserves the complete closure of its selected history, with only §4.3's disclosed B-only oldest-first trim, and includes A's last acknowledged-root checkpoint by default when one exists in the rescue record's one dedicated recovery-reference slot, with complete closure and reopen/recovery validation, outside the unchanged two-predecessor inventory. Rescue may, with explicit consent, omit unavailable source recovery roots and records them. Record each omitted root's identity, root SHA and unavailability cause as provenance; never omit an available root or any dependency still needed by current state, selected history or independent roots. Evaluate full-history omission alone before offering Undo loss, and offer omission alone if it fits and is self-contained. Recompute closure after omission." |
| `docs/JA-S0-CONTRACTS.md:376-386` | Qualify exact history/required source closure with §4's separate trim and unavailable-recovery omission choices, in the stated order. Retain the capture freeze through the initial outcome, authority-before-mutation, complete final selected closure and prior-destination protection. Unknown B resumes the original dirty edit. |
| `docs/JA-S0-CONTRACTS.md:388-391` | Append: "Only immediately verified A1 success with validated handoff seals A's actor and associates editing with B's fresh authority/Core. Unknown B keeps the original dirty edit; later B commit is a capture copy and never changes active editing. A's authority/Core and pending state remain session-owned; no A authority transfers. Reconciliation of a rescue-retained destination settles its receipt/pending state and may only settle that same attempt's own ticket on the live, unsealed Core through A1 §5's S1 exact-ticket path, preserving content, post-floor Undo and Redo. No other floor, restore, install or history replacement on any Core is allowed; sealed A's ticket stays until session end. A separate B open is read-only while its owner is retained; an editable open requires explicit release of B's completed destination owner after proven commit." |
| `docs/JA-S0-CONTRACTS.md:368-374,393-398` | Define originating session for New/Open through explicit Close/exit and the S2 `Ja1EditingSession` owner in §5. A1 overrides ordinary retirement: all rescue-retained holds/source temporary bytes survive this context, switches, reconciliation and last-reader retirement. Four slots limit rescue-retained attempts only; ordinary saves remain ordinary S0. Previews/re-probes/cancel consume nothing; source adoption and source-slot consumption occur atomically only when B mutation begins, after the second probe and explicit consent. The explicit release of a proven-committed B's completed destination owner permits a separate editable open without retiring rescue source holds. |
| `docs/JA-S0-CONTRACTS.md:403-406` | Replace exactly as §1. |
| `docs/JA-S0-CONTRACTS.md:408-416` | Retain whole-owned-graph/LUT and design-only meaning; A1 never exempts a dependency still reachable from selected roots. |
| `docs/JA-S0-CONTRACTS.md:859-866` | Label last-reader retirement as ordinary handoff. Replace "before acknowledgment/retry" with "before acknowledgment or retry at that destination; A1 independent rescue has separate S2 witnesses". |
| `docs/JA1-STUDIO-ORCHESTRATION.md:151-158,244-256` | Retain ordered history equivalence, gesture baseline, and pinned IO. A1 confines rescue-selection floors to B's fresh actor; later reconciliation may settle only its same attempt's already-selected live ticket under §5. |
| `docs/JA1-STUDIO-ORCHESTRATION.md:708-714`; `crates/kinewright-project/src/store.rs:1893-1915`; `crates/kinewright-project/src/bundle_commit.rs:683-688` | Keep the two-predecessor limit, validator, ordinary rotation and nonrecursive checkpoint rule unchanged. A1's R_ack lives as one dedicated recovery reference in the durably pinned rescue record, an independent pin under :712-713. Ordinary rotation/compaction cannot retire its required closure; B open/recovery validates it. No v3 format-counter change. |
| `docs/JA1-STUDIO-ORCHESTRATION.md:260-270` | Append A1's separate selected-history/unavailable-source-recovery choices, complete recomputed closure, path/temporary-root separation (including absent mount directories), session retention, and immediate-success-only attachment to B's fresh Core/authority with A's actor seal. Late B commit never switches editing. Ordinary same-project aliases remain normal-save/no-op; rescue aliases refuse. |
| `docs/JA1-STUDIO-ORCHESTRATION.md:334-338` | Append: "A1 permits explicit independent rescue after eligible refused reconciliation. It acknowledges B only; A's pending command/effect is excluded and named as unresolved, never reapplied or acknowledged there. Later rescue-retained destination reconciliation updates that destination's receipt/pending state and may settle only its own exact ticket on the live, unsealed Core under A1 §5. It never restores/installs/replaces acknowledged history or imposes another floor on any Core; sealed A's ticket remains. Disclose observed contents while current edits remain unchanged. A subsequent explicit save is a separately admitted successor and must disclose omission of an agent effect known committed at A." |
| `docs/JA1-STUDIO-ORCHESTRATION.md:348-368` | Retain receipt precedence/caps. Add A1 unresolved-origin lookup before fresh-command admission; record compaction never turns an unknown/retired ID into absent. |
| `.review/ja/ja1-s1-plan-rev2.md:373-377` | Replace "blocks the next mutation" with "blocks mutations at that unresolved destination and ordinary attempts from its owner; A1 alone allows an independent rescue with separate authority/Core". |
| `.review/ja/ja1-s1-plan-rev2.md:554-568,592-597` | Qualify source closure with §4's separately authorized history result and unavailable recovery-root omissions, bound as provenance; copy/verify the recomputed complete closure. Add A1 session-long retention overriding ordinary last-reader retirement. A's last acknowledged-root checkpoint is selected by default in the rescue record's dedicated recovery slot, outside the unchanged two-predecessor inventory and durable across ordinary rotation, under the same unavailable-only omission rule; every available selected source recovery root and B's prior destination remain protected. Omission alone is evaluated before a trim is offered. |
| `.review/ja/ja1-s1-plan-rev2.md:569-575` | For A1, associate editing with B's fresh actor only at immediate verified handoff; seal A in-actor. Unknown B leaves the original dirty edit; later commit only records/shows a capture copy. Qualify reconcile-before-another-copy with A1 and its rescue-only slots consumed only at the first mutation boundary after re-probe/consent; retain A's actual owner independently. Reconciliation permits receipt/pending settlement plus only §5's same-attempt exact-ticket settlement on the live, unsealed Core, never restore/install/replacement or another floor; a retained B opens separately read-only until explicit release of its proven-committed destination owner. |
| `.review/ja/ja1-s1-plan-rev2.md:381-388,518-522,905,1263` | Retain inventory closure, original command/receipt semantics, and ordinary-copy witnesses. A1 omits only explicitly consented unavailable source recovery roots as provenance and never acknowledges A's uncertain command. Add separate rescue witnesses. |
| `.review/ja/task-ja1-s1-fix-r4.md:51-55`; `.review/ja/task-ja1-s1-fix-r5.md:38-75,94`; `.review/ja/task-ja1-s1-fix-r6.md:41-60,95-108` | Preserve historical rulings/withdrawals. Note A1 supersedes only the global prohibition and adds its retained-A reconciliation rule (§6). One pending floor per actor remains. Rescue never releases A's barrier to authorize an ordinary copy; reconciliation cannot restore/install/replace history or impose another floor on any Core. Only §5's S1 settlement of that same attempt's exact live ticket is allowed after its own proof, never on sealed A or B's captured Core. |
| `docs/reports/ja1-s1/README.md:191-193,481-486`; `docs/reports/ja1-s1/fix-r5.md:30-41,63,193-195`; `docs/reports/ja1-s1/fix-r6.md:12-16,31` | Preserve as S1 evidence/history. Add "A1 specifies S2 rescue; unavailable until implemented and gated." Last-reader retirement is ordinary-handoff evidence; unreadable-graph equality still proves neither commit nor release. |
| `docs/reports/ja1-s1/README.md:470`; `docs/reports/ja1-s1/fix-r5.md:37-38,63` | Mark changed-parent release as the S1 rule. For rescue-retained A, §6 instead preserves unknown state on unproved remount identity or B/descendant bytes. |
| `.review/ja/s1-deferred-minors.md:7-11,12-15,23-27,31-33` | #1 eager Core binding/install gating must cover the actor seal. #2 retains committed/unknown publication phase on `HistoryCommitInvariant`. #6 retains/charges all rename leftovers for cleanup retry. #7 lands with A1 in S2 so definite-root-refusal/uncertain-cleanup at a named owner permits an ordinary fresh-destination copy without settling unreadable A first. It consumes no rescue slot and creates no A publication tag; A's actual cleanup authority/charges remain held until safe retirement. Replace the closing amendment question with A1's actual S2 disposition. |

S2 touches `bundle_copy.rs:35-79,125-166,296-350,359-616` for explicit rescue, frozen live
capture, independent actors/holds, two consented closure selections, rescue-only slots and
immediate-only handoff. It places selected R_ack in the rescue record's one dedicated
recovery-reference slot and content dependencies, never in a third predecessor slot or by
truncating the selected predecessors with `.take(2)`. Preserve existing inventory validation
and rotation; respect the exact unavailable-only omission set and never reinsert an omitted
R_ack or other source recovery root. Evaluate omission alone before Undo trim. Its late
`Committed`/`into_destination` path must not auto-attach B; a separate open stays read-only
until the person explicitly releases proven-committed B's destination owner. Do not bypass
ordinary reconcile-first globally. `store.rs:630-637,2247-2400` supplies retained verified
reads and session-long holds under existing accounting. `bundle.rs:700-730,984-998,1313-1412`,
`bundle_commit.rs:175-241,414-475,816-875,899-1044`, `bundle_recovery.rs`, and namespace
adapters own capture/record/open, pending commands, closure selection, structured probes,
path separation, and publication outcome. **`receipts.rs:508-523`** must split rescue-retained
receipt/pending reconciliation from its unconditional `core.restore_history` and preceding
`reconcile_core_save`; cover live and sealed A without a seal exception. Likewise audit
`bundle.rs:1313-1328,1367-1381` and `bundle_copy.rs:125-166`: a destination may reach S1's
`history_save::settle` only with its own retained attempt's exact live ticket and proof, not
whatever pending ticket an incidental caller finds. `history_save.rs:31-37`,
`core/history_transition.rs:55-62,91-107` and `core/actor.rs:668-766` retain S1's exact-ticket
commit/release and current-suffix equivalence checks. Gate sealed A and B's captured Core
from settlement, reject cross-attempt proof, and forbid every restore/install or other floor.
Keep the actor seal enforced across all mutation/install paths. `store.rs:1893-1915` and
`bundle_commit.rs:683-688` keep the two-predecessor count/rotation unchanged. Record attachment,
manifest dependency declarations/durable pins and `bundle_recovery.rs:305-375,555-582` must
validate and expose R_ack as a recoverable root through the rescue record, preserve its
independent pin across ordinary B saves/recopy/compaction, and verify closure on reopen. `bundle_records.rs:87-132` and `bundle_recovery.rs:138-143,580-582`
need recognized rescue semantics/support admission beyond opaque evidence. App `project.rs`
and agent/headless save flows attach the §5 lifecycle owner, explicit choices, typed
diagnostics/lookup, producer fencing, immediate B attachment, destination outcome messages with §5's exact-ticket distinction,
late-copy-only status/read-only open, and the explicit proven-B release choice. Preview
resources remain provisional; first-mutation admission owns the atomic source adoption/slot
transition, before any parent/lock/store write, rather than burning slots at capture.

The withdrawn abandon action cleared A's barrier and detached it before another ordinary
copy. A1 amends the contract and retains A's actual owner/Core/authority instead. Only B's
immediately verified handoff switches the active edit, and A stays independently
owned/reconcilable. A later commit after an unknown B never repeats that switch.

## 9. Required S2 witnesses

These are specifications, all **NOT RUN** here. Use actual save/copy/open and agent paths,
distinguishable object bytes, exact histories/tickets/locators, and per-object pins/charges.
Every listed mutation needs its targeted runtime red and restored green. Controlled local
filesystem/fault evidence, physical-device removal, storage-crash persistence, and native
Windows are separate lanes. Windows success remains unsupported, not a portable test claim.

| ID | Control/assertion and targeted mutation |
| --- | --- |
| W1 | First-save unknown A on a supported external filesystem, then removed A: ordinary Save As refuses; explicit B copies retained local bytes, reopens independently of A/temp and immediately switches editing. Repeated ordinary B saves retain its ProjectId and consume no slot. Mutate A gating B, stale acknowledged history, A-authority transfer or counting ordinary saves. exFAT/FAT and current Windows refuse before writes. |
| W2 | Opened A with separately missing current, Undo/Redo, branch, recovery, manifest and evidence objects: typed bounded refusal/incomplete-child flag, no B root unless §4 explicitly permits a selection. Reconnect exact objects for retry. Nonprivileged permission controls: revoke read/search on an ancestor above already-pinned directories, leaving object directories/search and files readable, and require retained-handle rescue success/hash checks; revoke search on the actual objects/assets directory used by retained-relative opens and require typed EACCES refusal for a still-required object. Mutate hint substitution, diagnostic omission, incomplete publication or pretending a pinned directory bypasses its own permissions. |
| W3 | Transient failure: action never happens silently, selection re-probes, recovered A settles normally before rescue adoption. Snapshot slot count and ordinary holds before selection; a second probe that settles A leaves both under ordinary rules, with no rescue-retained source entry or B mutation. With A still unavailable, cancel at both the full and loss previews: no source adoption, consumed slot, or B mutation; provisional resources retire under ordinary accounting. Repeat glitches/cancels, then perform a real rescue to show capacity remains. Changed tag/capture invalidates consent. Inject each excluded cause, including invalid/unsupported sync, interruption, contention, cleanup-only uncertainty, and an error at B. Mutate classifier/consent exclusions or source adoption/slot consumption at preview or re-probe. |
| W4 | Full over-cap rescue refuses before mutation; cancel preserves A exact history/ticket/holds. Explicit oldest-first trim preserves open gesture, complete Redo, surviving input/batch/lineage identities and reopened current content; never floor at now. An Undo-only LUT disappears only if no final root needs it; an available recovery/branch/evidence reference prevents loss. Cover history-only missing-byte relief, total quota, irreducible baseline and one-byte/exact-cap admission. Mutate silent trim, floor on A, lost Redo/gesture, independent-reference pruning or automatic full-source-manifest retention. W13 supplies the composed recovery-only and trim-plus-omit cases. |
| W5 | Race input at capture/before B5: drain one accepted cut, reject later arrivals as retryably paused; a bypassed accepted suffix conflicts before handoff and stays active, with any durable B labelled a capture copy. After immediate switch, send DoBatch/edit, Undo, Redo, restore_history, install_checkpoint and install_history_transition through retained A clones/shadow handles: typed noneditable, no content/history change. Queries/export and destination receipt/pending settlement still work on sealed A without any Core history settlement or a seal exception; its ticket remains until session end, and A stays sealed (W9/W10). Mutate each missing actor seal/install gate, shared A/B actor, stale-cut handoff or reconciliation-only mutation bypass. The normal post-freeze acceptance premise in round-1 Opus m8(a) remains rejected under S0 :376-377 / JA-1 :260. |
| W6 | Reject equal/inside/containing paths and shared asset trees against A, every retained destination and temp. Remove A's mount with (a) underlying directory present on another device and (b) mount directory itself absent; reject B anywhere in the recorded parent subtree before recreating parents, even on supported root ext4. Include unprovable-parent identity and remount-shadow control. Swap aliases/parents before/after the final check through publication/cleanup: refuse or stay on pinned disjoint namespace. Mutate device-only exclusion, ignoring absent parents, path-only/identity-only admission or reopen-by-path. |
| W7 | Unknown A without a floor ticket still retains actual bundle/copy state. Unknown B exposes no writer/Saved; explicit C records A/B. Fill four slots with distinct unknown rescue attempts, settle one, and refuse the next rescue before mutation; success/definite refusal returns destination reservations. W3's re-probe settlement and full/trim/omission-preview cancels consume no slot and never adopt new rescue source holds. At the first B mutation, after the second probe and required consent, assert atomic source adoption/slot consumption plus destination reservation before parent, writing lock claim or store writes; a definite refusal after mutation retains only the consumed source slot. Add the reachable repeated-source-attempt sequence below to defeat destination-keyed accounting. Ordinary active saves never reserve/consume slots, including three transient unknowns independently settled then a fourth ordinary save, and continue at four consumed rescue slots. Staging+cleanup refusal has no publication tag/consumed destination slot. Mutate single-slot overwrite, shared uncertainty, destination deduplication, settled-slot reuse, counting rescue success, ordinary-save reservation/counting, early adoption/consumption at preview/re-probe/cancel, or adoption/admission after mutation starts. |
| W8 | Drop returned A/B copy objects and A views, retire readers, clear caches, switch to B, and reconcile A: session-owned A namespace lock/ancestor claim, exact leases/pins/charges/temp bytes survive. Session teardown performs safe exactly-once retirement. Mutate each early-drop/retirement path, double charge shared bytes, or uncharge distinct copies. Measure post-reconcile contention and retained-byte/import-cap costs. |
| W9 | A returns with own/older/absent/damaged graph; reconcile only A before any separately admitted write there. Changed-device remount or a moved B root at A's locator cannot settle A or say not saved without continuity/proof. Receipt/pending settlement permits only §5's own-attempt exact-ticket commit/release on the live, unsealed Core: content, suffix, post-floor Undo and Redo stay exact; only a proven durable prefix may leave Undo. No other floor/restore/install/replacement or cross-Core/cross-attempt settlement is allowed. With no matching live ticket, all history/floor state stays exact. Sealed A stays sealed and its pending item can still clear without a seal exception (W10). Mutate changed-parent release, shared Core, B-proof settlement, reconciliation-driven extra floor/restore, cross-attempt ticket settlement, settlement-unseals-A or early retention release. |
| W10 | Pending agent command at unknown A remains eligible. B reopens without its tentative effect/receipt, names ID/digest/exclusion, and original-ID retry is unresolved/conflicting without execution. Live case: B returns unknown; resume the original Core and make distinguishable further edits. Keep this staged-command control free of a live floor ticket, so SaveInProgress cannot mask the restore mutation; W16 separately covers own-ticket floors. A returns and proves (a) the command committed and (b) the older root in separate runs. Its receipt/pending item/result settles exactly once and reports A's observed contents; original actor/content, exact Undo/Redo/history suffix/floor, writer association and dirty/Saved baseline all survive unchanged. A repeated reconcile is idempotent. Sealed case: after immediate verified switch to B, repeat both A outcomes; A's pending item clears without touching any Core, A stays sealed, and B edits/history remain exact. A later explicit save back to A is a separate successor, disclosing when it omits A's known agent effect, with no implicit replay or bypass of ordinary admission. Mutate pending-command blanket refusal, tentative-effect inclusion, forged receipt acknowledgment, absent lookup, automatic replacement/reapplication, double settlement, or restoring A's acknowledged history onto the live Core through reconcile_command; also mutate restore/install/extra floor on sealed A or B, settlement of sealed A's retained ticket, a different attempt's ticket settlement, or a seal exception used to clear the pending item. |
| W11 | Record, separate Undo/recovery-loss disclosures and A warning survive supported reopen/recopy. More than 16 inherited attempt entries or 64 KiB compacts to counted unresolved warning while current-session attempts/current-selection omissions stay exact; no recursive warning-only old-record retention or provenance-overflow-only refusal. Preserve any record/closure independently required by R_ack or another retained root. Also run W14 through reopen/recopy/compaction with W10, retaining independent receipt/evidence roots. Mutate record omission/corruption, pre-record B5, unbounded ancestry, summary-as-settlement, dropped exact omissions or retired-ID reactivation. |
| W12 | Post-B5 `HistoryCommitInvariant` preserves committed/unknown phase (#2); non-`EACCES` rename leftovers retain exact charge and retry (#6). Named-owner definite root refusal plus uncertain cleanup, followed by unreadable A, can ordinary-copy readable retained bytes to B with full history (#7), or gives §3's typed missing-byte refusal. No A1 action/tag/barrier appears. Mutate post-B5 definite-failure mapping, cleanup tracking, or named-owner reconcile-first obstruction. |
| W13 | Opened A has readable R_ack not already checkpointed and two readable selected predecessors P1/P2, with sufficient B capacity. Rescue succeeds and independently reopens/recovers after A and temp become unavailable: B has exactly P1/P2 in its two-predecessor inventory plus R_ack in rescue_copy_v1's dedicated recovery reference, with complete verified B-owned closure and no extra inventory entry. Advance ordinary B saves until P1/P2 rotate out: R_ack remains reachable/recoverable via the durable record, including through recopy/warning compaction. Corrupt or remove its edge/checkpoint/required child and require validation failure rather than silent omission. A trim that removes the Undo path to R_ack's state still keeps available R_ack and any shared LUT; insufficient relief refuses rather than dropping it. Repeat with R_ack already checkpointed. In unavailable-recovery-only cases, including unavailable R_ack, default refuses; cancel preserves A; separate exact omission consent succeeds with full history and root/SHA/cause provenance plus the last-save warning when relevant. An available recovery root is never omitted and A's inventory/holds stay byte-for-byte identical. Capacity control: the default candidate is over-cap but full-history omission of only unavailable recovery roots makes B fit and be self-contained; offer omission alone with zero cut, no trim preview and complete Undo/Redo. Mixed case: unavailable old Undo-prefix LUT is also used by unavailable recovery roots (including R_ack when it needs that LUT); omission alone still fails, so conditional oldest-first trim preview/consent precedes a separate recovery-omission choice. Only both choices yield verified self-contained B. Cancel either, or keep an independent branch/evidence/selected-Redo reference, and no B publishes. Cover capacity-triggered trim plus omission and unreadable omitted roots/closures. Mutate omitted/default-digest-only R_ack, reuse of save_as's `.take(2)` that drops P2, count-bound refusal of R_ack plus two predecessors, weakening the two-predecessor validator, retirement through ordinary rotation/compaction, skipped record-edge/closure validation, dropping an available root for quota, silent omission, an unnecessary trim when omission alone fits, treating one consent as both, failing closure recomputation, independent-data pruning, or save_as reintroducing an explicitly omitted R_ack/predecessor. |
| W14 | Supporting reader validates rescue_copy_v1 schema/role/API and its dedicated checkpoint edge/complete closure, reopens/recovers B with R_ack outside the automatic inventory, shows warning/losses, preserves unresolved-origin lookup and recopies/compacts under W10/W11. A reader lacking rescue-copy@1 refuses writes/resume before app/headless/agent session, sidecar, journal or store mutation even though it supports OwnedEvidence; read-only reports unsupported. Missing/mismatched inventory declaration or unrecognized role/schema also refuses from actual raw graph/record validation. Mutate omitted support declaration, skipped raw cross-check, generic-evidence acceptance or admission after caller mutation. |
| W15 | B returns unknown; record original active actor/history/dirty/Saved baseline, resume producers there and make distinguishable new edits. B later proves commit through destination receipt/pending settlement: its captured Core receives no call, no Core is restored/installed or given a new floor, and B's proof cannot settle A's or any other attempt's ticket; active actor, content, history suffix/floor, writer association and dirty/Saved baseline survive exactly. B is labelled a copy as of capture and opens separately read-only while retained; writable open refuses until the person explicitly releases its completed destination owner after proven commit. Release preserves the active edit, consumed slots and A/source holds. No release/editable B is offered while B is unknown. Repeat with active C and equal histories. Also run W10's pending-command-at-unknown-A, unknown-B, further-edits sequence with A later proving committed or older: both A and B reconciliation orders preserve the no-ticket control's exact history and settle only each destination's own pending state. Compose with W16 for a live own-ticket commit/release, allowing only its proven prefix/floor change while preserving content, suffix, post-floor Undo and Redo. Repeat A reconciliation after a verified switch to B with A sealed: pending item clears once with no Core call or seal exception. Mutate late into_destination/attachment, A-history restore onto live Core, restore/install/new floor/seal of any Core, a different attempt's ticket settlement, settlement on sealed A, dirty reset, Saved-baseline advance, writable open under retained B's lock, or automatic/unproven destination release. No automatic-switch exception on equality. |

W7 repeated-attempt sequence (all in one originating editing context, using fresh disjoint
rescue targets; every adoption below occurs only when the target's first mutation begins,
after the second probe, explicit consent and admission): unknown A is adopted (slot 1),
immediate rescue to B succeeds (no destination slot). Ordinary save B-u1 becomes unknown
(still only slot 1); explicit rescue C1 adopts B-u1 (slot 2), begins mutation, then definitely
refuses (destination reservation returned). Independently
settle B-u1 and continue ordinary editing/saves at B. A new ordinary B-u2 becomes unknown;
explicit rescue C2 adopts B-u2 (slot 3) and C2 itself becomes unknown (slot 4). B-u1/B-u2
have distinct attempt/tags and holds despite one namespace owner. Include over-allowance
B-u1/B-u2 histories with their own live floor tickets: after each destination's own proof,
§5's exact-ticket settlement preserves the current suffix/Redo and permits the next ordinary
floored save. Do not sidestep this with a within-allowance-only fixture. Settle C2 later: active B
is unchanged and slot 4 stays consumed. With B-u2 independently settled, ordinary B saves
still succeed; any further rescue refuses before mutation. Destination-keyed accounting
would count only A/B/C2 and incorrectly admit another rescue. This applies Astra R2-m2 to
rescue-retained attempts; its proposed ordinary-save counting/refusal is superseded by
rev3 ruling 2. Reusing a prior rescue target is not needed and remains prohibited by §6.

W16 over-allowance save continuation (N2). Start with an unsaved session whose history
exceeds the admitted allowance and first-save it to A with an unknown publication, leaving
A's exact ready ticket on the live Core. Rescue to B returns unknown, resume that original
Core, and accept distinguishable later edits. Use separate legal cases for nonempty Redo and
an open gesture with its pre-gesture baseline; do not require an impossible combined cursor.
A later proves its own committed tagged root. Settle only A's own ticket once through S1,
assert the exact replay/equality oracle for content, suffix, post-floor Undo and all Redo,
the expected floor/prefix change, and unchanged dirty/Saved baseline and writer association.
A repeated reconcile is idempotent. A later ordinary save at A succeeds, including a required
new floor, and independent reopen preserves every retained edit. Repeat with proved
older/absent A under the same pinned namespace: release only that ticket, retain every
original Undo/Redo entry and floor, then save at A successfully. Run the immediately verified
rescue-to-B variant too: A seals and keeps its ticket until session end, while ordinary saves
and new floors at active B succeed without touching A.

At active B, make an ordinary over-allowance B-u1 unknown, then adopt it by explicit rescue
to fresh C. Run both definite C refusal after mutation and unknown C outcomes, continue edits
on B, and let B-u1 prove committed or older in separate runs. Only B-u1's own live ticket
settles once, with the corresponding commit/release oracle. Later ordinary saves at active B
succeed and reopen without lost edits or `BarrierInvariant`. C's late commit cannot settle
B's ticket, restore a Core or switch the active edit. These cases also exercise W7's repeated
attempts without its former within-allowance restriction.

Targeted mutations for W16: leave the ready ticket unsettled and observe the later save
refuse; settle a different attempt's ticket (including matching numeric IDs on distinct
actors and a later ticket at the same destination); use B/C proof to settle A/B's ticket;
restore acknowledged history instead of settling; bypass replay/equality checks; settle
sealed A or the never-active captured B/C Core. For the restore mutation, retain W10's
no-live-ticket control as well as W16's later-save oracle, so an existing barrier cannot mask
the edit-loss hazard. A mismatched attempt must leave both owners' ticket identities and
histories exact, never consume the newer ticket. Each mutation needs its targeted red and
restored green; all these runtime witnesses remain NOT RUN.

No open lead question remains. The findings below use reviewer prefixes to keep their IDs
distinct. Fixes are design dispositions only; none claims runtime acceptance.

## 10. Round-1 review finding dispositions (updated by rev5)

| Finding ID | Disposition |
| --- | --- |
| Astra M1 | Fixed in §§4-5, W4-W5. Legal oldest-first B-only selection, exact redo/gesture/input/lineage result, full closure recomputation, full/selected bindings, explicit loss, cancel/failure invariance, and irreducible refusal. |
| Astra m1 | Fixed in scope/§3, W2. Opened-A limits, typed object/use/hash/length/cause diagnostic, incomplete inventory, retained dirty edit, and reconnect control. |
| Astra m2 | Fixed in §2, W3. Phase/tag/native-cause classifier, excluded errors, and Windows design-only mapping. |
| Astra Q1 | Fixed in §5/§8, W8. Actual session ownership outlives copy/view drop and ordinary hold retirement. |
| Astra Q2 | Fixed in §2, W3/W5. Consent bound to one attempt/cut, one re-probe, no silent fork, return race safe. |
| Astra Q3 | Fixed in §6, W6. Pre-creation path and identity separation across the chain, with pinned operations through cleanup. |
| Astra Q4 | Fixed in §5, W7. Four rescue-retained unknown attempt slots including adopted A; rev4 binds source adoption/consumption to the first B mutation after re-probe/consent, with preview/cancel free, settled-slot retention and no success consumption. Rev3 ruling 2 excludes ordinary saves; W7 tests repeated rescue-adopted source attempts. |
| Astra Q5 | Fixed in §§2,4-6, W5/W9. Fresh actor, one accepted cut, and destination-only receipt/pending reconciliation preserving every Core's current edits/suffix. Rev5 permits only the same attempt's exact live ticket settlement under §5; W16 covers the allowed floor/prefix change. |
| Astra Q6 | Fixed in §8. Added history-selection and session-retirement qualifications, ordinary-handoff evidence labels, and active-edit/reconcile consequences. |
| Astra Q7 | Fixed in §9. Retaining-use negatives, races after final checks, exact per-object observations, and targeted mutations. |
| Opus M1 | Fixed in scope/§3/§4, W1-W2/W4. Honest source/platform limits and retained verified reads. M1(d) is resolved by rev3 ruling 1/§4.4/W13: default preservation, plus separate explicit omission of exactly unavailable recovery roots as provenance. Blanket/default omission remains rejected; available roots stay required. |
| Opus M2 | Fixed in §1/§5, W1/W7. Immediate verified handoff moves active editing to B; only rescue-retained unknown attempts consume slots. A remains owned and actor-sealed only after immediate handoff; unknown B preserves active A and late commit never switches it (W5/W15). |
| Opus M3 | Fixed in §6/§8, W6/W9. Native locator/asset/changed-parent subtree exclusions plus identity checks; B bytes never settle A. |
| Opus M4 | Fixed in §§2,7, W10. Pending agent command eligible; exclude tentative effect and receipt, retain ID/digest and unresolved-origin lookup with no reapplication. |
| Opus M5 | Fixed in §4/§8, W4. Full-history default plus explicit oldest-first fitting selection; no cut-at-now or A floor change. |
| Opus m1 | Fixed in §5, W7-W9. All three uncertainty carriers, independent destination holds, retained actual owners, and fresh spawned B actor; no clone or shared-flag settlement. |
| Opus m2 | Fixed in §2, W3. Expanded illustrative Linux cases, interruption/contention/capability exclusions, B-error distinction, and future-only Windows mapping. |
| Opus m3 | Fixed in §6/§8, W9. Changed-device remount retains unknown without proved A continuity; no false "not saved" status. |
| Opus m4 | Retention reduction rejected by rev2 lead ruling 8. §5 and W8 retain session-long holds and assign contention/quota cost measurement to S2. |
| Opus m5 | Fixed in §7, W11. Flattened bounded provenance, exact current-session entries, counted/digested unresolved overflow summary, and no ancestry-overflow-only save refusal. |
| Opus m6 | Fixed in §6, W6. Temporary-root path/identity exclusion in both containment directions before mutation. |
| Opus m7 | Fixed in §8, W12. Deferred Minor #7 is a co-requisite landing with rescue in S2; cleanup-only uncertainty remains ineligible. |
| Opus m8 | Fixed in §9, W4-W5/W7-W8. Added floor, no-ticket, cleanup, and owner/ancestor-claim drop cases. The normally accepted post-freeze command premise in m8(a) is rejected under S0 `:376-377` and JA-1 `:260`; W5 tests a real attempted arrival and rejects an accepted suffix before publication/handoff. |
| Opus Q1-Q7 and ruling answers | Fixed by the corresponding §2 consent/classifier, §4 history, §5 ownership/slots, §6 separation/reconciliation, §8 contract mapping, and §9 witnesses. These answers are covered by the numbered dispositions; §11 records the round-2 changes. |
| Opus proposed missing-current-item save | Rejected as outside A1 by rev2 lead ruling 9 and S0 `:384-386`. §3 refuses rather than publish a partially available owned graph. |

## 11. Round-2 review finding dispositions (qualified by rev5)

These dispositions apply the binding rev3 rulings to both round-2 reviews, as refined by rev4 and rev5. "Fixed" means
specified here with required S2 witnesses, not implemented or runtime accepted. Earlier
closed findings remain closed except that Opus round-1 M1(d)'s rejection is replaced by the
lead's explicit-consent middle course. Round-1 m8(a)'s post-freeze acceptance rejection stays.

| Finding ID | Fixed where / rejected why |
| --- | --- |
| Opus r2 M1 | Fixed in §1/§5, W1/W7. Only adopted source unknowns and unknown rescue-destination attempts consume slots; ordinary saves never reserve/consume one or fail because the rescue slots are full. A new unknown ordinary attempt counts only upon explicit rescue adoption. Rev4 makes preview reservations provisional and source adoption/consumption atomic at the first mutation after re-probe/consent; settled slots remain consumed. |
| Opus r2 M2 | Fixed in §§1-2/§5/§7 and S0/plan edits in §8, W15. Unknown B resumes the original dirty A-associated Core. Later commit only labels B as a capture copy; no actor/content/dirty/Saved-baseline/authority change, even for equal captures or active C. Only the same attempt's own live ticket may settle under rev5 §5; the captured B Core never settles and B proof never settles A's ticket. |
| Opus r2 M3; round-1 Opus M1(d), still OPEN in Opus r2 | Fixed by rev3 ruling 1 in §1/§4.4/choice order/§7/§8, W4/W13. Preserve source recovery roots by default. Offer omission only when the chosen candidate otherwise refuses solely for unavailable recovery roots/closure bytes, omit exactly those roots, bind identity/SHA/cause as provenance, recompute complete closure. Rev4 first evaluates full-history omission alone and offers it if sufficient; otherwise a conditional eligible trim then separate omission choice makes the mixed case reachable. R_ack is included by default under the same unavailable-only rule. Never omit available or independently needed data or reintroduce an explicitly omitted source checkpoint. Reject blanket/default dropping of all predecessors; the lead did not authorize that loss. |
| Opus r2 m1 | Fixed in §5/§8, W5/W9. A's actor seals at immediate handoff across every retained clone and all edit/Undo/Redo/restore/install paths; queries and receipt/pending-only destination settlement remain for sealed A, whose ticket stays until session end. Rev5 §5 permits only same-attempt exact-ticket settlement on the live, unsealed Core; it cannot unseal A or restore/install history. Eager B binding/install gating coordinates with deferred #1. |
| Opus r2 m2 | Fixed in §6/§8, W6. Exclude the recorded parent subtree when its path is absent, changed or unprovable against pinned identity, before creating parents. Include removed mount-directory/remount-shadow witness. |
| Opus r2 m3 | Fixed in W2. Positive revocation is on an ancestor above already-pinned directories while actual object directories/files remain accessible. Negative revocation removes search on the actual objects/assets directory and requires typed refusal for required bytes. Both are nonprivileged and hash-check actual readable sources. |
| Opus r2 m4 | Fixed in §8. Add S0 :274-276's explicit, consented A1 selection/provenance qualification without weakening independent evidence edges. Correct deferred #2 to :12-15, #6 to :23-24, #7 to :25-27; #1 is :7-11 and the closing amendment question :31-33. |
| Opus r2 m5 | Fixed in §5/§8, W7/W8/W15. Define the New/Open→Close/exit editing context and proposed ProjectSession.ja1: Ja1EditingSession owner of RescueSessionState, including opened-A/headless contexts and lifetime across active-project/focus changes. No assumed preexisting bundle owner or new store/collector. |
| Astra r2 R2-m1 | Fixed in §7/§8, W10/W11/W14. Add persisted rescue inventory member, recognized role/schema and required rescue-copy@1 semantics under S0 §3.3. Unsupported, missing or mismatched admission refuses writes/resume before caller mutation; generic OwnedEvidence support is insufficient. |
| Astra r2 R2-m2 | Fixed as a rescue-attempt witness in §5/§9 W7's explicit B-u1/B-u2 sequence, with distinct tags/holds and destination-dedup mutation. Its extension to consuming slots/refusing ordinary B saves is rejected as superseded by rev3 ruling 2. Same-target rescue overwrite stays forbidden; repeated adoption of unknown source attempts at the same active destination makes the per-attempt witness reachable. |
| Astra r2 upheld rejection of Opus M1(d) | Superseded only to the extent of rev3 ruling 1. Its no-silent-discard/default-complete-closure principle remains in §4.4/W13; a separately consented, exact unavailable-root omission now has an explicit S0 :349-354 amendment. |
| Both r2 reviews: Opus round-1 m8(a) rejection | Retained in §2/W5/§10. S0 :376-377 and JA-1 :260 pause new mutation acceptance through handoff; drain work already accepted before the cut. Test attempted late input and a deliberately bypassed fence, without inventing normally accepted post-freeze work. |

## 12. Round-3 review finding dispositions (qualified by rev5)

The five binding rev4 rulings resolve every new Opus round-3 finding. Rev5 narrows only the
floor-settlement prohibition and fixes R_ack's representation. The restore/install prohibition,
seal, separate-open rule, omission-first order and mutation-time slots remain. Astra's
round-3 APPROVE/no-findings review is design-only evidence. "Fixed" below means specified
here with required S2 witnesses, not implemented or executed.

| Finding ID | Fixed where / rejected why |
| --- | --- |
| Opus r3 M1 | Fixed in §1/§5/§7 and consequential S0/JA-1/plan/code edits in §8. Every rescue-retained destination reconciles its own receipt/pending state; rev5 allows only its same attempt's exact ticket settlement on the live, unsealed Core under §5. No restore/install/history replacement or other floor on any Core, and no settlement on sealed A or B's captured Core. `receipts.rs:508-523` is explicitly split from `reconcile_core_save`/unconditional restore. Report A's actual contents with current edits unchanged; a later explicit successor save discloses an omitted known agent effect. W10/W15 require committed/older-root live cases after unknown B plus further edits, idempotent pending-item settlement, and sealed-A completion without any Core call or seal exception. Mutation restores A's history onto the live Core. W5/W9 bar all other history operations, and W16 covers the sole own-ticket exception without reopening the restore hazard. |
| Opus r3 m1 | Fixed by rev4 ruling 2 in §1/§4.4/§8, W4/W13. Include R_ack's checkpoint and complete closure by default whether or not previously checkpointed; first-save unsaved A has none. Omit it only if unavailable with separate consent and exact provenance/last-save disclosure. Available R_ack remains required even when it limits trim relief. Rev5 places R_ack in one dedicated durably pinned record reference, outside the unchanged two-predecessor inventory, with B reopen/recovery validation and ordinary-rotation retention. Ordinary backup construction cannot reinsert an explicitly omitted checkpoint. |
| Opus r3 m2 | Fixed in §5/§7/§8, W15. Opening late-committed B separately while the originating session holds its owner is read-only. Editable open requires the person's explicit release of B's completed destination writer/namespace owner after proven commit and ordinary safety checks. No automatic handoff/release, active-edit replacement, consumed-slot refund or release of A/rescue-source holds. |
| Opus r3 m3 | Fixed in §1/§4 choice order/§8, W13. Evaluate full-history zero-cut closure, record and overlap with only unavailable recovery roots provisionally excluded before offering Undo loss. If omission alone fits and is self-contained, offer omission alone with no trim preview. Only otherwise offer an eligible oldest-first trim, alone or with separately consented omission. Exact final admission, independent roots and changed-consent rules remain. |
| Opus r3 m4 | Fixed in §1/§2/§5/§8, W3/W7 and the repeated-attempt sequence. Previews, re-probes and cancel do not consume slots or adopt ordinary source holds into rescue retention. After the second probe, required consent and final admission, source adoption/consumption is atomic at the first B mutation; the destination reservation is consumed only by unknown publication. Mutation/cancel controls distinguish before- and after-mutation definite refusal; transient re-probe recovery consumes nothing. |
| Astra r3 APPROVE; no findings | Retained as the round-3 design verdict, with no Astra finding to reject or carry. Its prior acceptance does not supersede the lead's rev4/rev5 rulings or provide runtime evidence. Earlier closed findings and the rejected post-freeze acceptance premise remain as qualified in §§10-11. |

## 13. Round-4 review finding dispositions

The binding rev5 rulings resolve Opus N1 and N2. All five round-3 findings remain closed at
design level as both round-4 reviews reported. Astra's APPROVE and Opus's REQUEST CHANGES
are retained as historical design verdicts, not execution evidence. "Fixed" means specified
here with required S2 implementation and mutation witnesses.

| Finding ID | Fixed where / rejected why |
| --- | --- |
| Opus r4 N1 (Major) | Fixed by rev5 ruling 1 in §1/§4.4/§7 and the S0/JA-1/plan/source map in §8. One optional R_ack RootCheckpoint reference lives in the durably pinned rescue_copy_v1 record, with an exact declared content edge and complete B-owned closure. It is reachable/recoverable through that record and validated on B reopen/recovery. B's two-predecessor validator, JA-1 :708 and v3 format counter are unchanged; ordinary rotation cannot retire this independent edge. Unavailable R_ack still needs the same exact omission consent/provenance/disclosure. W13 restores the positive R_ack-plus-two-readable-predecessors success/reopen/recovery control, rotation/recopy retention and edge/closure validation. Mutations reuse save_as's .take(2), refuse on the count bound, drop the durable edge, or weaken the validator. |
| Opus r4 N2 (Major) | Fixed by rev5 ruling 2 in §1/§5/§7 and every consequential reconciliation rule/source site in §8. Only the reconciled attempt's exact ticket on its live, unsealed Core can settle through S1's existing project-proof path. Own-root commit replays the current suffix and checks content, post-floor Undo and complete Redo; proved older/absent root permits only exact lossless release under §6. No other attempt/Core, restore, install, replacement or new floor is allowed. Sealed A keeps its ticket until session end, and captured never-active B/C receive no call. W7 removes the no-floor restriction; W9/W10/W15 distinguish own-ticket settlement from forbidden history restore. W16 covers over-allowance unsaved A, unknown rescue, further edits, committed/older proof and later successful saves at A and active B without edit loss. Mutations leave the ticket pending, settle another attempt's ticket, restore instead of settling, or settle a sealed/captured Core. |
| Astra r4 APPROVE; no findings | Retained as the round-4 design verdict with no finding to reject or carry. Its acceptance of refusal at the two-predecessor bound and blanket no-floor settlement is superseded only by the two binding rev5 rulings. All other round-3 closure and earlier dispositions remain, including the rejected post-freeze acceptance premise. No implementation/runtime acceptance is inferred. |

No open lead question remains. Cargo, runtime red/restored-green witnesses, physical-device
removal, storage-crash persistence, native Windows execution and CI are **NOT RUN**. This is
a design revision only; S2 must implement and gate it before any rescue availability claim.

### S2-R C6 App realization

The local App displays the §7 outcome/loss distinctions and the binding revision-7
availability disclosure at preview, rescue handoff, Close and destination changes.
Its same originating lifecycle owner survives finite background operations, wrapper
replacement and focus changes. A late capture's explicit destination release applies
only to a proven, never-active capture; active and source owners cannot use it. View
subscriptions/resources are prepared before actor sealing. C6's source-bound report
records the in-session selection pin's accepted crash window and all unrun native,
hosted and physical-storage lanes without revising older acceptance.

### C9 manual producer qualification

A manual permit does not weaken A1's Frozen/Sealed or exact-cut rules. Raw Core
clones, retained permits and owner install/floor paths cannot bypass a frozen or
sealed actor. Accepted manual messages remain retained while publication is
pending; only an exact durable acknowledgement releases their ingress capacity.
Source ownership, rescue slots, exact own-ticket settlement and capture-only late
outcomes remain with their existing owners. C9 adds no recovery authorization,
shared floor handshake, switch receipt or fresh-actor rescue-tail repair.

### C10 recovery authority remains separate from rescue settlement

A frozen manual recovery endpoint accepts only the captured actor/cut and complete
pending chain. It may install that cut's validated floor or recovery-close result
while preserving the freeze. Neither an A1 own-ticket proof nor a matching document
confers this permission. Original-to-new command links and close/floor outcomes are
ordinary receipt/history records in the same root, with existing admission limits.
A replacement callback borrows the same actual GUI source admission and cannot
retire its bytes, pins or charges. Failure to re-establish retained ownership keeps
all save/rescue routes refused; only explicit originating Close retires the source.
C10 does not change the deferred sealed-A/fresh-B rescue resume case.
