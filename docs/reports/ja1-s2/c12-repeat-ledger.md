# C12 final-owner repeat ledger

Parent: C11 `2aa6633db61912a416948edb580e2df8a3ddd58f`. This ledger is built before adding
the missing host repeats. A selector is credited only when the C12 gate records its execution.
The final report binds this ledger and the executed source to the C12 commit.

Host integration selectors below are in `kinewright-agent --test ja_s2_host`.
Host unit selectors are in `kinewright-agent --lib writer_host::`.
Small internal graph/IO faults run under the real `HostCommandOwner`; process restart
is separately exercised through the shipping host. Neither is physical power-loss evidence.

| Item | Final-owner execution / remaining work |
|---|---|
| W1 independent rescue/fresh actor/owned bytes | C12 `writer_host::c12_tests::rescue_cases::w01_first_save_rescue_is_independent`; composed process witness |
| W2 missing required bytes/complete uses/retained permissions | C12 `writer_host::c12_tests::source_cases::w02_required_missing_bytes_and_incomplete_inventory`, `w02_retained_handle_permission_boundaries`, `w02_first_save_capture_prefers_local_owned_bytes` |
| W3 phase/cause/two probes/cancel/no implicit consent | C12 `writer_host::c12_tests::source_cases::w03_probe_consent_and_premutation_cancel`, `w03_native_probe_causes_keep_phase_operation_and_errno`, `w03_changed_capture_invalidates_consent`; host choice/consent dispatch in rescue repeats |
| W4 legal trim/Redo/gesture/input/independent roots | C12 `writer_host::c12_tests::rescue_cases::w04_oldest_first_trim_preserves_redo_gesture_and_inputs`; omission/combined-choice repeats below |
| W5 exact seal/all actor gates/queued sends | Existing `g05l_two_clients_one_editor_ordered_history`, three fast restart/switch tests; C12 composed witness; Core `attachment::tests` and actual App producer tests remain lower-layer controls |
| W6 native separation/aliases/absent parent/pinned IO | Existing host roots test and C8 lifetime separation; C12 `writer_host::c12_tests::source_cases::w06_native_path_identity_and_missing_mount_separation`, `rescue_cases::w06_rescue_bounds_refuse_before_destination_mutation` |
| W7 repeated attempts/first mutation/four slots/no refund | C12 source `w07_mutation_boundary_and_four_attempt_slots`, `w07_repeated_source_attempts_do_not_deduplicate_by_destination`, `w07_no_ticket_attempt_keeps_actual_namespace_owner`; rescue `w16_repeated_active_b_attempts_settle_only_their_tickets` |
| W8 actual owners/bytes/pins/charges/readers/Close | Existing `host_failure_preserves_originating_context_or_refuses_recovery` executes kill/orderly/unwind; C12 source `w08_actual_owners_survive_drop_settlement_and_reader_retirement`, rescue `w08_context_close_fences_handles_and_retires_temporary_sources_once`; composed witness |
| W9 destination-only exact own-ticket/continuity | C12 source `w09_lifecycle_continuity_refuses_changed_parent_and_foreign_root`; W16 host rescue repeats and existing recovery barrier tests |
| W10 tentative exclusion/original ID/both orders/no-ticket | C12 rescue `w10_pending_command_exclusion_and_original_id_lookup`, source `w10_lifecycle_no_ticket_destination_settlement_keeps_live_history`; local review-fix comment cases retained in complete Project target |
| W11 bounded provenance/compaction/independent roots | C12 `writer_host::c12_tests::w11_host_recopy_compacts_provenance_and_keeps_independent_bytes` performs 18 actual host rescues/reopens and checks bounded warning compaction and independent evidence bytes; complete Project W11 targets retain exhaustive record boundaries |
| W12 publication phase/exact cleanup charge/ordinary copy | Complete `ja_s2_save`; C12 `writer_host::c12_tests::w12_host_cleanup_only_fresh_copy_keeps_exact_charge_and_history` exercises the clean acknowledged-history host Save As route after definite failure/charged cleanup. Dirty Save As still requires its ordinary Save first; the local BundleCopy dirty-live/acknowledged-copy distinction is not silently bypassed. |
| W13 R_ack/P1/P2/one-level/ordinary floor/return | Existing C11 host switch/pin/target/frozen-rescue/session tests; C12 rescue `w13_one_level_rescue_recopy_and_explicit_ack_retirement`, `w13_omission_only_precedes_trim`, `w13_trim_then_separate_omission_recomputes_closure`, `w13_available_recovery_roots_cannot_be_omitted_for_quota`, `w13_recovery_on_active_rescue_copy_settles_the_new_attempt` |
| W14 raw support before actual mutations | Existing host `w14_headless_and_agent_admission_precedes_mutation`; both C6 local witnesses execute separately in `ja_s2_rescue_record` and `ja_s2_rescue_admission`; App raw admission executes too |
| W15 capture-only late B/release/active C | C12 W10/W16 rescue repeats; existing actual App `w15_*` text/release tests; composed witness |
| W16 retained A writer/new floors/repeated B attempts | C12 rescue `w16_first_save_a_writer_and_later_floor_continue`, `w16_repeated_active_b_attempts_settle_only_their_tickets`; source `w16_first_save_a_writer_and_later_floor_continue`, `w16_first_save_older_root_writer_reuses_retained_namespace` |

