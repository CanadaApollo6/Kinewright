**Score: 5/10. Verdict: request changes.** One security Blocker (C1) blocks completion under the brief's rule. I reviewed source matching the r1 snapshot: all 43 hashes in `.review/c0-review-r1-source.json` matched.

## Code findings

- **C1 [blocker]** `crates/kinewright-media/src/owned_io.rs:1087-1098`
  - **Scenario:** `Avio::open` sets `io_open = deny_open` but no `protocol_whitelist` or `format_whitelist`. Pinned FFmpeg n8.0's `concat` demuxer opens its nested input through the default `io_open`, not ours. The `file:` label then resolves a relative name like `uncertified.wav` against the process working directory.
  - **Evidence:** I compiled a throwaway C probe against `third_party/ffmpeg` with C0's exact settings. Its scratch directory is deleted; no Cargo was run. It showed `format=concat`, 96000 bytes decoded, `denied_calls=0`. Import admission accepts the same file, so mutable bytes that were never chunk-verified reach decoded output.
  - **Fix:** pass `protocol_whitelist="none"` (it reaches nested contexts through `ff_copy_whiteblacklists`; in my probe concat then refused and a normal WAV still decoded all 96000 bytes). Also set `format_whitelist` to the supported demuxers. Add ffconcat and HLS refusal witnesses with mutation pairs, and extend `media-ffmpeg8-avio-audit.md` to cover nested opens.
- **C2 [should]** `crates/kinewright-project/src/editing_session/owned_media.rs:226-233, 275-277`
  - **Scenario:** on the saved-bundle path, `BundleCommit::command` publishes and installs AddAsset, then `bundle.save` fails. `self.inputs` stays on the old manifest and the retained lease is dropped, so every later save fails `validate_document_inputs`.
  - **Fix:** advance `self.inputs` and push `input_leases` as soon as the command commits, before the save.
- **C3 [should]** `crates/kinewright-agent/src/writer_host/owned_media.rs:104-116`
  - **Scenario:** a GUI-origin import uses `/` as its read scope and checks neither the forwarded `roots` nor `config.roots`. A same-user bearer that holds the manual lease can import any readable file. Exploiting it depends on the S2-deferred "bearer = editor" rule.
  - **Fix:** check the forwarded roots, as `admit_person_destination` already does.
- **C4 [should]** `crates/kinewright-project/src/namespace/linux.rs:905`, used by `store.rs:1130`
  - **Scenario:** on `EEXIST`, a same-named object that isn't in the ledger is checked only for type and length, then registered as verified. The newly hashed copy is discarded. Chunk checks catch the bad bytes at use, but the good copy is already gone.
  - **Fix:** hash the existing object or refuse. Thread position 368 shows the implementer is already fixing this.
- **C5 [should]** `crates/kinewright-project/src/store.rs:2025-2032`
  - **Scenario:** `validate_store` runs a full FFmpeg probe plus one decoded frame for every media entry on every manifest read or admission, with no memo. Through `validate_manifest_extension`, each import costs about 4N probes. It also turns rescue's typed `RescueMissingOwnedBytes` into a generic `Corrupt` error.
  - **Fix:** memoise certification by full identity, or certify at lease time instead.
- **C6 [nit]** `crates/kinewright-media/src/owned_io.rs:261,333`: `expect("selected stream")` runs on decoder-controlled state. Return a typed error instead.
- **C7 [nit]** `crates/kinewright-media/Cargo.toml:49`: the local lint copy (`unsafe_code = "deny"`) can drift from the workspace lints. Add a comment linking the two.

**Checked and sound:** drop order, latched output, verified-only slots, short final chunk and EOF, the per-callback size check, `ReadOnlyInputs` identity checks, fixtures built through the production import path, foreground dedup of lost replies, and exact-superset lease rotation.

**Not run, and evidence gaps:**
- Mutations: NOT RUN.
- Native Windows: NOT RUN.
- No host-death test while an import or refresh is pending.
- No mutation pair against the production tamper test `s3_certified_input_through_use`.
- No secondary-open or whitelist mutation.
- I did not verify the thread's post-snapshot claim that default decoder reservations reject HD media. The 17 media tests use small fixtures.
- Full suites and inherited S0–S2 regressions are left to the root rerun.

## Decision-trail audit (`decisions.tsv` against thread `ja1-s3-a-c0-r2`)

- **T1, row 6, weakly supported:** `s3_unsaved_source_capacity_refuses_before_document_install` (`owned_media.rs:848-870`) asserts only `is_err()`. It never shows the second import was refused for capacity, so "exact-limit and one-beyond refusal" is not proven.
- **T2, missing row (relates to row 4):** the inert `file:` label and the pinned FFmpeg audit (thread positions 171 and 195) are security decisions with no row. Neither is the decision not to set protocol or format allowlists, which is where C1 comes from.
- **T3, missing row:** the media crate's unsafe-code lint was relaxed from `forbid` to `deny` (position 36).
- **T4, missing row:** the `TransientDecoder` reservation kind skips the per-record decoded cap (positions 229–230).
- **T5, after the trail:** two pieces of work have no rows: the S2 fixture moved to production admission (position 343), and the review fixes now in progress (position 368: duplicate-import digest, HD reservation, mutation setup failures). The source may move past the r1 snapshot.
- **Note:** all seven rows were logged together at 07:00:54–55 (position 318), not as decisions were made.
- **Supported:**
  - Row 2: position 0.
  - Row 3: position 273 and the logs; its evidence column cites a source file rather than a log.
  - Row 4: `media.log.gz` shows 17 passed.
  - Row 5: position 203 shows `decode.rs:182` always reports an unknown white point.
  - Row 7: positions 275 and 315.
  - Row 8: position 317.
