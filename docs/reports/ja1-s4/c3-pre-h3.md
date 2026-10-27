# C3 Editor lifecycle before H3

This commit is based on H2 `cdfe7c3b1c0ef54c4701de7abbc8f7f1f683b847` on
`t3/ja1-s4-c1`. Work stays in the assigned worktree. No push is made.

## Scope

`tasks.rs` prepares a registered specialist from the exact candidate binding.
A runtime endpoint binds project, run, task, attempt, endpoint, owner epoch,
expiry and Core identity. The existing seven-tool dispatcher serializes admitted
requests with freeze. Freeze closes admission before draining the current call,
then H2 persists one continuously frozen cut. A prepared plan cannot commit after
closure. Merge checks the full applied delta again against the original base and
retains the original receipt-bearing envelope across reply loss and replay.

`policy.rs` exhaustively classifies all Core Operation variants. It checks exact
operation, object, field and track sets, then privately replays each operation to
check intermediate effects and shared routing/sidechain closure. Registry effect
classes determine motion, audio and colour authority. Unknown operations, fields,
effects and input-acquisition routes refuse. Reviewer is mutation-free. Editor
has structural authority; Director still requires every explicit scope domain.
The server checks both ordinary and direct capability dispatch, planner expansion,
prepare and actual mutation. The pure motion planner is admitted only after its
expanded operations pass the task policy; other unclassified planners remain
unavailable. Source probes and export cancellation remain outside task authority.

The pure kernel adds direct Director child assignment and Freeze decisions.
`Kernel::propose_result` receives a staged typed result reference and prepares a
complete `ValidatedRunUpdate` under the same sole pending-publication guard.
Mailbox preparation binds the typed result bytes, terminal task, stable delivery
identity and pending parent wake into one proposed cut. Only H1 can stage and
acknowledge that cut. Provider text does not publish a result or acknowledge a
delivery.

`adapters.rs` defines the shared `GuardianLauncher` port with §5's exact
`AdmittedAttempt`, `CertifiedConnectorProfile`, `ContainedSession` and `LaunchError`
types. `ContainedDriver::start_session` consumes one durable reservation and calls
that launcher. A changed session profile refuses before consuming it; failed
containment cannot retry the reservation or invoke an uncontained fallback. The
certified profile has no public constructor or deserializer. The current witness
uses a refusing test double with an actual validated C1 reservation.

H3 retains resolved S3 attachment issuance, HTTP/MCP/direct-owner dispatch,
run-start and attention authority, task narrowing and the host/App adapters.
There is no production scoped-server constructor or certified profile enabled in
this commit. Fixture entry points are explicitly test/eval-only. C2 supplies
native containment and its launcher implementation; existing provider drivers
are not edited across worker B's ownership boundary.

## Witnesses and evidence

`server::tests::s4_drain_and_durable_base` uses the production dispatcher and
registered project owner. An admitted commit finishes after freeze closes new
ingress. Its split is in the frozen delta; prior Undo/Redo does not duplicate it.
Late work refuses. The test reopens before merge, rejects a different durable
head despite reusable runtime counters, merges once and retries the original
envelope. It also rejects expired/project/epoch admission and an unauthorized
frozen delta before changing the target.

`server::tests::s4_role_scope_closure` applies a real audio mix setting change.
Invalid mixed batches, video/sync/ripple/global-colour changes, shared sidechains,
imports/relinks, export/task actions, direct dispatch, wrong Core identity,
revoked plans and expired authority leave the exact prior history. An input-probe
observer stays empty on a rejected import. Motion planning expands its real
operations before scope refusal. This is server-fixture scope evidence,
not real-connector G12 certification or rendered audio certification.

`s4_editor_result_to_candidate` is compiled and ignored. Its allowlist entry names
H3 and C2. The fixture scenario covers consecutive Editor and Audio work from
successive durable candidate heads with a pending manual edit, fresh read before
merge, original-envelope retry and exact manual preservation. H3/C2 must supply
production authentication, provider release, Director wake/observation and their
integration before the named witness can close G13. **G13 is not claimed.**

The policy tests cover explicit domains, no-op arguments, intermediate effects,
shared old/new routes, solo/master collateral, registry effect classes, effect
copying and generated identities. The kernel fixture checks child assignment,
Freeze and typed-result/pending-delivery proposals. Full touched-crate and
workspace suites stay in CI under the binding speed rules.

`bash docs/reports/ja1-s4/c3-pre-h3-gate.sh` exited 0 on the final source.
All Cargo commands use `c1-cargo.sh`, which checks at least 6 GiB available,
sources FFmpeg in the same shell, then executes the required
`flock /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo`.
Python runs as `nice -n 19 python3 -I`; media-related tests use one test thread.
Touched-file rustfmt uses `skip_children=true`. No workflow file changed.

