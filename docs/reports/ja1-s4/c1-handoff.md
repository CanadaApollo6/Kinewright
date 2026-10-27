# C1 contracts for S3 H1 and S4 C2

The base is `7fa471fcd2ee0455142f7ee41893ec4ccaf646df`. H1 has not merged. These
contracts do not enable RunIndex support, publish a Studio root, or authorize
provider execution. S3 retains every shared store, host, App, manifest, and CI file.

## Receiving H1 publication

The project-owned definitions are in `orchestration_records.rs` and
`orchestration_commit.rs`. The agent temporarily compiles those exact files with
path declarations in `orchestration/mod.rs`. Replace those declarations with
project reexports when the project owner registers the modules in `lib.rs`.
There is no project dependency on the agent.

`OrchestrationRecord { version: 1, payload }` has explicit RunIndex, snapshot,
event-segment, effect, request, delivery, result, and usage payloads.
`OrchestrationRecord::dependencies` supplies their required closure edges.
H1 must register the tagged record through the sealed machinery and admit
`orchestration@1` within v3. It must preserve the existing required-version
predicate and legacy omission rules. No payload is admitted as opaque evidence.

`ValidatedRunUpdate::new` checks the project, expected root generation,
envelope digest, event cut, projection, graph identities, and dependency refs.
It exposes immutable getters for the owner. It is validation, not authorization
or a writer. The constructor receives the owner's actual generation.

The single H1 publisher must commit the segment, projection, original-envelope
receipt, shared issuer state, and pending effects together. It must charge and
pin the full closure in the existing physical ledger, including transient copies.
Detailed receipts and issuers share the existing 4,096 and 128 limits.
This contract supplies no alternative receipt ledger.

The host verifies current project read authority and the required resolved S3
endpoint role before calling `checked_receipt`. Matching receipts precede fresh
attempt and stale-head admission. Call `checked_receipt` again after reservation
loss. Call `ValidatedRunUpdate::validate_returned_receipt` on the final result.
Both checks bind the entire original envelope. Unknown, pending,
retired-unresolved, and unresolved-origin remain distinct.

`Kernel` retains at most one pending cut. Unknown publication freezes proposals
and retains `original_envelope()`. The owner reconciles that exact envelope.
`Kernel::acknowledge` is crate-private and receives only a post-B5 or reconciled
receipt. Before acknowledgment, `acknowledged()` exposes no proposed effects.
Replay returns the stored receipt and never repeats an effect.

H1 still owes the B1-B5 independent-reopen witnesses and graph preservation
through Save, floor, switch, Save As, recovery, and A1. A1 selects only the
acknowledged scheduler cut and retains tentative commands' unresolved origins.
Copies retain paused history without live grants or executable outbox authority.
S3 also owns `studio_records.rs` and the shared RunView, RunEvent, RunIntent DTOs.
No file in those owners' rows was changed here.

## C2 durable reservation before COMMIT

The agreed serializable protocol identity is
`orchestration_records::ExecutionIdentity`. Its fields are `version`, `project`,
`run`, `task`, `attempt`, `nonce`, and `owner_instance`. Version is 1. Each identity
uses the existing validated 32-character lowercase hex UUID encoding. The
encoded identity is limited to 1 KiB. It contains no credential or callback.

`AttemptReservation` retains the exact identity, original owner epoch,
`AttemptState`, and recovery reference in the acknowledged run projection.
Attempt IDs are unique within the projection. Every unretired reservation
appears in aggregate cleanup with its exact identity and recovery ref. Reserved
work has Pending cleanup; blocked task or attempt facts require Blocked cleanup.
Settled cleanup excludes every outstanding retirement.
`AttemptState` distinguishes Reserved, Owned, Contained, Executing, Waiting,
Stopping, CleanupBlocked, and Retired. Provider execution and native containment
are absent from C1.

Agent `orchestration::AdmittedAttempt` is opaque and nonserializable. C2 consumes
`identity() -> &ExecutionIdentity` and `reservation_receipt() -> &OrchestrationReceipt`.
There is no public constructor. The crate-private receiving owner hook is
`acknowledge_attempt_reservation(update, returned_receipt, identity)`.
It requires an Applied result, exact returned cut, exact project/run/task/
attempt/nonce/owner identity, Reserved state, and original owner epoch.
H1 must supply the receipt only after durable acknowledgment. Reading an identity
or a prepared update never supplies the proof.

The launcher accepts `AdmittedAttempt`, then uses that exact identity through
OWNED, CONTAINED, and COMMIT. C2 owns native proof and machine-claim retirement.
Neither an EOF nor a free execution lock replaces retained cleanup proof.
The protocol is original Kinewright work and is not attributed as copied T3 code.

`orchestration` is registered in agent `lib.rs`. The A owner registers
`guardian` and `execution_claim` after C2 files are integrated, plus C5's
`usage` and `limits` at that handoff. No absent module or binary is registered.
The later C2 handoff assigns module and binary-manifest registration to A.
No manifest change is needed before those C2 sources arrive.

## Attribution handoff

`LICENSES/t3code-MIT.txt` contains the complete pinned upstream MIT license.
The agent exposes that exact text in `T3CODE_MIT_NOTICE`. Translated sources and
tests identify upstream repository, pin, paths, notice, and Kinewright changes.
The App and packaging owner still owes the attribution-list and packaged-notice
link. The workspace license remains GPL-3.0-only.
