# Native history: legacy retirement

The product has one executor and one owner record. `kinewright-core::undo`
retains the baseline and command steps; the Project actor adds approvals,
lineage, resource ownership, immutable creation sizing and pool leases. The
writer publishes its ordered receipts with the complete owner checkpoint.

The retired implementation comprises Core's boundary-document actor, manual
candidate rollback path, journal/cursor-hash transport, save-side floor choice,
floor barrier, prefix/suffix replay proof, Project history/lineage journals and
task-history admission projection. There is no old-format reader. A saved
checkpoint publishes the executor's existing cut, rather than choosing a cut
at Save. The retired stage-1 representation report and its old-only measurement entry
are removed; historical native benchmark tables remain in
[undo measurements](undo-measurements.md). Rescue preserves both stacks; omission consent and destination
ownership checks remain independent requirements.

The Muse F3 site in `actor/manual_history.rs` is removed with that entire file.
`CoreState::restore`, including its fallback, is removed with the old actor.
`history_save.rs` remains because it now defines the native publication tag and
attachment capability, not an old floor protocol. The terminal result escrow's
physical-byte reservation remains unchanged as `TERMINAL_OBJECT_BYTES`: it
protects the run's recovery envelope and is not a history-role allowance. The
unused per-record history-limit hook is removed. Native `SpecialistBase` is a
fixed-size identity/reference binding; its referenced owner owns the history
lease, and generic run-record admission remains.

All 207 original reader rows remain in the
[reader inventory](undo-reader-inventory.json). The 28 rows previously awaiting
obsolete-proof retirement now name their replacement witnesses. Eight portable
kernel/actor contract modules move from Core's old actor tests to Project's
native actor test binary. The 67-kind fixture, independent H1 state/fate model, its 12 pure-oracle
regressions, the step-bound matrix and session model remain. The old-Core
comparisons in the H1 test binary are removed; malformed-install, literal-byte,
fate, close and document-limit guards still test the live reference model. The owner differential reconstructs
lineage and every reachable document by direct operation application; it no
longer runs a second production history implementation as its oracle.

The old recovery discard selector and its refusal-only wire field are removed.
C10's additive first-save fixture now asserts exact stack preservation; it does
not label a zero fold frontier as evidence of recovery. Every unrelated fixture
index entry is unchanged.

The old unleased operation-log queries are removed. Evaluation explicitly asks
for a bounded activity recorder; applied delta readers hold their own leases.
The dead optional kernel replay-budget path is removed without changing the
kernel's validation, duration recomputation or identity passes. Native workspace
reservations use the caller's running bound and exhaustive operation table.

The final tree passes 5,460 workspace fast tests and 5,584 with the Linux slow
features enabled, including all 64 session seeds / 65,536 attempts. The 283 fast
and 159 slow ignores retain their declared platform or on-demand reasons.
Opt-in TE1 passes 41 library and 13 unpaid measurement tests. Workspace and
feature clippy, fmt, workspace build, all three separate shipping builds and
slow-test lint pass. Pure Core test compilation for MSVC passes; the workspace
MSVC check stops before Project tests in missing FFmpeg/Whisper/AWS-LC cross-build
prerequisites. This does not qualify native Windows execution.

The first fast attempt exposed an oracle assertion that compared runtime
allocation charges and a stale CC5 source path; both are corrected. It also
reported a guarded `switch_changed` refusal once in the concurrent TE14 media
control test. Its cause remains unproven; the focused test and final full fast
and slow runs pass with the switch guard unchanged. An interrupted confirmation
command ended with SIGTERM 143 before any result. The complete supervised gates
record no signals; no result from the interrupted command is counted.

[Gate and admission-probe evidence](measurements/undo-legacy-deletion-proof.json.gz)
retains the failures, complete final logs, commands and source hashes. The
subsequent C6 checkpoint accounts separately for every original mutation entry,
including retargeted and retired floor/trim cases. Its runtime qualification and
the remaining switch mutations are not claimed complete here.

The shared operation oracle compares the entire canonical record encoding,
combined identity and document after reopen, and independently checks the
opened encoded and actual-capacity bounds. Runtime `StepCharge` values are not
serialized: decoding can change spare capacities, so equality of those runtime
values is not the persistence contract. A separate admission probe repeated all
67 operation fixtures to their Task-role refusal and reopened each admitted
record under the same role; all 67 opened. This probe covers those shapes,
not arbitrary payloads, and is not presented as the permanent generated proof.

## Retired source paths

- `crates/kinewright-agent/tests/fixtures/ja/s2-manual-floor-control.json`
- `crates/kinewright-agent/tests/fixtures/ja/s2-manual-floor.json`
- `crates/kinewright-core/src/actor.rs`
- `crates/kinewright-core/src/actor/manual_history.rs`
- `crates/kinewright-core/src/history.rs`
- `crates/kinewright-core/src/history_journal.rs`
- `crates/kinewright-core/src/history_size.rs`
- `crates/kinewright-core/src/history_transition.rs`
- `crates/kinewright-core/tests/au1_core.rs`
- `crates/kinewright-core/tests/au2_core.rs`
- `crates/kinewright-core/tests/au4_core.rs`
- `crates/kinewright-core/tests/cc3_core.rs`
- `crates/kinewright-core/tests/cc4_core.rs`
- `crates/kinewright-core/tests/cc5_core.rs`
- `crates/kinewright-core/tests/contracts.rs`
- `crates/kinewright-core/tests/history_journal.rs`
- `crates/kinewright-core/tests/ja_s1_history.rs`
- `crates/kinewright-core/tests/mo2_core.rs`
- `crates/kinewright-core/tests/ownership_measure.rs`
- `crates/kinewright-project/examples/ja_s1_history_read.rs`
- `crates/kinewright-project/src/cleanup_measurement_tests.rs`
- `crates/kinewright-project/src/history_record.rs`
- `crates/kinewright-project/src/history_size.rs`
- `crates/kinewright-project/src/lineage_journal.rs`
- `crates/kinewright-project/src/manual_history.rs`
- `crates/kinewright-project/src/task_history_admission.rs`
- `crates/kinewright-project/tests/history_capacity_heap.rs`
