# PF1 S4 reproduction checkpoints

## R91 round-3 final-source context

Source: `4783d5f`, **1,190 counted lines**. The authoritative report is the
main-checkout `target/review/pf/s4c-report.md`. First collection: G4 exit 2
(Q2 invalid, three missed callbacks, only 3/24 rows), G15 exit 2 (startup), D13 exit
0 (three children, 30 cycles each). These bytes remain unchanged. G4 is an open
in-scope finding and is not silently rerun or deferred.

The startup fix starts the launch clock after diagnostics and gives G15 input
consumption one native owner. Actual selected-device readiness may precede input
validation; consumed hashes remain mandatory before metrics/final validity and
on cached validation. No timing/RSS threshold or input was changed.

The final-source G15/D13 plans and actual Python modules are under
`target/review/pf/s4/s4c/round-3/startup-final/`. G15 passed all 42 fresh children (maximum paired delta +2,727,936 bytes versus a +4,194,304-byte limit); D13 passed all three fresh children × 30 cycles. Both cached figures runs exited 0. D13 whole-process RSS deltas were 99,082,240 / 94,298,112 / 104,382,464 bytes, all above U=93,312,000 and report-only. The full fast tier passed separately: 3,618 passed, 0 failed, 100 expected ignores; strict Linux `verify-fast` exited 0. `previous-context.json` links the sealed startup failure;
the full new order uses no old rows. The new runner archive is
`target/review/pf/s4/archives/8dab58af55029abcfea54e2c5188c3fbcd9f1037cbf737ad2bd2b34289115916.tar`,
SHA-256 `8dab58af55029abcfea54e2c5188c3fbcd9f1037cbf737ad2bd2b34289115916`.
`startup-final/runner-archive.json` records all seven member hashes. Its source
archives contain the exact compiled trees; S0's production basis is `4082d5d`
with the common `4783d5f` collector and the same 31-line test-only readback overlay.
The binary metadata's Rust commit names the common overlay for S0, not a claim
that its shipping implementation is the candidate's.

Restore the archive by hash and use the frozen Python root named in the chosen
`frozen-series.json`. The launch supervisor stays at nice 0 only to spawn owned
native work at nice 10; the sampler lowers itself to 19. Checks run at nice 19.
Wait for the 6 GiB start threshold, source FFmpeg, then run each plan serially via
`setsid nohup python3 .../work/final-measure.py <plan>` and validate with
`nice -n 19 python3 .../work/final-figures.py <plan>`. Each native/wrapper invocation
has its own sub-600-second watchdog; the series schedules those invocations.
Existing sealed series are audited on resume, never overwritten. An unsealed
journal blocks further collection and requires a ruling.


D13 cleanup failure can emit one late `D13 sample` line within the same already-failed test binary (RR-2, R88).

- AVBuffer padding beyond the plane spans is not observed;
- FFmpeg-internal buffer-pool lifetimes are not observed;
- identity means plane address plus a lifetime generation, and a generation is recorded at each registration so that a reused pooled address is a new identity.

The historical 8,250 ms hold remains unexplained.

The R91 qualified scope carries these named deferrals, each **deferred, not passed**:

- I4, the LH part of I5, I8's lane-based L6, G3, G6, G7b, G8/L1–L6 and L4b, G11, G16, G17, both G18 decisions, R65 historical/intervening/cumulative, the standing PERFORMANCE reports, D6/the manual-VM WARP floor, and the RTX 4050 rerun;
- automatic journal-only sealing (B3). Until it exists, an interrupted unsealed series blocks: preserve every byte, stop, and return for a ruling (critic §2);
- the numerical whole-process RSS allowance, which stays unresolved;
- the D13 witness-body refactor (R86 B-7).
- the common LH collector and Q1(b) default-policy variant — deferred to PF1-LH, not passed;
- LH G1, G2 (including the 240 s talk) and G14 — deferred to the scheduled PF1-LH follow-up slice, not passed.

G3 and LH L4a remain unresolved under driver defaults. A failed in-scope LL gate remains a finding for lead disposition.


## S4c round 3 frozen-input checkpoint

R91 limits new lanes to LL G4/G15/D13; LH G1/G2/G14 move to PF1-LH,
deferred, not passed. This input checkpoint does not establish a native verdict.
The shared test-only `pf1_harness::s4_prepare_inputs` exports one selected workload
into a new `PF1_ROOT/PF1_PREPARE` directory before pins are frozen. The preparation
copies each source, rewrites the document to those paths, and checks the exported
bytes. Invoke it with `PF1_ONLY=<workload>` and `--ignored --exact`.

