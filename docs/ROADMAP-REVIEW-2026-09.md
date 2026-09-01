# Roadmap review — September 2026

Status: post-CC7 review of the operating plan, the codebase, and the evidence
backlog, written against `main` at `bbf8969`. The roadmap asks for this review
after each completed vertical slice. This one covers the run from CC0 through
CC7 (with M41 and M42 in between), which landed between 2026-08-24 and
2026-08-27.

## What is in the tree

| Fact | Value |
| --- | --- |
| Slices committed since the roadmap landed | CC0, M41, CC1, M42, CC2, CC3, CC4, CC5, CC6, CC7 |
| Rust lines at `d09ab6c` (roadmap commit) | 85,480 |
| Rust lines at `bbf8969` (CC7) | 232,476 |
| Test functions (`#[test]` and `#[tokio::test]`) | ≈1,760 |
| Release tags | none |
| CHANGELOG sections | one, `[Unreleased]` |
| Workspace on a fresh Ubuntu box (Rust 1.94.1, pinned FFmpeg 8) | build 3 m 39 s; `fmt --check` clean; 1,710 tests passed, 0 failed, 20 ignored, about 4 min wall |
| Clippy | green on CI's current stable; on Rust 1.94.1 one `manual_range_contains` hit in `color_qc.rs:1507` fails `-D warnings` |

The roadmap's colour table (CC0–CC7) is complete on paper. There is no CC8 in
the repository or in the plan; the colour programme's own closing line says HDR,
RAW, ACES/OCIO, calibrated output, and temporal noise reduction are deliberate
later programmes, not the next slice.

## The evidence backlog is the critical path

Every slice from CC3 onward is recorded as "implemented, pending platform
smoke". The definition of done requires hands-on validation on Windows and
Omarchy. That gate has now been deferred five slices in a row, and the queue of
things only a person can close has grown faster than the code:

- CC3, CC4, CC5, CC6, CC7: hands-on smoke of the wheels/curves widgets, look
  browser, matte overlay, Colour QC window, and export verification block.
- CC6 P11: every delivery budget is a Linux measurement until the Windows CI
  job has run the `yuv420p10le` lane on the MSVC FFmpeg package.
- CC7: the `color-workflow-v6` real-harness run (three samples per scenario)
  and the blind review form, both owed before CC7 can be called complete.
- M40: no family has met its exit gate. Event/multicam has one human accept
  with no numeric ratings; interview's caption-recovery SHA and montage's V10
  SHA both await review.
- M39: still "not complete" for want of a SHA-bound human rubric.
- EVALS baseline snapshot: the latest live seed-suite run is recorded as FAIL
  on `e7 flagship rough cut` (22/24).

Agents can generate a slice in a day. The human gate on that slice takes an
evening on a real machine, and it has not been happening. Until it does, the
last four days of colour work are unverified on the two platforms the project
says it supports.

## Answer 1: what to do next

In this order.

### 1. Close the owed gates, then cut v0.1.0

The release workflow exists and has never been exercised. Cutting the first
tag forces the Windows and Omarchy smoke that CC3–CC7 already owe, produces the
first installable artifact the README promises, and gives the changelog its
first real section.

Pair it with the eval actions that need a person: the v6 real-harness run, the
blind review, numeric ratings for the accepted event/multicam sample, and the
two pending M40 SHAs. These are hours of human time and unblock every "pending"
line in the docs.

### 2. One hardening cycle, bounded to two weeks, with an exit list

The workspace grew 2.7× in one week. The invariants held (see below), but the
seams did not. Concrete list, each item a measurable exit:

- **`test-util` ships in the release binary.** `crates/kinewright-agent/Cargo.toml`
  enables `kinewright-media`'s `test-util` feature unconditionally so that the
  `kinewright-eval` binary can use `cc7_sources` and `test_support`. Feature
  unification then compiles `test_support` (which asserts on a missing FFmpeg
  CLI) and `cc7_sources` into `kinewright-app`. Gate the eval binary behind its
  own feature, or move the generators into a non-test module. Exit: the app's
  release build has no `test_support` symbols.
- **`server.rs` at 23,799 lines.** One `impl KinewrightMcp` block with 93
  methods spans lines 594 to 8,421, a single `match` dispatches every tool, and
  13,000 lines of tests sit in the same file. The tool families are already
  comment-delimited by slice. Split by family into submodules and move the
  tests to `tests/`. Exit: no source file over 5,000 lines in the agent crate.
- **CI has no cache and no timeout.** `ci.yml` builds, tests, and runs
  `clippy --all-targets` on both operating systems with FFmpeg provisioned per
  run and no `actions/cache`. CC7 §14 itself notes the failure mode is "slow
  rather than red". Exit: a recorded wall-clock number per job, a
  `timeout-minutes`, and the media fixture lane split so a GPU flake does not
  hide behind a 40-minute job.
- **Fixture duplication.** About twenty helper names (`cpu_reference_monitor`,
  `spec_grade709_decode_f64`, `pixel_centre_uv`, and so on) are reimplemented
  across `cc1_fixtures.rs` through `cc7_fixtures.rs`. Some duplication is
  contractually required (expected values may not be derived from production
  code); the helpers are not. Exit: one shared fixture module, each name
  defined once.
