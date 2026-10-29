# S3 deferred work

These are implementation follow-ups, not accepted S3 gates. They remain on the S3 deferred list. A next-stage first-commit destination is named where the brief assigns one.

| Item | Destination and reason |
| --- | --- |
| Studio C4/C5 composition | Closed by C8/C9: host voice ports, bounded capture, private range reads, receipt-first recovery, snapshot read leases, attached Studio controls, acknowledged capture cleanup, installed-model G11 App control and screenshots; see c8.md and c9.md. |
| C6 ASR completion retry | Closed by C8/C9: the host retains and retries the completion carrier on `Unsettled`; the attached App supplies progress, edit and choice controls. Complete-suite and real installed-model evidence are in c8.md and c9.md. |
| Batch disposition and S4 ports | Mostly closed by C7: Revise cut reaches S4's `ReviseCut`, and Notes reads the linked run through `ObserveRun`. The control intents remain open: see the C7 rows below. |
| C7 Pause, Stop and Continue | S3 deferred list. A `RunIntent` from Notes needs its own durably retained envelope and lost-reply retry, like the Revise cut request. That is more than a focused addition to C7. |
| C7 run refresh | S3 deferred list. Notes polls `ObserveRun` once a second until the run finishes. There is no host push channel for run events. |
| C7 in-process issuer | S3 deferred list. Notes saved in-process before the host attaches carry an issuer that the host refuses ("issuer belongs to another attachment subject"). The C7 tests reset the issuer. The App needs one issuer that survives the host handover. |
| C7 host-attached reader store | S3 deferred list. C9's snapshot lease now serves `HostFeedback::published()` and frozen contexts. Note evidence goes through C7's `ReadFeedback`. Check for any remaining reader-store path in the end-to-end commit. |
| C7 ReadFeedback pin cap | S3 deferred list. `ReadFeedback` pins at most 64 revisions per read, so a refresh with more saved notes needs paging. |
| C7 Director scope | S3 deferred list. `director_scope` names the whole document but leaves out effect-parameter fields, and it refuses documents over 256 items. |
| C7 orphan candidate branches (Astra minor 2, Muse minor 1) | S3 deferred list, host lifecycle. Each ReviseCut attempt that fails after the fork leaves an unused durable branch, plus an in-memory session registration and pins in that host. This includes every `scope_refused` retry on an over-limit document. Persist a batch-to-candidate binding and reopen it on retry, or retire unused preparation. Extend the crash witness to count branches and registrations. |
| C7 `revises_head` contract | S3 deferred list, S4 review. S4's consumption check now treats a forked candidate with a fresh head identity as revising the submitted manual head. This changes S4's contract and needs S4 review. Astra minor 3: the check compares document hashes, not fork provenance, so an older empty fork with the same document can pass. Bind it to the recorded fork source and the input and lineage identity. |
| C7 screenshot scene | Studio visual leftovers. The `revise` screenshot scene binds no picture. Fix 1 made it read-only: it never sends Revise cut. |
| C7 adopting another attachment's batch (Astra M1, ruled Minor) | Ruled intended, no change. An Editor may start the run for a batch that a Viewer or an earlier attachment submitted. There is one live Editor, the receipt names the real actor, and `DispatchOrchestration` already allowed it. |
| C7 run slots (Astra minor 4, Muse minor 3) | S3 deferred list, scheduler. `RUN_SLOTS` (64) counts every run this host process retains, finished ones included, so finishing a run does not free a slot until the host restarts. Durable queued runs from an earlier host are not counted. Attachment capacity can refuse before the 64-run Waiting path. It fails safe, with no spend. |
| C7 mutation outcome (Astra minor 5, Muse minor 6) | Recorded qualification. The envelope mutation goes red as a refused retry, not as two runs, because the project's one-consumption check stops the second run. Replay comes from the durable consumption link plus the exact request bytes in the Director assignment, not from an envelope built from the command id. Do not weaken the project guard to get two runs. |
| C7 Director setting writer (Muse minor 4) | S4 or a settings surface. Nothing writes `scheduler/director.json` yet, so in production every Revise cut answers Waiting until a Director is set up. |
| C7 late head move (Muse minor 7) | S3 deferred list, wording. A head that moves between the early check and the publication reaches the App as `identity_refused`, not Conflict. The next attempt reports Conflict, and nothing is lost. |
| C7 queued run after a host restart (Muse minor 8) | S4. Replay after a host restart does not re-retain the run in `hooks.runs`, the same as `DispatchOrchestration` replay. No test shows that a queued run resumes executing after a restart. |
| Delivered labels | S5 delivery integration or an explicit S3 delivery-record contract. Neither S4 `DeliveryRecord` nor Studio `DeliveryView` carries a delivery timestamp; internal task-message delivery is not reviewed cut delivery. Do not invent age or label unfinished work Delivered. |
| Astra 4 persisted remark-edit retention | S3 deferred list. The metadata API has immutable SaveFeedback revisions but no persisted remark deletion or split/merge operation. Add those App semantics before claiming a persisted edit/delete witness or its mutation pair. Existing local-vector edits remain backend evidence only. |
| Hooks Muse 1 | S3 deferred list, host lifecycle. Pending-publication reconciliation after actor expiry must preserve receipt authorization and all producer/attention ingress checks; this exceeds the App retirement change. |
| Hooks Muse 2 | S3 deferred list, host lifecycle. Seal owner transition construction to the machine prepare path together with its production caller. |
| Hooks Muse 3 | S3 deferred list, host lifecycle. Add owner-port uncertain-publication fault injection with that caller. |
| Hooks Muse 4 | S3 deferred list, host lifecycle. Terminal cleanup must retire producer attachments and runtime slots while retaining live continuation readers; App media retirement does not own these objects. |
| C3 Muse M3 | S3 deferred list. Source orientation remains Upright at capture; needs source-to-display orientation binding. |
| C3 Muse M4 | S3 deferred list. GPU-route pixel digest needs a bounded proof readback. |
| C3 Muse M6 | Before a producer observer gets Viewer. Narrow ListFeedback scope and enforce private ranges. |
| Hooks Muse 5 | S3 deferred list. Guard observe_run's tasks[0] assumption. |
| Hooks Muse 6 | S3 deferred list. Define duplicate live-subject policy. |
| Hooks Muse 7 | S3 deferred list. Route attention prompts to the correct observer/holder. |
| Hooks Muse 8 | S3 deferred list. Bind LiveAttentionResponse.command fully. |
| Hooks Muse 9 | S3 deferred list. Expand continuation readiness probes. |
| Hooks Muse 10 | Native Windows/hosted gate. No local Linux result substitutes for either. |
| Hooks fresh-attempt grants and replacement holder | S3 deferred list, host lifecycle. Issue grants for fresh retries and route attention response delivery to the replacement holder. |
| Hooks re-check Minor 3 | S3 deferred list, host lifecycle. Private issuer reissue reply-loss continuity. |
| Studio visual leftovers | S3 deferred list. Picture slack, waveform peaks, clip beat labels, open A/B menu and narrow Timeline captures remain open. Delivered labels have their separate contract gap above. |
| C1F3-m1 | Next stage's first commit. Detached Save As/Switch handoff continuity across a replacement client. |
| C2 50 ms wake cost | Efficiency gate if idle CPU flags it. No numeric S3 speed gate is added. |
| C2-A3 | S3 deferred list, acceptance. Combined PF1 tamper/damage, uncached mixer seek, real switch after failed copy/lost reply, CPU/GPU route continuity and output-frequency legs. |
| C8C9-M4 completed voice jobs | Next stage first commit. Evict completed host jobs after deriving missing-model/no-speech progress from durable ASR, so eviction preserves the current plain reasons and completion retry. |
| C8C9-M5 Whisper context reuse | Next stage first commit. Reuse a context behind the shared model-lifetime guard without weakening pinned hash validation or note/asset mutual exclusion. |
| C6 end-to-end | S3 deferred list, acceptance after composition. Drive all stages in one App/host/store/media route, including Save As and rescue; existing individual witnesses do not replace this test. |
