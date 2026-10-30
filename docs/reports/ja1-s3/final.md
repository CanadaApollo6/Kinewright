# JA-1 S3 final source report

S3 certification remains **partial**. This ledger is the gate status at the final head. The per-commit reports are the history: [C0](c0.md), [C1](c1.md), [C2](c2.md), [C3](c3.md), [C4](c4.md), [C5](c5.md), [C6a](c6a.md), [C6b](c6b.md), [hooks](hooks.md), [C7](c7.md), [C8](c8.md), [C9](c9.md), [C10](c10.md), [C11](c11.md) and the [lease timeout CI fix](ci-lease-timeout.md). See the [deferred list](deferred.md) for each open item and its owner.

## Final-head gate ledger

**Covered on Linux.** At the final head, the App suite with slow-test features has 959 passed, 0 failed and 4 ignored. Workspace all-target Clippy with warnings denied, formatting and the manifest lint pass.

| Gate | What the tests show |
| --- | --- |
| G7-S | Snapshot save, load and copy. Attached playback of frozen versions through the snapshot lease, and of the Current edit through the current-edit lease, at launch, after edits, First Save, Save As and rescue. `s3_studio_end_to_end` runs Save As and the unknown-reply leg. |
| G8 | `s3_bound_ab_identity` and `s3_bound_ab_gpu_identity` on lavapipe: presentation identity of the bound A/B picture. |
| G9 | Typed and drawn notes on the displayed cell, retained drafts, the immutable batch and its lost reply, and the mixed-version host route in the end-to-end test. |
| G10 | Lineage fixtures, host `ResolveFeedback`, and remapping when the saved checkpoint changes. |
| G11 | Spoken notes through the host, with seeded speech state in the end-to-end test. A note that is still recording, or finished but not yet kept on this computer, refuses Save As, New, Open, switching project and Close. It then finishes under its own project (stage fixes 1 and 2). New feedback work waits while the notes are still handed to a new project (stage fix 3), and kept notes meet only their own project's host (stage fix 4). |

**Remaining acceptance legs.** No test covers these yet:

- C2-A3's combined damage and switch legs: tamper and damage, uncached mixer seek, a real switch after a failed copy or lost reply, CPU/GPU route continuity and output frequency.
- The rescue copy inside the end-to-end test. Separate C1 and C6 rescue tests cover rescue on its own.
- Reopening B after Save As inside the end-to-end test.
- For G8, an out-of-order B completion and a single audio owner in the combined route.

**Equipment-only controls.** CI cannot certify these:

- Live ASR. `s3_studio_end_to_end_live_voice` and `c9_studio_real_spoken_note_edit_and_batch` need the pinned Whisper small model and espeak-ng. CI skips the first, and the second returns early without the model. The first last passed on demand at C10, the second at C9.
- The physical microphone. Never run. The capture tests read a WAV file through FFmpeg.

**Pending CI.** This head is not pushed. Hosted Linux and Windows CI, fast and slow tiers, has not run on it, and the previous head's run had not finished when the stage review checked it. The Windows jobs do not run the Linux-only Studio tests, so they are no native Windows Studio acceptance.

## Stage fix 1

The stage review found that Save As or Close during a spoken recording dropped the recording (M1, data loss). The fix refuses each transition with "Stop the recording first. Your note is still recording." A recording now counts as unsaved work, and the close warning names it. Discard never drops a live recording. If the project's identity changes anyway, the Studio stops the note and keeps it with the project it was recorded in, and only then opens the new project's notes.

Three witnesses hold a real capture worker before it finishes, with FFmpeg reading from a pipe that stays open. Save As is refused and the note finishes under A. Close is refused and the note finishes under A. A Save As that bypasses the App's refusals leaves the capture with A, and reopening A uploads the exact audio, NoteId, anchor and log. Removing the guard turns all three red; removing only the identity-change defence turns the third red; removing only the Close refusal turns the second red. With the guard restored, all three pass.

## Stage fix 2

The re-check found that the guard lifted as soon as the recording finished, even when keeping the finished note failed. If the first write of the note's link to its audio failed, the WAV was left with no NoteId, anchor or log. Save As or Close could then lose that link for good.

The guard now asks whether the note is durably kept, not whether it is recording. A finished note whose write failed refuses the same transitions with "Your note hasn't been saved yet. Retry, or wait for it to save." It counts as unsaved work, and the close warning names it. The Studio retries the write twice a second until it succeeds, and Retry upload retries it at once. A project change waits until the note is kept. Leaving a project whose last write fails now keeps that project and its notes, reports the failure and tries again.

