# PF1 S4a run 4 reproduction checkpoint

Run 4 stops at exactly 1000 retained counted lines against the final 1000 stop, with zero shipping production changes. Acceptance still requires the lead's narrow re-review and both hosted CI platforms. No timing lane, GPU setting change, SSH or push was performed. The run report records checks and outstanding verification.

## Versioned contract

`docs/PF1-PINS.schema.json` defines the strict v1 format. `scripts/pf1/validate.py` rehashes original artifact bytes and applies supported gate contracts. `scripts/pf1/launch.py` contains the NVIDIA table parser, software-device readback checks, live monitor and owned-child cleanup. `scripts/pf1/series.py` implements the same collection/resume path exercised by portable deterministic tests. The Linux wrappers import these files through `PF1_REPO`.

The pins, approval records and example status are synthetic fixtures. The NVIDIA process-table block is literal captured text from the S3b 2026-10-14 preflight, not a current hardware query. Its provenance and test variants are recorded in `s4/s4a-r4/nvidia-fixture-origin.json`. LF attributes preserve original bytes in Windows checkouts. Schema and fixture changes trigger CI.

```bash
python3 scripts/pf1/test_validate.py
python3 scripts/pf1/validate.py --pins docs/PF1-PINS.json --run docs/reports/pf1-s4/example/status.json --root .
```

Windows uses `py -3`. Exit 0 means a supported gate passed, 1 means a failed threshold, 2 means invalid/incomplete evidence, 75 means pending/environment-limited/report-only, and 101 means a runtime assertion failure. `--spec-only` validates launch inputs without accepting a gate. Unsupported contracts cannot pass. CPU load remains annotation-only. GitHub CI has no WARP timing leg.

The registry fixes workloads, lanes, protocols and essential parameters. G15 requires all six playback workloads plus title-only, S0 and three fresh-child pairs. G18 requires S0, the canonical 24-clip/120-frame export, wall ratios and complete-file equality. I4 pins 30 warmup and 300 measured frames. Native/P-play records pin 2000 ms warmup and 60 s duration. G3 and VM-D6 floors are strict. R50 uses the reference full-range spread ratified by R82. R65 preserves immutable counter types, zeros and the six three-seed rotations.

## Immutable Linux runner archive

The archive SHA-256 is `2a60782fa92b5a5a7483ceb872ce8368caef81ec57929a9a6943b1524481da6d`. Its seven members contain the three extensions, unchanged S3b parents and parent hashes. All member bytes were read back and checked. It is a local archive, not a published artifact. Earlier archives remain unchanged.

```bash
pf1_archive=/home/riels/Projects/Personal/Kinewright/target/review/pf/s4/archives/2a60782fa92b5a5a7483ceb872ce8368caef81ec57929a9a6943b1524481da6d.tar
printf '%s  %s\n' 2a60782fa92b5a5a7483ceb872ce8368caef81ec57929a9a6943b1524481da6d "$pf1_archive" | sha256sum --check
mkdir -p target/pf1-s4-reproduction
tar -xf "$pf1_archive" -C target/pf1-s4-reproduction
```

Use this commit's validator, schema, launch and series modules. Include the actual restored wrappers and imported versioned modules in the frozen harness manifest. The run report and `s4/s4a-r4/runner-archive.json` bind their hashes and source commit. Real production/build/host/input/policy manifests and binary hashes remain required; synthetic examples cannot authorize a lane.

## Collection and replacements

A series plan has the form `{"id":"unique-attempt-id","gates":[{"pins":{"path":"relative/pins.json","sha256":"<sha256>"},"test":"existing_test_name","series":{}}]}`. The wrapper creates an exclusive attempt journal and immutable series manifest under `work/series-<id>`. It records returned attempts and caught OSError/ValueError launcher failures, writes a rejected aggregate, and hashes the journal plus attempt artifacts, including malformed/busy raw output. Resume checks the saved plan and revalidates every referenced artifact. It never relaunches a cached aggregate automatically.

For a manually initiated complete replacement, choose a new ID and set the gate's `series` to `{"previous":{"path":"old/status-0.json","sha256":"<sha256>"},"rejection":{"exit":2,"reason":"explicit rejected-series reason"}}`. The old status must identify its original series and exact pins hash. Complete replacement uses the entire frozen order, not selected successful pairs. Cross-pins links, repeated ancestry and a predecessor without attempt artifacts are invalid. Rejection codes must match the predecessor's evaluated disposition. Keep every rejected status, journal, raw file and hash.

A threshold-failed predecessor requires a hashed `rejection.disposition` object containing `authority`, `previous_sha256`, `reason` and a hashed `record`. That record supplies the explicit fix/ruling disposition of all threshold failures. Incomplete or assertion-failed predecessors containing collected rows also require this disposition conservatively, because an incomplete aggregate must not conceal a threshold failure. An interruption before an aggregate was written still needs a frozen predecessor receipt naming the original series and every preserved artifact; this checkpoint does not provide or verify a sealing command for an abruptly killed parent. Do not erase an existing journal or start an unlinked replacement.

The minimal collector is LL/lavapipe native G4. Its test-only hook reads the selected session's `AdapterInfo`, emits software status, driver information and final fps/Q2 validity, and publishes atomically. The launcher derives Mesa/LLVM and the configured single ICD manifest hash from that readback, verifies it against pins, and binds publications to a fresh per-launch token and the launch timestamp. The monitor checks before every sampling step. Absolute misses reach complete paired decisions; genuine correctness assertions still return 101. The preflight's final sample must be at or before actual launch and at most ten seconds old.

Real LL collection is unmeasured in this unit. Host manifest integrity is checked, but current CPU/RAM/OS readback is not compared at launch. Other protocols do not acquire a collector merely by appearing in the portable registry. Native I4/R65/RSS collection and complete parent-interruption recovery remain unproven. LH UUID correlation, hardware policy readback and native LH integration stay in S4c after Q1. D13 stays in S4b; E16 and closure stay in S4c. Q1/Q5 and all historical unresolved measurements remain open.
