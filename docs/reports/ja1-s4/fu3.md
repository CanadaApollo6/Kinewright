# JA-1 S4 worker A follow-up 3

Parent `8e0ade02f130f82f3ee5dc97e2e29a4e044d7811`, branch `t3/ja1-s4-c1`,
worktree `/home/riels/.t3/worktrees/Kinewright/t3-f4cb0356`.
One commit is authorized. No push or worker-run review round is authorized.

## Result

Follow-up C has no measurable startup regression against the S2 merge. Original
S4 has a small, repeatable startup cost increase in these debug builds. Reusing the
already verified, immutable bootstrap snapshot restores startup below `main`.
The production five-second limit and the 12,000-edit fixture remain unchanged.

The lead supplied the same timeout on S3 CI run `40509206480`, which contains no
S4 code. This rules out S4 as the sole cause of the CI failure. These local runs
measure the cost and validate the change. They do not reproduce or certify the
hosted runner's five-second failure.

## Startup measurements

Each entry is five successful runs. Times are seconds. Bootstrap is the monotonic
wait that the production five-second limit bounds, after starting the pipe reader
and before accepting discovery. Attachment includes the complete GUI handover.
Launch serialization is recorded separately in the raw evidence.

| Source | Series | Bootstrap median | Bootstrap range | Attachment median |
| --- | --- | ---: | --- | ---: |
| `7fa471fcd2ee0455142f7ee41893ec4ccaf646df`, S2 merge | Historical triplets | 1.591240 | 1.555757 to 1.611457 | 2.965411 |
| `4250d146eb767b64a058bcbafb5f9adbccbb5264`, follow-up C | Historical triplets | 1.592807 | 1.557736 to 1.634254 | 2.964640 |
| `8e0ade02f130f82f3ee5dc97e2e29a4e044d7811`, original S4 | Historical triplets | 1.682576 | 1.651766 to 1.707461 | 3.094327 |
| `0856b2efa8c506afaa83daa0bdd3fe53a1412419`, pinned main | Final paired comparison | 1.588325 | 1.560920 to 1.592023 | 2.961790 |
| Final source, verified snapshot reuse | Final paired comparison | 1.177151 | 1.177023 to 1.192426 | 2.589679 |

C differs from the S2 merge by 1.567 ms at the median, well inside the observed
spread. Original S4 is 89.769 ms, or 5.6%, slower than C in the same alternating
triplets. The final source is 25.9% faster than its paired main baseline in the
startup window. Its complete attachment is also faster.

The first, unpinned main/S4 comparison measured medians of 1.600947 and 1.685641
seconds. A reversed-order comparison pinned to CPU 10 measured 1.566052 and
1.674227 seconds. The small S4 gap survived that control, so it was not dismissed
as flakiness. Historical triplets alternate their order between rounds. The final
comparison uses the same CPU 10 placement.

All builds use the Cargo test profile, reported as unoptimized with debug
information, with identical compiler and FFmpeg link flags. `4250d14` and
`0856b2e` have identical crate sources, manifests, and lockfile; both were built
explicitly. Historical builds have separate Cargo targets. Final gates use the
separate `target/fu3-final` target. The rustc version is
`1.99.1 (58cf8f9c3 2026-10-13)`.

The lost-save test does not exist at `7fa471f`. For both historical checkouts, a
temporary `fu3_floor_startup_probe` copies the exact fixture setup and launch
prefix, asserts that the 12,000 edits require a real Save floor, then closes and
reaps the host. It measures startup, without claiming the later lost-reply
behavior at the pre-C revision. The original test remains the harness for main,
original S4, and the final-source comparison. All snapshots contain 13,059,379
serialized bytes. Across the four series, 35 complete lost-save-floor witnesses
and ten historical startup probes passed, with zero runtime errors or timeouts.

The final host is measured with the preserved S4 GUI test binary. The GUI fixture
and parent-side launch implementation do not change in this commit. This keeps
the parent harness fixed while measuring the changed child startup path. The
full final-source agent and attachment gates below separately exercise the
shipping build.

### Shared machine load

The machine has 20 logical CPUs. No quiet window was requested or awaited.
Every series holds `/tmp/kinewright-cargo-heavy.lock` across its timing window.
Start and end records include `uptime`, instantaneous `top` output, the top five
CPU processes, and GPU state. A five-second sampler records processes above 20%
CPU. Other work remained active and was left alone.

| Series | One-minute load range | Five-minute load range | Fifteen-minute load range |
| --- | --- | --- | --- |
| Initial main/S4 | 2.56 to 4.93 | 3.09 to 3.64 | 3.36 to 3.54 |
| Reversed CPU-10 control | 2.53 to 4.19 | 3.15 to 3.65 | 3.22 to 3.36 |
| Historical triplets | 2.83 to 3.98 | 3.79 to 4.37 | 4.13 to 4.35 |
| Final main/HEAD | 3.32 to 7.29 | 4.50 to 5.61 | 4.27 to 4.52 |

