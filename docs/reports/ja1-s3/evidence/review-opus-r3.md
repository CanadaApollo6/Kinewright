**Score: 8/10. Approve, conditional.** I found no data-loss or security Major or Blocker in the round-3 snapshot. All 49 SHA-256s match `.review/c0-review-r3-source.json` (`all_guards_present:true`), and I read no live changed source. You still need to prove the restored live files match those 49 hashes before you use this approval.

**Guards confirmed in the snapshot**
- **Probe guard.** `CertifiedInput::open` (`owned_io.rs:715`) refuses zero-stream and NOHEADER tables (`:727`) and non-A/V streams (`:746`) before any stream-info probe.
  - `find_stream_info` sets `max_streams` to the reserved count before `avformat_find_stream_info` (`:1098`). In pinned FFmpeg, `avformat_new_stream` then refuses any new stream (`options.c:250`), so it fails closed.
  - Every probe and open path goes through this function (`:224`, `:438`, `:469`).
- **Incident colour.** Both senders return before `DoIfRevision` (`app.rs:2580`, `:2926`). The incident stays open and nothing is queued.
  - Both witnesses check unchanged history, then save, then a durable reopen equal to the saved history.
  - In legacy projects the colour fix still applies, and the two legacy controls cover that.
- **Concat counterfactual.** It is valid. With `file` allowed, concat opens the WAV without calling our callback and 96,000 PCM bytes come through.
- **Runners.** The media runner checks the exact panic thread, file and assertion line, and refuses unwrap panics. The incident runner checks the assertion line computed on the mutated source. Every runner uses `flock -o`.
- **Opus r2 C5 is withdrawn.** You were right: `host_request` (`mod.rs:487`) has no root check, and `mcp_request` (`:565`) does. The test's bootstrap root is the project directory, so the new external source really sits outside it.

**Findings**
- **C1 [should; evidence, not a code hole] `owned_io.rs:2232`.** The `hls-format-policy` mutation is predicted to survive.
  - The production label `file:certified-owned-input` (`:1208`) makes `hls_probe` return 0. The baseline logs show "Not detecting m3u8/hls with non standard extension", so removing the format whitelist still cannot make HLS call the parent callback.
  - This case has never run; it is absent from the r1 partial results. The HLS halves of the two loop witnesses are also refused by detection, not by policy.
  - Fix: give the format-isolation arm a test-only `.m3u8` label, or withdraw the case with a TSV row. Don't credit it either way. Concat already proves the format whitelist.
- **C2 [should; C1 first commit, no data risk] `owned_io.rs:727/746`.** The new refusals turn away common camera files in Studio projects, with the misleading error `BundleError::Corrupt`.
  - All MPEG-TS is refused, including AVCHD `.MTS`/`.M2TS`: pinned `mpegts.c:3171` always sets NOHEADER.
  - Files with data tracks are refused too, such as timecode, GoPro telemetry and iPhone metadata tracks. That covers much of Riel's on-location talking-head footage.
  - The refusal happens before install, so the document and manifest are preserved.
  - Fix: admit data and attachment streams that have no probe decoder, keep refusing subtitles, and consider MPEG-TS now that the stream cap exists. Report the refusal as `Unsupported`. Log the decision now.
- **C3 [nit] `owned_io.rs:2166`.** The stream-limit witness checks the cap after `open` returns. A mutation that sets the cap after probing would survive. This is acceptable because the zero-stream and NOHEADER refusal is the primary guard.

**Evidence status**
- **Done:**
  - Incident pairs, 2 of 2, on frozen `app.rs` `cd96878b`. The reds land at 11262 and 11296, the frozen assertion lines shifted by the 3 removed lines.
  - Clippy passed at 08:50Z, with no edits before the 08:57Z freeze.
- **On an earlier hash:** the protocol pair, the 27 media tests and the production MPEG-PS control ran on `owned_io.rs` `5e9dd06d`, not the frozen `d091ef51` (you say it was a doc-only edit). They need the final rerun.
- **Pending:** the 26-case media campaign (C1 will stop it), project 24, host 4, the reply-loss rerun and the final gates.
- **NOT RUN:** native Windows, CI and full suites.

**Trail audit** (`decisions.tsv` rows 20–32, against thread positions 567–646)
- Row 23 correctly marks row 11 and the host half of row 19 as historical.
- Rows 20–22, 25, 27, 29, 31 and 32 are supported; row 32 is backed by positions 641 and 644.
- **Row 26:** it says the rerun is pending, but no run of `s3_owned_import_lost_reply_refreshes_the_gui_lease_once` appears in this window. The claim at position 638 that this test passes has no support here. It needs a row and a log.
- **Rows 28 and 30:** the cited evidence is from `owned_io.rs` `5e9dd06d`, which predates the edit at position 609. Neither row says so.
- **Missing row:** the admission-scope cost in C2.
- **Minor:** no row records the Clippy pass at position 623.
