# TE1 safety

Fixes 1 and 2 certify frozen run inputs outside the owner mutex for StartRun,
Revise cut, and native recovery. The final locked bind validates the run scope
and rechecks the project, document hash and reference, manifest, owner epoch, and
certified descriptor metadata. A fork can have a new head while retaining the
exact same certified input data. Changed descriptor metadata refuses execution. Decoder
cursors still verify every used chunk.

Shared-owner direct callers use `execute_attached_shared`, so certification runs
without their mutex guard. The unshared direct API remains available. It refuses
new run preparation when called through a registered shared owner. Revise replays
do not certify footage again.

Task calls check live authority while waiting for the initial owner lock and
the final lock after a media read. Reply metadata, refusal accounting, and size
accounting do not wait behind a held owner lock after Stop. Edit-consent waits
retain their existing 350 ms ceiling and also check stopped authority.

## Measurements

The same generated RF64 file contains 6,528,000,114 bytes. Each measured path
clears the media admission memo first, so these are cold certificate
measurements with OS-cached footage after import. A sampler tries the owner
mutex every 100 microseconds. The numbers are the longest observed blocked
intervals, including scheduling delay,
not exact mutex instrumentation or an isolated-machine speedup claim.

| Path | Before | After |
| --- | ---: | ---: |
| StartRun owner hold | 10.917966 s | 0.364930 s |
| restore_native_run owner hold | 9.755663 s | 0.005151 s |
| Complete media reply | 11.085252 s | 7.627047 s |

The baseline media call spent 10.637631 s certifying the source, 0.015960 s
opening the audio decoder, 0.000055 s seeking and reading the frame, 0.002435 s
assembling the sample and page, 0.004888 s authorizing and preparing the read,
0.004226 s reacquiring and authorizing the final lock, 0.392863 s publishing the
page, and 0.010456 s refreshing reply metadata. The remaining 0.016738 s includes
polling, role lookup, serialization, and transport. The reported historical 44 s
was not reproduced on this fixture at the pinned base.

The shared host had unrelated CPU load during both runs. Baseline binary SHA-256
was `cccac6c4f035832329ec492dfe849ddba401c4525563c77e15596136e76283b8`.
The measured fixed binary was
`7c195f687eacd6475474b3f65ff040d2290290eb79ddb615d91fe082bd56740f`.
Logs are under `target/te1-safety/`, in `baseline.log` and `after-final.log`.
The generated RF64 file was deleted after the final measurement.

## Fix 3: media only

The lead selected media only. Native media tools now clone the certified source
retained by StartRun or restore. They do not open a fresh input lease or cold
certify footage, even after the store's admission memo has been evicted. Child
scopes share the admitted base inputs; native import/relink remains unavailable.

Each task retains one current media job, keyed by endpoint, exact read request,
revision, run/base binding, cursor, and switch generation. Queued, running, and
ready are one state enum. The existing endpoint permit bounds active work. A
supervisor continues checking authority, attachment, owner, and switch liveness
after the HTTP caller's existing 30-second wait ends. A timeout returns the
existing retry words. An unfinished retry returns those words without starting
another decoder; a finished retry receives the retained result. A distinct read
can replace a finished job. Nothing is persisted by the background worker.

Stop, pause, retirement, attachment loss, and owner/switch loss cancel decoding.
Only `finish_media_context` publishes the result after reacquiring the owner
mutex, resolving live authority, and checking the revision and binding again.
Mutating tools have no background-result contract or added deadline. No tools,
arguments, result fields, or retry phrases were added.

The HTTP witness uses the real 30-second server wait with an outer 45-second
client deadline. It returned retry words in 30.414991 seconds, completed the
same read in the background, returned its sample on retry, and recorded zero
cold certifications and zero retry decodes. A second delayed read was stopped
after its timeout; its worker ended and published no page. This is a generated
media/localhost HTTP witness, not a live Claude CLI run. Its current-source
focused run passed in 44.12 s including setup and Stop checks; the longest
measured first response was 30.414991 s. The full-suite result is recorded below.