| Gate log | Raw result |
| --- | --- |
| `c3-agent-architecture.log.gz` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 723 filtered out; finished in 0.02s` |
| `c3-app-architecture.log.gz` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 0.02s` |
| `c3-branches.log.gz` | `test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.32s` |
| `c3-build.log.gz` | `    Finished `dev` profile [unoptimized + debuginfo] target(s) in 15.41s` |
| `c3-clippy.log.gz` | `    Finished `dev` profile [unoptimized + debuginfo] target(s) in 10.74s` |
| `c3-confirmation.log.gz` | `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 715 filtered out; finished in 1.15s` |
| `c3-diff-check.log.gz` | `exit 0` |
| `c3-fmt.log.gz` | `exit 0` |
| `c3-h2.log.gz` | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.60s` |
| `c3-host-authority.log.gz` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 20 filtered out; finished in 1.46s` |
| `c3-kernel.log.gz` | `test result: ok. 23 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.12s` |
| `c3-m31.log.gz` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 723 filtered out; finished in 0.00s` |
| `c3-manifest.log.gz` | `slow-test manifest and markers agree: 89 tests, features kinewright-agent/slow-tests,kinewright-app/slow-tests,kinewright-media/slow-tests; 57 allowlist entries well-formed` |
| `c3-policy.log.gz` | `test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 707 filtered out; finished in 0.10s` |
| `c3-receipts.log.gz` | `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 62.22s` |
| `c3-review-fixes.log.gz` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 723 filtered out; finished in 0.00s` |
| `c3-store.log.gz` | `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s` |
| `c3-witnesses.log.gz` | `test result: ok. 3 passed; 0 failed; 1 ignored; 0 measured; 720 filtered out; finished in 1.74s` |
| `c3-writer.log.gz` | `test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.96s` |

The campaign and targeted refreshes recorded 18 protected runtime
red/restored-green pairs. They cover prepare, commit, direct dispatch, planner
expansion, endpoint/Core identity, live authority, freeze ingress, merge scope,
Audio and Editor role limits, owner project/epoch/expiry, typed frozen-result
identity, pending publication, routing closure and solo collateral. All 18 restored
guard hashes match the final source. The raw logs and `c3-mutations.json` retain
literal assertion failures and restored passes. Compilation failures and zero-test
runs are excluded. H2 retains six pairs, giving 24 across the two commits.

`c3-identities.json` records final source, test-binary and compressed-log identities.
Local `target/` artifacts are removed after verification under the cleanup rule.


## Independent review

The first Astra xhigh review returned CHANGES for three findings.

- C3-R1 identified solo-gating collateral when removing the only soloed track.
  Before/after `track_audible` comparisons now require every affected surviving
  track and clip to fit the scope. The server witness observes the audibility
  change, refuses an excluded survivor, and compares the exact prior history.
- C3-R2 identified a Director routing-membership bypass when no bus participated.
  Every track in an audio-routing validation now needs explicit routing membership,
  independent of role. The witness refuses an empty closure, then applies the same
  clip-audio operation when its routing permission is present.
- C3-R3 identified ordinary Editor splits/deletions refusing existing transforms
  and grades. Direct operation authority is checked first. Core's incidental effect
  preservation, rebasing and removal then keep their object/field/track and registry
  checks without demanding direct Motion/Colour authority. The regression covers an
  animated transform plus grade, and still refuses direct Editor effect edits and
  missing generated-object/parameter permissions.

The second Astra xhigh review returned APPROVE and marked C3-R1, C3-R2 and C3-R3 resolved. It checked all 18 mutation pairs and the final source identities. No findings remain open or were declined. Review usage was not returned.

## Handoff and NOT RUN

- H3 resolved attachment and transport integration, run-start and attention
  authorization, and host/App adapters.
- C2 native launch/containment, startup/cleanup timing and machine claims.
- H1 sealed orchestration publication, actual durable result-plus-Director-wake
  acknowledgment and observation. The mailbox tests use proposed cuts only.
- The ignored `s4_editor_result_to_candidate` witness and end-to-end G13.
- Real connector ambient shell/file bypass certification, provider subscriptions,
  native/rendered audio samples and complete G12 certification.
- Native Windows/G1-W, hosted fast/slow CI, full touched-crate/workspace suites,
  C4 request/delivery continuation, C5 accounting and C6 headless continuation.
- No new comparative performance campaign was required or run.
