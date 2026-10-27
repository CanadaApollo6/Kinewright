# JA-1 S4 C4 before H3

Base `340eb79def51cb1cecb303a126a47da29fff0feb`, branch `t3/ja1-s4-c1`,
worktree `/home/riels/.t3/worktrees/Kinewright/t3-f4cb0356`.
One commit is authorized. No push is authorized.

C4 is implemented at the receiving-port and contract-store boundary. Riel explicitly
clarified that the lifecycle witnesses must run through the real pump and the C1
kernel or outbox while H1 is absent. Durable owner publication and reopen acceptance
remain **NOT RUN pending H1**. H3 approval and secret holder acceptance also remain
**NOT RUN**. No S3, `writer_host/*`, App `ja1_*`, C2/C5 worker production files, or other worktrees
are changed. The test-only ACP fixture is the explicit T1 exception noted below.

## Behavior

Task progress retains its TaskId, current attempt/session/turn/epoch/head identity,
turn purpose, background work, original feedback, and decisions. Provider turn
termination does not publish a result. Readiness also requires settled descendants,
explicitly observed deliveries, no unresolved question, and a work turn. A question
attention turn cannot select a final result. Late events and replacement results
refuse without changing the acknowledged projection.

Delegation reserves the child's execution capacity in the same proposed cut as the child and
prepare effect. Capacity reservation does not mint C2 execution or recovery identity.
The existing C2 attempt record must be admitted separately before a turn begins.
The three-slot bound includes waiting ancestors. A required writer
that depends on an ancestor releasing the candidate, or a fourth reservation,
returns `DependencyCapacity` before adding a child or dependency. Refusal preserves
the acknowledged task, delivery, effect, usage, and reservation state.

Result publication and pending parent delivery share one validated cut. Pending
sibling results share a delivery generation, stable message identity, and wake.
Offers can retry with the same result and message IDs. Observation is a separate
owner command, authorized by the live receiving turn. A timeout retains child work
and later delivery. Status consumers must use that explicit observation command.

Ordinary questions route to the Director. Answers retain exact references in both
the question and the Director's decisions. Unresolved material, fact, and delivery
choices remain retained questions for the Director to batch or surface. Wake
lifetime is checked at admission and execution. Session expiry and cancellation
dispose wakes without overwriting completed output. Approval and secret requests
are separate types and return `H3AttentionUnavailable` here. No approval callback,
credential, secret value, or fabricated consent enters project records.

The existing pump owns Studio broker consumption and session interruption. IN2
retains `RejectAndStop`; Studio never invokes `ApproveAll`. Existing sessions default
to unsupported runtime responses. Runtime requests carry a bounded nonsecret DTO,
and the agent registry rechecks the live binding after the Director responds.
The nonblocking control queue holds 64 messages of at most 256 KiB. Uncertified
active steering queues after the current turn. Idle waits retain budget,
cancellation, broker, and observer ticks. Studio's stop hook must acknowledge
intent and expire session requests before interruption. Unknown publication keeps
the live session and cleanup reservation with the owner for reconciliation.

Pause blocks new scheduling and reaches the current turn's safe boundary. Stop and
Cancel retain published results, cancel queued/active descendants, dispose pending
wakes, and hold native reservations until C2 proves retirement. The bounded restart
brief uses exact plan, script, input, assignment, feedback, decision, question,
answer, and pending-result references. Remaining limits come from the owner.
Pending or unknown native injection cannot silently retry.

The Model the Domain principle shaped the separate task, delivery, question, and
turn states. One implementation owner serializes their shared cut. Contracts and
machine checks came first; independent implementation workstreams did not apply.
A read-only Astra reviewer checks the completed change separately.

Attribution retains the pinned T3 Code repository, commit, source paths, Rust
adaptation, and MIT notice in the adapted modules and lifecycle fixture. The
workspace license and provider/model settings are unchanged.

## Verification

All commands in `c4-pre-h3-gate.sh` passed on the final implementation source. The main gate exited 0; M31, confirmation and S2-authority checks were appended after round 2 and run separately. The final wrapper uses `flock -o`.

| Log | Raw summary |
| --- | --- |
| `c4-build.log.gz` | ``    Finished `dev` profile [unoptimized + debuginfo] target(s) in 20.81s`` |
| `c4-clippy.log.gz` | ``    Finished `dev` profile [unoptimized + debuginfo] target(s) in 12.00s`` |
| `c4-lifecycle.log.gz` | ``test result: ok. 17 passed; 0 failed; 2 ignored; 0 measured; 17 filtered out; finished in 2.02s`` |
| `c4-kernel.log.gz` | ``test result: ok. 23 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.12s`` |
| `c4-store.log.gz` | ``test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s`` |
| `c4-session.log.gz` | ``test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 707 filtered out; finished in 12.98s`` |
| `c4-c3-witnesses.log.gz` | ``test result: ok. 3 passed; 0 failed; 1 ignored; 0 measured; 721 filtered out; finished in 1.67s`` |
| `c4-app-architecture.log.gz` | ``test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 0.02s`` |
| `c4-fmt.log.gz` | ``exit 0 (no output)`` |
| `c4-manifest.log.gz` | ``slow-test manifest and markers agree: 89 tests, features kinewright-agent/slow-tests,kinewright-app/slow-tests,kinewright-media/slow-tests; 59 allowlist entries well-formed`` |
| `c4-diff-check.log.gz` | ``exit 0 (no output)`` |
| `c4-m31.log.gz` | ``test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 724 filtered out; finished in 0.00s`` |
| `c4-confirmation.log.gz` | ``test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 716 filtered out; finished in 1.17s`` |
| `c4-s2-authority.log.gz` | ``test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 20 filtered out; finished in 1.46s`` |