| Gate | E/P/B/M execution |
|---|---|
| G4 writer identity/journals | Existing host `g04_two_process_lock_save_alias_and_discovery`, complete Project `ja_s2_writer`, actual App `g04_*`; positive locked writes, alias/suffix/discovery refusals, exact graph/claim controls. Original baseline flips remain archived C7 evidence; final code executes again. |
| G5-L ingress | Existing host `g05l_two_clients_one_editor_ordered_history`, `g05l_disk_barrier_keeps_acceptance_local_and_saved_exact`, `g05l_ack_prefix_is_the_independently_reopened_history`, `g05l_input_context_is_admitted_before_local_acceptance`; actual App local paint and all-producer cut tests. |
| G5-L restart/floor/close | Existing fast `g05l_fast_restart_absent_vs_committed_lost_reply_suffix`, `g05l_fast_restart_shared_floor_and_close_reply_loss`; slow matrices `g05l_host_death_recovers_absent_and_committed_drag_suffix`, `g05l_shared_floor_and_recovery_close_reply_drop`; pressure witness explicitly enabled. |
| G5-A adoption/return | Existing fast `g05a_fast_b5_switch_reply_loss_before_and_after_restore`, slow `g05a_switch_reply_drop_and_participant_death`, complete `ja_s2_switch`, actual App first-edit/Undo/return. |
| L2 | C8 host kill/orderly/unwind and App `c8_retention_loss_fences_every_action_and_keeps_actual_source_until_close` repeat unchanged; separate deletion/uncharge Drop mutations repeat at C12. |

## W13 cases and mutation attribution

These are current-head executions, not reuse of old local passing logs. The complete
Project/record/App functions retain their independent graph/history oracles. Host
switch/return, actor/owner identity, publication and restart observations execute
the final owner and dispatcher. Fine-grained private parser/permission/profile
controls stay separately identified as shared lower-layer controls; a standalone
SessionStore permission test is not claimed as a process restart.

`H` below means `writer_host::switch::tests::`; `R` means
`writer_host::c12_tests::rescue_cases::`. Project selectors are in
`ja_s2_rescue_record`, unless a target is explicitly named.

