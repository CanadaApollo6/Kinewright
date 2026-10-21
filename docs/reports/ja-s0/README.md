# S0 baseline evidence

The accepted contract is [JA-S0-CONTRACTS](../../JA-S0-CONTRACTS.md), particularly
the complete shared inventory (§3), handoffs (§§4–6), wire/observation protocol
(§7), exact regressions (§8), fixture slots (§9), and all five adopted conflict
resolutions (§12). These are frozen together; no production types or version
ceiling change is authorized by S0.

The machine-readable catalogs live in
`crates/kinewright-agent/tests/fixtures/ja/`. `source-pins.json` distinguishes
the accepted design pin from the implementation predecessor. `regressions.json`
uses full test names and targets. `hash-slots.json` keeps private practice and
independent editorial labels pending, including the two later retained-evidence
and first-save witnesses. No slot contains invented source material.

Run `source scripts/setup-ffmpeg.sh` and then
`nice -n 19 python3 -I scripts/ja_s0_verify.py N` for each stage N. The script
serializes the workspace, explicit harness and shipping App builds, clippy,
touched-file rustfmt, exact selected regressions and classification lint. Cargo
requests four build/test threads at nice 10, as the implementation brief specifies;
the nice-19 Python supervisor also confers its lower scheduling priority on
children. Media tests run serially. Stage 1 explicitly builds the existing eval harness;
stages 2–5 explicitly build the new baseline binary. Raw results and source/diff
identities are written to this worktree's ignored `target/ja-s0/`.
The runner waits at the required 6 GiB available-memory guard before each gate;
resource deferral is not a compiler/test result and launches no heavy work.

S0 results do not certify Studio gates. Windows durability (JA-1 S1), native
decoder snapshots (JA-1/media), guardian containment (JA-1 S4/AW2), connectors
(JA-1 S4/S5), person models (JA-2 A), independent editorial oracles (JA-4),
private practice and Riel's watch (JA-1 S7) retain their own pending owners.
PF1 H-T/G14 and decoder-bound A-1 remain independent.

S0-1 schema cross-check: the read-only Songbird checkout is still at
`aef7c95f1829754c5bc5b8f3bdbea91e175eeb39`. The actual export builder and
resolved-chart declarations match §5.1, including independent section row
counts, coupled nullable chart data/generation time, and ordered reference
lines. Their exact file hashes are in `source-pins.json`; this does not supply
any practice export or certify a Kinewright adapter. All five §12 resolutions
are adopted: one v3 policy, one project-owned store/handoff, explicit legacy
derivatives, the shared post-kill deadline, and separate AW2 capability gaps.

Each `S0-N-gates.json` contains the exact commands, exit codes, log hashes and
unabridged `test result:` lines. The corresponding evidence archives retain
native run headers, raw events, assertions, input/oracle/artifact hashes and
outer process receipts. The verification-log archive retains the per-commit
build, lint and exact-test logs. These were run before their respective commits:
their parent SHA plus dirty-diff/tree identities describe the tested bytes.
They are not reruns of a subsequently created commit SHA. The accepted contract's
draft-era NOT RUN text is historical; this ledger records the actual S0 runs.

`fixtures-index.json` freezes 17 native cases. After building, reproduce them
with the same fixture bytes in either mode:

```sh
source scripts/setup-ffmpeg.sh
nice -n 19 python3 -I scripts/ja_s0_freeze.py \
  --mode baseline --output target/ja-s0/reproduction \
  --app-test-binary /absolute/path/to/the/built/kinewright-app-test-executable
```

The verification script records that executable in `app-test-binary.txt`.
`--mode revised` uses the same protocol and fixture hashes; S0 installs no revised
production semantics. Output directories must be new. A pending-only case exits
3, missing prerequisites exit 2, actual failed baseline properties exit 1 and
successful current controls exit 0. A zero exit with a capability absent records
that absence and pending gates, without awarding the missing feature a pass.

