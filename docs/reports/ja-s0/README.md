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
uses four build/test threads at nice 10, as the implementation brief specifies;
media tests run serially. Stage 1 explicitly builds the existing eval harness;
stages 2–5 explicitly build the new baseline binary. Raw results and source/diff
identities are written to this worktree's ignored `target/ja-s0/`.

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