| Plan §4.2.6 case | Exact final-host selectors and independent controls |
|---|---|
| 1 dirty Save before one selection; P2 rotation, no-floor/floor/gesture | `H w13_host_frozen_rescue_abandons_original_switch_and_preserves_disclosures` reaches both dirty publication boundaries; `H w13_host_same_target_pin_survives_quota_retirement_rotation_and_retry` retains original P1/P2 identities through rotation; process `g05a_fast_b5_switch_reply_loss_before_and_after_restore`; `c5::w13_checkpoint_recovery_preserves_dirty_b_before_activation`, `c5::w13_preliminary_save_uses_ordinary_floor_with_redo_or_gesture`, `durable::w13_selection_pin_capacity_refuses_before_publication`; actual App `w13_checkpoint_recovery_preserves_dirty_b_before_activation` |
| 2 faults at both publications, absent/committed, frozen cut, refusal unfreeze | The same H frozen-rescue selector now executes landed/absent × preliminary-Save/selection. `g05a_switch_reply_drop_and_participant_death` and all three fast restart tests retain actual process outcomes. `c5::w13_preliminary_save_faults_keep_exact_cut_and_do_not_select`, `c5::w13_selection_refusal_and_absence_keep_p2_for_same_target_retry`, `durable::w13_local_selection_mapping_limits_and_retry` |
| 3 original request/one R/one generation, duplicate replies, writable controls | `g05a_fast_b5_switch_reply_loss_before_and_after_restore`, `g05a_switch_reply_drop_and_participant_death`, `rescue_and_switch_share_one_owner_after_restart`; complete Project `ja_s2_switch`; `durable::w13_local_selection_mapping_limits_and_retry` |
| 4 shared occurrence/batch/input identities, three R saves/floor, return, B-only reopen | Process G5-A fast/slow selectors and actual App first-edit/Undo/return; complete `ja_s2_switch`; `c5::w13_checkpoint_recovery_preserves_dirty_b_before_activation`; Agent exact `durable_branch::tests::w13_shared_prefix_occurrence_identity_survives_branch_saves`; composed rescue removes original GUI source access before B reopen |
| 5 exact/one-over fresh-open, all publishers and independent roots | H pin/refusal/retry tests, actual host Save/Save As/rescue/18-round recopy and receipt-bearing switch publications; `ja_s2_save::linux::w13_every_publication_fits_fresh_open`, `durable::w13_selection_fresh_open_exact_and_one_byte_over`, `c5_pressure::w13_definite_rescue_sparse_cache_growing_saves_and_same_pin_quota_retry` retain same-limit independent Open. These limits are test profiles, never added production headroom. |
| 6 sparse definite-success/no-reconcile growing saves | C12 `R w13_one_level_rescue_recopy_and_explicit_ack_retirement` and host ordinary Save/Save As routes call the same guard; `c5_pressure::w13_definite_rescue_sparse_cache_growing_saves_and_same_pin_quota_retry` and `ja_s2_save::linux::w13_every_publication_fits_fresh_open` retain the precise no-reconcile discriminator and independent reader. Cache membership cannot select candidate branches. |
| 7 retirement/quota, P1 same pin, prepared target, live unsaved session/base | `H w13_host_same_target_pin_survives_quota_retirement_rotation_and_retry` has separate P1 and P2 retry lanes and an exact-limit refusal-reply lane; `H w13_host_prepared_target_exclusion_and_current_head_retry`; `H w13_registered_host_create_reopen_refuses_retirement_and_preserves_unsaved_edits`; `c5::w13_prepared_existing_target_is_never_a_retirement_offer`, `c5::w13_existing_target_retry_requires_current_membership_and_head`, `c5::w13_one_level_retention_and_explicit_retirement` |
| 8 failed/unknown frozen rescue, immediate B handoff, late A, both publications | `H w13_host_frozen_rescue_abandons_original_switch_and_preserves_disclosures` executes all four landed/absent × publication combinations; `R w13_recovery_on_active_rescue_copy_settles_the_new_attempt`; `c5::w13_frozen_cut_rescue_at_both_unknown_publications_and_late_a_isolation`; actual App W13/W15 selectors; composed process test checks restart then rescue and repeated late A proof without changing B |
| 9 session-only P2, preview/handoff/Close/destination disclosure and accepted loss | H frozen-rescue selector checks host availability/Close/destination status; H same-pin selector checks actual rotation. `c5::w13_frozen_cut_rescue_at_both_unknown_publications_and_late_a_isolation` retains the A-absent independent-reopen oracle; actual App `w13_preview_close_and_every_destination_disclose_session_pin_loss` and `w13_actual_destination_controls_and_background_freeze` check rendered text and refused Save As/open. The accepted crash limit below remains. |

