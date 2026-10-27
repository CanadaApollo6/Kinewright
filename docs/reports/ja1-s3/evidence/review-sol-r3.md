**8/10. Approve the scoped round-three code changes, conditional on final evidence.** I found no remaining in-scope data-loss or security Major/Blocker. This is not approval to declare C0 complete before the outstanding gates pass.

All **49 frozen source hashes matched**, including at review completion. I reviewed the captured base diff, round-three changes, principal new source/tests/scripts, all six prior reviews, and both response documents. The deferred nonblocking findings remain C1 follow-ups.

The reviewed blockers are addressed as follows:

- **Probe allocation.** [owned_io.rs:727](/home/riels/.t3/worktrees/Kinewright/t3-1c0887c5/.review/c0-r3-source/crates/kinewright-media/src/owned_io.rs:727) rejects empty/dynamic stream tables and non-A/V streams before stream-info probing. Every admitted stream receives its reservation and decoder options first. [owned_io.rs:1098](/home/riels/.t3/worktrees/Kinewright/t3-1c0887c5/.review/c0-r3-source/crates/kinewright-media/src/owned_io.rs:1098) sets `max_streams` to exactly that reserved count. I checked pinned FFmpeg’s `avformat_new_stream`: it refuses at the cap before reallocating the stream array or allocating a codec context. The later count/kind comparison is correctly defensive. Real MPEG-PS, subtitle, WAV, and native stream-cap controls exercise these boundaries.

- **Incident colour producers.** Both [automatic recovery](/home/riels/.t3/worktrees/Kinewright/t3-1c0887c5/.review/c0-r3-source/crates/kinewright-app/src/app.rs:2578) and [card recovery](/home/riels/.t3/worktrees/Kinewright/t3-1c0887c5/.review/c0-r3-source/crates/kinewright-app/src/app.rs:2926) return before sending `DoIfRevision`, allocating a command token, or recording a pending apply. The shared refusal covers an attached Studio owner and an active Studio job. The witnesses check unchanged Core history, an open incident, no outstanding send, successful save, and durable reopen.

- **Nested-input counterfactual.** The concat fixture now declares its independently known one-second duration. The unrestricted control decodes uncertified PCM. The inspected protocol red records **96,000 accepted bytes** at `C0_PROTOCOL_OUTPUT_ZERO`, followed by restored green. I independently reproduced the recorded pre-review source hash by reversing only the documentation-backtick change in the frozen copy. That validates the earlier pair’s relevance; the final-source campaign is still required.

- **Mutation attribution.** [The media runner](/home/riels/.t3/worktrees/Kinewright/t3-1c0887c5/.review/c0-r3-source/scripts/ja1-s3-c0-media-mutations.py:285) now requires the exact selected test thread, witness file, marker-bearing macro line, and one focused failure. It rejects setup unwrap/expect panics. The JSON catalog and incident runner are hash-bound. Reviewed runners use `flock -o`.

During review, both **incident mutation pairs completed** on the frozen App hash. Each red reaches its intended history-preservation assertion while the other three controls pass; each restoration returns all four controls to green.

Recorded logs also corroborate 27 media controls, the production MPEG-PS control, four App controls, build, and Clippy completion. These are inspected results, not reviewer executions.

The complete **26-media, 24-project/read-only, and four-host mutation campaigns**, final restored-source gates, and migrated inherited regressions remain uncredited here. Historical partial campaigns earn no final-source credit. Root must verify restored live files against **all 49 hashes** before using this approval.

No edits, Cargo, mutations, or subagents were run. Native Windows, CI, and full suites remain **NOT RUN**. The decision-trail audit remains the Opus seat’s assignment.