Two witnesses finish a held capture while the notes folder is read-only, try Save As or Close, and then make the folder writable again. Both transitions are refused. The note is then kept under A, and Retry upload saves the exact audio, NoteId, anchor and log. A unit test shows that a failed last write keeps the project and its notes. When the guard ignores the failed write, Save As loses the capture and Close is not refused. When leaving ignores the failed write, the project is not kept. With both restored, all the witnesses pass.

## Stage fix 3

The second re-check found a gap that stage fix 2 opened. When leaving project A fails its last write, the Studio keeps A's notes, but the attachment and the displayed picture are already on B. Record still worked then. Its capture came from B's picture but was kept in A's file, where neither project could restore it.

New feedback work now waits until the attachment and the kept notes name the same project. Until then, Record, New note, drawing, Save, Submit, Revise cut, Start run and choosing a transcript all say "Still saving the last project's notes. Try again in a moment." Record is shown disabled. Record also checks the picture it records at, because that picture's project owns the capture. A version opened before Save As still names the project left, so Record asks for that version to be selected again. Typed and drawn notes on it still work, as before. A spoken note is also kept only in its own project's file, and any other write counts as a failed save, so the note stays guarded. A note A already holds still retries through A, as before. The save retry now runs every frame, so storage that comes back is used while the Editor layout is shown, not only in Studio.

One witness makes A's last write fail as the project moves to B, makes the folder writable again, and presses Record before the next handoff. Record is refused. After the handoff, a note recorded under B is kept in B's file without any acknowledgment from the host. Reopening B restores it, and Retry upload saves the exact audio, NoteId, anchor and log. When Record's checks are removed, or those checks and the file check together, the witness fails because B holds no capture. A second witness makes the folder writable again while the Editor layout is shown, and the note is kept there. When the per-frame retry is removed, it fails. With the fix restored, both pass.

## Stage fix 4

The third re-check found another path through the same gap. A spoken note's acknowledgment from A could be read after Save As to B while A's last write was failing. Attaching it then sent A's picture evidence to B's host and kept references to B's objects in A's file. After that, A could not save the note, and the note's audio and log files had already been deleted.

Fixes 1 to 3 closed one path each, so this fix closes the whole class in one place. Kept notes now meet a host only when both name the same project. This covers every command that runs kept notes against the host or session, and every request the spoken notes make to the host. Otherwise nothing runs, and the caller is told the notes are still being handed over. An acknowledgment that arrives during that time waits unchanged: the note keeps its picture evidence, audio and log, and Save As and Close wait too. Its binding and upload stay in A's file, so reopening A finishes it there. Fix 3's checks are unchanged.

The witness queues A's acknowledgment and moves the project to B. A's last write fails, the folder is made writable again, and the Studio reads the acknowledgment before the next handoff. The acknowledgment waits. Then B's files are deleted and A is reopened. The note is acknowledged by A and saved there with its exact NoteId, anchor, frame proof, strokes, audio and log. When the boundary always lets the projects through, the save from A fails on an object only B has. With the check restored, the witness passes.

## C6a

The rest of this report describes C6a and is kept as it was.

The base is `295619cc`; commit 1 is `ce6e257c6148e0fe98b2b91ae33435b02c73b179`. Both commits stay local on `ja1/s3`, without a push. Source and runtime fixtures are bound separately for each code cut. C6a's three protected pairs cover second-owner save, second-owner submission and frozen-manual trim. Astra 4's persisted remark-edit pair is NOT RUN.

The pinned model was fetched only through `transcript::ensure_model`, via the test-util provisioning adapter. Its SHA-256 is `1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b`. The test-local cache is `target/ja1-s3-c6a/model-data/models/whisper/ggml-small.bin`. The first equipped control revealed a missing `espeak-ng` prerequisite. The unchanged fixture helper at the base has the same executable dependency; a fresh model-equipped parent run is NOT RUN. Task-local packages, checked against the local Arch repository SHA-256 records, supply synthesis without changing system packages or settings. With that prerequisite, the real CPU Whisper spoken-note backend control passes. This earns no microphone, capture-clock, native allocation-accounting or Studio UI credit.

Windows compilation is blocked before touched-crate validation by FFmpeg/Whisper/AWS-LC native prerequisites. Exact base `295619cc` reproduces the native failure, including `pthread_rwlock_t` and missing `errno.h`. Native Windows runtime, G1-W durability and Windows end-to-end success remain NOT RUN.

## Gate scope

The final App suite includes the Studio selection/A-B and typed/drawn feedback witnesses. Project S3 controls repeat input identity, index preservation and old-feedback lineage. Host controls repeat writer/receipt/capability behavior. These are separate witnesses; they do not replace the missing combined App/host/store/media route. The C2-A3 combined damage/seek/switch/audio and CPU/GPU transition legs remain open.