| Mutation family retained by §4.2.6 | Exact witness executions / evidence class |
|---|---|
| omit or target-only fresh-open fit, hash dedup/decoded/other-reader limits | Complete `ja_s2_save::linux::w13_every_publication_fits_fresh_open` plus C5 pressure/selection boundaries; original C1 semantic mutation evidence remains archived. C12's root/receipt guard mutations and actual final-host candidate publications execute afresh. No archived pair is counted in C12's fresh pair total. |
| inventory-position resolution, missing/early-release target pin | H same-pin P1/P2 selector; fresh C12 `host-releases-target-pin-before-quota-retry`; original C4/C5 primitive pin mutations remain separately archived. |
| document-only restore/truncated history/changed non-linkage payload | Process G5-A/return and composed rescue; `durable::w13_projection_preserves_every_non_linkage_payload`, `durable::w13_checkpoint_restore_uses_b_identity_without_rewriting_inputs`, complete C5 recovery; original graph-valid rehash mutations remain archived, without new corruption-as-preservation credit. |
| shared prefix/baseline tuple or BatchId reminting; drop L/R on later saves | Fresh C12 `first-r-save-remints-shared-batches`, `return-save-remints-old-batches`, `later-r-save-drops-l`, `later-l-save-drops-r`; each executes `ja_s2_switch`'s independent payload/history oracle. Final host switch/return selectors repeat the owning route. |
| source identity/head/revision/generation/sequence ignored | Fresh C12 `source-project/branch/epoch/head/checkpoint/checkpoint-generation/floor/switch/revision/sequence-ignored`; exact `linux::g05a_each_source_identity_is_required_before_selection` |
| pointer/generation refresh, duplicate R/retry, selection before Save, early Saved/authority | Fresh C12 `next-save-resets-switch-generation`, `original-retry-starts-another-selection`, `switch-receipt-not-atomic`, `receipt-publication-agreement-skipped`, `receipt-reader-agreement-skipped`, and host confirmation/lease/restore/restart pairs; complete Project/host/App switch functions repeat |
| frozen save/floor bypass, selection-unknown resume, failed/unknown rescue resume | Fresh C12 `host-resumes-after-failed-rescue`, `host-resumes-after-unknown-rescue`, `shadow-resumed-before-gui-confirmation`, `definite-refusal-keeps-old-actor-frozen`, plus existing Core attachment and actual App all-producer tests. H frozen-rescue runs both publication seams. |
| pending target offered/removed, missing membership/changed head/resurrection | Fresh C12 `host-offers-prepared-target-for-retirement`, `host-removes-prepared-target-directly`, `host-ignores-existing-target-head`, `host-resurrects-retired-target`; H prepared-target selector |
| live session/base retirement and unregistered create/reopen | Fresh C12 `host-fork-without-registration`, `host-reopen-without-registration`, `host-ignores-live-session`, `host-ignores-live-specialist-base`; H registered-host selector. Local C5 guard controls execute too; no App durable-session route is invented. |
| late A activates abandoned R or changes B | Fresh C12 `host-late-a-replaces-b`, `rescue-handoff-keeps-host-switch-freeze`; H frozen-rescue and composed process selectors |
| R_ack/P1/P2 edge/count/role/default omission, A identity, rotation/compaction | `R w13_one_level_rescue_recopy_and_explicit_ack_retirement`, `R w13_omission_only_precedes_trim`, `R w13_trim_then_separate_omission_recomputes_closure`, `R w13_available_recovery_roots_cannot_be_omitted_for_quota`, host W11 and complete Project record/raw controls; original C4/C5 graph/choice mutation pairs remain archived. Fresh C12 provenance-summary mutation reaches actual host recopy. |
| omission-before-trim, combined consent, independent Redo/input roots, closure recomputation/reintroduction | The four R choice/trim functions above plus `R w04_oldest_first_trim_preserves_redo_gesture_and_inputs` run the actual host rescue command with exact consents. Source graph/choice mutations retain their C5/C6 evidence class; none is relabelled a new historical S0 flip. |
| actual App disclosure omitted at preview/handoff/Close/destination, quota retirement and safe-Close wording | Full App `ja1_attachment::tests`, including the exact W13 text/destination/blocker functions and W15; original C6 rendered-text mutation pairs remain archived. Fresh C12 N2 disclosure/Open B pairs exercise the new outcome and actual rendered control. |

