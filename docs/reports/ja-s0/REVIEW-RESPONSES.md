# S0 first review responses

First reviewed head: `0e9b91bafb050d8a780feb56c4c62db86a127da9`.
Astra requested changes; Muse accepted with one non-blocking suggestion. These
responses are incorporated by amending S0-5, preserving the five-commit sequence.
Fresh independent final-head reviews follow the amendment.

| Finding | Response and exact witness |
| --- | --- |
| Input/oracle role collision | Reject the reserved `oracle:` prefix on input roles before loading/executing commands. `ja_harness::tests::input_roles_cannot_alias_the_oracle_namespace` uses two valid, differently hashed command records; removing the admission check fails the witness, restoring it passes. |
| Deadline checked only after blocking work | The native CLI supervises its fixed fixture subprocess and setup/action/cleanup deadlines. A timed-out fixture is interrupted and reported NOT RUN with phase/process/reap evidence. `native_deadline_interrupts_pending_confirmation_and_reaps_fixture` first observes a real approved prepared commit, then leaves a second real confirmation pending. Its five-second budget interrupts the broker wait; adding six seconds to enforcement fails the witness, restoration passes. This does not certify production guardian/host-death/resume or Windows containment. |
| Repeated media artifact collisions | Preview/proof/export/decode artifact names include participant/action ID; decoder caches also distinguish actions. `ja_harness::tests::repeated_media_actions_retain_distinct_complete_artifacts` retains eight separate full-byte/hash identities across two seeks, proofs, exports and decodes. Removing action identity fails; restoration passes. |
| Media participant misrouting | Admission refuses non-main participants for the main-only media/App/session adapters before any Core or device action. `ja_harness::tests::media_adapter_refuses_a_non_main_participant_before_execution` fails when the check is removed and passes after restoration. No unsupported participant is silently routed to main. |

Shared controller dispatch also precedes every main adapter, so media/App calls
cannot bypass retry identity checks. Main Core replies use the same outward-drop
bookkeeping as workers. The additional
`ja_harness::tests::main_adapter_uses_shared_reply_drop_and_retry_validation`
witness exercises actual accepted main edits/reply loss and rejects a changed
media retry; bypassing main-adapter routing fails it, restoration passes.

The first repeated-media mutation attempt had duplicate action IDs in its new
fixture. Both attempted runs stopped at admission. The archive explicitly labels
that attempt NOT RUN with no mutation-red/restored-green credit. The corrected
fixture passed its positive control before the accepted mutation pair.

The missing mutation reports were present in crate-relative ignored output
directories. All 18 phase report directories from the original nine accepted
captures were recovered into their original capture groups. The archived App
failures now include `actual-headless-callback-ran` and
`independent-letterbox-landmark` with literal observed/expected values, alongside
the Rust assertion logs. New captures resolve the output directory before
passing it to cargo/libtest. Earlier evidence retains its original source/run
identities; it is not relabelled as a final-source rerun.

Only the S0-3 recording SIGSEGV receipt was retained. The README now distinguishes
that sealed failed process attempt from the earlier S0-4 diagnostic failure whose
receipt was not retained. Neither receives passing-control or baseline-red credit.

Muse suggested a separate exit code for failed controls. The frozen §7.4 defines
exit 1 for an exercised assertion failure and exit 2 for unavailable/setup work.
The current report separately records `control_failed` and `baseline_red`, stops
after a failed control, and requires a successful named control before any
baseline-property assertion. Retain this contract rather than change exit-code
semantics during S0. Aggregate exit 1 alone is not baseline-red evidence.