### Admission memo follow-up

The admission memo now retains the metadata stamp captured by the original
complete verification. Reissue compares the newly opened source's captured
stamp with that original stamp before accepting it. A mismatch refuses reuse;
it never becomes a fresh baseline. This also checks the stamp captured by the
constructor, closing the gap between an earlier metadata check and construction.
The mismatch occurs during preparation outside the shared owner mutex.

The regression keeps a verified memo, overwrites its owned object in place at
the same length, then attempts StartRun. Preparation refuses and no run projection
exists. The fixture reopens its store after import, so the regression explicitly
warms the original-verification memo and proves a warm reissue before corrupting
the file. No mutation pair was added.

The Windows length-plus-mtime limitation from Astra F2 is unchanged, as directed.
No paid run, provider launch, or credential read was performed.

## Non-media growth inventory

This inventory covers the admitted native task tools and the allowed compact task
adapter. It reports existing work without changing it. Native run admission
and persisted task histories use a 1 MiB budget; the live Core also has its
separate 32 MiB/65,536-batch ceiling. Ordinary native control records are capped
at 8 MiB encoded and 32 MiB decoded, runs have eight tasks, and the pinned RMCP
3.1.2 default POST limit is 4 MiB. Result lists admit 256 artifacts and 256
evidence references. The store's provisional owned-byte ceiling is 20 GiB.
These component ceilings need not be jointly attainable in one request.

One warmed debug-test calibration measured 4,915,210 JSON bytes at 0.133825 s
serialize, 0.002229 s SHA-256, 0.031706 s parse, and 0.012150 s clone. Two valid
document layouts filled the native charged-history budget: 59 independent
clips at 1,041,197 charged bytes took 0.004167 s for overview materialization;
47 clips with reverse sync relationships at 1,040,343 bytes took 0.027263 s.
Empty-journal private Core restore took 0.001725/0.002927 s, respectively.
The optional `te1_media_only_profile_non_media_steps` fixture passed and records
this inventory separately from the HTTP regression in
`target/te1-media-only/non-media-profile.log`.

Validating 128 legal marker-label operations against those documents took
0.593950/0.506804 s. Each encoded operation was 65 bytes; allowing 1,024 bytes
for the RPC envelope leaves 63,534 such operations under the 4 MiB POST cap.
Linear extrapolation gives **294.8/251.6 s for policy validation alone**.
The large request was not executed. These are operation-count estimates at the
measured document shapes, not latency bounds for arbitrary operations or maximum
Core documents. They identify a remaining path that can exceed Claude's limit;
it is deliberately unchanged under the media-only ruling.

Estimates below are CPU work estimates from one run, not RPC latency bounds.
Cold record I/O, filesystem synchronization, lock contention, shape changes,
and scheduler delays are additional. No blanket 45-second guarantee follows.