## Section 7 rule/site closure at the final owner

| Plan row | Current-head route / witness attribution |
|---|---|
| S1 Minors #1–#8 | Core actor/attachment, complete `ja_s2_save`, actual App wording and final-host Save/cleanup/copy repeats. C1/C2 fixes and their original mutation provenance are retained; C12 source hashes identify the unchanged production sites. |
| §2 cause/probes/consents and frozen-cut refinement | W3 source classifier controls, host choice/consent rescue repeats and H frozen-rescue at both publications |
| §3 retained bytes, incomplete diagnostics, no partial graph | Host W2 missing-evidence negative and restored-byte positive, host W11/W12 and composed process B-only reopen; shared retaining-use/permission cases are lower-layer controls |
| §4 complete history/legal floor/omission/closure/R_ack | Host R choice/trim/recopy functions, H target pin/selection, all W13 Project/record functions and actual App controls |
| §5 context lifetime and explicit late-capture release | All three C8 host-failure modes, actual GUI L2 owner/Close test, C12 R W8/W10/W16 and composed process witness; actual App W15 release controls |
| §5 separate actor/seal/late capture; source resume failure | Same host/process rescue witnesses, Core attachment gates, N2 Project/host/App witnesses; no new paused state |
| §5 destination-only receipt/exact live ticket/suffix continuation | Host W10/W16 rescue repeats, source no-ticket continuity controls, process late A repetitions, all three fast restart tests and shared floor/close matrices |
| §5 four attempt slots and first mutation | Host R repeated B attempts plus source exact slot/first-mutation controls; ordinary host saves continue at full consumed slots |
| §6 native separation/absent parent/pinned IO | Host roots/forwarded roots, real C8 source-subtree refusal, actual writer/alias tests, source path/continuity controls; physical mount/device removal remains NOT RUN |
| §7 tentative command/original ID/late A | Host R W10 and source no-ticket controls, actual App W10/W15, composed original switch receipt and late A; local review-fix pending-comment cases execute independently |
| §7 role/API/raw admission/provenance bounds | Both C6 local W14 callers and host W14 repeat, complete raw record/switch controls, host 18-round W11/independent evidence |
| §7 person-visible outcome/loss/cleanup | Actual App selection/rescue/floor/Close/status and new N2 Open B; host status messages and bounded missing-data details |
| BundleCopy/store/namespace/receipts/actor consequential sites | Complete PROJECT + S2-SAVE + S2-RESCUE + S2-HOST + CORE + MEDIA-PINS; no alternative collector, interpreter or source ledger. Candidate-root guard, exact settlement and native identity remain their existing owners. |

The exact expanded selections, runtime counts, source/fixture hashes and raw results
are retained by `c12-gate.sh` and `scripts/ja1-s2-c12-audit.py`. Fresh mutation
definitions are `scripts/ja1-s2-c12-mutation-cases.json`; the report records each
accepted red/restored-green and distinguishes archived earlier mutation evidence.

Windows durable success is inapplicable under the existing native durability refusal.
Native Windows execution, physical device/power loss, native providers/G6, G20,
G7-D/G1-W and interactive display acceptance remain NOT RUN here.

The selected-target session pin is in-session only. A crash after automatic rotation
and before durable activation can lose that point. This accepted limit is not a
crash-survival claim.

## Exact §7 rows requiring C12 repeats

The following keys preserve the plan's row identity; `A` means the `ja_s2_host`
integration target, `H` means the host library and `U` the App binary.