G7-S and G8 have existing App/PF1 evidence plus the focused Current edit trim and rapid-focus checks. G9 has typed/drawn App and immutable-batch backend evidence. G10 has lineage backend evidence. G11 has backend ownership and equipped inference controls. The deferred composition and acceptance legs prevent final closure of these gates. No unreviewed version is marked Delivered.

## NOT RUN

- `s3_studio_end_to_end`, the combined App/host/store/media preservation and unknown-reply route, including the requested combined Save As and rescue-copy legs. (At C10 this runs, without the rescue-copy leg; see above.)
- Studio record/spoken-note controls, editable voice transcript and exact chosen revision in a UI batch; authenticated streaming, private unsubmitted ranges, native model/workspace grants and capture-file residue cleanup/accounting.
- Persisted remark split/merge/re-anchor/delete with collection/reopen and its post-edit mutation pair.
- App S4 request/result, run/event/intent controls, dispositions, holder replacement and same-attempt native continuation integration; hooks follow-ups listed in the deferred table.
- Delivered age/run labels and all three requested screenshots. No screenshot paths are claimed because those UI controls are deferred.
- Native Windows compilation beyond the prerequisite failure, runtime/decoder/handle execution, LockFileEx and G1-W durable writes; hosted CI and independent review rounds; push.
- Physical microphone/audio-device listening, actual displayed-frame/audio-clock synchronization, owned-font rendering, equipped GPU/provider/media-pack acceptance, real provider/secret callbacks and a live S4 specialist.
- Source orientation and GPU-route proof pixel digest; C2-A3's combined acceptance legs; live-app manual-cut capture/first-save refusal; two live GUI windows, a real second launch or lock-holding crash, a stroke during real playback and post-restart evidence-pin acceptance.
- Full workspace fast/slow verification and forced ignores. The touched App/Media suites are recorded separately. Existing manual release/performance and on-demand fixture campaigns are not forced.
- Nonempty ClaimIndex runtime and a Reviewer attachment role; historical entries and exact reasons remain in the [inherited inventory](c6a-evidence/inherited-not-run.json).
- Laptop offload, previously blocked by Tailscale authentication. No new offload attempt was made.

Historical stage omissions are preserved verbatim in the inherited inventory. Only explicitly recorded fresh controls supersede an omission. Backend model controls do not supersede Studio voice UI, physical device or native allocation/residue omissions.

## Final Linux gates

[Final source hashes](c6a-evidence/commit2/source.json), [runtime fixture identities](c6a-evidence/commit2/runtime-fixtures.json), [raw summaries and log hashes](c6a-evidence/commit2/gates.json), [focused control totals](c6a-evidence/commit2/final-extra-totals.json), and [verification](c6a-evidence/commit2/verification.json) bind the final code. Media's [inventory ledger](c6a-evidence/commit2/gate2-media-partitions.json) accounts for all 1,258 unit entries, every integration target and the doctest through bounded serial runs. The initial default-feature monolithic timeout receives no passing-suite credit; an exact-base monolithic timeout comparison is NOT RUN.

| Suite or control | Passed | Failed | Ignored |
| --- | ---: | ---: | ---: |
| App full suite, slow features | 939 | 0 | 3 |
| Media full serial unit inventory, slow features | 1,229 | 0 | 29 |
| Media integration targets, slow features | 33 | 0 | 3 |
| Media doctest | 1 | 0 | 0 |
| Project `test-util s3_` controls | 49 | 0 | 0 |
| Agent `writer_host::` controls | 109 | 0 | 0 |
| Equipped Whisper backend controls | 2 | 0 | 0 |
| Explicit App selection/copy/reopen control | 1 | 0 | 0 |
| Manifest parser self-tests | 44 | 0 | 0 |

Control rows repeat tests from the suite rows and are not added into a grand total. Workspace build, all-target Clippy with warnings denied, formatting and manifest lint pass. The final Windows attempt again stops in the same native prerequisites as the exact base. All three protected pairs' guarded source hashes still match the final code; all archived compressed and raw log hashes verify. No additional protected pair or full C6 App acceptance is claimed.

Task-created temporary entries and the isolated parent source/build were removed. The pre-existing `target/tmp/Kinewright` directory was left untouched, so complete TMPDIR emptying is NOT RUN. The verified test-local model, speech tools and evidence remain under `target/ja1-s3-c6a/`. The final process inventory contains only two pre-existing deleted host executables; neither was signalled. [Cleanup record](c6a-evidence/commit2/cleanup.json).
