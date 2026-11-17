# MS1 review rework

Base: `e8f48e64ec19c4ae32b9a10b1d766f4eb21983b2`, branch `ms1`.
Scope: the lead's three rulings in `.review/arch/ms1-review-fixes.md`, the
validated still-image exception, and failed Windows run 44045226331.

1. All production readers now share stream-table admission and the sole
   `avformat_find_stream_info` call. Zero-stream and NOHEADER inputs stop before
   that parser-capable call. The validated single-image exception requires exact
   PNG/JPEG pipe identity, bounded matching header, one stream, fixed codec and
   allowlist membership. Probe `max_streams` prevents later table growth.
   MPEG-PS/TS are removed from shipping; testgen keeps them for negative fixtures.
2. The common CI action restores exact keys only for the canonical repository
   and excludes fork PRs. Only push producers save caches. Every native install
   emits binary SHA-256 observations. Release workflows use no binary cache;
   packagers require and enforce a fresh-source flag even with a matching marker.
   The single-owner cache trust risk is explicitly accepted in the design.
3. One builder fingerprint serves CI keys and all local markers. It includes
   actual tool binaries/full versions, dependency versions/flags, build flags,
   source/profile/configure vectors and setup/builder/action digests. The previous
   independent cache-platform key script is deleted. GCC's random linker-query
   scratch filename is normalized; actual linker version changes still invalidate.

The frozen JA S1 historical control is unchanged in its separate `target/` prefix.
The app, recorder and fixture tools retain their separate owned install paths.

## Windows diagnosis and boundary

The full failed job log (job 133909132238) contains six awk syntax diagnostics,
make depfile failures and zero MSVC C/C++/linker error diagnostics. The inherited
PATH previously allowed native Windows make/tools ahead of MSYS. Python already
used argv and `shell=False`; there was no Python `shell=True` or `bash -c` layer.
The fix uses the [action installation output](https://github.com/msys2/setup-msys2#msys2-location),
selects absolute MSYS make, passes `SHELL=/usr/bin/sh`, and puts MSVC
cl/link then MSYS utilities before inherited tools. A pre-build witness runs the
upstream awk backslash substitution through that same make/sh selection. No
FFmpeg source or makefile is patched.

The shared profile requires `libavfilter` for decode/export/verification graphs. Its
registered filters are audited against buffer, buffersink, scale, format and the
upstream audio buffer source/sink dependencies; ebur128 is absent from this profile.
Windows execution after this change is NOT RUN; it awaits the lead's push.
The first lead push runs the full MSVC source build and depfile witness in CI.

## Verification

Native rebuild/audits pass: 38 decoders, 14 demuxers, 16 parsers;
11,634,104 shared-library bytes (49,184 fewer than e8f48e64).
Recorder: 4,612,472 bytes; device-only audit and software ALSA recording pass.
The key before setup, after setup, and all three installed markers is
`d0d68c4f3550f4338e09f4edeb9cc96a54ba104ab7e06fb035378904c4aa491e`.
`binary-sha256-final.txt` records every installed native binary observed locally.
The final rebuild is byte-identical to the prior Linux outputs (all binary hashes,
including the six shared libraries), despite the Windows setup fingerprint change.
Ten Python witnesses, actionlint, Bash syntax, PowerShell AST parsing, and the
PowerShell environment idempotence witness pass. The final lighter gate passes (table below). Logs are under `target/ms1-rework/`.

The initial PNG control incorrectly expected its parser pointer to survive EOF;
upstream demux.c closes it there. The final oracle counts the sole native probe
entry and checks that rejected contexts have no parser before or after refusal.
Both guard-removal mutations failed with entry count 1 instead of 0; restoring
the guards passed. Each negative context also had zero native parsers.

The first full media pass found one recovery-message regression: 1203 passed,
1 failed, 65 ignored. Early still-header refusal had omitted the existing 8192px
limit/downscale recovery. The guard now includes both; the unchanged existing
boundary test passes for refusal above 8192 and admission at 8192. The failed log is
retained as `media-failed.log.gz`. Path-reader probe threading remains as before;
reserved certified probes retain their one-thread setting. The complete media crate was rerun and passed after this correction.

| Final Linux gate | Result |
| --- | --- |
| Named still-size boundary | 1 passed |
| MS1 focused codec/header witnesses | 9 passed |
| Dynamic-table and PNG-table guard removal | each fails at native-entry count 1 vs 0; each restored control passes |
| Touched-crate clippy, all targets, slow features compiled | passed with `-D warnings` |
| Media full fast targets | 1236 passed, 70 ignored, zero failures |
| Project full targets with test-util | 420 passed, 2 ignored, zero failures |
| Workspace fast tier | 4531 passed, 144 ignored, zero failures |
| kinewright-app build | passed |
| Touched-file rustfmt, slow manifest lint, actionlint, Bash/PowerShell syntax | passed |

The workspace skip audit observed exactly 83 current-OS slow tests and 61 allowed
ignores, with 34 other-OS manifest tests running and passing. Heavy commands ran
under the shared lock at nice 10, four Cargo jobs; media-alone tests ran serially.
Tests used a synthetic child HOME and software Vulkan. Hardware GPU/camera,
new hosted Windows execution, local slow tier, and full release packaging are
NOT RUN in this rework. `[slow-tier]` requests hosted slow-tier coverage on push.
No push was made. The historical control remains unchanged.