| Tool/step | Growth and maximum-size estimate |
| --- | --- |
| All tools, role refresh and reply controls; `tools/list` | Full run projection reads/copies and metadata issuance grow with control history. At 8 MiB, one serialize/hash/parse/clone cycle is estimated at 0.307 s; repeated cycles and synchronous publication add to it. The earlier small-fixture publication stage measured 0.393 s. Owner-lock waiting has no wall-clock bound. |
| `task_status`, `wait_task` | Observation builds a run view, accounting and completion status. At the 8 MiB record cap, allow repeated 0.307 s CPU cycles plus scans of up to eight tasks, 24 attempts, 128 sends and 512 usage samples. Control-state I/O is not bounded by the byte calculation. |
| `get_timeline_state` | Snapshot hashing, dependency closure, row construction and whole-row paging happen before the small reply is trimmed. Measured 0.004/0.027 s in the two 1 MiB charged-history layouts above. The separate 32 MiB live-history ceiling permits larger documents; a 32 MiB byte cycle is estimated at 1.228 s. Relation discovery has nested clip/group scans and repeated closure passes, so the small-layout measurements do not bound maximum topology cost. |
| Non-media `read_task_evidence`: timeline, transcript, reference, proof and preview | Same snapshot hash and possibly dependency expansion. Owned-evidence and other supported non-RunIndex references have a 192 KiB preflight limit. RunIndex can reach its control-record cap before parsing/fragmenting. A 1 MiB serialize/hash/parse/clone cycle is estimated at 0.038 s, and a 32 MiB cycle at 1.228 s. Transcript word arrays and classified JSON are materialized before paging. |
| Note catalog, proof lookup and wake/overview construction | `context_notes` reads every feedback revision, up to 2,000 comments; overview invokes it twice. This can mean 4,000 record reads. Assuming 1 ms per cold record read gives a 4 s I/O estimate, separate from JSON work. Reading all 2,000 maximum 16 KiB texts is 31.25 MiB, roughly 1.199 s per JSON cycle. These I/O assumptions are estimates, not measurements of the 2,000-note case. |
| `read_task_result` | The legacy route can load a RunIndex and serialize it before enforcing the 20 KiB response limit. At the 8 MiB control cap, approximately 0.307 s CPU per full record cycle, plus authentication and storage. |
| `read_child_result`, including free replays | It rebuilds artifact/evidence/explanation/delta rows, usage and saved history before returning one page. 514 records at 192 KiB total 96.375 MiB; a JSON cycle over that payload is estimated at 3.699 s, with additional copies/fragmenting and I/O. The earlier maximum recorded-page replay measured 4.22 s under the owner mutex. This path is unchanged. |
| `prepare_edit_plan` | Decoding and policy validation scale with operation count and document size; policy clones/compares intermediate documents. There is no semantic operation-count cap in `decode_plan_operations` or the argument type. The 4 MiB transport cap permits large legal repeated-operation arrays. A near-cap 63,534-operation request is estimated at 294.8/251.6 s of policy validation at the measured document shapes, before plan application and storage. This is not a hard maximum for all operation shapes. |
| `commit_edit_plan` | Copies the stored plan, rechecks consent and policy, applies the operations, and records approval metadata. It repeats history/document work and may verify source-availability metadata. It has no media-worker deadline. Large-plan validation costs from the preceding row apply again. |
| Compact task adapter `invoke_capability` → `plan_motion` | The adapter also allows this evidence-only planner. It scans the snapshot, clones/applies a scratch document, then runs task policy validation for at most seven emitted operations (one add, three clears, three writes). Seven operations scaled from the marker-policy calibration estimate 0.032 s at the 1 MiB charged-history shape; the operation types differ. At the 32 MiB live-history ceiling, one document clone is estimated at 0.083 s and a full byte cycle at 1.228 s, with validation passes additional. Other planners are refused by this adapter; capability search/schema lookup has no footage/history-sized payload beyond common role/reply work. |
| `discard_edit_plan` | Frees an operation vector up to the request-size scale and maintains a 64-plan index. A 4 MiB payload clone/free is estimated at about 0.010 s per clone-sized pass; allocator work varies. It does not read footage. |
| `undo_task_edit`, `redo_task_edit` | Export history, restore a private Core, validate the selected gesture, perform the real transition, and compare/export again. The private restore replays batches; the Core ceiling is 65,536 batches and 32 MiB, while persisted native history is limited to 1 MiB. A 32 MiB byte cycle is estimated at 1.228 s; replay and gesture-policy validation add an operation-count term. Empty-journal restore measured 0.0017/0.0029 s. Policy validation of one selected gesture at the near-4 MiB marker-operation scale has the same roughly 295 s estimate as plan validation; smaller persisted histories and the charged admission limit reduce that space. Core replay traverses all retained batches and has different costs from the measured policy loop, so that estimate is not an end-to-end undo/redo bound. |
| `assign_task` | Branch checkpoint selection, copied/restored history, document/scope validation and durable actor preparation grow with history. The persisted native 1 MiB limit gives about 0.038 s per byte cycle, plus replay and synchronous storage. Provider preflight adds latency independently of footage/history and is not changed here. |
| `freeze_task`, freeze portions of `finish_task` | Capture/validate/serialize history, build the delta and write its checkpoint. At 1 MiB, byte work is about 0.038 s per cycle; a live 32 MiB history can incur about 1.228 s per cycle before refusal/flooring, plus replay/delta traversal and fsync. |
| `publish_result`, publication portions of `finish_task` | Validate bounded result lists, serialize/hash control records, check dependency closure and write records. At 8 MiB, about 0.307 s per byte cycle; up to 512 result references add object-count work. This is synchronous publication, not background media work. |
| `merge_task` and merge reconciliation | Load frozen/base history, derive/revalidate the delta, check receipt identity, replay operations into the candidate, and durably commit. Linear history estimates above apply, plus the plan/gesture operation-count term and I/O. A replay still reads its saved receipt/history. |
| `observe_delivery`, `cancel_task`, `answer_question`, `send_message` and their replies | Update and republish cumulative projection/accounting/outbox state, which grows to the same 8 MiB control cap. Allow repeated 0.307 s CPU cycles, history/reference scans and storage. A message body or referenced record may add a bounded record read. Cancellation additionally closes/drains actor state. |
| Non-media overhead around media reads | Initial role/owner acquisition, snapshot capture, manifest/clip lookup, final document serialization/hash and evidence bookkeeping remain outside the worker's wait guarantee. Final expansion checks scan up to 256 task decisions twice and may read the entire run projection. These use the same document/control estimates above. |

