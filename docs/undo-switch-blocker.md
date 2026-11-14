# H2b merge-budget conflict

The switch is paused before product callers move. Decision 6 of the H2b brief requires a delta Core's growth bound to fit its merge path; the earlier performance ruling requires room for at least 2,000 ordinary task commands. The accepted Task and Candidate profiles do not satisfy both requirements.

The portable diagnostic creates a one-track document, accepts 2,000 alternating `SetTrackSyncLock` commands in a Task owner, and forwards its complete applied delta as one batch to an empty Candidate owner. This is the shape used by `editing_session/durable.rs` in `preview_candidate_merge`: it forwards `frozen.delta` without compaction. Editorial task scopes can permit the operation and the `track.sync_lock` field (`orchestration/policy.rs`).

| Measurement | Result |
| --- | ---: |
| Accepted Task commands / steps | 2,000 |
| Independently encoded operation bytes | 95,000 |
| Task charged encoded / decoded bytes | 95,000 / 1,024,000 |
| Candidate encoded / decoded caps | 65,536 / 524,288 |
| Candidate execution | `Err(History(StepTooLarge))` |

The encoded requirement alone exceeds Candidate's cap. The Task decoded figure includes its separate command/step overheads and is not presented as the merged batch's decoded charge. The diagnostic also prints the legacy actor's live estimate; it exceeds the old Candidate cap, so this is not a claim that the old native merger accepted this case.

The failing assertion is `ADMITTED_TASK_DELTA_MUST_FIT_CANDIDATE_MERGE_PATH`. Both profiles are unchanged. Raising Candidate's component limits, rebudgeting its fixed 2 MiB retained envelope, lowering Task's admitted growth, or introducing delta compaction needs a ruling against the existing constraints before implementation proceeds.

The source is [the diagnostic](measurements/undo-switch-task-candidate-probe.rs). To reproduce from the repository root, copy it temporarily to `crates/kinewright-project/examples/hx_task_candidate_probe.rs`, source `scripts/setup-ffmpeg.sh`, and run:

```sh
flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo run -p kinewright-project --example hx_task_candidate_probe
```

Remove the temporary example afterward. The captured output is under `target/hx-h2b/task-candidate-probe.log` in this worktree. No product switch, shipping checkpoint, product mutation run or product-path measurement has been performed.
