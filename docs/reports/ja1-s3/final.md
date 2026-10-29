# JA-1 S3 final source report

S3 certification remains **partial**. This commit pair fixes the focused C6a data paths and records final-source checks. It does not close the missing Studio voice/scheduler composition or `s3_studio_end_to_end`. See the [deferred list](deferred.md) for each item, reason and destination.

The base is `295619cc`; commit 1 is `ce6e257c6148e0fe98b2b91ae33435b02c73b179`. Both commits stay local on `ja1/s3`, without a push. Source and runtime fixtures are bound separately for each code cut. C6a's three protected pairs cover second-owner save, second-owner submission and frozen-manual trim. Astra 4's persisted remark-edit pair is NOT RUN.

The pinned model was fetched only through `transcript::ensure_model`, via the test-util provisioning adapter. Its SHA-256 is `1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b`. The test-local cache is `target/ja1-s3-c6a/model-data/models/whisper/ggml-small.bin`. The first equipped control revealed a missing `espeak-ng` prerequisite. The unchanged fixture helper at the base has the same executable dependency; a fresh model-equipped parent run is NOT RUN. Task-local packages, checked against the local Arch repository SHA-256 records, supply synthesis without changing system packages or settings. With that prerequisite, the real CPU Whisper spoken-note backend control passes. This earns no microphone, capture-clock, native allocation-accounting or Studio UI credit.

Windows compilation is blocked before touched-crate validation by FFmpeg/Whisper/AWS-LC native prerequisites. Exact base `295619cc` reproduces the native failure, including `pthread_rwlock_t` and missing `errno.h`. Native Windows runtime, G1-W durability and Windows end-to-end success remain NOT RUN.

## Gate scope

The final App suite includes the Studio selection/A-B and typed/drawn feedback witnesses. Project S3 controls repeat input identity, index preservation and old-feedback lineage. Host controls repeat writer/receipt/capability behavior. These are separate witnesses; they do not replace the missing combined App/host/store/media route. The C2-A3 combined damage/seek/switch/audio and CPU/GPU transition legs remain open.

G7-S and G8 have existing App/PF1 evidence plus the focused Current edit trim and rapid-focus checks. G9 has typed/drawn App and immutable-batch backend evidence. G10 has lineage backend evidence. G11 has backend ownership and equipped inference controls. The deferred composition and acceptance legs prevent final closure of these gates. No unreviewed version is marked Delivered.

## NOT RUN

- `s3_studio_end_to_end`, the combined App/host/store/media preservation and unknown-reply route, including the requested combined Save As and rescue-copy legs.
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
