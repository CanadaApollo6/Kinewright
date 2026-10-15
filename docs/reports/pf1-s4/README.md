# PF1 S4 runner reproduction checkpoint

This is a bounded S4a checkpoint, not PF1 acceptance. The retained count is 450/450, including c3bb9e1. The native harness does not yet emit the required live collector JSON. The Linux launcher therefore rejects current binaries instead of manufacturing selected-adapter, driver, policy or metric readback. No lane was run for this checkpoint.

## Versioned contract

`docs/PF1-PINS.schema.json` is the strict v1 schema. `scripts/pf1/validate.py` implements the portable subset of JSON Schema used here, nested artifact rehashing, GPU state checks and gate decisions. `scripts/pf1/test_validate.py` contains the same standard-library witnesses for CI-L and CI-W. Resolve their introducing commit with `git log -1 --format=%H -- scripts/pf1/validate.py docs/PF1-PINS.schema.json docs/PF1-PINS.json`; the implementation report records its literal hash. The parent parser checkpoint is c3bb9e199c2708da9134092a511a07337c30dcb0.

`docs/PF1-PINS.json` and `example/` are synthetic contract fixtures. They contain no measured binary, production source, media, machine, GPU approval or CI evidence. The example policy's authority field illustrates the schema only. The file named synthetic.txt is deliberately labelled. A synthetic gate pass is not PF1 acceptance. Real pins must replace every synthetic input, source, harness, binary, approval and raw-log reference with independently collected artifacts under the supplied evidence root.

```bash
python3 scripts/pf1/test_validate.py
python3 scripts/pf1/validate.py --pins docs/PF1-PINS.json --run docs/reports/pf1-s4/example/status.json --root .
```

Windows uses `py -3` in place of `python3`. The validator has no Linux launch dependencies. Exit 0 means this pins record's complete gate passed; 1 means a failed decision; 2 means invalid or incomplete evidence; 75 means GPU busy, an environment-limited absolute miss, or a report-only result. `--spec-only` checks launch inputs and prints launch_spec_valid. It does not claim acceptance. CPU magnitude never gates. GitHub CI has no WARP timing leg; deterministic Windows GPU coverage remains in the existing jobs.

## Immutable Linux runner archive

Local content-addressed archive:

`/home/riels/Projects/Personal/Kinewright/target/review/pf/s4/archives/43c456b7cbd46c0b3dc66d7331a1042419776ddf9d9d093882607603ccc6aa48.tar`

SHA-256: `43c456b7cbd46c0b3dc66d7331a1042419776ddf9d9d093882607603ccc6aa48`.

It contains work/lane.py, work/final-measure.py, work/final-figures.py, work/parents.json and the three original parents under parents/. All parent hashes are in work/parents.json. The archive has fixed member timestamps, is read-only, and is not regenerated on extraction. Its stable location is local to this workstation; no remote copy or push is claimed. Restore by its digest, not by copying the mutable s4/work directory:

```bash
pf1_archive=/home/riels/Projects/Personal/Kinewright/target/review/pf/s4/archives/43c456b7cbd46c0b3dc66d7331a1042419776ddf9d9d093882607603ccc6aa48.tar
printf '%s  %s\n' 43c456b7cbd46c0b3dc66d7331a1042419776ddf9d9d093882607603ccc6aa48 "$pf1_archive" | sha256sum --check
mkdir -p target/pf1-s4-reproduction
tar -xf "$pf1_archive" -C target/pf1-s4-reproduction
```

The report's runner-archive.json records each member and versioned contract hash. Its existing-records.json names the old input/source/binary/log records and missing fields; none is relabelled as S4 evidence. No real S4 pins, accepted reference binary or raw measurement log is supplied by the synthetic example.

## Collector and execution contract

A frozen series manifest is `{"gates":[{"pins":{"path":"relative/pins.json","sha256":"<64 lowercase hex>"},"test":"existing_test_name"}]}`. Every gate must be explicitly listed. Each pins protocol.order lists the exact workload/pair/arm/seed sequence; manifests include all arm builds and input identities. Ordinary comparisons name accepted-S3b, G18/G15 name S0, and the three R65 comparisons name their historical or intervening bases. The cumulative R65 comparison is diagnostic and cannot return acceptance. Complete cached aggregate statuses are rehashed before reuse. Attempts use exclusive journals and files; an incomplete or failed series is retained and requires an explicitly ruled new series, not silent retry filtering.

The future test-only collector must atomically publish PF1_RECORD within ten seconds of device creation and refresh its gpu readback during sampling. Its final object follows schema $defs/run, including actual device-creation metadata, approved-policy readback, timestamped telemetry covering started_s through ended_s, identity hashes, CPU annotation and metrics. The Linux launcher supplies actual exit/command/relevant environment, preflight, raw artifact hashes and fingerprint. It checks identity before launch, checks live readback during monitoring, and terminates its owned process group on invalid readback. A collector schema file is an interchange contract; current S3B_ADAPTER text lacks vendor/device/driver/UUID/policy fields and is intentionally insufficient.

After the collector is implemented and a lane is separately authorized:

```bash
export PF1_REPO=/home/riels/Projects/Personal/Kinewright-pf1
export PF1_ROOT=/absolute/evidence/root
python3 target/pf1-s4-reproduction/work/final-measure.py /absolute/series.json
python3 target/pf1-s4-reproduction/work/final-figures.py /absolute/series.json
```

The launcher remains Linux-specific. Windows and manual VM collectors must emit the same portable records. This checkpoint does not add those collectors, authorize GPU settings, dispatch CI, run lanes, implement D13 hooks, or resolve Q1/Q5. Further collector integration and decision/runner oracle coverage need Amendment R81 and a new retained-line allowance. Source completeness and S4a acceptance remain partial until then.
