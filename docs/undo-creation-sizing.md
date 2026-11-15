# Creation-sized history admission

## Ruling 7 implementation

The immutable descriptor records creation kind, original baseline capacity and
lineage/resource charge, and the initial staging batch charge. It is mandatory in
the stored shell and in the canonical owner identity. Creation and open call the
same checked pure function. Captures retain exactly that lease; subsequent edits
and discarded initial deltas do not change it. Open checks the actual retained
states against the derived limits and uses normal pool admission.

Ordinary is fixed: 1 MiB encoded operations, 24 MiB decoded history, 8 MiB encoded
and live document limits, four anchors, 32 MiB owners: `6*8+24+32=104 MiB`.
Task and Candidate document room is
`min(max(2*original baseline live+512 KiB,original baseline live+2 MiB),8 MiB)`.
The doubled-baseline floor stays 512 KiB; the minimum incremental room is 2 MiB
so a tiny baseline still holds the fixed 2,000-removal fixture. Owners are the original baseline charge plus 3 MiB (construction refuses a
baseline above 29 MiB). Task has one anchor and 1/24 MiB history. Live Candidate
has the same history room and no anchors. Staging Candidate has no anchors and
`min(initial batch+64 KiB encoded / 512 KiB decoded,1/24 MiB)` history.

| Clips, 8 tracks | Document room MiB | Owner room MiB | Task lease MiB | Live / full staging lease MiB | Tasks with capture room | Revise live Cores with one staging/capture free |
|---:|---:|---:|---:|---:|---:|---:|
| 1,000 | 2.235 | 3.894 | 34.599 | 32.364 | 10 | 3 |
| 5,000 | 4.252 | 7.434 | 44.189 | 39.937 | 8 | 2 |
| 20,000 | 8.000 | 20.709 | 68.709 | 60.709 | 4 | 1 |

The fixture is unchanged from undo-merge-limits.md. This table follows the
production constructor path: an Ordinary Core's long-lived baseline is replay
padded before a Task or Candidate borrows it. The earlier raw-document table was
optimistic at 5,000 clips: the first constructor-path regression found seven,
not eight, Task slots with the old 2 MiB additive floor. The floor above restores
eight without changing the pool caps or the fixture. Assertion:
`PRODUCT_FORK_TASK_SLOTS_INCLUDE_CAPTURE_HEADROOM`.

The Task pool leaves one largest live Task lease free for checkpointing. Live
Candidate admission similarly leaves one largest live Candidate lease free for
staging or persistence. Staging previews will transfer their owned state when
consumed; they need no second independently retained preview. Concurrent command
scratch is admitted separately, so one full workspace can run at a time.

Candidate pool: `2*(2*8+24+32)=144 MiB`. The measured plain 20,000-clip
baseline needs only 60.709 MiB per candidate, but the descriptor permits a
32 MiB owner envelope. Using the full product maximum preserves merge fit for
every admitted source, including richer lineage than the plain fixture.

The App retains both tab Cores in `App::projects`; `new_project` pushes the
second without dropping the first. Saving either owns a third Ordinary lease.
The default Ordinary pool is therefore `3*104=312 MiB`, with new-Core admission
leaving one largest capture free. The measured owner-flow assertion is
`TWO_TABS_AND_SAVE_NEED_THREE_ORDINARY_LEASES`; the actual App path is also gated
when it switches. A third live tab is a typed refusal before it starts; both
existing tabs can still save. The generated one-project-per-process corpus
continues to assert a peak of two.

Workspace is 168 MiB: the large ripple's warm subtotal is
`7*8+3*32=152 MiB`, plus 64 KiB metadata and 10% margin, under 168 MiB.
Cold ingress is covered by the incoming retained lease together with scratch:
raw shell <=88 MiB, structural body decoder <=64 MiB, one untrusted document
under its separate 64 MiB decoder ceiling, and the previous document already
checked against D. Including fixed headers, this is under `72+168 MiB` for a
maximal Candidate. The structural decoder charge already includes twice the
encoded input; the executor separately applies its 10% replay margin. The descriptor's smaller shell cap is checked before
body decoding. Raw ingress is dropped before semantic replay builds indexes;
replay uses its existing checked reservation with at most two A owner scratch.
The running merge guard includes the destination's three A scratch and margin.
Pool total: `312+400+144+168=1024 MiB`.
Kernel validation, duration and identity passes remain unchanged.

A NeverFold Core also keeps running additive observation charges and a largest
kernel scratch peak (excluding observations). Admission reserves the eventual one-batch replay with the
Ordinary destination's owner scratch. This is necessary because observations
from many individually accepted commands coexist when forwarded as one batch.
Undo/Redo move the existing charges; a new Do drops Redo's charge. No command
scans retained history to compute this bound. The source refuses before accepting
work that cannot fit a fresh destination's workspace. The recorded scratch
adds eight bytes to Step (now 112 bytes) and is charged in decoded history;
observations are counted once even after commands are forwarded as one batch.