GPU records were consistently P2, graphics clock 1935 MHz, memory clock 9501 MHz.
The measurements concern CPU startup work. No GPU settings or other projects'
processes changed. The contamination records include other Python and CLI work;
these are live shared-machine measurements, not isolated hardware acceptance.

### Cost and fix

`HostCommandOwner::open` verified the GUI snapshot's digest and capacity, retained
an immutable copy, proved retention, admitted the named owner, and then called
`install`, which performed the same digest and capacity scans again. Each scan
serializes the complete history for hashing and counts its encoding. Replay
validation follows installation. Stage timings place the bulk of startup in
these whole-history CPU passes, rather than in namespace admission or an H2
candidate publication.

The duplicate scan already exists at the S2 merge. It was not introduced by S4.
The measurements establish a small S4 binary-cost increase; they do not isolate
that increment to an individual S4 commit or prove why a hosted runner crossed
five seconds. The concrete removable cost is the repeated scan of identical
immutable bytes. Removing that work restores the measured cost below main.

A private `VerifiedHostSnapshot` records the successful digest and capacity
checks. Bootstrap verifies before cloning or proving retention, then passes that
value to `install_verified`. Every later, untrusted foreground snapshot still
passes through the checked owned constructor. The existing manual-stream guard,
retention and snapshot proofs, root and project checks, and Core replay checks
remain in place. `HostSnapshot::verify` itself is unchanged. There is no timeout
increase, unchecked boolean, wire-format change, or new public launch API.

The new witness rejects a bad bootstrap digest before the retention callback,
installs a valid nonempty history exactly, and rejects a later corrupt foreground
snapshot without replacing that history. Removing either constructor's check
independently compiles and fails this witness at runtime. Each restored source
passes. Compiler failures and launcher setup errors receive no witness credit.

If CI still finds this functional fixture borderline, the smallest separate
fixture-side follow-up is an explicitly passed, test-only bootstrap allowance
for this one 12,000-edit test. Keep the ordinary `HostClient::launch` allowance at
five seconds and keep all floor, frozen-history, reply-loss, and reopen assertions.
That follow-up is proposed here and is not applied. A production-limit change
still requires the lead's ruling.

## Windows durability gate

`bundle_commit::fu1_tests` is Linux-only again. The comment names G1-W.
The witness creates a durable bundle and publishes a receipt before checking that
receipt lookup precedes the unknown-branch refusal. `ProjectBundle::activate_checked`
and namespace acquisition refuse with `DurabilityUnsupported` on Windows.
There is no supported Windows bundle owner on which to exercise that ordering
without implementing G1-W. A fabricated owner would bypass the boundary being
tested, so no substitute branch-ordering test is added. The existing Windows
`ja_s1_storage::unsupported_profile_refuses_before_store_mutation` remains the
refusal witness. No durability refusal changes.

`fu2.md` records this witness as NOT RUN on Windows and corrects its earlier
claim that removing the Linux gate enabled Windows coverage.

## Verification

Every Cargo command sources `scripts/setup-ffmpeg.sh` in its shell, checks at
least 6 GiB available memory, and uses
`flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo`.
The full affected suites use four test threads. Every Python invocation uses
`nice -n 19 python3 -I`. The sealed `076a96c` predecessor recipe completed with
its own source and target directories inside this worktree's `target/`; its
Cargo and control invocations use the same shared lock. Its identity file has
SHA-256 `99541c816876d1e992ef91c07a5cf57c7a54ad63a3d11f9c8bba1f062b632bb2`.

| Gate | Result |
| --- | --- |
| `cargo build --workspace` | Exit 0. ``Finished `dev` profile [unoptimized + debuginfo] target(s) in 16.86s``. |
| `cargo clippy --workspace --all-targets -- -D warnings` | Exit 0. ``Finished `dev` profile [unoptimized + debuginfo] target(s) in 37.59s``. |
| `cargo test -p kinewright-agent --lib --tests --no-fail-fast -- --test-threads=4` | Exit 0. 965 passed, zero failed, 35 declared ignores, summed across the target summaries. |
| `cargo test -p kinewright-project --features test-util --no-fail-fast -- --test-threads=4` | Exit 0. 366 passed, zero failed, two declared ignores, summed across the target summaries. |
| `LOCALAPPDATA=<owned durable directory> cargo test -p kinewright-app ja1_attachment -- --test-threads=4` | Exit 0. `test result: ok. 71 passed; 0 failed; 1 ignored; 0 measured; 812 filtered out; finished in 166.73s`. |
| `rustfmt --edition 2024 --check --config skip_children=true` on both touched Rust files | Exit 0. Direct touched-file checking follows PITFALLS. |
| `nice -n 19 python3 -I scripts/slow_tests.py lint` | Exit 0. `slow-test manifest and markers agree: 89 tests, features kinewright-agent/slow-tests,kinewright-app/slow-tests,kinewright-media/slow-tests; 59 allowlist entries well-formed`. |
| `nice -n 19 python3 -I scripts/slow_tests_selftest.py` | Exit 0. `Ran 38 tests`, `OK`. |
| `bash docs/reports/ja1-s4/fu3-windows-check.sh` | Exit 0. Isolated actual project Rust source, all integration targets, and two pure examples, MSVC Clippy with `-D warnings`. Scope is described below. |
| Full MSVC project graph | Exit 101 in `aws-lc-sys`, including unsupported GNU/MSVC compiler setup and missing pthread definitions. BLOCKED before full crate validation. |
| Full MSVC agent graph | Exit 101 in `ffmpeg-sys-next`, `fatal error: 'errno.h' file not found`. BLOCKED before full crate validation. |
| Both validation guard reversions | Each compiled and produced `test result: FAILED. 0 passed; 1 failed`; each restoration produced `test result: ok. 1 passed; 0 failed`. |
| `git diff --check` | Exit 0. Rechecked after the final report update. |

