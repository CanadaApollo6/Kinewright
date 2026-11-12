# Process-exit thread ownership

Source audit for the process-exit fix on `te1-linear`, based on `8673b79c`.
“Native” below means in-process Vulkan, FFmpeg, audio or Whisper work, not a
provider or FFmpeg subprocess. Joining an OS thread also waits for its TLS
destructors. An event/channel saying “finished” does not do that.

## Media

| Spawn site / thread | Owner and shutdown after this change | Can native work outlive the owner? |
|---|---|---|
| `engine.rs`, `kinewright-media` | Engine's `OwnedWorker`: close command sender, OS join. `RunningWorker::drop` closes lane/audio and joins preview, including unwind. | No; previously the successful spawn handle was discarded. |
| `engine.rs::spawn_preview`, `kinewright-preview` | Worker retains `PreviewThread`; terminal shutdown joins. Renderer is owned by Preview. | No after engine joins. Previously worker itself could outlive engine. |
| `preview.rs::spawn_reader`, `kinewright-decode-*` | Preview retains `ReaderThread`. Terminal Drop signals retirement/interrupt, then joins all readers before dropping renderer. Interactive retirement keeps its 200 ms admission deadline and retains unfinished handles. | No. Previously terminal Drop sent unfinished readers to an unowned retire reaper. |
| FFmpeg decoder/encoder frame and slice workers | Library-owned codec contexts live on joined reader, analysis and export threads. Rust wrappers close/free their codec contexts on scope exit. | No detached Kinewright handle; codec-internal worker termination was not independently instrumented. |
| `derived_cache.rs::spawn_worker` for `VisualAssetService` | Service owns queue sender and thread together; close and join. | No; previously handle discarded. |
| Same helper for `DerivedAnalysisService` | Cancel existing cooperative tokens for every authority; close queue and join. | No; previously handle discarded. |
| Same helper for `TranscriptService` | Cancel all authorities' tokens; close queue and join. | No; previously handle discarded. |
| `voice_note.rs::VoiceTask::spawn`, `voice-note-whisper` | Task cancels unfinished inference and always joins, including after receiving a result. | No; previously cancellation without join. |
| `audio/simulated.rs`, pacer | `SimulatedOutput::drop` sets stop and joins existing pacer handle. Stepped mode has no pacer. | Already ordered. |
| `audio.rs`, CPAL output callback | AudioRuntime owns Stream; shutdown drops Stream before preview join. CPAL 0.18.1 ALSA and WASAPI Stream Drop signal and join their callback threads. | Library-owned and ordered by Stream lifetime. |
| Whisper/ggml CPU workers | Native context/state owned on joined service/inference thread. Vendored `ggml_threadpool_free` broadcasts stop and calls `ggml_thread_join` before freeing workers (non-OpenMP backend). | No extra Kinewright pool; OpenMP implementation teardown not independently tested. |
| wgpu/Vulkan driver workers | No Kinewright poll thread. Rendering/readback does synchronous `Device::poll`; Preview owns renderer/device/instance. Native driver pools are library-owned. | Our renderer teardown now finishes before its OS owner returns; correctness of internal driver teardown is not separately proved. |
| rayon | No direct pool creation, spawn, or parallel iterator in Media/Agent source. | No Kinewright-owned rayon worker found; dependency-internal pools not exhaustively instrumented. |

Terminal cleanup deliberately waits for an uninterruptible native call instead
of detaching it. This does not promise a new finite exit-time bound. The old
terminal reader-detachment test becomes a join test; interactive retirement
bounds and tests are unchanged. OS process termination is not used as product cleanup.

## Agent

