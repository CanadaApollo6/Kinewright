**Score: 5/10. Request changes.** Two blockers remain, plus a missing piece of required security evidence.

I reviewed only the snapshot copies. All 47 SHA-256s match `.review/c0-review-r2-source.json`. The `owned_io.rs` copy has every guard in place (`protocols = c"none"`, the format allowlist, `deny_open`). Root still has to prove the restored live files equal those 47 hashes.

**Findings**

- **C1 [blocker, security] `owned_io.rs:714` and `:1061`.**
  - Scenario: `open` reserves decoder memory and sets `max_pixels` only for streams that exist before `avformat_find_stream_info`. FFmpeg applies `options[i]` only to those original streams. The allowed `mpeg` demuxer (and `mpegts` when its PMT changes) creates new streams during probing. A crafted MPEG-PS can therefore carry up to 16 large H.264 streams (the `max_streams` cap) that get probe-decoded with no ledger charge and FFmpeg's default `max_pixels`.
  - Sol's round-2 review raised this first; I confirmed it in the source.
  - Fix: set `max_streams` to the reserved stream count just before probing, so new streams fail closed, or drop `mpeg`/`mpegts` from C0. Add an MPEG-PS witness and a mutation pair.
- **C2 [blocker, data at risk] `app.rs:2584`.**
  - Scenario: `send_operation` (`:2320`) routes `SetAssetColorDescription` through `ChangeMediaInterpretation`. `audit_new_incident` sends the automatic colour fix for unknown primaries, transfer or matrix straight to `DoIfRevision` instead. The document then holds a colour interpretation that has no manifest entry. Every later save refuses until the person undoes the automatic change. This is the Wonder r1 C2 scenario on a second path.
  - Fix: route the automatic colour fix through the same staging, or suppress it for owned media while the host is attached. Add an App witness.
- **C3 [blocker, missing security evidence] `owned_io.rs:2010`.**
  - Scenario: the nested-protocol witness proves nothing. With the protocol whitelist mutated to `file`, it still refuses, but on zero duration (`protocol-diagnostic.log`). The mutation campaign halted at that case, so six security cases never ran. SPEED RULES require mutation pairs for security witnesses.
  - Fix: give the concat fixture a valid duration, assert the nested-open refusal itself, then run the remaining cases.
- **C4 [should, C1 follow-up] `owned_io.rs:721`.**
  - Scenario: each decoder is charged twice, once at probe and again when it opens. A 3840×2160 stream needs about 7.1 GiB against the 4 GiB limit, so 4K imports refuse at default limits. With 1080p audio and video, a second clip is refused.
  - Fix: release the probe charge when the decoder opens, and add a 4K control.
- **C5 [nit] `ja1_host.rs:629`.** The app forwards the file's parent directory, but `writer_host/mod.rs:565` requires it to sit inside the bootstrap roots. Imports from an SD card or `/run/media` refuse with an unexplained 403.
- **C6 [nit] `editing_session/owned_media.rs:425`.** If `freeze()?` fails after the install succeeds, the function returns before the release path, and the staged borrowed source leaks.
- **C7 [nit, evidence]**
  - `c0-mutations.json` is not covered by the hash manifest.
  - The media runner's `assertion_red` counts any `owned_io.rs` panic that prints the marker. It does not check the assertion line or reject unwrap panics.

**Confirmed sound:**
- Full hashing of an existing object before discarding the incoming copy.
- Reads that check only the chunk-table binding.
- Matching selected-stream checks for video and audio.
- The inherited protocol and format allowlists.
- Retaining committed inputs after the import commit.
- The superset colour entry and undo on the `send_operation` path.
- The forwarded-root check.
- The project mutation driver's fidelity checks.
- Retrying a lost reply.

**Not run:** I ran no Cargo, no mutations, no native Windows, no CI and no full suites. The project and host mutation campaigns and six media cases have not run.

**Decision trail audit** (`decisions.tsv` against the source thread, positions 1–566)

- **Rows 2–9:** retrospective, and row 9 labels them as such. No flag.
- **Row 10:** "23 media tests passed" is supported (position 481). "Nested-open mutations pending" is now out of date: the surviving mutation and the halted campaign (positions 561–563) have no row.
- **Row 11:** "Workspace clippy passed" is not supported for the frozen source. Clippy last ran at 07:37:16Z, before the pin refactor (502), the `read_only_inputs.rs` split (512) and the test edits (527–535).
- **Row 12:** the HD claim is supported by the `project-review-fix-r1` log. The 4K refusal trade-off (C4) is not recorded.
- **Rows 13–15 and 17:** supported by the `fix-r1` log (all 9 owned-input witnesses passed) and `review-comments-r1.md`.
- **Row 16:** the static claim holds. No row records the decision to keep the mutation catalog outside the hash manifest.
- **Row 18:** supported. `project.log` at 07:42:25Z shows 13 + 9 + 10 = 32 passed, before the row was written at 07:42:58Z.
- **Row 19:** the App half is supported (`app.log` at 07:49:55Z, 4 passed, including host death). The host half is not: `host.log` (07:44:11Z) ran before `s2h_tests.rs:242` was edited at 07:46:15Z (position 527), and the host gate was not rerun.
- **Missing rows:** the unrouted automatic colour fix (C2) and the nested-protocol survivor.
