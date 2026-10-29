# S3 deferred work

These are implementation follow-ups, not accepted S3 gates. They remain for the next S3 composition commit unless a later stage is named.

| Item | Destination and reason |
| --- | --- |
| Studio C4/C5 composition | Next S3 composition commit. Authenticated streamed capture, private audio-range reads, model/workspace and capture-file reservations, cleanup, transcript revision selection and completion retry need an owner adapter together. Enabling the current direct-owner helper as a GUI writer would bypass that contract. |
| C6 ASR completion retry | Next S3 composition commit. Retain the borrowed completion carrier on `Unsettled` in the new App caller; no such production caller is added here. Existing backend controls exercise this contract. |
| Batch disposition and S4 ports | Next S3 composition commit. Translate the actual S4 `RunProjection`/host commands into presentation and typed intent, including refresh/revision checks; the fixture `RunView` is not the production projection. |
| Delivered labels | S5 delivery integration or an explicit S3 delivery-record contract. Neither S4 `DeliveryRecord` nor Studio `DeliveryView` carries a delivery timestamp; internal task-message delivery is not reviewed cut delivery. Do not invent age or label unfinished work Delivered. |
| Astra 4 persisted remark-edit retention | Next S3 composition commit. The metadata API has immutable SaveFeedback revisions but no persisted remark deletion or split/merge operation. Add those App semantics before claiming a persisted edit/delete witness or its mutation pair. Existing local-vector edits remain backend evidence only. |
| Hooks Muse 1 | Next S3 host lifecycle commit. Pending-publication reconciliation after actor expiry must preserve receipt authorization and all producer/attention ingress checks; this exceeds the App retirement change. |
| Hooks Muse 2 | Next S3 host lifecycle commit. Seal owner transition construction to the machine prepare path together with its production caller. |
| Hooks Muse 3 | Next S3 host lifecycle commit. Add owner-port uncertain-publication fault injection with that caller. |
| Hooks Muse 4 | Next S3 host lifecycle commit. Terminal cleanup must retire producer attachments and runtime slots while retaining live continuation readers; App media retirement does not own these objects. |
| C3 Muse M3 | S3 deferred list. Source orientation remains Upright at capture; needs source-to-display orientation binding. |
| C3 Muse M4 | S3 deferred list. GPU-route pixel digest needs a bounded proof readback. |
| C3 Muse M6 | Before a producer observer gets Viewer. Narrow ListFeedback scope and enforce private ranges. |
| Hooks Muse 5 | S3 deferred list. Guard observe_run's tasks[0] assumption. |
| Hooks Muse 6 | S3 deferred list. Define duplicate live-subject policy. |
| Hooks Muse 7 | S3 deferred list. Route attention prompts to the correct observer/holder. |
| Hooks Muse 8 | S3 deferred list. Bind LiveAttentionResponse.command fully. |
| Hooks Muse 9 | S3 deferred list. Expand continuation readiness probes. |
| Hooks Muse 10 | Native Windows/hosted gate. No local Linux result substitutes for either. |
| Hooks fresh-attempt grants and replacement holder | Next S3 host lifecycle commit. Issue grants for fresh retries and route attention response delivery to the replacement holder. |
| Hooks re-check Minor 3 | Next S3 host lifecycle commit. Private issuer reissue reply-loss continuity. |
| Studio visual leftovers | S3 deferred list. Picture slack, waveform peaks, clip beat labels, open A/B menu and narrow Timeline captures remain open. Delivered labels have their separate contract gap above. |
| C1F3-m1 | Next stage's first commit. Detached Save As/Switch handoff continuity across a replacement client. |
| C2 50 ms wake cost | Efficiency gate if idle CPU flags it. No numeric S3 speed gate is added. |
| C2-A3 | Next S3 acceptance commit. Combined PF1 tamper/damage, uncached mixer seek, real switch after failed copy/lost reply, CPU/GPU route continuity and output-frequency legs. |
| C6 end-to-end | Next S3 acceptance commit after composition. Drive all stages in one App/host/store/media route, including Save As and rescue; existing individual witnesses do not replace this test. |
