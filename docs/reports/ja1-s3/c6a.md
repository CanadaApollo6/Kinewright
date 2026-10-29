# JA-1 S3 C6a

Base: `295619cc`. Branch: `ja1/s3`. Local work only, without a push.

S3 certification is **partial**. The focused data-path fixes below do not close Studio voice composition or G11. See [the deferred list](deferred.md) for the work that exceeds the brief's focused-change rule. No Delivered status is inferred from scheduler outcome or internal task-message delivery.

## Commit 1

- C3F3-m1 is fixed. `Keeping::Failed` refuses Save note and Revise cut before preparation, admission, issuer publication, or dispatch. `s3_fix3_one_owner_per_recovery_file` counts evidence admission, envelope issuance and metadata calls and requires zero for the refused second owner. Separate save and submit mutations reach `C6A_SECOND_OWNER_NO_ADMISSION` and `C6A_SECOND_OWNER_NO_ISSUER`.
- C2-A2 is fixed. Every document event refreshes or invalidates the manual playback cache, including events for a frozen or inactive project. The frozen picture remains selected. Current edit and project focus require the cached document to match the session document. `s3_current_edit_refreshes_trim_while_frozen` trims a real video clip while a frozen PF1 picture is selected, verifies the updated manual document, returns through the actual Current edit handler, and waits for the current-edit presentation. Its mutation reaches `C6A_MANUAL_TRIM_REFRESHED`.
- Registry retirement is wired into App focus changes, successful close and legacy-save source-admission replacement. The engine cancels/removes that authority's analysis registrations and drops its visual-result registry channel. Accepted visual jobs retain their original sender and accounting until completion. Duplicate admission compares that sender with the current result channel; a request on a fresh channel refuses for retry until the old job finishes, then receives its own reply. Re-focusing creates a fresh result channel and clears the UI cache's retained receiver and pending requests. `s3_rapid_focus_round_trip_rebinds_visual_cache` focuses away and back without an intermediate UI poll, then waits for a real waveform result through the App cache. `c6a_retire_analysis_cancels_only_inactive_authority` and `c6a_retire_visual_channel_releases_registry_and_preserves_other_project` cover authority separation, cancellation and fresh requests. `s3_retire_media_after_studio_revokes_legacy_grant` verifies that the App can still remove the original channel after Studio revocation. The session keeps a cleanup reference to the original grant, including its revoked state. Replacing source admission retires the previous registrations before installing the new cleanup reference. This does not revoke grants belonging to continuing consumers.
- C1F2-D1 is fixed. The cumulative `c1.md` hash is refreshed in `c1-evidence/fix2/final-binding.json`, with the previous hash and reason retained separately. Historical source and runtime bindings remain unchanged.

Model the Domain shaped the per-authority retirement operations. Prove It Works shaped the runtime guard mutations and exact source restoration. No independent review agents were started by this implementation worker.

## Verification

Raw gate records and source hashes are recorded in the accompanying evidence directory when each commit's gate completes. Initial fixture/compile failures receive no mutation credit. The first Clippy run found a new misplaced `must_use` attribute on the unit-returning retirement method; moving it back to `legacy_media` fixes that new error. It is absent at the base. The initial trim fixture installed its cache before initial scope synchronization, which correctly cleared it; installing after the first bound frame fixes this new test setup. The first compile attempt used the wrong clip field and inserted the counter assertion into the wrong test. Both were corrected before the credited mutation runs. Later all-target checking caught new retirement-test grant borrows and a missing test-closure semicolon; both were corrected before the final successful Clippy run. These new helper errors do not exist at `295619cc`.

No final App end-to-end voice, scheduler controls, disposition or delivery-label acceptance is claimed. Backend speech controls, even if equipped, cannot certify the missing Studio control.

The initial serial Media command reached its 600-second deadline without an assertion failure. The first partition attempt was intentionally stopped after finding the new retired-channel duplicate-admission hole during source audit. Those incomplete runs earn no full-suite credit. The corrected source is rerun exhaustively from its compiled inventory.

The first rapid-focus control reached waveform delivery but failed its engine teardown deadline because the test retained two analysis handles. Explicitly dropping those fixture-owned handles before engine retirement fixes the new test setup. This test is absent at `295619cc`; the production waveform path did not fail in that attempt.

## Commit 1 final gate

[Source and fixture identities](c6a-evidence/commit1/source.json), [runtime fixture identities](c6a-evidence/commit1/runtime-fixtures.json), [raw gate summaries](c6a-evidence/commit1/gates.json), and [protected pairs](c6a-evidence/commit1/mutations.json) bind this source cut. The App run is `gate1-app-frozen.log.gz`; earlier App logs are diagnostics for earlier source cuts. Media runs are the exhaustive partitions in `gate1-media-partitions.json`, each with its exact selected names, raw summary and exit.

| Gate | Passed | Failed | Ignored |
| --- | ---: | ---: | ---: |
| App, full default suite | 936 | 0 | 6 |
| Media, full serial unit inventory | 1,193 | 0 | 65 |
| Media, every integration target | 31 | 0 | 5 |
| Media doctest | 1 | 0 | 0 |

Workspace build, all-target Clippy with warnings denied, formatting and manifest lint pass. Windows compilation exits 101 in native prerequisites at both this source and exact base `295619cc`; neither validates the touched Windows Rust. The final trim mutation was refreshed after the last `app.rs` lifecycle edit; all three pairs restore the exact guarded source and pass. Astra 4's persisted-edit pair is NOT RUN because that composition is deferred.
