# OpenReel — Architecture & Build Path

> A native Windows and Linux video editor written in Rust that is, at its core, an **agentic harness for video editing**. The agent brain is whatever coding-agent CLI the user already pays for — Claude Code, Codex, Cursor's `cursor-agent`, OpenCode — spawned as a subprocess, exactly the way T3 Code drives coding agents. OpenReel's job is to hand that agent *eyes and hands for video*: its tools are the same edit operations the human uses. Think "T3 Code, but the tools are cuts, trims, effects, and exports."

**Not a video generation app.** Models never generate footage here. They edit footage the user shot. That's the difference between this and most "AI video" products, and it's load-bearing: the whole design assumes the source of truth is the user's media plus an edit log, not model output.

**Fully open source, GPLv3, free forever.** No accounts, no server, no keys shipped. The only thing that costs money is the agent subscription the user already has.

This doc is written to be handed to a Fable 5 orchestrator + a fleet of implementation agents. It defines the module boundaries, the contracts between them, the concurrency model, and a milestone build path. The contracts are the important part: if the agents agree on the message and trait boundaries, they can build the four crates in parallel without stepping on each other.

---

## 1. The one architectural idea everything hangs on

**Every mutation to the project is an `Operation`. The GUI emits Operations. The agent emits Operations. Nothing else touches the document.**

This is the keystone. It buys you, for free:

- **Undo/redo** — the operation is the undo unit.
- **Human/agent parity** — the agent literally cannot do anything the human can't, and vice versa. No divergent code paths.
- **The AI tool schema** — the agent's tool definitions are *generated from the Operation set*. One source of truth.
- **Determinism & replay** — a project is `initial_state + ordered_operation_log`. Great for crash recovery, tests, and debugging what an agent did.

Two corollaries that keep the keystone honest:

- **Operations are pure state mutations.** No I/O inside `apply` — no file probing, no decoding, no network. Anything that needs I/O (importing media, say) does the I/O *first*, then applies an op carrying the results. Otherwise replaying the op log re-does I/O and can fail differently, and the determinism claim dies.
- **`apply` validates invariants.** Clips on a track are non-overlapping and sorted; every clip's `asset` exists in the media pool; deleting an asset with live clip references is rejected (or cascades explicitly). Invariants are enforced in one place — inside apply — not assumed.

If an agent ever proposes "let the chat panel mutate the timeline directly," reject it. Everything funnels through the Operation bus.

The second keystone: **a single Core actor owns the document.** It is the only thing that holds the mutable project state. GUI and agent are both just *clients* that send Commands to it and subscribe to its Events. This removes shared-mutable-state footguns entirely, which is exactly what you want when the code is being written by many agents that don't share a mental model.

```
        Commands (ops + queries)                Events (state diffs, errors)
GUI  ───────────────────────────►  ┌─────────┐  ─────────────────────────►  GUI
                                    │  CORE   │
Agent ──────────────────────────►  │ (actor) │  ─────────────────────────►  Agent
                                    └─────────┘
                                    owns Document
                                    owns undo/redo stack
                                    processes Commands serially
```

---

## 2. Workspace layout (a clean dependency DAG)

A Cargo workspace of four crates. The dependency graph is a DAG rooted at `core`, which is what makes parallel agent work safe.

```
openreel/
├── Cargo.toml                # workspace root — PINS ALL SHARED VERSIONS here
├── crates/
│   ├── openreel-core/        # document model, Operation set, Core actor, undo, serde
│   ├── openreel-media/       # ffmpeg decode/encode, frame cache, wgpu compositor, audio out, export
│   ├── openreel-agent/       # MCP server (tools), CLI harness drivers (Claude Code/Codex/...), session mgmt
│   └── openreel-app/         # eframe/egui binary — wires it all together, the UI
```

