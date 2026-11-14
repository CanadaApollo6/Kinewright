# Measured dialogue tools

The tools are built through the real compact dispatcher and remain unserved.
`dialogue_tools::SERVE_DIALOGUE_TOOLS` is false. The same switch controls the
capability catalog, native role manifests/admission, and provider-backed
transcript addresses. The nonempty legacy transcript golden pins the switch-off
bytes. Pure fixtures need neither `SessionStore` nor a bundle and compile for both
systems; Windows execution remains a CI gate. The separate Linux-only CLI correction is `a95f55d4`; the DQ3 review
follow-ups are `2bfdf65c`, already accepted and pushed by the lead.

## Audible-clock ruling

The executed 25-in-24 witness was real. Source 12..38 at 25 fps stores an end of
1,520,000 us, while its 24-frame project portion stops audibly at 1,480,000 us.
The old probe and exact sample-count calculation remain in `target/dq-tools/`.
It was not a PCM render or listening test. The lead resolved this in
`.review/arch/task-dq2-audible-clock.md`.

Core now defines `audible_span(source, asset_rate, project_length, project_rate)`.
It returns reduced exact source microseconds and known padded silence, using
`min(stored_end, source_start + project_duration)`. `ExactMicros` never uses
floats. A rational is `[numerator, denominator]`; an exceptionally large
numerator is a decimal string rather than a lossy JSON number. Equal rates,
25-in-24, 30000/1001-in-30 and padding have independent numeric controls.

`FrameGrid` generalises the existing Core frame-edge projection to an anchored
grid. Source proposals use `Dialogue::frame_edge`; project mapping uses that
same grid implementation. No second rounding formula lives in the Agent.
The audit now takes exact audible spans, guards their fractional edges without
flooring, and lists padding separately from measured quiet. Whole-microsecond
level/word statistics quantise each endpoint within one microsecond; the
safety guard does not. Fractional dip warnings round intrusion up, independently
of the guard. The audit borrows prepared dialogues rather than copying profiles.

The Agent calls the existing public `timeline_audio_segments` and feeds its
source/project ranges to Core. No Media source changed. Preview/export
conformance within one output sample, and the nonseamless mixed-rate split
follow-up, belong to the later Media job. The tools stay off until that gate.

## Provider and edit path

`DialogueMeasurements` selects the audio stream for a content identity and
returns `Arc<BoundDialogue>`. Each immutable value contains SHA-256 + byte length,
audio stream, recogniser/cache-build key, ordered `Word`s in source microseconds,
10-ms peak/RMS/voice hops with true duration, and the guard settings. The real
voice flag comes from earshot on the recogniser's 16-kHz mono signal. Constructor
validation and a streaming digest bind all of them. The provider is installed
through `McpServer::install_dialogue_measurements`; handler clones share the
service slot. One value is pinned per content for a call. The Media job must
supply the stream the renderer plays and build the cache adapter on the Agent
side of the dependency boundary; Media does not depend on Agent.

The address is `(timeline clip, word index)`, with that clip's binding key and
the expected document revision. Stale bindings, absent measurements, wrong
identity/stream, unavailable words and retiming refuse without an edit. Word
ranges address one clip; a batch spans recordings, as transcript deletion does.
The transcript groups consecutive words with one starting index, not a number
on every word. Its provider and switch are the tools' provider and switch.

A batch accepts 1..32 removals. Each resolves on its recording, proposes source
frames and maps toward keeping material on the anchored project grid. Real
operations build a scratch document. Its heard edges must pass the guard, or
move at most three project frames per side toward keeping more material.
Timeline-order construction subtracts the accepted ripple prefix, preserving
previous incoming anchors; a final pass checks every created edge. Minimum
source removal and project duration remain Core's minimum and two frames.
The scratch copies are intentional speculative edit states, not profile copies.

`apply_edit_plan` remains the single consent, scope, history and atomic Core
transaction. The additional observer receives Core's committed snapshot; it
never re-reads the pre-cut document. One Undo restores the complete document.
Other sync-locked tracks follow transcript deletion's isolate/delete/ripple
policy. An explicit overrule bypasses only acoustic/run-together policy and
still requires the existing edit consent, bindings, scope and timing bounds.

Results keep each removal's original `cut` frames under `from_revision`; edge and
join frames name the committed `revision`. Compact edge tuples carry the
recording, exact heard instant, placement and optional dip. Adjacent removals
share the surviving join; its outgoing and incoming guards/overrules belong
to the removals that own those outer sides, independently. Resolved batches
list every removal with its outcome; an acoustic refusal lists its reason and
neighboring quiet point. Revision/identity preflight refusals are request-wide
before resolution. The cut readback uses those same pinned values and lists
only joins created by this batch. The read-only check defaults to flags, retains
omission counts, supports all-detail/range, names unsupported clips and counts
other audible clips without judging their mix.

## Evidence

Logs and mutation proofs stay under `target/dq-tools/`; none is paid evidence.
The 11 dispatcher tests cover exact Undo, pause air, a refused middle removal,
consent, identity/stream/revision and changed bindings, absence, retiming,
mixed-rate guards, both sides' keep-rounding, ten NTSC removals, padding,
60 joins, two recordings and music counts, discovery, and legacy transcript
bytes. The mixed-rate control distinguishes the passing stored end from the
refused heard end, then checks the committed replacement edge.