For example, the full agent library reports `test result: ok. 724 passed; 0
failed; 2 ignored`, and the project library reports `test result: ok. 140 passed;
0 failed; 2 ignored`. The totals above sum the separate raw summaries; they are
not presented as a fabricated single Cargo summary.

## Evidence and setup corrections

The final shipping `writer_host/mod.rs` SHA-256 is
`87cd473edae2283ec27b8cf365e3478ca28ac79fb85444f8efffe21dd6cb558a`.
All temporary timing probes are removed from the shipping source.

The initial timing launcher attempted unavailable `/usr/bin/time` and failed
before starting a fixture. The runner then used Bash's built-in elapsed/user/system
counters. The failed setup is excluded from every timing and success count.
An initial gate attempt reused main's Cargo target and picked up a stale Core
artifact. That failed compile is excluded. Fresh final-source build and Clippy
use the separate target and pass. The timing build logs separately show Core
compiled from each intended checkout before its binaries were preserved.

The initial attachment group had 70 passes, one failure, and one declared
ignore. The unsaved Studio activation fixture used the default recovery directory
under `/tmp` because `LOCALAPPDATA` was unset. This machine's `/tmp` is tmpfs;
`namespace::linux::create_session` correctly refuses that filesystem through
`check_profile`, before a host is launched. The assertion then unwrapped the
absent attachment. Setting `LOCALAPPDATA` to this worktree's private btrfs-backed
`target/fu3-localappdata`, with its recovery directory created, makes the focused
witness pass. The entire attachment group passes with that same configuration.
No source assertion or durability refusal is changed for this environment issue.
The original failed attempt remains in the evidence.

The isolated Windows script follows the C2 source-check technique. It compiles
the actual project library, unit tests, all integration targets, and two pure
examples against the actual pure media modules and fixture-directory helper.
It excludes the FFmpeg-dependent `m4_verify` and `performance_workloads` examples.
An initial attempt included those native examples and failed because the pure
media closure does not supply `FfmpegMediaEngine`. That setup attempt is excluded;
the corrected source check passes. It is not a complete native Windows build.

Raw build, timing, load, source/binary identity, guard mutation, prerequisite, and
gate evidence is retained in [fu3-evidence.log.gz](fu3-evidence.log.gz).

The temporary main worktree and the nested predecessor checkout were removed
after verification. The worktree target directory and owned fixture artifacts
were removed after evidence was archived. The shared Cargo lock was retained.

## NOT RUN

- `bundle_commit::fu1_tests::fu1_unknown_branch_refuses_after_receipt_lookup`
  on Windows. G1-W does not support the durable owner and receipt publication
  required by the witness. The exclusion is intentional and is also recorded in
  `fu2.md`.
- Complete native Windows builds, Windows runtime tests, and the two
  FFmpeg-dependent Windows examples excluded from the isolated check. This is a
  Linux workstation; the full cross-check attempts stop in native dependencies.
  The passing isolated project source check is not promoted to native acceptance.
- The 38 existing declared ignores in the affected suites, comprising 35 agent, two project, and one App ignore. Agent reasons include
  slow-feature tests, manual release performance, known media-engine teardown
  SIGSEGV, unprepared speech/media packs, App/GUI bridge prerequisites, live
  subscriptions, and absent H1/H3 integrations. Project reasons are the
  historical uncooperative journal fixture and manual old-reader corpus. The
  selected App group has an existing slow lifecycle stress exclusion. Each
  exact test name and reason is retained in the raw evidence and is not counted
  as a pass.
- Hosted CI and the hosted five-second failure itself. No push is authorized;
  all local measured startup runs succeed, including the original revisions.
  Cost restoration is locally verified; hosted CI closure remains for the lead.
- Full workspace test runs and slow-feature suites. SPEED-RULES leaves those to
  CI; the full local library and integration targets of both touched crates and
  the required App attachment group run here.
- Worker-run reviews. The brief reserves review rounds to the lead.