The table's 8 MiB control-record figures describe ordinary native publications.
Pre-existing typed records and terminal paths can use the store's 64 MiB ceiling;
one byte cycle at that ceiling is estimated at 2.456 s. A media tool's manifest
read validates up to 10,000 entries and loads/checks their chunk-table metadata;
it does not read the footage again. The retained input-lease search is linear
in the entry count. These metadata costs are outside the decoder wait.

Record admission, pinning, and root publication also walk the retained dependency
graph. The store ceiling is 10,000 objects and 512 MiB of charged metadata, with
160 charged bytes per edge. Ignoring other charges gives a loose ceiling of
3.36 million edges; assuming 1 microsecond per edge gives a 3.36 s traversal
estimate, plus object allocation and repeated walks. This is an explicit cost
assumption, not a measured maximum graph. Synchronous object writes and directory
barriers add storage latency with no finite wall-clock bound. Ordinary native
admission reserves 256 MiB and 256 objects below those store ceilings.

Ordinary native tools no longer cold-certify footage. The current UI builds
empty `asset_roots`/`consent_refs`; child scopes cannot broaden them and native
import/relink is refused. The person-facing StartRun/Continue validation path
can still call raw `store.verify` on explicit plan/script/assignment/scope
references. For a custom host brief containing large blobs, this is linear
in referenced bytes and can rescan footage; extrapolating the earlier 6.528 GB
cold verification gives roughly 35 s for 20 GiB per pass. This is a host-control
path, not a native model tool, and is reported without changing it.

## Verification for fixes 1 and 2

Removing the source recheck under the lock fails the source-change witness at
`TE1_SAFETY_RECHECK: changed input started a run`. Restoring it passes. The Stop
witnesses cover the initial and final owner-lock waits, hold the owner mutex
through Stop and the refusal, and check that no evidence decision is added.

Media passed with 1,267 tests and 32 ignored across unit, integration, and doctests.
Its only later edit moved a Unix import before a statement for Clippy.
Project passed with 190 tests and 2 ignored after building the required sealed
predecessor. Agent passed with 1,680 tests and 92 ignored across the full crate
suite, with `slow-tests` enabled and `RUST_TEST_THREADS=1`.

The earlier four-thread Agent library run reported 1,293 passed, 1 failed,
and 56 ignored. Its existing 45-second counting-child timeout failed under
concurrent fixture load; the same test passed alone in 29.45 seconds and then
passed in the full single-thread run. Its timeout and assertions are unchanged.
The additional `te1-eval` Stop checks passed both tests, including optional
observation while the owner mutex remains held after Stop.