The original ten-minute 60-join fixture measures 1,003 bytes flags-only and
21,170 bytes all-detail in the Agent wrapper (bounds 2,048 and 32,768). Core's
embedded reports are 93 and 20,260 bytes; the wrapper/provenance adds 910 bytes.
An applied removal is 106 bytes in compact JSON, below the 200-byte target.
For 1,800 words over twelve minutes, one transcript group's complete address
metadata costs 87 bytes: 10 for its starting index and 77 for its binding key.
That synthetic grouped transcript is 79,102 bytes in total; this is address
cost evidence, not a new claim about transcript pagination or output admission.

The five deliberate breaks are run against the completed implementation:

| Pair | Named red assertion | Red / restored green |
| --- | --- | --- |
| ignore-guard | `DQ_TOOLS_ACOUSTIC_GUARD_REQUIRED` | 0/1 at named assertion / 1/0; exact restore |
| remove-side-rounding | `DQ_TOOLS_ROUND_KEEP_OUT` | 0/1 at named assertion / 1/0; exact restore |
| precut-readback | `DQ_TOOLS_FRESH_COMMITTED_READBACK` | 0/1 at named assertion / 1/0; exact restore |
| partial-refused-batch | `DQ_TOOLS_WHOLE_BATCH_REFUSED` | 0/1 at named assertion / 1/0; exact restore |
| stored-end-guard | `DQ_TOOLS_GUARD_AT_AUDIBLE_END` | 0/1 at named assertion / 1/0; exact restore |

The rounding mutation reverses both the proposal rounding and its independent
final keep-side validation, preserving minimum duration. Each script run checks
the named assertion, exact 0/1 red and 1/0 green counts, original source SHA-256,
and a byte-identical tracked diff after restore. Preliminary runs before the
adjacent-edge ownership correction are retained separately, not counted again.

The final workspace build passed (52.83 s wall, including lock wait). The final
workspace fast run exited 0 in 2,450.64 s wall; its 90 summaries total 5,416
passed / 0 failed / 265 ignored, including doctests:

| Crate | Passed | Failed | Ignored |
| --- | ---: | ---: | ---: |
| Agent | 1,717 | 0 | 165 |
| App | 1,082 | 0 | 10 |
| Core | 884 | 0 | 2 |
| Hash | 6 | 0 | 0 |
| Job sys | 11 | 0 | 11 |
| Media | 1,233 | 0 | 73 |
| Project | 483 | 0 | 4 |

All 11 new dispatcher tests passed in 0.12 s. Workspace all-target clippy passed
with `-D warnings`, both default and `te1-eval`; fmt passed, including a separate
rustfmt check of the include-file tests. The actual measurement-pin selector
ran 2 tests, both passed (48.32 s test time). An earlier zero-test selector is
kept under a `zero-selector` log name and is not counted as evidence.

Only `arm_bridge_sha256.mcp_observation` changed in the pin: server SHA-256
`3e941ede35386a22b0722b69eea8a2a6074f92036ce6894638044843128962b9`
→ `5f0e4dd2df3cc3f5788b8bc9d04e4cd394181465d1d3a2ae933a2e8390c370e5`.
A recursive comparison against `2bfdf65c` confirms that single leaf; model-visible
hashes are unchanged. Parser selftests passed 52 and shard selftests passed 23.
The five final pairs took 1,512.32 s wall, including shared-lock waits.

The default-build guard passed on 3 artifacts (115.41 s wall). The platform
script exited 0 in 535.03 s wall. Its three Linux commands exactly match CI:
`cargo build -p kinewright-agent --no-default-features --lib`,
`cargo build -p kinewright-app --no-default-features --bin kinewright-app`, and
`cargo build -p kinewright-agent --no-default-features --bin kinewright`.
All passed. MSVC workspace clippy and Agent slow/App default/App slow clippy
passed with `-D warnings`; the three isolated MSVC shipping **checks** passed.
These are not Windows execution. Feature-disabled builds/checks emitted unused
constant/import warnings in unchanged authentication, certification and task-name
files; the all-target clippy matrices were clean. Both disposable shipping
targets were removed on success (Linux 3.7 GiB, MSVC 907 MiB).

Final slow-test lint passed: 140 slow tests, 160 ignored entries, 27 equipped
skip sites. Final logical target size is 17,130,507,103 bytes (15.95 GiB).
At the no-cargo boundary after the workspace suite, retiring 173 unlinked
executables reduced logical target bytes from 38,685,584,285 to 15,510,379,885;
evidence logs, diagnostic patches, libraries and fixture-host links were kept.

## Handoff and boundaries

The Media job supplies the bound cache values above, installs the provider,
proves renderer conformance and flips the one serving switch. This does not
change the existing Media transcript contract. A future speaker field would be
needed to suppress repeats on authenticated different-speaker evidence; current
Core words carry no speaker and recordings are not guessed to be speakers.

`get_dialogue_pacing` / `pacing.rs` are untouched. The new check supersedes their
short/long gap classification and transcript-bound/merged-silence gap estimate
when served. It adds measured edge-in-sound, independent short-air, room-tone
and dip warnings, plus repeat and sounding-hole evidence. The older speaker,
punctuation and capitalization boundary hints are not new acoustic evidence;
authenticated speaker-aware repeat suppression remains a follow-up.

Additional explicit refusals: the cut planner requires an audible, enabled
media clip without an animated enable curve, and disjoint selections on one
dialogue track. The checker handles audible segment lists, lists retimed or
overlapping dialogue clips as not checked, and does not judge a mixture.

NOT RUN: real recogniser/profile extraction, renderer conformance, PCM/listening,
slow-tier execution, Windows execution, provider/paid/dry/interactive-app
launches. No push or subagents.