The session model gives GUI and writer their own process pools. Its independent
cold verification reader also has a separate pool; it does not count as a third
Core inside the writer process. The product pool admits two live Ordinary Cores plus one capture; the permanent
one-project model assertion still checks a measured peak of two. Adversarial
unit stories holding several sibling histories use an explicitly larger test
pool; those fixtures are not product admission evidence.

The table is accounting admission capacity, not a new scheduler limit. The host
still has 64 run slots and each run has at most eight tasks (`revise.rs` and
`run_admission.rs`); the process pool is the tighter simultaneous limit above.


Replay safety also accounts for allocation geometry. An in-memory vector may
have a different spare capacity from the same vector decoded on another process.
The running live bound admits the larger of actual capacity and geometric wire
capacity per field. Construction pads an initial long-lived baseline once when
needed; install canonicalises its long-lived baseline. Warm commands preserve
capacity in their one atomic candidate copy. The independent bound oracle still
measures actual capacities without a normalising copy. The structural body
forecast remains capped at the decoder's existing 64 MiB ceiling even though
workspace is larger; a command cannot create a saved body its decoder rejects.
| Clips, 8 tracks | Encoded bytes | Fork baseline live bytes | Document room MiB | Owner room MiB | Task lease MiB | Live / full staging lease MiB | Tasks with capture room | Revise live Cores with one staging/capture free |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1,000 | 156,882 | 246,608 | 2.235 | 3.894 | 34.599 | 32.364 | 10 | 3 |
| 5,000 | 788,882 | 1,966,928 | 4.252 | 7.434 | 44.189 | 39.937 | 8 | 2 |
| 20,000 | 3,179,988 | 7,865,168 | 8.000 | 20.709 | 68.709 | 60.709 | 4 | 1 |

The fixture is unchanged from undo-merge-limits.md. This table follows the
production constructor path: an Ordinary Core's long-lived baseline is replay
padded before a Task or Candidate borrows it. The earlier raw-document table was
optimistic at 5,000 clips: the first constructor-path regression found seven,
not eight, Task slots with the old 2 MiB additive floor. The floor above restores
eight without changing the pool caps or the fixture. Assertion:
`PRODUCT_FORK_TASK_SLOTS_INCLUDE_CAPTURE_HEADROOM`.

The Task pool leaves one largest live Task lease free for checkpointing. Live
Candidate admission similarly leaves one largest live Candidate lease free for
staging or persistence. Staging previews will transfer their owned state when
consumed; they need no second independently retained preview. Concurrent command
scratch is admitted separately, so one full workspace can run at a time.

Candidate pool: `2*(2*8+24+32)=144 MiB`. The measured plain 20,000-clip
baseline needs only 60.709 MiB per candidate, but the descriptor permits a
32 MiB owner envelope. Using the full product maximum preserves merge fit for
every admitted source, including richer lineage than the plain fixture.

The App retains both tab Cores in `App::projects`; `new_project` pushes the
second without dropping the first. Saving either owns a third Ordinary lease.
The default Ordinary pool is therefore `3*104=312 MiB`, with new-Core admission
leaving one largest capture free. The measured owner-flow assertion is
`TWO_TABS_AND_SAVE_NEED_THREE_ORDINARY_LEASES`; the actual App path is also gated
when it switches. A third live tab is a typed refusal before it starts; both
existing tabs can still save. The generated one-project-per-process corpus
continues to assert a peak of two.

Workspace is 168 MiB: the large ripple's warm subtotal is
`7*8+3*32=152 MiB`, plus 64 KiB metadata and 10% margin, under 168 MiB.
Cold ingress is covered by the incoming retained lease together with scratch:
raw shell <=88 MiB, structural body decoder <=64 MiB, one untrusted document
under its separate 64 MiB decoder ceiling, and the previous document already
checked against D. Including fixed headers, this is under `72+168 MiB` for a
maximal Candidate. The structural decoder charge already includes twice the
encoded input; the executor separately applies its 10% replay margin. The descriptor's smaller shell cap is checked before
body decoding. Raw ingress is dropped before semantic replay builds indexes;
replay uses its existing checked reservation with at most two A owner scratch.
The running merge guard includes the destination's three A scratch and margin.
Pool total: `312+400+144+168=1024 MiB`.
Kernel validation, duration and identity passes remain unchanged.

A NeverFold Core also keeps running additive observation charges and a largest
kernel scratch peak (excluding observations). Admission reserves the eventual one-batch replay with the
Ordinary destination's owner scratch. This is necessary because observations
from many individually accepted commands coexist when forwarded as one batch.
Undo/Redo move the existing charges; a new Do drops Redo's charge. No command
scans retained history to compute this bound. The source refuses before accepting
work that cannot fit a fresh destination's workspace. The recorded scratch
adds eight bytes to Step (now 112 bytes) and is charged in decoded history;
observations are counted once even after commands are forwarded as one batch.