All logs below were read after their commands ended. Cargo used the required
FFmpeg environment, heavy-job lock, four build jobs, and niceness 10. Media and
the final Agent suite used one test thread; Project used four.

| Gate | Result | Log under `target/te1-safety/` |
| --- | --- | --- |
| `cargo fmt -- --check` | PASS | `workspace-fmt.log` |
| Workspace Clippy, all targets, `-D warnings` | PASS | `clippy.log` |
| Supplied Windows Clippy script | PASS: workspace and Agent `slow-tests`; compile/lint only | `windows-clippy.log` |
| Slow-test lint | PASS: 121 tests, 144 allowlist entries, 27 skip sites | `slow-lint.log` |
| Slow-test lint self-tests | PASS: 44 tests; existing unclosed-file ResourceWarning | `slow-selftest.log` |
| Media full crate suite, `slow-tests` | PASS: 1,267 passed, 32 ignored | `media-tests.log` |
| Project full crate suite | PASS: 190 passed, 2 ignored | `project-tests.log` |
| Agent full crate suite, `slow-tests` | PASS: 1,680 passed, 92 ignored | `agent-tests.log` |
| Stop checks with `te1-eval` | PASS: 2 passed | `eval-stop-witness.log` |
| Locked source-recheck mutation | Intended red, restored green | `mutation-red.log`, `mutation-green.log`, `mutation-summary.log` |
| Final same-fixture timing | PASS: 1 passed | `after-final.log` |
| Generated-footage cleanup | PASS: RF64 source deleted | `cleanup.log` |

The maximal fixture is excluded as requested.
The free recorded-page replay path and its earlier 4.22 s measurement are left
unchanged. The existing scale witness now calls the prepared recovery boundary.

## Files touched by fixes 1 and 2 (`ec0673ff`)

- `ci/ignored-tests.txt`.
- `crates/kinewright-agent/src/orchestration/effect_pump.rs`.
- `crates/kinewright-agent/src/orchestration/tasks.rs`.
- `crates/kinewright-agent/src/writer_host/auth.rs`.
- `crates/kinewright-agent/src/writer_host/completion_fixture.rs`.
- `crates/kinewright-agent/src/writer_host/continuation.rs`.
- `crates/kinewright-agent/src/writer_host/hooks_tests.rs`.
- `crates/kinewright-agent/src/writer_host/ja_s3_host.rs`.
- `crates/kinewright-agent/src/writer_host/mod.rs`.
- `crates/kinewright-agent/src/writer_host/orchestration.rs`.
- `crates/kinewright-agent/src/writer_host/provider_readiness.rs`.
- `crates/kinewright-agent/src/writer_host/run_inputs.rs`.
- `crates/kinewright-agent/src/writer_host/task_consent.rs`.
- `crates/kinewright-agent/src/writer_host/task_mcp.rs`.
- `crates/kinewright-agent/src/writer_host/task_mcp/reply.rs`.
- `crates/kinewright-agent/src/writer_host/te14_fix2_tests.rs`.
- `crates/kinewright-agent/src/writer_host/te1_safety_tests.rs`.
- `crates/kinewright-media/src/owned_io.rs`.
- `crates/kinewright-media/src/owned_io_memo.rs`.
- `crates/kinewright-project/src/owned_inputs.rs`.
- `crates/kinewright-project/src/owned_media.rs`.
- `crates/kinewright-project/src/studio_store.rs`.
- `docs/reports/te1-safety.md`.

## Files touched by media only and memo stamps