- **`openreel-core`** — *No ffmpeg. No GPU. No network. No egui.* Pure logic and types. Fully unit-testable in isolation. Defines `Document`, `Operation`, `Command`, `Event`, the `MediaEngine` and `AgentDriver` traits (as contracts), undo/redo, and project (de)serialization. **This is the keystone crate — build its types first; everyone depends on them.**
- **`openreel-media`** — implements the `MediaEngine` trait using `ffmpeg-next` + `wgpu` + `cpal`. Decode, frame cache, compositing, audio playback, export.
- **`openreel-agent`** — the harness: an MCP server exposing the Operation set + inspector tools, plus per-CLI drivers that spawn and speak to Claude Code / Codex / cursor-agent / OpenCode subprocesses.
- **`openreel-app`** — the `eframe` binary. Timeline widget, preview panel, media bin, inspector, agent chat panel. Thin view over `Document`; dispatches Operations.

**Slice for the swarm:** M0 defines all of `core`'s public types and the two trait contracts. Once those compile, `media`, `agent`, and `app` can be built by three independent agent teams against stubbed implementations of the traits.

---

## 3. Core contracts (the interfaces agents build against)

These are sketches, not final code — but the *shape* is the contract. Nail these in M0.

### Document model (`openreel-core`)

```rust
pub struct Document {
    pub tracks: Vec<Track>,        // video + audio tracks, ordered z-bottom → top
    pub media_pool: Vec<MediaAsset>, // imported source files (probed metadata, incl. per-asset time base)
    pub fps: Rational,
    pub resolution: (u32, u32),
    pub duration: TimeCode,
}

pub struct Track {
    pub id: TrackId,
    pub kind: TrackKind,           // Video | Audio
    pub clips: Vec<Clip>,          // INVARIANT: non-overlapping, sorted by start — enforced by apply()
}

pub struct Clip {
    pub id: ClipId,
    pub asset: AssetId,
    pub source_range: Range<TimeCode>,   // in/out within the source, in SOURCE time base
    pub timeline_start: TimeCode,        // where it lives on the track, in PROJECT time base
    pub effects: Vec<Effect>,
    pub transition_in: Option<Transition>,
}
```

Keep `Document` `Clone` + `serde::Serialize/Deserialize`. Time is a `TimeCode` newtype over integer frames — **never floats**, agents will introduce drift bugs. Source clips whose fps differs from the project's need an explicit, tested mapping (per-asset `Rational` time base → project frames); this is a named responsibility of `core`, not something to improvise at call sites.

### The Operation set (the heart)

```rust
#[derive(Serialize, Deserialize, Clone, JsonSchema)]  // schemars derives the AI tool schema
pub enum Operation {
    AddAsset { asset: MediaAsset },   // pure: probing happened BEFORE this op was built
    AddClip { track: TrackId, asset: AssetId, at: TimeCode, source: Range<TimeCode> },
    SplitClip { clip: ClipId, at: TimeCode },
    TrimClip { clip: ClipId, new_source: Range<TimeCode> },
    MoveClip { clip: ClipId, to_track: TrackId, to: TimeCode },
    DeleteClip { clip: ClipId },
    AddEffect { clip: ClipId, effect: Effect },
    SetEffectParam { clip: ClipId, effect: EffectId, key: String, value: ParamValue },
    AddTransition { clip: ClipId, transition: Transition },
    // ...grows over time; each variant is one atomic, validated edit
}

pub trait ApplyOp {
    /// Validate + apply to the document. Pure: no I/O, no side effects beyond `doc`.
    fn apply(&self, doc: &mut Document) -> Result<(), OpError>;
}
```

`#[derive(JsonSchema)]` (via `schemars`) is deliberate: the agent's tool definitions are generated straight off this enum and served through the MCP server. Add a variant → the agent can use it. No second registry to keep in sync.

Note what's *not* here: `ImportMedia { path }`. Import is a two-step: the media engine probes the file (I/O, can fail, can be slow), and only then does an `AddAsset` op — carrying the already-probed metadata — enter the log. Replay never touches the filesystem.

### Undo: snapshots, not inverse operations

Undo is **snapshot-based**: the undo stack holds `Arc<Document>` states, one per applied op (plus the op itself, for labeling and the replay log). `Document` is small metadata — no media bytes — so cloning it per edit is near-free, and snapshot undo is *unbreakable by construction*.

