# PF1 S4 runner reproduction checkpoint

Run 3 is a partial checkpoint at the actual 650/650 retained-line stop. It does not complete S4a or authorize lanes. Shipping production changes remain zero. The LL collector, strict NVIDIA table parser, complete paired native integration, replacement-series workflow and live/resume integration witnesses remain owed under R81. LH policy collection remains in S4c after Q1.

## Versioned contract

`docs/PF1-PINS.schema.json` defines the strict v1 interchange format. `scripts/pf1/validate.py` validates original artifact bytes, nested provenance, gate contracts and decisions. `scripts/pf1/launch.py` owns Linux child cleanup. `scripts/pf1/test_validate.py` runs the portable witnesses on both CI platforms and the disposable process-group witness on Linux. Resolve the checkpoint commit with `git log -1 --format=%H -- scripts/pf1/validate.py scripts/pf1/launch.py`. The report records the literal commit.

The pins and `example/` tree are synthetic fixtures, including their approval records and selected adapter. They are not accepted pins or measured evidence. Git attributes preserve the checked-in hash inputs as LF on Windows. The tests exercise an autocrlf checkout without normalizing arbitrary evidence bytes. Non-Markdown docs changes now trigger CI, including schema and fixture changes.

```bash
python3 scripts/pf1/test_validate.py
python3 scripts/pf1/validate.py --pins docs/PF1-PINS.json --run docs/reports/pf1-s4/example/status.json --root .
```

Windows uses `py -3`. Exit 0 means this supported contract passed, 1 means a failed decision, 2 means invalid/incomplete evidence, 75 means a valid pending/environment-limited/report-only result, and 101 means a collected runtime failure. `--spec-only` does not accept a gate. The launcher still needs the corresponding native failure classification integration. CPU load remains annotation-only. GitHub CI has no WARP timing leg.

The implemented registry names workload sets, protocols and lanes. G15 requires all six playback workloads plus title-only and three fresh-child measurements. G18 uses the unchanged canonical 24-clip, 120-frame export lane and S0. Timing comparisons require three pairs. G7a is an absolute one-build, one-run oracle, with no fabricated comparator. Other gates without an implemented registry contract return unsupported/incomplete; their obligations remain open. R65 requires all registered work/zero rows and all six three-seed rotations. The counter-name/readback mapping still requires native integration verification.

R74a's final preflight sample must precede run start by at most ten seconds. Policy sampling coverage is independently checked. Required metric semantics precede busy/runtime dispositions. G3 and VM-D6 floors do not use R50's shared-miss exemption. The reference full-range spread used by R50 remains the accepted plan's inferred operational definition, to be confirmed before real measurements.

## Immutable runner archive

Archive SHA-256 is `65524e8e64092f290d901ec2d90d088ef1c255af076059ac0d411584b6439e15`. Its seven members include the three extensions, the parent hash manifest and the unchanged S3b parent sources. Member bytes are checked after writing. The archive is local and read-only; no remote copy is claimed.

```bash
pf1_archive=/home/riels/Projects/Personal/Kinewright/target/review/pf/s4/archives/65524e8e64092f290d901ec2d90d088ef1c255af076059ac0d411584b6439e15.tar
printf '%s  %s\n' 65524e8e64092f290d901ec2d90d088ef1c255af076059ac0d411584b6439e15 "$pf1_archive" | sha256sum --check
mkdir -p target/pf1-s4-reproduction
tar -xf "$pf1_archive" -C target/pf1-s4-reproduction
```

The original checkpoint archive remains immutable at SHA-256 `43c456b7cbd46c0b3dc66d7331a1042419776ddf9d9d093882607603ccc6aa48`. The run-3 report and `s4/s4a-r3/runner-archive.json` bind both archives and every member. `PF1_REPO` must identify this checkpoint, since the restored Linux runner imports its versioned validator and child-lifecycle module.

## Runner limitations

A frozen series is `{"gates":[{"pins":{"path":"relative/pins.json","sha256":"<sha256>"},"test":"existing_test_name"}]}`. Exact arm/order/seed manifests remain required. Aggregate resume rehashes artifacts, but deterministic integration witnesses and the approved complete-replacement contract are unfinished. The old blanket replacement rejection remains conservative and is not an approved completed workflow.

The runner now starts its owned process group directly and cleans it in a finally path through bounded TERM, KILL and reap. Its disposable child witness includes ignored TERM and an exit racing cleanup. Invalid readback remains recorded through the parent attempt journal. Complete launcher integration remains partial. The existing native harness does not emit PF1_RECORD, and the runner still rejects missing readback. Do not run LL or LH from this checkpoint. No GPU settings, lane, CI dispatch, D13 implementation or PF1 closure is authorized here.