Four literal baseline assertions remain red: save/reopen loses undo history;
the legacy save trace contains no parent-directory sync; rename refusal takes
the in-place fallback; retrying an accepted edit after suppressing its reply adds
a second undo entry. Each has a successful changed-document/full-byte or accepted
edit control and its actual/expected values in a separate assertion artifact.
These are observations of existing behaviour, not Studio acceptance results.

`scripts/ja_s0_mutations.py CASE --output NEW_DIRECTORY` temporarily removes one
observer/assertion behaviour, runs only its exact witness, restores the original
source bytes, and runs that witness green. Twelve cases cover history, IO,
comparison evaluation, optional cost nulls, duplicate JSON keys and the actual
paint callback, the independent letterbox landmark, role namespaces, participant
routing, shared main-adapter dispatch, repeated media evidence and native deadlines. The archive retains the literal failed test, restored test,
source hashes, binary hashes and common reports. Later recording and paint
rechecks are separately labelled; earlier attempts are not relabelled as final.
The first nine captures used crate-relative report paths. Their 18 phase report
directories were recovered from the crate-local ignored `target/` trees and
included under their original phase identities. Later captures use absolute
paths. Paint and letterbox reports retain the actual failed protocol assertion;
Rust-only assertions also retain their original test failure logs.

Native CLI work runs in a fixed fixture subprocess. The supervisor observes
setup/action/cleanup boundaries and interrupts a blocked fixture at its deadline,
retaining the phase, owned process identity, termination and direct-child reap
receipt as NOT RUN. Unix fixtures use a dedicated owned process group; Windows
tree termination remains an unrun native prerequisite. This is eval supervision
for fixed roles, not the production guardian or a host-death/resume certificate.
The in-process library adapter is for witness tests; hard interruption is supplied
by the native CLI. Main-only media/App/session adapters refuse other participants
at admission, and repeated media artifacts include participant/action identity.

The App lane uses actual coalesced edit/Core/finalization/presenter/journal paths
and an actual headless paint callback. Its independent solid-colour and letterbox
landmarks are synthetic controls. It does not certify a physical display or a
model's ability to watch or listen. Actual preview and paint adapters are recorded
separately, including software fallback. Native process samples identify only
the processes sampled, at stated intervals; they are not whole-job peak RSS,
VRAM allowance, end-to-end host latency or performance acceptance.

Loopback sessions use the real ScriptedDriver, pump, server and prepared-plan
confirmation paths. Arbitrary scripted filesystem calls and external-media/LUT
admission are refused by this harness boundary and remain NOT RUN. Generated
solid controls exercise shared preview/export/H264 decode. No decoder snapshots,
guardian, receipts, scope policy or delivery sealing were implemented.

`gate-observers.json` retains 48 qualified pending rows with E/P/B/M definitions,
current observers and next owners. JA-1 G1–G20 (including qualified subgates),
JA-4 E1/G1–G11, JA-2/AW2 and shared PF1/CI obligations remain pending even when
their current-path controls pass. All 24 source/hash slots and 21 editorial slots
retain NULL hashes. Windows/storage-crash, installed-provider/model, private
practice, capture clocks/coverage, independent editorial labels, whole-output
checks, human review and fast/slow CI have not run here. The lead owns CI/push;
stage owners are recorded individually in the gate catalog.

Two test-lifetime adjustments were needed to obtain honest process receipts:
the S0-3 recording control exposed the existing media-worker finished signal
earlier than S0-4, and three retained generated-media tests now wait on that
test-only observer. The retained S0-3 recording attempt printed a passing summary
and then exited with SIGSEGV during teardown; it receives no green/red credit.
A separate S0-4 diagnostic failure was observed before the waits, but its failed
process receipt was not retained. No evidence claim relies on that attempt.
Production teardown behaviour
was not changed. The G9 geometry observation was completed in S0-5 while sealing
the ledger; it is cfg(test) and introduces no comment semantics.