| Plan §7 row | Exact executed route at C12 |
|---|---|
| A1 §2 :141–152 frozen finished-command cut / no resume | H `writer_host::switch::tests::w13_host_frozen_rescue_abandons_original_switch_and_preserves_disclosures` (both publications); A `g05l_fast_restart_absent_vs_committed_lost_reply_suffix`; H `writer_host::c12_tests::rescue_cases::w10_pending_command_exclusion_and_original_id_lookup` |
| A1 §5 :368–418 lifetime / actual owners / L2 | A `host_failure_preserves_originating_context_or_refuses_recovery` (kill, orderly, caught-unwind); U `ja1_attachment::tests::c8_retention_loss_fences_every_action_and_keeps_actual_source_until_close`; A `rescue_and_switch_share_one_owner_after_restart`; four orderly/unwind source-delete/uncharge mutations and composed uncharge mutation |
| A1 §5 :420–455 separate actor / seal / abandon / never-reactivate | A `rescue_and_switch_share_one_owner_after_restart`; H frozen-rescue selector above; H `writer_host::c12_tests::n2_committed_capture_never_acknowledges_sealed_a`; U `ja1_attachment::tests::n2_sealed_a_discloses_committed_b_and_offers_open` |
| A1 §6 :540–567 identity / absent parent / pin / try-lock | A `roots_and_forwarded_roots_cover_all_file_surfaces`, `host_failure_preserves_originating_context_or_refuses_recovery`, `g04_two_process_lock_save_alias_and_discovery`; H `writer_host::c12_tests::source_cases::w06_native_path_identity_and_missing_mount_separation`; A composed witness hides/restores A's actual parent |
| receipts/bundle/copy live and sealed settlement sites | H `writer_host::c12_tests::rescue_cases::w10_pending_command_exclusion_and_original_id_lookup`, `w16_first_save_a_writer_and_later_floor_continue`, `w16_repeated_active_b_attempts_settle_only_their_tickets`; H `source_cases::w10_lifecycle_no_ticket_destination_settlement_keeps_live_history`; A composed repeats late A after B activation; complete Project review-fix pending-comment tests retain lower-level discriminators |
| actual App + local headless/agent save entrypoints | U `ja1_attachment::tests::n2_sealed_a_discloses_committed_b_and_offers_open`, P `ja_s2_rescue_record::w14_local_headless_admission_precedes_mutation`; A `ja_s2_rescue_admission::w14_local_agent_admission_precedes_mutation`; A `w14_headless_and_agent_admission_precedes_mutation` |
| historical S1/rulings/closing question | Preserved unchanged; C12 report carries the exact Minor disposition, S0 flip ledger and S2-H C7–C12 map. This row is document closure, not another runtime test. |

## Fresh mutation attribution and architectural substitutions

C12 repeats the applicable C4/C5 semantic mutation definitions against the final
source, in addition to the C11 host/switch pairs and C12 integration pairs. The
canonical JSON and final report identify every exact witness and runtime result.
The earlier statements above identifying archived C4/C5 evidence preserve its
historical origin; accepted `repeat-c4-*` / `repeat-c5-*` pairs in the C12 report
are fresh executions. Source-only private schema/graph mutations stay lower-layer
controls; rescue/choice/recopy cases that use `writer_host::c12_tests` invoke the
real final dispatcher and use the original independent graph/history assertions.

Two stale C5 anchors are relocated without changing their semantics: independent
acknowledged evidence is retained by `acknowledged_evidence()`, and a retained
copy's new unknown publication refreshes its own status/tag in `reconcile_retained`.
Both mutations now execute the host rescue repeats. The old C4 frozen generic
restore mutation still executes the Core all-mutation-path witness with its
current adjacent manual-permit guard. Its old local selection witness no longer
calls that generic restore API: C10 uses the separate owner/cut-checked frozen
install. That duplicate attribution is inapplicable; host early-resume/frozen-cut
and exact-install witnesses exercise the current path. No survived mutation is
counted as a pass. The former additive fixture's `false && property` mutation is
excluded: changing an observer's assertion is not a behavioral product mutant.

The attempted App path-relabel mutation survived because the existing
`Ja1EditingSession::set_path_spelling` already checks native file identity.
The redundant proposed move and new test were removed; this is excluded
survivor evidence, not an integration bug or a passing mutation. Required App
pairs are repeated after this simplification. Other packages do not link App;
the audit retains their original full source snapshots, records the unlinked App
delta and requires exact restored-green/final-gate binary equality.