The tempting alternative — `apply` returns an inverse op — is rejected deliberately. With a growing op set built by many agents, hand-written inverses are a factory for subtle corruption bugs that only appear when ops are undone in particular sequences. Snapshots cannot have that bug class. The op log is still kept, append-only, for crash recovery and "what did the agent do" auditing; it's just not the undo mechanism.

### Core actor messages

```rust
pub enum Command {
    Do(Operation),          // validate + apply + push snapshot onto undo stack
    Undo,
    Redo,
    Query(Query),           // read-only: get timeline, get clip, etc.
}

pub enum Event {
    DocumentChanged { doc: Arc<Document>, last_op: Option<Operation> },
    OpRejected { op: Operation, error: OpError },
    QueryResult(QueryResult),
}
```

The Core loop is a `while let Ok(cmd) = rx.recv()` that mutates the document serially and broadcasts `Event`s. GUI and agent both `subscribe()` to the event stream. Simple, testable, deterministic.

### Media engine trait (`core` defines it, `media` implements it)

Playback frames are **requested, not returned** — a synchronous `frame_at()` call is exactly the "UI blocks on decode" bug §4 forbids, dressed up as an API. The engine owns a prefetching frame cache and answers asynchronously.

```rust
pub trait MediaEngine: Send + Sync {
    /// I/O: probe a source file. Happens BEFORE an AddAsset op is constructed.
    fn probe(&self, path: &Path) -> Result<MediaAsset, MediaError>;

    /// Inform the engine the document changed (it owns caches keyed off this).
    fn set_document(&self, doc: Arc<Document>);

    /// Request a composited frame at `t`. Non-blocking; the frame arrives on the
    /// frames() channel when ready. The engine prefetches around the playhead.
    fn request_frame(&self, t: TimeCode);
    fn frames(&self) -> Receiver<(TimeCode, FrameTexture)>;

    /// Playback transport. The engine owns the clock — which is the AUDIO clock
    /// (the cpal output callback); video chases it. Position ticks arrive as events.
    fn play(&self, from: TimeCode);
    fn pause(&self);

    /// For the agent's *vision*: a CPU RGBA image at a timecode. Blocking is fine
    /// here — it's called from the agent path, never the UI thread.
    fn thumbnail_at(&self, t: TimeCode, max_w: u32) -> Result<RgbaImage, MediaError>;

    fn export(&self, out: &Path, settings: ExportSettings, progress: ProgressSink) -> Result<(), MediaError>;
}
```

**One render path.** Preview and export use the *same* wgpu compositor: export = composite each frame on the GPU → read back → feed the encoder. ffmpeg is decode/encode only; effects and transitions are wgpu shaders. This is non-negotiable — a second effects implementation in ffmpeg filters means exports that don't match the preview, which users experience as "the app lied to me."

### Agent driver trait (`core` defines it, `agent` implements it)

```rust
/// A driver speaks to ONE agent CLI installed on the user's machine.
pub trait AgentDriver: Send + Sync {
    fn id(&self) -> HarnessId;                       // "claude-code", "codex", "cursor", "opencode"
    fn detect(&self) -> Option<HarnessInfo>;         // is the CLI installed + authenticated?
    /// Spawn a session: subprocess + structured stdio protocol + our MCP server attached.
    fn start_session(&self, cfg: SessionConfig) -> Result<Box<dyn AgentSession>, AgentError>;
}

pub trait AgentSession: Send {
    fn send_user_message(&mut self, text: String) -> Result<(), AgentError>;
    fn events(&self) -> Receiver<AgentEvent>;        // streamed text, tool calls, results, cost, done
    fn interrupt(&mut self);
}
```

---

## 4. Concurrency & threading model

A video editor lives or dies on keeping the UI thread free. Spell this out for the agents explicitly or they'll block the render loop on decode.