The launcher forwards the pinned workload manifest with its digest. The native
importer verifies the document, exact media list/paths and every media digest;
its publications carry the three consumed manifest digests. Launch and final
readback rehash both the manifests and nested media files and reject omitted or
substituted consumption. Preparation is separate from measurement.

Runner archive: `target/review/pf/s4/archives/226fd2764305be0c9982b6111f691d208ac74176d75f7302c67e1aa29245a9d4.tar`,
SHA-256 `226fd2764305be0c9982b6111f691d208ac74176d75f7302c67e1aa29245a9d4`. Restore it as below using that digest and this commit's versioned
modules. `s4/s4c/round-3/input-runner-archive.json` binds the seven members.
Control `Wrapper.test_frozen_consumption --mutation consumption` removes the
consumption guard in a private runner copy; the native hash control is recorded
with its restored green under the round-3 logs. The native collectors and final
lane outcomes are recorded separately in the round-3 report.

## S4c round 2 host and runner checkpoint

This checkpoint adds Linux CPU, total physical RAM, kernel version and
architecture comparison before child launch. RAM means the OS-addressable
physical-page total (`SC_PHYS_PAGES * SC_PAGE_SIZE`), never available/free RAM.
Readback is retained in the attempt receipt, including a mismatch that launches
no child. Portable mocked readback witnesses run on both OSes; Linux wrapper
integration does not claim a Windows launch implementation.

Gate dispatch uses fully qualified, exact test names. The frozen order's seed
is forwarded as `PF1_SEED`; an unsealed sibling series journal blocks collection
without changing its bytes. Each lane has a 590-second alarm that unwinds owned
child cleanup, bounded two-second diagnostic queries, and a 599-second external
watchdog with one second before KILL. A hard interruption still requires a ruling
on the preserved unsealed series; automatic sealing remains deferred.

The seven-member immutable runner archive is
`target/review/pf/s4/archives/5aa36aac6fe7191ece8f7ebd068790486f1734d88d757a21cff188281864162d.tar`,
SHA-256 `5aa36aac6fe7191ece8f7ebd068790486f1734d88d757a21cff188281864162d`.
Use the restore/check commands below with this digest and this checkpoint's
versioned modules. `s4/s4c/commit-2/runner-archive.json` binds all member hashes;
the round-2 report records controls, checks, line counts and the re-forecast.
Additional wrapper controls are `host`, `dispatch`, `unsealed` and `seed`;
portable controls are `host_shape`, `host_ram` and `deadline`. These are contract
witnesses and do not supply native G4/G15/D13 or LH acceptance.

## S4c wrapper witness

The S4c wrapper checkpoint covers a real disposable publishing process through
measure, lane, collection, cached resume and figures. Its pins and adapter are
synthetic; it supplies no native LL/LH measurement or PF1 closure claim.
The report and source/log manifests live under the main checkout's
`target/review/pf/s4/s4c/commit-1/`; `target/review/pf/s4c-report.md` records the
checkpoint commit, counted lines, gate results and remaining allocation.

The immutable runner archive is
`target/review/pf/s4/archives/8334078951a2ebb8959caced0a16f47adf4886166e8bf50e15df15b814d28376.tar`
in the main checkout. Its SHA-256 is
`8334078951a2ebb8959caced0a16f47adf4886166e8bf50e15df15b814d28376`.
It contains the three wrappers, their unchanged S3b parents and parent hashes.
`s4/s4c/commit-1/runner-archive.json` records every member hash. Restore by this
digest, check the archive's SHA-256 before extraction, then set `PF1_RUNNERS` to
the extracted directory and run `python3 scripts/pf1/test_lane.py` from this
implementation commit. The versioned validator, schema, launch and series
modules must come from the same commit. Mutation controls use
`--mutation publication|final|receipt|assertion|readback|kill|validity|failure_pins|failure_order` with the
corresponding test named in the report. These are executable synthetic witnesses,
not replacement native collectors.

```bash
pf1_archive=/home/riels/Projects/Personal/Kinewright/target/review/pf/s4/archives/8334078951a2ebb8959caced0a16f47adf4886166e8bf50e15df15b814d28376.tar
printf '%s  %s\n' 8334078951a2ebb8959caced0a16f47adf4886166e8bf50e15df15b814d28376 "$pf1_archive" | sha256sum --check
mkdir -p target/pf1-s4c-reproduction
tar -xf "$pf1_archive" -C target/pf1-s4c-reproduction
PF1_RUNNERS="$PWD/target/pf1-s4c-reproduction" PYTHONDONTWRITEBYTECODE=1 nice -n 19 python3 scripts/pf1/test_lane.py
```