The session model gives GUI and writer their own process pools. Its independent
cold verification reader also has a separate pool; it does not count as a third
Core inside the writer process. The product pool admits two live Ordinary Cores plus one capture; the permanent
one-project model assertion still checks a measured peak of two. Adversarial
unit stories holding several sibling histories use an explicitly larger test
pool; those fixtures are not product admission evidence.

The table is accounting admission capacity, not a new scheduler limit. The host
still has 64 run slots and each run has at most eight tasks (`revise.rs` and
`run_admission.rs`); the process pool is the tighter simultaneous limit above.


Replay safety also accounts for allocation geometry. An in-memory vector may
have a different spare capacity from the same vector decoded on another process.
The running live bound admits the larger of actual capacity and geometric wire
capacity per field. Construction pads an initial long-lived baseline once when
needed; install canonicalises its long-lived baseline. Warm commands preserve
capacity in their one atomic candidate copy. The independent bound oracle still
measures actual capacities without a normalising copy. The structural body
forecast remains capped at the decoder's existing 64 MiB ceiling even though
workspace is larger; a command cannot create a saved body its decoder rejects.

Metadata vectors reserve the larger of their actual capacity and the cold JSON
sequence capacity. The manifest-context regression filled a Task with
one-reference contexts; the former length-one allocation charge admitted a
record that cold decode refused at `ADMITTED_MANIFEST_CONTEXT_HISTORY_REOPENS`.
Input references and approval targets now pay their wire allocation shape at
admission, without normalising copies. The same test retains at least 2,000
commands and checks identity and every accepted command after open.

## Historical ruling 6 scope diagnostics

The task constructor can take a fixed creation profile and pass that profile
through its capture/open paths. The full ruling cannot be implemented only
there with the existing Ordinary snapshot leases and live RunCandidate
lifecycle. No product profile or caller was changed during this scope check.

## Ordinary capture and replacement admission

`OwnerHistory::capture` first calls `self.reservation.capture()`
(`operation_history/snapshot.rs:73-75`). That calls `pool.admit(role)`, which
reserves another full role envelope (`reservation.rs:101-102,165-169`).
`StoredOwnerRecord::read` also admits a full role lease before decoding
(`stored.rs:277`).

Ruling 6 fixes an Ordinary Core at 104 MiB and its retained pool at 128 MiB.
The Core leaves 24 MiB free. Capturing it, or opening a replacement while it
is still present, needs another 104 MiB: **208 > 128 MiB**. This refusal occurs
before copying anything. Releasing a finished Task cannot free Ordinary room
because the role pools have separate counters.

The executable probe uses the current 32 MiB Ordinary envelope with exactly
that same 24 MiB headroom. Capture returns `Reservation` and source usage is
unchanged. Assertions: `ORDINARY_CAPTURE_NEEDS_A_SECOND_FULL_LEASE` and
`REFUSED_CAPTURE_LEAVES_THE_SOURCE_RESERVATION_UNCHANGED`.

Simply raising Ordinary retained room to 208 MiB does not preserve the stated
ceiling with today's scratch accounting. Task 400 + Candidate 64 + Ordinary
208 leaves 96 MiB of 768 for workspace. The Ordinary executor reserves
`3 * owners` as input (`owner.rs:488`), hence 96 MiB before any document work;
ReplayBudget's ten-percent margin alone makes that exceed 96 MiB.

The smallest additional seam is a distinct transaction-snapshot reservation
for capture/validation, with an explicit lease handoff at install/publication.
For example, a bounded frozen view could use the already admitted workspace
through encoding or validation, then retain a stored reference or take over
an existing Core lease. It cannot hold a second full Ordinary Core lease.
Its ownership and release rules must also cover uncertain publication and
snapshots that outlive the freeze. That changes the cut/install lifecycle,
not just the sizing constructor. This alternative is not implemented here.

## A live RunCandidate has no frozen input yet

`writer_host/revise.rs:228-242` forks and persists a RunCandidate before the
Director and its tasks run. `editing_session/durable.rs:646-661` constructs
that Core from the manual document and records an empty applied delta.
The frozen task delta is produced later at `durable.rs:386-409`, then merged
by `preview_candidate_merge` at `durable.rs:478`.

Sizing the initial empty Core from an empty delta plus the existing small
headroom still makes the first large admitted Task fail to fit this fresh
destination. Growing its lease at first merge is forbidden by ruling 6.
Replacing it with a newly sized Core at that point changes registration,
publication and target-binding handling; it is not only a constructor edit.