| Thread / task | Owns | Talks via |
|---|---|---|
| **UI thread** | egui render loop, view state | sends `Command`, receives `Event` |
| **Core actor** | the `Document`, undo stack | inbound `Command` channel, outbound `Event` broadcast |
| **Media workers** (pool) | ffmpeg decode, frame cache fill | request/response channels |
| **Compositor** | wgpu device/queue (shared with egui's) | called from playback, hands back `FrameTexture` |
| **Audio output** | cpal stream + callback — **this is the master playback clock** | lock-free ring buffer in; position ticks out |
| **Agent task** (tokio) | CLI subprocess I/O, MCP server, session state | sends `Command::Do(op)`, subscribes to `Event` |

Rules to hand the agents:

- **The UI thread never blocks on decode, network, or subprocess I/O.** Everything heavy is a message round-trip.
- Prefer message-passing over shared `Arc<RwLock<Document>>`. The Core actor owning the document is the whole point — it serializes writes and removes lock reasoning.
- The GUI renders from an `Arc<Document>` snapshot it receives in `DocumentChanged`. Cheap to clone the Arc; the doc itself is only cloned inside Core when snapshotting for undo.
- **Share one wgpu device** between egui and the compositor. eframe exposes its `wgpu::Device`/`Queue` via the render state — the compositor should reuse it, not spin up a second instance.
- **The audio callback is realtime code.** No allocation, no locks, no channel sends that can block — it pulls from a pre-filled ring buffer and publishes its position atomically. Video presentation chases that position.

---

## 5. The AI harness (the part that makes it OpenReel)

This is the actual product. The design follows the T3 Code model, verified against how it actually works: **do not implement model API clients, do not handle credentials.** The user already has an agent CLI installed and authenticated on their machine — Claude Code, Codex, `cursor-agent`, OpenCode — riding their existing subscription. OpenReel spawns that CLI as a subprocess and speaks its structured stdio protocol (e.g. Claude Code's `--input-format stream-json --output-format stream-json`; Codex and the others have equivalents). The CLI does the model calls, the auth, the token refresh, the context management. OpenReel never sees a key.

This buys us, for free: subscription billing that is unambiguously the provider's own supported path, streaming, model selection, and every capability upgrade those CLIs ship. The cost is a per-CLI driver (`AgentDriver` impl) that knows how to spawn it, attach tools, and parse its event stream — small, isolated, testable code.

**Tools are served over MCP.** `openreel-agent` runs an MCP server (the official Rust SDK, `rmcp`) exposing two tool families, and every driver attaches it to its CLI session (all four CLIs speak MCP):

1. **Mutators** — auto-generated from the `Operation` enum via its `JsonSchema` derive. `split_clip`, `add_effect`, etc. Each call becomes `Command::Do(op)` to Core; the op outcome (applied, or rejected with the validation error) is the tool result.
2. **Inspectors** — read-only:
   - `get_timeline_state` / `get_clip_info` — a compact, token-efficient text rendering of the document. Design this representation deliberately; it's most of the agent's context.
   - `get_frame_at(timecode)` — returns an actual image (MCP supports image content) via `MediaEngine::thumbnail_at`, downscaled hard: the agent needs *eyes*, not 4K. "Cut on the action" and "trim the dead air at the start" are impossible from metadata alone.
   - `get_transcript(range)` — **the killer inspector** (post-MVP, see M6): a local Whisper pass at import gives word-level timestamps. "Remove the ums," "cut everything before I say hello," "tighten the pauses" become precise, cheap text operations instead of expensive frame-scrubbing. Transcript-based editing is the highest-leverage agentic edit primitive there is; the architecture reserves the slot now.

**The loop:** user message in chat panel → driver forwards to the CLI subprocess → the CLI's agent reasons and emits MCP tool calls → mutators become `Command::Do(op)`, inspectors read state → results stream back → the chat panel renders the CLI's event stream (text, tool calls, cost) → done when the CLI yields the turn.

**Because ops flow through the same Core as the GUI:** the agent's edits land on the same undo stack. The human can Ctrl-Z an agent's cut. The agent can read state the human just changed. Parity is automatic — that's the payoff of keystone #1.

**Guardrails (don't skip these):**
- Destructive/hard-to-reverse ops (delete asset, export-overwrite) surface a confirmation in the chat panel before executing: the MCP tool call blocks pending the human's click. The agent *proposes*; the human *confirms* the sharp ones.
- Restrict the CLI session to OpenReel's MCP tools — the video-editing agent has no business running Bash or editing files. Every CLI has a permission/allowlist mechanism; drivers must configure it.
- Surface the CLI's own cost/token reporting per turn in the panel. Cap turns per request.
- **Harness detection UX:** on first run, detect which CLIs are installed and authenticated (`AgentDriver::detect`); if none, point the user at install docs rather than failing cryptically. Note: Cursor's programmatic surface is the `cursor-agent` CLI specifically — verify its MCP + permission story during M3 before promising it in the README.

---

## 6. Build path (milestones, each demoable)

Sequenced to kill the scariest unknowns first — the ffmpeg + wgpu pipeline — since that's the least-charted territory. Each milestone has a hard done-criterion.

### M0 — Skeleton & contracts *(do this alone, not parallelized)*
Workspace compiles. `core` public types exist: `Document`, `Operation` (a few variants), `Command`/`Event`, the `MediaEngine` and `AgentDriver` traits as stubs. Snapshot undo implemented and property-tested (`do/undo/redo` sequences over generated docs). Empty eframe window opens. **`ffmpeg-next` builds on Windows in CI** (this is the real gate — see §7).
**Done when:** `cargo build` is green on Windows with ffmpeg linked, and an empty window opens.

### M1 — "Frame on screen, sound in ears" *(de-risks the whole pipeline)*
Decode one file with `ffmpeg-next`, convert to RGBA, upload to a wgpu texture, draw it in the preview panel. **Audio decodes and plays through cpal, and the audio callback's position is the transport clock** — play / pause / seek / scrub chase it. Audio is in the first milestone on purpose: A/V sync is a foundational constraint, not a feature to retrofit.
**Done when:** you can open an mp4, hear it, and scrub it smoothly with picture and sound in sync.

### M2 — "Timeline & cuts" *(proves the Operation spine)*
Media bin (probe → `AddAsset`). Single video track. Drag a clip onto it. `SplitClip` / `TrimClip` / `MoveClip` / `DeleteClip` via Operations, with working snapshot undo/redo. Preview composites the (single) active clip. Project save/load via serde.
**Done when:** you can build a rough cut by hand, undo any step, save, and reopen it.

### M3 — "The agent" *(the reason the app exists)*
MCP server serving the generated tool schema + inspectors including `get_frame_at`. First driver: **Claude Code** (best-documented stdio protocol; `-p --input-format stream-json --output-format stream-json` + MCP config). Chat panel renders the session stream. Full loop wired through Core. Second driver (Codex) proves the abstraction generalizes; Cursor/OpenCode drivers can trail.
**Done when:** typing "cut the first 3 seconds and delete the last clip" makes the correct edits via the user's own Claude Code install, they appear on the timeline, and Ctrl-Z reverses them.

### M4 — "Real compositing & export"
Multi-track compositing on wgpu, a handful of effects and transitions **as wgpu shaders** (the one render path), and export: GPU-composited frames → readback → encode, with audio mixdown and a progress bar.
**Done when:** a multi-track project with an effect exports to an mp4 that matches the preview.

### M5 — Polish & ship
Keybindings, agent guardrails hardening (confirmations, cost display, tool allowlisting per CLI), crash recovery from the op log, harness-detection first-run UX, and a Windows installer (GPL-compliant ffmpeg build — see §8).
**Done when:** a stranger can install it and make + export a cut with agent help.

### M6 — "Edit by transcript" *(the killer feature, first post-ship priority)*
Local Whisper (whisper-rs / whisper.cpp) transcription at import, word-level timestamps stored per asset, `get_transcript` inspector, transcript panel in the UI with click-word-to-seek. The agent can now cut by what was *said*.
**Done when:** "remove the filler words from this take" works, accurately, from a single chat message.

---

## 7. Sharp edges that will bite the agents (front-load these)

- **`ffmpeg-next` on Windows is the #1 faceplant.** It links the FFmpeg C libraries and won't "just build." Pin the setup in M0: use `vcpkg` (or a prebuilt shared-libs FFmpeg) and set the env the crate expects before `cargo build`. Verify in CI *before* anyone writes editor logic. `ffmpeg-next` is in maintenance mode but stable and supports FFmpeg up to 8.0 — fine for this. Consider `video-rs` as a friendlier high-level wrapper over it for encode/decode.
- **Version-sync hell across `winit`/`egui`/`wgpu`/`eframe`.** If independent agents each pull "latest" you'll get incompatible combos. Mitigation: **depend on `eframe` and use its re-exported `egui`/`wgpu`/`winit`** rather than declaring those independently, and pin exact versions in the *workspace root* `Cargo.toml`. One place, one set of numbers.
- **Pixel format conversion (YUV → RGBA).** ffmpeg hands you YUV; the GPU wants RGBA. Do the convert in swscale (simple) or a shader (faster). Don't let agents guess — specify it.
- **Time as integer frames, never floats.** Anchor everything to the project `fps`. Float seconds → drift → clips that don't line up.
- **Mixed frame rates.** A 23.976 source on a 30fps timeline needs an explicit per-asset time-base mapping in `core`, with tests at clip boundaries. Agents *will* write drift bugs here if the mapping isn't a named, tested function.
- **Seeking accuracy.** ffmpeg seeks to keyframes by default; exact-frame seek needs decode-from-keyframe-then-step. Tell the agent, or scrubbing will be off by frames.
- **The audio callback is realtime code.** No allocations, no mutexes, no blocking channel ops inside the cpal callback. Ring buffer in, atomic position out. An agent that "just grabs the lock" there ships audible glitches.
- **CLI protocol drift.** The agent CLIs version their stdio protocols independently of us. Keep each driver thin, isolate protocol parsing behind the `AgentSession` trait, and pin known-good CLI version ranges in `detect()` with a clear "your Claude Code is too old/new" message.
- **The agent must never bypass the Operation path.** Any PR where the chat panel mutates `Document` directly is wrong by construction. It corrupts undo and parity.

---

## 8. Suggested starting dependencies & licensing

Let `eframe` own the graphics-stack versions; pin everything at the workspace root.

```toml
# workspace Cargo.toml — the single place versions live
[workspace.dependencies]
eframe      = "..."   # brings matching egui + wgpu + winit; use its re-exports
ffmpeg-next = "..."   # or video-rs for a higher-level API
cpal        = "..."   # audio output; its callback is the playback clock
tokio       = { version = "...", features = ["rt-multi-thread", "macros", "sync", "process", "io-util"] }
serde       = { version = "...", features = ["derive"] }
schemars    = "..."   # derive JSON Schema off Operation -> MCP tool definitions
rmcp        = "..."   # official Rust MCP SDK — the tool server the CLIs connect to
crossbeam-channel = "..."  # or std::sync::mpsc / tokio channels for the Core actor
# M6: whisper-rs (whisper.cpp bindings) for local transcription
```

(Leave the exact numbers to the agents to resolve at build time — the constraint is *pin them once, here*, not scatter them across crates.)

**Licensing.** OpenReel is **GPLv3** (LICENSE in repo root). This is a deliberate simplification: it lets the Windows installer bundle a full GPL build of FFmpeg (x264 and friends included) with zero license gymnastics, where a permissive license would force an LGPL-only FFmpeg build and dynamic-linking care. Everything OpenReel links must be GPL-compatible — the Rust ecosystem's MIT/Apache norm satisfies this. Contributions land under GPLv3; no CLA, no copyright assignment.

---

## 9. How to fan this out to the swarm

1. **One agent builds M0 solo** — the `core` types + trait contracts + snapshot undo + the Windows ffmpeg build gate. This is the bottleneck; don't parallelize it.
2. **Once M0 compiles, split three ways against the trait stubs:**
   - Team A: `openreel-media` (M1 → M4 pipeline, including audio).
   - Team B: `openreel-agent` (M3: MCP server + Claude Code driver first; can develop against a fake in-memory MediaEngine for `get_frame_at`).
   - Team C: `openreel-app` (egui widgets, driving Core with a fake MediaEngine until A lands).
3. **Integration happens at the Core boundary**, which is the only shared surface — keep it stable and version it.
4. Give every agent this doc + the compiled `core` crate as ground truth. When an agent wants to "just add a field to Document," that's a `core` change → route it through whoever owns the contract.

The whole thing is buildable because the contracts are narrow: four crates, two traits, one message bus. That's the shape that survives a swarm.