| Spawn site / thread | Handle owner / stop and join | Native lifetime / remaining boundary |
|---|---|---|
| `server.rs`, both MCP server constructors | `McpServer` signals shutdown/rejects confirmations and now joins on Drop as well as explicit shutdown. `request_shutdown` is signal-only; caller retains owner until join is appropriate. | Can own media through handler and export queue. Previously plain Drop discarded server handle. |
| `server.rs::server_runtime`, Tokio workers and blocking jobs | Runtime local to joined MCP server thread. Network grace is 2 s; runtime Drop still waits for started blocking work. | Includes native tool calls. Abort of an async future alone does not join blocking work; runtime ownership supplies final join. |
| `writer_host/mod.rs::serve`, HTTP thread + Tokio runtime | Existing `ServerThread` / WriterHost Drop stop and join; runtime drops in server thread. | Native host backend cannot outlive server thread through its handler. |
| `writer_host/mod.rs`, owned-session turn thread | `HostOwnedSession` retains handle, interrupts and joins on Drop. | Existing owner, no new detach. |
| `writer_host/mod.rs`, bootstrap discovery pipe reader | Local handle joins on success/error after kill/reap on failed bootstrap. | Pipe-only; already joined. |
| `writer_host/studio_runtime.rs`, recovery worker | Runtime retains handle; stop requests cancellation, then joins outside owner mutex. | Recovery and durable writes complete before runtime returns. C7 shutdown fence preserved. |
| Same file, effect pump | Runtime retains Worker handle; stop cancels then joins outside owner mutex. | Existing owner; underlying native provider reader is described separately below. |
| `writer_host/task_context.rs`, `task-media-watch` | HostLifecycle now retains watchers; WriterHost fences/cancels, joins watchers outside owner mutex, then joins server. Finished handles are reaped between starts. | Previously detached watcher could retain a native read past host Drop. |
| Same file, `task-media-read` | Watcher owns child handle, signals shared decode cancellation when endpoint/host stops, then joins. | FFmpeg input and its permits remain owned until child completes. |
| `export_queue.rs`, queue worker | Existing Clone queue now shares one runtime owner. Last queue closes sender, cancels jobs and joins worker. Worker owns queue state, not runtime, so no ownership cycle. | Can own media exporter/analysis; previously worker handle discarded. |
| Same file, progress relay | Export worker retains and joins relay handle after normal/caught-panicking export. | No native work itself; ordered before export worker exits. |
| `scripted.rs`, session worker + current-thread Tokio runtime | ScriptedSession interrupts and joins worker, including setup failures. Runtime lives inside joined worker. | Fixture transport; already owned. |
| `writer_host/te1_eval.rs`, scripted transport | ScriptedSession owns returned handle and joins. | Feature-gated fixture transport. |
| `te1_eval.rs`, local Tokio runtime | Owned by synchronous unpaid/paid entry stack; dropped there. | Not run with paid authority in this task. |
| `child_process.rs::spawn_pipe_reader` | Returns handle; bounded CLI probe joins stdout and stderr after process wait/kill. | Pipe only. No provider probes executed for this diagnosis. |
| `provider_stderr.rs::stderr_summary` | Returns handle; adapter stream workers join summary before their Done. | Pipe only. Parent adapter ownership is below. |
| `drivers.rs`, Codex session worker | Handle discarded; session interrupt kills/reaps process. | Protocol/subprocess work; no renderer/FFmpeg/Whisper owner captured. Remains unjoined. |
| Same file, Codex turn reader helper | Returns handle, but production caller checks only Err and discards successful handle; tests join it. | Pipe/protocol only; remains unjoined. |
| Same file, Claude reader | Handle discarded; session interrupt kills/reaps process. | Pipe/protocol only; remains unjoined. |
| `acp.rs`, writer | Handle discarded; AcpInner closes/kills process and channel. | Pipe/framing only; remains unjoined. |
| Same file, reader | Handle discarded; reader joins its stderr helper when EOF arrives. | Pipe/framing only; remains unjoined at session-owner level. |
| `acp_session.rs`, configuration teardown | Detached, bounded restore and process cleanup. | Protocol/config filesystem only; remains unjoined. |
| Same file, prompt-turn worker | Detached, waits on ACP reply then restores configuration. | Protocol only; remains unjoined. |
| Same file, incoming-event worker | Detached; drains ACP incoming stream. | Protocol only; remains unjoined. |
| `copilot.rs::spawn_copilot_reader` | Detached, child stdout reader. | Protocol only; remains unjoined. |
| `muse.rs::spawn_muse_acknowledgement` | Detached; channel acknowledgement. | Protocol only; remains unjoined. |
| Same file, incoming worker | Detached; forwards ACP messages. | Protocol only; remains unjoined. |
| `orchestration/native_launcher.rs`, contained stream reader | RestrictedSession retains handle, closes lifetime, then joins outside control/owner mutexes. Stream worker cleans process, publishes completion, joins stderr and emits Done. | Publication can briefly upgrade the host Weak and retain its lazy media backend. Joining this reader closes that indirect native-owner window too. |
| `orchestration/limits.rs`, stderr collector | Returns handle; test-util native/network witnesses join the summary. | Pipe only. |
| `orchestration/guardian.rs`, framed output reader | Detached; receiver/channel and process EOF end it. | Guardian protocol only; remains unjoined. |
| Same file, Launcher Drop reaper | Detached child wait. | Separate-process reaping only; remains unjoined. |
| Same file, manager query reader | Detached bounded response reader. | Guardian protocol only; remains unjoined. |
| `orchestration/provider_pipes.rs`, relay coordinator | Detached; McpRelay Drop closes/revokes sockets. | IPC only; remains unjoined. |
| Same file, two copy directions | Successful copier handles are discarded as well; socket shutdown ends the IO. | IPC only; remains unjoined. |
| `job_guardian/service.rs`, command reader | Detached in separate guardian executable; EOF drives service termination. | No media linked into worker's work; not app native teardown. |
| `job_guardian/linux.rs`, loopback relay | Detached in guardian namespace/payload process. | Network IPC in separate process; not app native teardown. |
| `ja_harness/controller.rs`, fixture stdout/stderr | Controller worker owns handles; normal finish joins after child reap; error Drop kills/reaps child but raw read handles may detach. | Feature-gated fixture pipe readers; no media backend. |
| Same file, App stdout/stderr capture | Local handles joined after supervised child completion; early errors can detach. | Feature-gated pipe readers; no media backend. |
| `ja_harness/recording.rs`, recording forwarder | RecordingSession interrupts and joins on Drop. | Existing owner. |
| Same file, capture-source Tokio runtime | Local current-thread runtime in synchronous fetch helper. | No persistent pool owner. |
| `ja_harness/supervision.rs`, child output capture | Detached pipe reader, bounded output bytes. | Fixture-only IPC; remains unjoined. |