The smallest policy extension is to distinguish the mutable live RunCandidate
from a staging Candidate made from a known frozen delta. The former can use
the fixed Task history room under the Candidate pool, while the latter uses
measured delta room. Both remain fixed for their lifetimes. The alternative
is to defer live Candidate Core creation until a delta exists and change its
publication lifecycle. Neither alternative was selected without a ruling.

## Current stored data cannot reconstruct original creation inputs

The probe creates two legal Candidate histories with different initial
batches. Each then does Undo followed by the same new edit, discarding that
initial batch. Their complete production StoredOwnerRecord bytes, including
content-addressed document references and combined identity, are identical.

| Input history | Initial encoded / decoded charge | Final record bytes |
| --- | ---: | ---: |
| One-operation initial batch | 48 / 512 | 864 |
| 64-operation initial batch | 3,040 / 10,592 | 864, identical |

Assertion: `DISCARDED_INITIAL_DELTA_IS_NOT_RECOVERABLE_FROM_RECORD`.
With additive creation headroom, their original bounds differ; a pure function
of the identical final records cannot recover both original choices.

The baseline values being fixed also does not fix their capacity. The second
probe creates two otherwise identical Task baselines with empty clip arrays.
One reserves 8,192 spare clip slots. Their live charges are **440** and
**1,966,536** bytes, but their complete stored records are the same **635**
bytes. Assertion: `BASELINE_SPARE_CAPACITY_IS_NOT_RECOVERABLE_FROM_RECORD`.
JSON preserves values, not spare capacity.

A small immutable creation-sizing descriptor in the record is the contained
fix for this information loss. It can retain the original measured baseline
charges and Candidate initial one-batch charge, together with its creation
kind. The pure sizing function consumes that descriptor on creation and open;
validation checks product maxima and that all retained states fit the derived
bounds. Include the descriptor in snapshot identity. Canonicalizing baseline
allocations at creation/open is another way to stabilize their charge, but
cannot recover a discarded initial delta.

## Evidence and reproduction

The portable probe source is
`docs/measurements/undo-creation-sizing-probe.rs`. It targets the owner APIs at
`e1089a72`. Temporarily append this to
`crates/kinewright-project/src/operation_history/stored_codec_tests.rs`:

```rust
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/measurements/undo-creation-sizing-probe.rs"));
```

After `source scripts/setup-ffmpeg.sh`, run:

```sh
flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 nice -n 10 cargo test -p kinewright-project --lib ruling6_creation_information_is_not_in_current_records -- --nocapture
```

Remove that temporary include afterward. The probe passed all named assertions
in `target/hx-h2b/creation-sizing-probe.log`; the include was removed. These are
diagnostics of today's representation, not permanent acceptance tests for a
future sizing descriptor. All runs started for this scope check finished.

## Encoded producer buffers

The producer uses three anchors while bound. The fourth 8 MiB anchor allowance
covers its encoded outbox, externally retained sender bytes and temporary
encoding allocations: `(2+3)*8+24+32+8=104 MiB`. The declared creation profile
stays K=4 and the role pool total stays 1,024 MiB. Detaching restores the fourth
anchor after all outgoing bytes have dropped. This is an accounting choice;
product Undo timings must measure the bound producer path.

The pending queue holds at most 64 commands and 2 MiB of encoded bytes. A fixed
arena admits every allocation before encoding, including temporary encoding
space and 128 bytes per shared raw allocation. Its limit is 8 MiB minus a
128 KiB header allowance. Old sender references keep their arena charge and the
source's existing full reservation until their last drop. They cannot become
another Core or enlarge its lease. Replacement admission only subtracts the
old reservation when no such hold remains. A busy arena is a typed refusal
before document or outbox changes; releasing old senders permits retry.

Tests pin canonical borrowed/owned wire equality, one-copy/zero-measurement
warm edits, both queue caps, Empty priority, atomic/idempotent acknowledgements,
detach fencing, typed allocation headers, fixed-arena saturation and final-drop
release. The actor permit and resource-certificate adapter still has to replace
the old product door; this unit does not yet change product callers.

## Ruling 7 qualification

The final Core/Project gate passed 1,195 tests (24 ignored); workspace Clippy,
format checking and slow-test lint passed. The complete 64-seed, 65,536-attempt
portable corpus plus its 128 primer transitions passed, as did the unchanged
2,000 four-track-removal fixture and spare-capacity save/open/merge fixture
(18 release qualifications, 256.13 seconds). Every generated range observed at
most two simultaneous Ordinary leases per process. Seven profile-dimension
mutations failed their named compile-time fit assertions and were reverted.
The manifest-context round-trip and real constructor-slot regressions each
failed before their accounting correction and passed afterwards.
These qualify the new history implementation; product callers have not switched
at this commit. Product-path timings and the shipping builds belong to checkpoint 1.