- **The teardown segfault.** CC7 F-E6 `#[ignore]`s a fixture because
  `FfmpegMediaEngine`'s process-exit teardown raises a SIGSEGV in two of three
  runs. The workspace forbids `unsafe`, so the fault is in the FFmpeg binding's
  drop order, and it is the same drop order the desktop app runs on quit. Root
  cause it. Exit: the ignored fixture runs.
- **Typed `stale_revision` on all seven colour planners.** CC7 D-E2 records
  four of them returning prose. Exit: one envelope, seven tools.
- **Two parallel registries.** `INSPECTOR_TOOL_NAMES: [&str; 75]` is
  hand-maintained beside the schemars-generated schema, and the inspector still
  has `match name` label tables next to the descriptor-driven path. Exit: one
  source of truth for each.
- Report the libswscale `bilinear + full_chroma_int` defect upstream (CC6 P7).
- **Pin the toolchain CI actually lints with.** `rust-version = "1.92"` promises
  a floor, there is no `rust-toolchain.toml`, and `clippy -D warnings` only
  passes on the newest stable. Either add the pin or drop the floor claim.

### 3. Then the next capability slice: audio delivery

The roadmap says non-colour slices take the primary lane once colour is
complete, and it says to pick by "the bottleneck observed in real edits". The
one M40 rejection with a purely technical cause was audio: the event/multicam
baseline shipped at −39.9 LUFS. Loudness measurement already exists in
`kinewright-media/src/loudness.rs` and the eval runner, and the app has no
audio UI file and zero LUFS references. The person cannot see what the agent
measures.

Shape it as CC6's audio twin: meters and a mixer/bus panel in the app, a
loudness target on the delivery profile, loudness normalization as a typed
operation, audio verification in the same decoded-output pass CC6 built for
picture, and a `get_audio_qc` tool alongside `get_color_qc`. M34 and CC6 both
deferred loudness-aware delivery; the scaffolding it needs now exists.

The second non-colour slice after that should be the keyframe editor. CC5
deferred manual keyframe authoring, the motion row of the portfolio needs it
for speed ramps, and every colour node now has animatable parameters with no
timeline affordance to edit them.

## Answer 2: roadmap changes

Four edits to `ROADMAP-AND-WORKFLOWS.md`:

1. **Add a gate-debt rule next to the two operating rules.** "A new primary
   slice does not start while more than one completed slice is still awaiting
   its hands-on platform gate." This is the rule the last week broke, and
   nothing in the document currently stops it.
2. **Add a release gate to the scorecard and the near-term sequence.** The
   scorecard measures time to first cut and parity error rates. It does not
   measure whether anyone can install the program. Add "installable release
   published for both platforms" and name v0.1.0.
3. **Give hardening cycles a shape.** The document forbids eval-only cycles but
   only allows "one bounded reliability improvement" per cycle. A workspace
   that grows 2.7× in a week needs a cycle type whose primary deliverable is a
   measured reduction (file size, CI time, duplicate definitions), with the
   same exit-gate discipline the capability slices have.
4. **Make the near-term sequence forward-looking again.** Items 1–10 are now a
   history log; move them to a "Completed" section. Update "Current foundation
   and limits", which still says there is no hue control and that file-backed
   LUTs "intentionally lack a human file-picker workflow"; CC3 and CC4 changed
   both.

The outcome, the ownership table, and the definition of done can stay as
written.

## Answer 3: the state of the codebase

**Invariants.** Zero `unsafe`. Every mutation still goes through one
`Operation` path and one `Core` actor. One render path. Effects are now
table-driven through `EFFECT_DESCRIPTORS`, which the August audit asked for.
The MediaEngine facet split (R3) landed. `sha256_file` exists once. CI enforces
`fmt` and `clippy -D warnings`. The core crate is pure, small, and fast to
test. The numeric-gate discipline paid for itself three times in one week:
CC6's decoded-output check found that every H.264 export dropped its last
frame, the delivery intermediate was quantized on the wrong white, and CC7's
probe caught an 8-bit constant written against a 16-bit field before it could
make one gate pass on everything.

**Structure.** Production panic surface is tiny (eleven `unwrap`/`expect`
calls in `server.rs` before its test module), but the file it lives in is a
monolith. Most tests are inline, which is why nine files exceed 5,000 lines
and why `kinewright-media` has 500 tests in `src/` and 16 in `tests/`. The
contract documents run to 1,400–1,600 lines per slice and are written for the
agents that implemented them; there is no user documentation and the only
onboarding text for a contributor is `BUILDING.md`. The fixture programme is
almost entirely synthetic rasters at 320×180; the project's own risk register
says every CC7 budget is a Linux measurement.

**The lopsided product.** The colour pipeline would satisfy a working
colourist: managed working space, wheels, curves, hashed LUT assets, node
mattes with tracking, Y′CbCr legality, decoded-delivery verification at two bit
depths. It sits on an editor with no audio meter, no keyframe editor, no proxy
generation, and a source monitor that cannot play. The competitive audit's
one-star rows (audio, speed, export breadth) are unchanged since August.

**The honest grade.** As an engineering artifact the core and media layers are
good, and in places excellent. As a product it is where PRODUCT-POSITION left
it: proof of the thesis, not yet a replacement for anything. Three weeks and
147,000 lines moved that second number less than the diff suggests, because
the person path of five slices has not been run on a real desktop. The
throughput problem is not the agents. It is that the human gate is the only
non-parallel step, and it is saturated.