Each spawned attempt writes an exclusive receipt after owned-child cleanup,
including the direct child's reap/exit result, attempted signals, launch token,
pins digest and raw artifact hashes. Success rows bind that receipt in their
fingerprint; failed series bind it explicitly and preserve assertion exit 101
through figures and resume. Incomplete publication and malformed readback remain
invalid evidence. A changed raw artifact invalidates cached results.
Automatic sealing after a hard-killed parent remains deferred under R89-3;
an interrupted unsealed real series blocks further collection.

## S4a run 4 reproduction checkpoint

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

At the S4a run-4 checkpoint, real LL collection and native I4/R65/RSS
publication were unmeasured; current CPU/RAM/OS readback was not compared at
launch. S4c adds the host comparison and G15/D13 publication. See the R91 section
above for actual native outcomes. Other protocols do not acquire a collector
merely by appearing in the portable registry. Native I4/R65 and complete
parent-interruption recovery remain unproven. R91 defers LH UUID correlation,
hardware policy readback and native LH integration to PF1-LH. R89 answers Q1/Q5
on policy and qualified scope; its named measurement deferrals remain.


The S4c G15 parent now consumes the wrapper token and publication path and runs
one fresh native child per frozen order row. Its final record includes the child
PID, value, successful reap/exit receipt and `native_children=1`. The 42-row
portable G15 contract therefore means 42 children. The parent logs each child's
stdout and stderr before checking its exit; failed children cannot supply
accepted measurements. `pf1_s4_publish.rs` is the shared test-only publication
owner used by the historical overlays; moving it is included in the line count.

D13 preparation also exports `calibration.json` from an independent decode of
the copied GOP-60 source. Its hash and measured `d_bytes` are frozen in pins.
The D13 wrapper order has one arm and three rows; each row starts one fresh child
and runs 30 cycles. Every final record retains the 60 applied-plan intervals,
nonempty lifetime identities, acknowledgement and two sample timestamps, native
cleanup receipt and raw RSS figures. The retained-plane oracle and cleanup are
invariants; whole-process RSS is report-only pending its separate ruling.

Cached native G4/G15/D13 rows revalidate the final collector artifact, token,
publication interval, protocol, metrics, selected adapter and consumed inputs.
G15/D13 additionally require one successfully reaped child per row and distinct
child PIDs; D13 rechecks calibration/U and every cancellation interval. Hashing a
modified collector and re-signing its row cannot replace a missing measurement.


## S0 comparator overlay

The S0 reference is `4082d5d2e860d88a46a6d01f5b3ac4357aebbf8d`, the final
S0/R27 measurement state before S1. Both arms compile the identical shared
`pf1_g15.rs`, input importer, publisher and child-receipt owner. The historical
compositor adds only test-gated adapter metadata; its production projection and
`lib.rs` match the original source. S0 has no engine-finished receiver: after
measurement it drops its original session, and successful process exit plus
parent reap supplies its cleanup receipt. The candidate also verifies its
engine's bounded teardown record. No S0 internal worker-join claim is made.

The complete S0 build source is retained in
`target/review/pf/s4/s4c/round-3/6dcc2858d6c45e1d33769946472702782d070138826f82df32f26c4e5dc6a2f0.tar.gz`
(SHA-256 `6dcc2858d6c45e1d33769946472702782d070138826f82df32f26c4e5dc6a2f0`). Restore by hash, link the pinned FFmpeg tree at
`third_party/ffmpeg`, source `scripts/setup-ffmpeg.sh`, and use the release build
command in `s0-build.json`. `s0-overlay.patch` and `s0-overlay.json` identify the
counted test-only differences and shared file hashes. The selected-device witness
has a runtime red with its metadata omitted, followed by a restored green.

The pre-startup S4c runner archive is
`target/review/pf/s4/archives/421745d3d7d983eb9b9fef14abcde62b035c70fbcb43bfee971f5cedc746c29f.tar`,
SHA-256 `421745d3d7d983eb9b9fef14abcde62b035c70fbcb43bfee971f5cedc746c29f`.
`round-3/priority-runner-archive.json` records its seven members. The launch
supervisor starts at nice 0 so its owned native command can start at nice 10;
after spawning it, the lane sampler lowers itself to nice 19. Load records retain
sampler and observed process priorities. Python checks run at nice 19. The
priority bootstrap witness additionally runs from nice 0 to exercise this
transition; removing it is a runtime red. It does not measure GPU performance.

Acceptance runs import the frozen Python modules under
`target/review/pf/s4/s4c/round-3/frozen-repo`. The pins bind those actual modules,
the actual wrappers, source archives, production snapshots, approval records,
release binaries, prepared inputs, host, software adapter and calibration before
launch. `frozen-series.json` indexes the three plans. The G4 reference rebuild uses
the accepted S3b shipping state with the common current test overlay; the candidate
has the same shipping implementation, so these pairs establish no speedup.