Test-only spawns in `session.rs`, `writer_host/snapshot_lease.rs`,
`writer_host/network_witness.rs`, included test/fixture modules and inline
`cfg(test)` modules are not production owners. They were included in the raw
spawn scan and checked separately. The new witness's child runs the ordinary
engine and real GPU; it does not bypass native Drop.

## App owners that can retain a native backend

| Owner | Change / existing ordering |
|---|---|
| `KinewrightApp::off_ui` and media import/relink/status/cache/LUT/room-tone/solo jobs | App retains JoinedThread handles, reaps finished jobs, joins remaining jobs on Drop. Closures can hold engine Arcs, so joining just the engine is insufficient. |
| Noise learning, QC mask, matte overlay, colour scopes and colour QC | Their worker handles now join on Drop. Interactive cancelled workers remain retained until finished. |
| ExportJob | Cancels backend on Drop, then joins owned export thread. |
| Investigator RunningSession and retired sessions | Signals cancel, rejects confirmations, signals MCP shutdown, joins session thread, joins server. Interactive completion/cancel hands full session to a project-owned reaper; project Drop joins reapers. Previously reaper itself detached. |
| NativeDisplay | Declared first: terminal/release display leases before joining native jobs or releasing engine owners. `on_exit` already calls terminal too. |
| Voice capture / pixel proof | CaptureStop signals capture before JoinedThread drops; readback owns its CPU image and joined digest thread. |
| Writer-host Shutdown | Accepted fence prevents new dispatch (C7 fix); runtime stop/join runs outside owner mutex. Host Drop also joins media watcher group and server. Refused Shutdown changes nothing. |
| Originating JA1 session, save/recovery actions | Save/history guards and continuation rules are unchanged. Existing source/host close has a 5 s process deadline then kill/reap; a detached continuation deliberately owns a separate host process. This change does not turn client close into cancellation of a kept run. |

App provider discovery, model catalog, chat/HTTP requests and upload jobs that
capture no native engine remain outside the native-owner fix. Their process/IO
completion is not claimed joined by this patch. No change to save acknowledgments,
recovery durability, provider teardown policy or forced-process-kill semantics
is inferred from the native join witness.