`nice -n 19 python3 -I docs/reports/ja1-s4/c4-pre-h3-mutations.py` exited 0.
It recorded **10 protected runtime red/restored-green pairs**, with the raw
assertion failures and restored passes in compressed logs. Compilation failures
and zero-test runs are excluded. The campaign covers descendant readiness,
question-attention result selection, current-turn identity, question wake admission
and execution, published-result lifetime, delivery-time runtime response freshness,
late background completion, expired-session ingress, and acknowledged stop intent.
Every recorded source hash matches the restored final file. `c4-mutations.json`
retains the witness names, raw summary lines, source and log identities.

`c4-identities.json` binds final source, harness, test binaries and compressed logs.
The worktree build artifacts are removed after hashing; no build artifact is
committed. Ordinary capacity, context, background-progress, coalescing and T1
controls use tests and the normal gate under SPEED-RULES rule 4.
Every Cargo invocation uses `c1-cargo.sh`, including its memory check, FFmpeg setup,
`RUST_TEST_THREADS=4`, and the exact required wrapper:

```text
flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo …
```

All media-bearing test invocations use `--test-threads=1`. Python runs at
`nice -n 19 python3 -I`. Formatting uses only touched files with `rustfmt`, never
`cargo fmt --all`. Full suites remain in CI under the binding speed rules.

## NOT RUN

- H1 sealed publication, durable owner result-plus-wake acknowledgment, physical
  graph/receipt retention, independent disk reopen and crash recovery. The ignored
  `s4_durable_owner_lifecycle_reopen` names H1 in `ci/ignored-tests.txt`.
- H3 person-facing holder, attachment authorization, real approval/secret callback
  completion and approval attention wake. The ignored
  `s4_person_holder_approval_and_secret` names H3 in the allowlist. The question
  half of `s4_stale_question_and_approval` and explicit H3 refusals run now.
- C2 native launch, containment and retirement, actual machine-wide ownership,
  and C5 durable allowance/accounting integration. No runtime native success is
  inferred from contract reservations or the native-free stop-order fixture.
- C6 headless continuation, installed connector certification, native context
  injection, native Windows/G1-W, hosted fast/slow CI, and full touched-crate or
  workspace suites. No new comparative timing campaign is required or run.

The ignored H1/H3 witnesses fail explicitly on their missing prerequisite if forced;
they cannot produce a false acceptance pass while those owners are absent.

## Review fixes and follow-up T1

Astra round 1 returned REQUEST CHANGES. Its four Majors are addressed with separate
behavioral controls. Expired turns cannot become ready or accept late ingress.
Director answers can remain pending across pump ticks and return through a bounded
response queue after C1 owner acknowledgment, with freshness rechecked at delivery.
A continuation refuses until its outstanding background work settles under the
original identity. Mixed observed/disposed sibling deliveries settle their exact
shared wake, which is linked by EffectId instead of result-content coincidence.

Follow-up T1 from `.review/ja/s4-followups.md` is included at Riel's explicit request.
The test-only ACP lease fixture replaces the shell with `exec sleep` rather than
spawning an unowned sleeping descendant. Every fixture session has an RAII child
guard that calls the existing transport's kill-and-wait path on normal return and
panic. Normal fixture exits check the exact child is gone; the panic witness catches
a deliberate failure and independently checks the former PID. Production ACP
behavior is unchanged. The Cargo wrapper now uses `flock -o` so test children do
not inherit the shared-lock descriptor. Earlier diagnostic runs used the previous
binding wrapper; all final validation and mutations use `-o`.


Astra round 2 returned APPROVE WITH FOLLOW-UPS for code. It closed all four
round-1 findings and found no new data-loss or security Blocker/Major. Review
usage was not returned. Under SPEED-RULES rule 1, these two nonsecurity lifecycle
Majors are assigned to the first next-stage commit, without another C4 review:

- **C4-R2-1.** Expiry with outstanding background work retains its count and
  correctly rejects the old session's ingress, but no owner settlement transition
  currently clears that count after proven native cleanup. Add that transition
  and cover cleanup followed by a fresh continuation. This scenario is NOT RUN.
- **C4-R2-2.** Task readiness accepts cancelled descendants, but final Ready
  projection validation still requires every task to be Terminal. Permit settled
  cancelled descendants while requiring a completed root and settled cleanup and
  deliveries. Cover successful root completion after cancelling a queued reviewer.
  This scenario is NOT RUN.

These follow-ups limit lifecycle acceptance; the scoped test gate does not close
all of G6, G12 or G13. The second review also requested retained M31,
confirmation, and S2-authority checks, which are included in the final gate.


## Execution deviation

Early file-editing snippets used plain `python3` rather than the required
`nice -n 19 python3 -I`. They edited files only; validation and mutation Python
runs use the required wrapper. Every Cargo invocation used the then-binding
flock wrapper, with `-o` from Riel's update onward. This process deviation is
recorded rather than claimed as exact machine-rule compliance.