- `ci/ignored-tests.txt`, `ci/slow-tests.txt`.
- `crates/kinewright-agent/src/writer_host/continuation.rs`.
- `crates/kinewright-agent/src/writer_host/hooks_tests.rs`.
- `crates/kinewright-agent/src/writer_host/orchestration.rs`.
- `crates/kinewright-agent/src/writer_host/task_context.rs`.
- `crates/kinewright-agent/src/writer_host/te14_fix1_tests.rs`.
- `crates/kinewright-agent/src/writer_host/te14_fix2_tests.rs`.
- `crates/kinewright-agent/src/writer_host/te15_a_tests.rs`.
- `crates/kinewright-agent/src/writer_host/te1_media_only_tests.rs`.
- `crates/kinewright-media/src/owned_io_memo.rs`.
- `docs/reports/te1-safety.md`.

## Verification for media only and memo stamps

The first full Agent library candidate ended with 1,294 passed, two failed,
57 ignored and the maximal fixture filtered out. One failure caught an error
mapping regression: cached decoder failures exposed internal error text. The
cache now preserves the established generic worker-failure response; its focused
regression passed. The other failure was the existing counting-child test
`te15_b21_grant_cannot_cover_foreign_run_tracks_or_missing_classes` reaching its
unchanged 45-second ceiling. It passed alone in 40.67 s. Its timeout and assertions
were not changed. The fresh full crate run after the error-mapping correction passed, including
that unchanged counting-child test and both new regressions.

All completed results below were read from ended command logs under
`target/te1-media-only/`. Cargo used the FFmpeg setup, shared heavy-job lock,
four build jobs, niceness 10, and one test thread.

| Gate | Result | Log |
| --- | --- | --- |
| Media full crate suite, `slow-tests` | PASS: 1,272 passed, 32 ignored, including integration tests and doctests | `media-tests.log` |
| Current-source HTTP wait/retry/Stop witness | PASS: first response 30.414991 s; zero cold certifications, retry decodes and stopped publications | `media-witness-final.log` |
| Original-stamp memo regression | PASS: 1 passed; overwritten object refused before StartRun | `memo-regression.log` |
| Decoder-failure wording regression | PASS: 1 passed | `error-words-recheck.log` |
| Existing consent counting-child recheck | PASS: 1 passed in 40.67 s | `consent-recheck.log` |
| Non-media inventory calibration | PASS: 1 passed | `non-media-profile.log` |
| Supplied Windows Clippy script | PASS: workspace and Agent `slow-tests`; compile/lint only | `windows-clippy-final.log` |
| Agent full crate suite, `slow-tests` | PASS: 1,682 passed, 93 ignored, one requested maximal fixture filtered; includes integration tests and doctests | `agent-tests-final.log` |
| Final workspace Clippy, all targets, `-D warnings` | PASS | `clippy-after-suite.log` |
| Final workspace fmt check | PASS | `fmt-final.log` |
| Final slow-test lint | PASS: 122 tests, 145 allowlist entries, 27 skip sites | `slow-lint-final.log` |
| Generated-footage cleanup | PASS: private media directory empty; abandoned counting-child fixture removed | `cleanup-final.log` |

All build, test and gate processes started for this follow-up have exited.
No extra mutation pair, live provider, dry launch or paid run is part of this
follow-up. The maximal fixture remains excluded as directed. The Windows F2
metadata limitation and the non-media paths in the inventory are left unchanged.

Follow-up on `3e79b967`: unusable admission memos are evicted and fully recertified off the owner lock; unchanged bytes receive a fresh stamp-bound memo, while damaged bytes fail before StartRun.
Media recertification uses the existing background job and cancellation signal, so timeout retries reuse completed work and Stop cancels pending verification.
Failed reads are delivered once then evicted; cached successes recheck their actual source's original stamp before publication, using existing failure words and no source hashing under the owner lock.
Narrow verification passed: Project owned_media 12, media owned_io/memo 41, te1_media_only 5, te1_safety 8, media wait/Stop/pause 6, te15_a 9, and four adjacent tamper tests (85 total; logs under `target/te1-recert/`).
fmt, workspace Clippy, Windows Clippy and slow-test lint passed; no whole-crate suites or mutation pairs ran, `scripts/__pycache__/` was removed, and owner-lock deadlines remain deferred.
