# Readers of the old approval target-satisfaction shortcut

At base `1e2c4170`, the old audit can call approval A kept when A was discarded and another command B removed the same target. H2a part 2 follows command fate instead, per the user's ruling. No second approval-satisfaction answer is introduced. The differential recognizes only that precise cause and counts it. No reader below is migrated in this part.

| Reader | What it does with the shortcut answer | H2b decision |
| --- | --- | --- |
| `crates/kinewright-project/src/edit_audit.rs:595`, `:641` | A Kept proof sets `EditApproval.applied = true` and suppresses missing/uncertain entries. The shortcut originates at `:383-388` and the live-cut override at `:575-580`. | Replace proof interpretation with the command's ledger fate; decide separately if final-document checks are needed. |
| `crates/kinewright-agent/src/writer_host/task_consent.rs:315` | Combines durable audit and current live actor cuts into the host's approval summary. | Select command fate as the summary contract or add an explicit final-document query in H2b. |
| `crates/kinewright-agent/src/writer_host/orchestration.rs:1386-1390` | Copies approvals and missing/uncertain lists into serialized `RunViewPage`. | Define the new wire answer and disclosures. |
| `crates/kinewright-app/src/studio_run_view.rs:27`, `:208-218` | Converts applied approvals into “Removed … in the draft version” lines; sums removal counts when more than five are reported. Missing/uncertain lists suppress those claims and generate action text at `:243-249`. | Choose whether displayed claims describe individual commands or final document contents. |
| `crates/kinewright-app/src/studio_feedback.rs:3482` | Renders each formatted approval line from `RunView`. | Move to the selected H2b display contract. |
| `crates/kinewright-project/src/editing_session/retained_work.rs:145-152`, `:326-337` | Groups missing/uncertain approvals by task and prefixes retained-work recovery disclosures with those warnings. A shortcut Kept answer suppresses both warnings. | Choose command-fate or final-document recovery guidance explicitly. |

`EditApproval` is also re-exported by the host and serialized as part of its page. Searches found no further production consumer of its `applied` bit or missing/uncertain lists. Approval-reference vectors in run projections are different data: they bound and retain consent records, not the shortcut's answer. Test-only page constructors and assertions are excluded from this reader inventory.

The retained-work disclosure is also rendered at `crates/kinewright-app/src/ja1_attachment.rs:1937` (standalone recovery UI) and `crates/kinewright-app/src/ja1_host.rs:1323` (host recovery UI). Both display the composed text before offering retained-work actions; neither recomputes approval truth.
