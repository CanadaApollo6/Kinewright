# Ruling 6 needs snapshot and Candidate lifecycle changes

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
