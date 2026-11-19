# MS2 media sandbox

Status: design with review-round-1 and round-2 rulings incorporated;
implementation unproven.
Source baseline is `origin/main` at `266f8890e259fb20d4c8f10d778b5b5fed6b3257`.
This document records decisions and future proof gates. No implementation,
build, confinement test, GPU run, or performance measurement accompanied it.

## Security boundary

Treat every imported media byte as hostile, including media in an IX project,
project-owned objects, still images, audio, proxies, and files exported earlier.
A project certificate establishes byte identity and access authority, not safety.
Run every container parser and compressed-media decoder in a dedicated native
`kinewright-media-helper`. Preserve MS1's decoder allowlist and the existing
custom AVIO, protocol refusal, demuxer allowlist, and allocation/probe limits
inside the helper. Confinement must succeed before any media byte is parsed.

The app owns project state, undo, the recovery journal, cache publication,
file selection, and resource admission. A helper receives one read-only media
object and bounded requests. It receives no project directory, provider token,
MCP connection, output path, GPU handle, or authority to open a second source.
The kernel, trusted installed binaries, and launch bootstrap are trusted.
Kernel escapes, hostile administrators, and incorrect decoded content remain
outside the guarantee. The app must remain safe even when helper output lies.

### Current calls and intended placement

All references below use the source baseline above. Line numbers name entry
points or calls, not a claim that the whole function moves unchanged.

| Current code | Decision |
| --- | --- |
| `crates/kinewright-media/src/decode.rs:46`, `:686`, `:804`, `:850` | Move probing, packet timing scans, and both path-based input opens into the helper. |
| `crates/kinewright-media/src/owned_io.rs:467`, `:644`, `:676`, `:707`, `:1520` | Move certified probe and decoder construction, custom AVIO, and all FFmpeg input processing. |
| `crates/kinewright-project/src/owned_media.rs:415`, `:451`, `:457`, `:471` | Replace certified admission/probe/decode calls with helper requests. Keep pinning, interpretation validation, and byte identity in the host. |
| `crates/kinewright-project/src/read_only_inputs.rs:188` | Preserve owner-bound read-only admission. Its subsequent probe must also cross the sandbox boundary. |
| `crates/kinewright-media/src/decode.rs:1971`, `:1599`, `:3292` | Move video decode, FFmpeg pixel conversion/scaling, orientation parsing, and source thumbnails. |
| `crates/kinewright-media/src/still_orientation.rs:62` | Move EXIF/container-byte inspection inside, using the source descriptor. |
| `crates/kinewright-media/src/render.rs:310`, `:1516`; `crates/kinewright-media/src/preview.rs:4076` | Preview, inspectors, and export renderers consume validated helper frames. |
| `crates/kinewright-media/src/audio.rs:78`, `:112`, `:3721`, `:4785` | Move audio decode and resampling. Replace whole-range PCM accumulation with bounded consumption. |
| `crates/kinewright-media/src/analysis.rs:369`, `:404`; `crates/kinewright-media/src/engine.rs:1941` | Run waveform peaks, source loudness, and source thumbnails inside. Return bounded summaries or pixels. |
| `crates/kinewright-media/src/derived.rs:516`, `:619`, `:914` | Move silence/beat/scene source analysis inside, with bounded incremental results. |
| `crates/kinewright-media/src/transcript.rs:326` | Extract recogniser audio inside. Keep local Whisper inference/model loading outside, fed bounded validated PCM windows. |
| `crates/kinewright-media/src/engine.rs:2261`, `:2269`; `crates/kinewright-media/src/room_tone_store.rs:532` | Route transcription entry probes and room-tone capture through the same helper boundary. |
| `crates/kinewright-media/src/export.rs:1142` | Export's source audio still needs sandboxed decode. |
| `crates/kinewright-media/src/export.rs:399`, `:425`, `:473`; `crates/kinewright-media/src/compositor.rs:181` | Keep muxing/encoding app-generated frames and mixed PCM, GPU compositing, effects, and timeline mix/loudness outside. |
| `crates/kinewright-media/src/frame.rs:49` (`from_rgba64_le`) | Keep safe Rust conversion of validated RGBA64 pixels into the existing scene-linear working frame. |

Source compositing for contact sheets stays outside. Imported compressed image
caches must use the helper too, or be replaced with validated raw cached pixels.
Do not send helper-created JPEG/PNG back to an in-process image decoder.
At completion, production app paths cannot obtain an FFmpeg input context or
compressed packet, including project admission and exported-artifact inspection.
Keeping FFmpeg linked for encoding does not authorize an in-process decode path.

## Process ownership and memory

Use a bounded set of source-bound processes. One helper serves one immutable
source identity and access-owner generation, never a second file during its
lifetime. Share it between clips referring to that identity. Keep one active
request/cursor per helper, with a selected video/audio decoder pair when needed.
Independent cursors on the same source serialize; they do not create hidden
extra processes. This trades some seek throughput for bounded memory.

The app's supervision thread admits at most two helpers across preview, export,
and analysis, counting probe helpers and blocked bootstraps. Playback has priority.
Background work yields at chunk boundaries.
The trusted outer PID-namespace shim is included in each helper's private
reservation and `TasksMax`, never an additional decoder. The independent
supervisor is charged to the global bootstrap allowance.
Reserve capacity before launch. Keep at most 32 pending requests globally and
one active plus one replaceable pending request per consumer. Coalesce obsolete
seeks. Evict an idle helper after five seconds or immediately under pressure.
A different source always requires a fresh process. No warm cross-file reuse.

The probe helper returns metadata only, but its bounded admission probe includes
first-frame decode and still inspection. These operations are confined in slice
1a, not deferred to frame streaming or thumbnails. The existing admission path
at `crates/kinewright-project/src/owned_media.rs:415` calls the probe that decodes
a first frame at `crates/kinewright-media/src/owned_io.rs:579` and inspects still
orientation at `:582`. No decoded pixels leave this probe.
After its validated raster is accepted, the host reaps it and starts a fresh
decode helper with inherited rings sized for that
raster. A decode helper never probes or negotiates a larger ring. This sequence
costs two confined launches and two FFmpeg input initializations per cold source.
Only the host opens the file; the decode helper parses required headers without
probing for or negotiating geometry.
It avoids overlapping probe/decode private reservations, but the host retains
validated metadata and reserves rings plus snapshots before the second launch.

Budgets below are provisional admission targets, pending worst-case compliant
codec/level/reference-set measurements. They are not evidence that 4K fits.

| Reservation | Per helper | Global bound |
| --- | --- | --- |
| Audio private memory, runtime included | 128 MiB provisional | 256 MiB provisional; alternative to video private rows |
| Admission probe private memory, shim/runtime included | 1,024 MiB; allocation component 896 MiB; AS 1,536 MiB | 2,048 MiB plus 128 MiB bootstrap; no rings/snapshots |
| Video private memory, runtime/native buffers included | 384 MiB | 768 MiB; new helper reservation |
| Preview rings, separately charged | Two slots × 64 MiB | 256 MiB; new shared-memory reservation |
| Export rings, replacing preview rings | Two slots × 128 MiB | 512 MiB; new shared-memory reservation |
| Host snapshots/conversion scratch | Two slot lengths per active helper | 256 MiB preview / 512 MiB export; existing media ledger |
| Accepted source-frame cache | No per-clip duplication | 128 MiB; inside existing media ledger |
| Bootstrap, IPC, and supervisor | Reserve before launch | 128 MiB; new reservation |
| Video-path sum, two helpers | Private + rings + snapshots + cache + bootstrap | 1,536 MiB preview / 2,048 MiB export |

The old table's additive rows totalled 1,792 MiB, not its stated 1,536 MiB.
The replacement sums exclude the alternative audio row and count each ring once.
Existing-ledger rows are included in these totals, not added again outside MS2.
Project data, Whisper models, compositor VRAM, and output encode buffers retain
their separate budgets. Limit snapshots to one active consumer per helper;
additional consumers share or serialize rather than multiplying scratch.
Target private peaks of 96 MiB audio and 384 MiB video; report shared pages apart.
Use one helper on systems that cannot reserve the two-helper maximum. Refuse new
work with `ResourceLimit` if even one reservation cannot fit. Do not retry OOM
with a larger automatic allowance. Preview slots cap at 64 MiB, export at
128 MiB. Packed RGBA64 DCI 4K is 67.5 MiB and 5120 × 2880 is 112.5 MiB.
The current full-resolution support limit excludes 6K and larger rasters.
Refuse any geometry over its slot cap as `UnsupportedMedia`, with the limit in
the UI. Preview may explicitly request a smaller frame; export never silently
reduces resolution or bit depth. Strip transport is later work.

Start with two FFmpeg decode threads per helper, one for measurement. Admit no
more than four decode threads globally. Keep reference frames and conversion
buffers inside the reservation. The 24-buffer multiplier at
`crates/kinewright-media/src/owned_io.rs:25` and padded 16-byte/pixel formula in
`crates/kinewright-media/src/owned_io.rs:1373` reserve 1,123 MiB at 1080p and
3,649 MiB (3.563 GiB) at UHD, already exceeding the proposed private targets.
Ruling 19's 3.65 GiB is an arithmetic error. The reproducible calculation is
`(ceil(w/128)*128+256) * (ceil(h/128)*128+256) * 16 * 24 + 1,048,576`
bytes, with UHD `w=3840, h=2160`.
Replace that formula with a format/DPB-aware allocation sub-budget using codec
level, reference count, bit depth, chroma, frame threading, and conversion work.
Untrusted headers may lower supported admission, never raise the hard budget.
In primary mode kernel containment is the hard total bound; the allocation
guard is a component inside it. Degraded mode has a hard address-space bound
and polled RSS, with the overshoot qualification below. Reconcile existing
cursor/buffer ledger charges, never charge twice.
Without measurement, do not remove the guard or claim these targets sufficient.
Slice 1a includes this replacement allocation sub-budget for the admission
probe, including its first-frame decode and still inspection. Measure probe
PSS and peak address space on 1080p, UHD, and still fixtures before freezing
probe `MemoryMax` or `RLIMIT_AS`. Slice 1a records those measurements in
`docs/reports/ms2-s1a.md` and its evidence archive before selecting the probe row.
The initial unknown-format UHD AV1 DPB guard needs 836 MiB; 896 MiB leaves
128 MiB inside MemoryMax for shim/runtime/native overhead. The measured
295.7 MiB VmPeak leaves 1,240 MiB of virtual growth below AS. A 448 MiB
component correctly refused that valid fixture; no automatic budget escalation
exists. These fixtures do not qualify every codec/reference-set combination.

Measure private bytes, shared mappings, PSS, and peak RSS separately on Linux.
Record cgroup `memory.current`, `memory.peak`, and `memory.events` as well.
On Windows record private commit, working set, mapped-section sizes, and Job
peaks. Job commit is not RSS and may exclude shared sections. Reserve every
parent-created section separately. Check two distinct 4K sources, 100 clips of
one source, long audio, cancellation churn, and repeated source replacement.

## IPC treats the helper as compromised

Use two inherited anonymous pipes for control, one in each direction. No TCP,
localhost server, named rendezvous, descriptor-passing command, or path-bearing
request exists after bootstrap. Put framing in a dependency-light IPC crate.
Use buffered `read_exact` for a fixed header, validate its length before payload
allocation, then read the bounded payload. Do not copy the one-byte-per-read
loop at `crates/kinewright-agent/src/job_guardian/protocol.rs:163`.
Do not depend on the agent crate from media.

Each control frame has a fixed header containing magic, exact build protocol
identifier, message tag, payload length, request ID, and source generation.
Control payloads are strict typed JSON using existing serde dependencies.
Raw pixel/summary-array chunks use the named binary frame `DataChunk`, with
request, generation, element count, and exact byte length in its fixed header.
They are never JSON/base64. Cap all pipe frames, including `DataChunk`, at
64 KiB before allocation, nesting at eight, streams at 16,
waveform peaks at 2,048, text at 256 bytes, and outstanding responses at two.
Reject unknown/duplicate fields, unknown enum tags, trailing data, invalid
UTF-8, arithmetic overflow, and a different build protocol. There is one
current protocol, no version ladder or mixed-build negotiation.
Reject duplicate keys at every object depth with a custom deserializer or a
bounded pre-scan before serde constructs a value; do not deserialize via a map
that overwrites duplicates. Include nested duplicate keys in the fuzz corpus.

Requests are `Probe`, `MeasureAudio`, `Thumbnail`, `ReadVideo`, `ReadAudio`,
`Seek`, `Cancel`, `ReleaseSlot`, and `Close`. Responses are fixed-state variants
such as `Ready`, `ProbeResult`, `SummaryChunk`, `FrameReady`, `AudioReady`,
`Complete`, and `Failed`. Decoder/filter names are host-owned enums. No helper
message can request a file open, command, allocation, or project edit.
`Ready` is a sequencing message, never sufficient evidence of confinement.

The host validates IDs, owner authority, state transitions, stream selection,
positive rational denominators, checked timestamps, dimensions, colour enums,
counts, and response cardinality before constructing domain values. Validated
domain types have private fields and checked constructors. Their invariants
must satisfy downstream `expect`s even when every helper field is a lie.
Fuzz real consuming paths for host panics. The host supplies asset IDs and cache
keys itself. Never trust a helper's hash as source identity.
Budget cumulative summary count/bytes per requested range and publish pages
incrementally; a 64 KiB per-message limit alone does not bound a long analysis.
Enforce limits before enqueueing, including small-message floods.

| Boundary field or traffic class | Maximum / acceptance rule | Existing guard or MS2 decision |
| --- | --- | --- |
| Streams and selected index | 16 streams; index in validated table | `crates/kinewright-media/src/owned_io.rs:21` |
| Source rate and channels | 1..384,000 Hz; 1..32 channels | `crates/kinewright-media/src/owned_io.rs:24`, `:1403`; no eight-channel regression |
| Raster | 1..8,192 each axis, plus slot-byte cap | `MAX_DECODER_DIMENSION` at `crates/kinewright-media/src/owned_io.rs:22`; transport cap is stricter |
| FPS | Positive reduced rational; value ≤4,096; components fit u32 | New wire cap; `crates/kinewright-core/src/time.rs:64` currently checks positivity only |
| Duration | Positive, at most seven days (604,800 s); checked frame/sample/microsecond conversions | New MS2 support limit; reject unknown/overflowing durations |
| Frame timestamp / PCM position | Exactly the host's next requested position in the agreed grid | Host resets expectation on seek; duplicates, skips, and drift are protocol errors |
| Every floating-point field | Finite, with a field-specific range; no unbounded generic float | PCM ±64; confidence 0..1; summary levels −200..100 dB and linear peaks 0..64; reject absent range definitions |
| Helper errors/text | Prefer host-rendered error enums; at most 256 UTF-8 bytes if retained | Strip control/bidi/terminal escapes; never use as markup, path, or format string |
| Progress | At most 4/s/helper, burst 1 | Exceeding it is `ProtocolViolation` |
| Data messages | At most two outstanding, backed by ring credits or two host-granted `DataChunk` credits | No data without a matching request and credit |
| Summary request | At most 8,192 chunks / 32 MiB cumulative output per admitted range | Host pages larger analyses as separate bounded requests |
| All pipe traffic | 256 frames/s and 16 MiB/s/helper, one-second burst; global twice those limits | Count headers, control, binary chunks, and invalid bytes before parsing |

Shared-ring payload bandwidth is separately bounded by credits, admitted fps,
and slot size; the pipe byte ceiling is not a 4K pixel-bandwidth ceiling.

### Frame and audio transport

Use a host-created shared-memory ring with two fixed slots per decode helper.
Only the earlier confined probe returns metadata. The host validates it,
chooses dimensions, sizes, and format, then launches the fresh decode helper.
Transport packed RGBA64LE, one plane, for managed source video, preserving the
existing conversion into `WorkingFrame`. Transport RGBA8, one plane, only for
explicit display thumbnails. No NV12/P010 or native decoder planes in MS2.
Those save bandwidth but would move colour conversion or more FFmpeg into the
host. Keep existing colour refusals and golden-frame results unchanged.

Dimensions are nonzero, at most 8,192 per axis, and subject to the 64 MiB
preview or 128 MiB export slot limit. Compute strides and lengths with checked
arithmetic. Mid-stream resolution changes fail as `UnsupportedMedia`;
the helper never resizes the ring or silently scales into a different contract.
`FrameReady` carries a slot index, sequence, timestamp, and agreed format ID,
never a pointer or helper-selected offset/stride. Each slot is single-use until
`ReleaseSlot`. With no credits, the helper stops producing; it cannot allocate
an overflow queue. The host owns all counters outside shared memory.

On Linux the host uses `memfd_create(MFD_CLOEXEC | MFD_ALLOW_SEALING)`, adding
`MFD_NOEXEC_SEAL` where supported and verifying `F_SEAL_EXEC` before sealing.
Size it in the host and apply `F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_SEAL`.
The helper maps writable data, then closes the section fd before lockdown.
The app maps read-only. Do not claim `F_SEAL_WRITE` for a writable ring. These seals prevent resize,
not malicious pixel modification. Seccomp denies executable mappings on kernels
without the newer flag too. [Kernel no-exec memfd contract](https://docs.kernel.org/userspace-api/mfd_noexec.html).
On Windows use unnamed `CreateFileMappingW(INVALID_HANDLE_VALUE, ...)` sections
created `PAGE_READWRITE`. The duplicated handle mask is exactly
`SECTION_MAP_WRITE | SECTION_QUERY`, with no execute, extend, or ACL rights.
Use `MapViewOfFile` for a writable helper view and read-only app view.
[Mapping API](https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-createfilemappingw).

A compromised helper can overwrite a credited slot at any moment. Copy its
fixed byte range once with `ptr::copy_nonoverlapping` into host-owned storage,
then validate and consume that snapshot only. Never form a `&[u8]` over racing
memory, even briefly for copying. Do not let wgpu/DSP read the mapping directly.
Confine raw mapping/copy operations to an audited OS wrapper with a memory-safety
argument. Slot acknowledgements provide back-pressure, not immutable ownership.

Count two transport copies for an RGBA64 frame: converted helper plane into
the ring, then ring into the host snapshot. Conversion into the working frame
and GPU upload are additional operations. At 3840 × 2160 × 8 bytes × 60 fps,
one stream carries 3,981,312,000 bytes/s, about 3.71 GiB/s. Two transport copies
move 7.96 GB/s of payload and roughly 15.93 GB/s of memory reads plus writes.
A same-size GPU upload adds about 3.98 GB/s, before working-frame conversion.
These are arithmetic estimates, not measured bandwidth. RGBA8 halves these
figures but is unsuitable as a replacement for the managed colour path.

PCM uses two slots of at most 256 KiB, little-endian interleaved f32, at an
explicit requested rate of 16 or 48 kHz, at most 32 channels and 8,192 sample
frames per slot, reduced to fit 256 KiB (2,048 frames at 32 channels).
Validate exact lengths and the host's expected sample position on the snapshot.
Reject NaN, Inf, subnormals, and amplitudes outside ±64.0 before DSP; never clamp
hostile PCM into apparent validity. This limit is fixed in the wire contract
before slice 3. `crates/kinewright-media/src/audio.rs:188` clips final mixes to
±1, but gain at `:515` and additive mixing at `:3630` precede clipping.
Room-tone input uses the same decode path (`crates/kinewright-media/src/audio.rs:68`).
These operations provide no tighter source-amplitude guarantee. Use the lead's
±64.0 ceiling (about 36.1 dB above unity), with gain/mix/room-tone qualification.
Whisper uses 16 kHz mono windows with bounded overlap, never the whole recording. Pure measurements return summaries without a
PCM ring. Cache writes and transcript merging remain host operations.

### Seeking, cancellation, and deadlines

A seek advances a host-owned generation, flushes pending slots and codec state,
and carries an integer source-frame target and rate. Decode from a keyframe
inside, discarding preroll until the exact requested grid position. Preserve
integer-frame, rotation, colour, and sample-boundary semantics. Reject stale
generations even when the old result is structurally valid. Source/owner revocation cancels the
lease, kills its helper, and invalidates outstanding results and cache grants.

Allow five seconds for startup/initial probe. Begin first-frame/seek attempts
with a ten-second deadline, extend it through an explicit bounded slow-decode
retry if necessary, and record validated first-frame cost per source/codec/raster.
Set streaming inactivity to max(2 s, 4 × measured first-frame cost), capped at
60 s per attempt. A source needing more receives retryable `TimedOut` with an
explicit slower attempt, never an automatic "broken media" label. Qualify this
policy on valid 4K AV1 and 10-bit HEVC. Whole-file scans use a separate analysis
request, with a 30-second chunk deadline and
checkpoint, never an unlimited extension of a probe. App deadlines count useful
validated output, not heartbeats. Suspend stream deadlines while the host has
withheld all credits. Cancellation has a 250 ms cooperative grace through the
AVIO interrupt flag. Then terminate, reap, and discard all partial results.
An app supervision thread enforces deadlines even inside a stuck FFmpeg call.

## Linux confinement

The host calls `execve` with an empty environment and an absolute packaged
executable path. Loader variables are consumed before `main`; clearing them in
the helper is too late. Package eager binding in the ELF rather than passing
`LD_BIND_NOW`. The single-threaded bootstrap initializes runtime state without
reading media. The host opens one regular local file `O_RDONLY | O_CLOEXEC`,
rejects devices/FIFOs/sockets, and passes the descriptor, never its path.
The host guarantee is full-file verification at admission, the retained pin,
and open-fd identity. `CertifiedSource::verify` at
`crates/kinewright-media/src/owned_io.rs:306` checks the full digest, while
`crates/kinewright-project/src/owned_media.rs:694` opens the descriptor and pins
the source roots before verification. Keep selected-stream and host authority
checks. Writable external sources require an immutable host snapshot first.
Drop in-helper per-chunk SHA checks; a compromised decoder cannot attest to
its reads or authorize publication. Revocation kills the helper and invalidates
its outstanding results, since it already holds the source fd. Never mmap source
bytes in the app, where truncation could cause SIGBUS.

### Resource ownership and app loss

The app spawns a blocked outer shim. The shim forks the payload, which is PID 1
in its new PID namespace. Arm `PR_SET_PDEATHSIG=SIGKILL` on both edges, after
each fork and after credential changes that clear it. The shim checks `getppid`
against the app PID captured before spawn. PDEATHSIG follows the spawning
thread, not the app process, and Linux can reparent the shim to another app
thread without changing `getppid()`. So all shims are spawned by one dedicated
spawner thread whose exit is process exit. It never returns, and a guard on
every exit path (return, unwind, or panic) calls `std::process::abort()`. Thread
loss without process loss is then impossible by construction, on every kernel.
Where `PIDFD_THREAD` is available (Linux 6.9+), the shim also inherits a thread
pidfd on the spawner and polls it after each arm and re-arm, as a second check.
The shim closes it before forking the payload. Witness: inject a spawner-thread
exit while another app thread survives, and check that the app aborts and the
chain dies, before arming and during decode. After
forking, the single-threaded shim only waits for and reaps its payload. It does
not exit early. App death kills the shim, whose death kills the payload even
with the watchdog disabled. Both primary and degraded modes use this chain.

Ruling 24 needs one technical correction to its payload race check. A namespace
init's parent is outside its PID namespace, so `getppid()` returns 0 even while
the shim lives. Reparenting can leave that value unchanged. Thus a comparison
with 0 cannot close loss before PDEATHSIG is armed. Before the fork, the shim
opens a pidfd on itself for the child to inherit. After arming PDEATHSIG, the
payload checks `getppid()` and polls that pidfd with zero timeout. Exit on parent
death, HUP, NVAL, or any check failure, then close the pidfd before lockdown.
The pidfd check covers pre-arm shim loss; PDEATHSIG covers loss after arming.
[PID-namespace parent visibility](https://man7.org/linux/man-pages/man7/pid_namespaces.7.html),
[PDEATHSIG timing and credential resets](https://man7.org/linux/man-pages/man2/PR_SET_PDEATHSIG.2const.html),
and [pidfd exit notification](https://man7.org/linux/man-pages/man2/pidfd_open.2.html)
define these checks. Parent death is signalled from the ancestor namespace;
the [kernel reparent path](https://codebrowser.dev/linux/linux/kernel/exit.c.html#forget_original_parent)
delivers PDEATHSIG to the payload's thread group, and
[ancestor signal handling](https://codebrowser.dev/linux/linux/kernel/signal.c.html#send_signal_locked)
forces delivery to namespace init.

The separate trusted `kinewright-media-supervisor` binary only creates,
verifies, and stops scopes. It never parents helpers or touches helper media,
control pipes, or rings. It links no FFmpeg. Keep its OS primitives in job-sys
without linking its D-Bus client into the app or helper. The app holds a pidfd
on the supervisor. Supervisor death closes launch admission; the app kills its
own shims, reaps them, verifies empty scopes, and restarts the supervisor before
accepting new launches. Dead members leave stale scopes empty. The supervisor's
own cgroup path must be outside every helper scope.

The app and supervisor use one private socketpair. Only the supervisor inherits
its endpoint across exec. Keep both endpoints `CLOEXEC` everywhere else and
exclude them from every shim/helper fd inventory. Use the bounded framing rules
above for typed lifecycle commands `CreateScope`, `VerifyScope`, and `StopScope`
and their typed results, with at most two live scope records and two outstanding
replies. A create request names only a PID the app spawned.
The supervisor independently resolves PID plus starttime, checks the app's child
relationship, and retains that identity. It passes its locally pinned pidfd to
systemd's `PIDFDs` property, rather than sending a numeric PID that could be
reused while the bus request is delayed. This does not add fd passing to the
app/supervisor contract. There is no numeric-PID fallback.
[systemd v255 scope PIDFDs implementation](https://github.com/systemd/systemd/blob/v255/src/core/dbus-scope.c#L145).
 It generates the unit name itself;
later commands use its bounded scope ID, never helper-supplied PIDs or unit
names. No generic D-Bus forwarding exists. Its curated exec environment contains
only `DBUS_SESSION_BUS_ADDRESS`, with no loader variables or FFmpeg settings.

The payload's lifetime pipe has a writer held only by the app. Its watchdog
calls `exit_group` on HUP, EOF, or NVAL. Self-`tgkill(SIGKILL)` cannot reliably
kill namespace init. The watchdog is cooperative and is never the app-loss
guarantee; the two parent-death edges provide that guarantee after bootstrap.

The primary resource mechanism is a transient systemd **user scope**, created
with D-Bus `StartTransientUnit`, never a shell or `systemd-run`. Set `MemoryMax`
to the private reservation plus separately reserved ring pages charged there,
`MemorySwapMax=0`, `TasksMax=8`, `CPUQuota=200%` for video or 100% for probe/audio,
and `OOMPolicy=kill`. The bus encodes CPUQuota as `CPUQuotaPerSecUSec` (2,000,000
or 1,000,000). Verify the resulting controls and keep the app and supervisor
outside every helper scope.
`Cgroup::create` at `crates/kinewright-job-sys/src/linux.rs:135` requires its
caller's service name, so a GUI process cannot use it as a generic allocator.
Add the scope path rather than relaxing that existing guardian invariant.

Choose the blocked-child join. Before any media read or payload fork, the shim
waits on the app's release pipe while the supervisor attaches it to the scope
and verifies host PID plus `/proc/<pid>/stat` starttime and actual membership.
The app releases the shim only after that typed verification succeeds. The
payload inherits membership, which is also verified before media parsing.
`CLONE_INTO_CGROUP` is an atomic alternative only with an already-owned cgroup;
no check-then-move running decoder is allowed. Set the limits before release.
[Scope contract](https://www.freedesktop.org/software/systemd/man/latest/systemd.scope.html)
and [D-Bus interface](https://www.freedesktop.org/software/systemd/man/latest/org.freedesktop.systemd1.html).

Without a user manager or enforceable controllers, log degraded resource mode.
Use `RLIMIT_AS`, provisionally 1 GiB video or 256 MiB audio including
mappings, and the measured 1,536 MiB admission-probe ceiling above, plus CPU
rlimits/wall deadlines. An app supervision thread polls RSS every 100 ms and kills above
the admitted total. Polling can overshoot; it is not a hard RSS guarantee.
Seccomp clone argument checks prohibit processes but cannot count threads.
Therefore degraded mode uses single-threaded FFmpeg and a fixed trusted watchdog
created before a final TSYNC filter that denies all further clone calls.
Do not claim a numeric thread cap from stateless BPF. Primary mode combines the
checked pthread flags below with `TasksMax`.
[Seccomp limitations](https://man7.org/linux/man-pages/man2/seccomp.2.html).

A scope does not automatically die with its creator. The parent-death chain
kills its members when the app dies, including when the supervisor has also
died. The supervisor can stop residual scope units when the app socket closes;
that cleanup is additional to the kill chain. The scope lifecycle follows its
member processes, not its creator
([systemd scope semantics](https://www.freedesktop.org/software/systemd/man/latest/systemd.scope.html)).

### Ordered bootstrap and host evidence

1. In the app-spawned shim, arm `PR_SET_PDEATHSIG=SIGKILL`, compare `getppid`
   with the captured app PID, and refuse if it changed. Call `setsid`, then
   perform the blocked resource join. A missing release, app EOF, or supervisor
   failure aborts bootstrap before any media parsing.
2. Unshare `CLONE_NEWUSER`, deny setgroups, map only the caller UID/GID, then
   enter `CLONE_NEWNS | CLONE_NEWNET | CLONE_NEWPID`. Re-arm the shim's PDEATHSIG
   and check the app parent after credential changes. Open the shim's self-pidfd,
   then fork so the payload enters the PID namespace. The payload immediately
   arms its own PDEATHSIG and performs the parent/pidfd race checks above.
   Repeat the arm/check sequence after any further credential change.
   Reuse the single-threaded fork pattern at
   `crates/kinewright-job-sys/src/namespace.rs:199`, called from
   `crates/kinewright-agent/src/job_guardian/linux.rs:131`. Retain/reap the outer
   bootstrap too. Keep loopback down. No media is parsed in either bootstrap.
3. Before pivoting, inventory fds, put command-read, response-write,
   lifetime-read, and source-read into fixed slots, map inherited rings, and
   close their fds. Close every other fd, including stdio, directory fds,
   sockets, the parent pidfd, resource controls, the app/supervisor socketpair,
   and release/verification pipes when spent. The waiting shim closes its
   copies of media/control/ring/lifetime descriptors after the fork.
   Verify the inventory while procfs is accessible. Make mounts private, pivot
   into an empty root, detach the old root, and set cwd to `/`. Mount no procfs,
   sysfs, devices, home, or project. Drop all capabilities.
4. Set `PR_SET_NO_NEW_PRIVS=1`, core/file-size limits to zero, `RLIMIT_NOFILE=16`,
   `RLIMIT_STACK=8 MiB`, and the address-space bound above. Set a 300 CPU-second
   `RLIMIT_CPU` ceiling. Hitting it is `TimedOut`; restart the unfinished job at
   its last validated checkpoint. No planned process rotation near the ceiling.
5. If Landlock ABI ≥3 is available, install a no-grants ruleset handling every
   filesystem access right known to the pinned implementation and supported by
   the detected ABI, including REFER/TRUNCATE. Add known network/scope rights
   when supported. Verify enforcement; record older/absent ABI or failure as
   defence-in-depth unavailable, never a reason to bypass namespace/seccomp.
   Already-open fds bypass path rules, hence the inventory and access modes.
   [Landlock ABI and rights](https://docs.kernel.org/userspace-api/landlock.html).
6. Install the table's seccomp filter, default-deny foreign audit architectures
   and x32, and filter full-width arguments. Primary-mode decode threads inherit
   it. In degraded mode, pre-created parked trusted threads receive it with
   TSYNC; cloning is denied thereafter. The watchdog terminates its thread
   group with `exit_group` on lifetime HUP, EOF, or NVAL. It is never the
   non-cooperative kill guarantee.
7. Pause before `PR_SET_DUMPABLE=0`. The host independently inspects `Seccomp`,
   `NoNewPrivs`, user/mount/network/PID namespace links, and cgroup membership
   against retained PID/starttime. Then complete the dumpable drop and readiness.
   User-namespace credentials may already prevent host access to namespace links.
   For every unobservable property, report that confinement rests on trusted
   bootstrap self-report plus negative tests, not independent host verification.
   `Ready` alone never establishes confinement.

Missing user namespaces or seccomp fails closed as `SandboxUnavailable`.
Other required boundary setup failures also refuse that launch; absence of
cgroups or Landlock alone uses the specified degraded mode. No in-process or
unsandboxed fallback or runtime kernel-policy change is permitted.

For Ubuntu's AppArmor restriction on unprivileged user namespaces, ship a
packaged-helper profile granting `userns` at the installed executable path.
Install and load it at package install time so the shim and its forked payload
can establish the boundary. The Ubuntu CI runner installs that same profile
with sudo in a CI configuration step before its availability gate. This is
installer/CI setup, not a change to Riel's machine or a runtime sudo prompt.
Without the profile, restricted Ubuntu reports `SandboxUnavailable`; it never
disables the restriction or falls back to host parsing. Riel's Arch machines
do not have this Ubuntu restriction. Slice 1a qualifies both the stock Arch
desktop and the Ubuntu runner, including the missing-profile refusal.
[Ubuntu AppArmor user-namespace policy](https://documentation.ubuntu.com/security/security-features/privilege-restriction/apparmor/#apparmor-unprivileged-user-namespace-restrictions).

### Seccomp syscall table

The default action is `SECCOMP_RET_KILL_PROCESS`, including unlisted syscalls,
rejected arguments, foreign audit architectures, and x32. The only policy errno
exception is the enumerated `clone3` → `ENOSYS` fallback below. Qualify this
allowlist against the packaged libc/FFmpeg; do not widen it from a generic trace preset.
Ring fds exist only before final lockdown; afterward their mapping rule has no
live fd to use. Descriptor numbers cannot be reused through opens or duplication.

| Syscall(s) | Allowed arguments / result after lockdown |
| --- | --- |
| `read` | arg0 exactly source-read, command-read, or lifetime-read fd; lifetime read is only for EOF detection |
| `pread64`, `lseek` | arg0 exactly source-read or command-read fd; no lifetime or arbitrary fd |
| `write`, `writev` | arg0 exactly response-write fd; no stderr |
| `mmap` | `PROT_EXEC` absent; anonymous with fd −1, or only a designated ring fd; never source-backed |
| `mprotect`, `pkey_mprotect` | `PROT_EXEC` absent, including mixed write/exec requests |
| `mremap` | Ruling 33: resize/move only the helper's own mappings; old_size strictly positive and flags exactly 0 or MAYMOVE (1), full width. FIXED, DONTUNMAP, duplication and high-bit aliases are killed. Native anonymous shrink/grow preserves bytes. [mremap(2)](https://man7.org/linux/man-pages/man2/mremap.2.html) |
| `munmap`, `brk`, `madvise` | Runtime memory only; madvise restricted to NORMAL (0), DONTNEED (4), FREE (8), GUARD_INSTALL (102), and GUARD_REMOVE (103), with exact full-width argument checks; every other advice value is killed. [madvise(2)](https://man7.org/linux/man-pages/man2/madvise.2.html) |
| `futex` | Exact op values `FUTEX_WAIT` (0), `FUTEX_WAKE` (1), `FUTEX_WAIT_PRIVATE` (128), `FUTEX_WAKE_PRIVATE` (129), `FUTEX_WAIT_BITSET` (9), `FUTEX_WAIT_BITSET_PRIVATE` (137), and those two bitset waits with `FUTEX_CLOCK_REALTIME` (265, 393). Bitset waits require arg5=`FUTEX_BITSET_MATCH_ANY`; deny every other operation/flag combination, including requeue, PI, wake-op, and wake-bitset |
| `clock_gettime`, `clock_nanosleep`, `nanosleep`, `sched_yield` | Reviewed runtime clocks/waits |
| `sched_getaffinity` | arg0 exactly 0, querying only the calling thread; all explicit PID/TID values and high-bit aliases are killed. Required by the pinned FFmpeg CPU-count query. [sched_getaffinity(2)](https://man7.org/linux/man-pages/man2/sched_getaffinity.2.html) |
| `poll`, `ppoll` | Lifetime-HUP watchdog; BPF cannot inspect pollfd arrays, but these calls grant no read/write authority |
| `fstat` | Source/command/response fd only; never path lookup |
| `close` | Only surviving source/command/response/lifetime fds |
| `getpid`, `gettid`, `getppid`, `getrandom` | Runtime identity/randomness only |
| `rt_sigaction`, `rt_sigprocmask`, `rt_sigreturn`, `sigaltstack` | Runtime signal management |
| `tgkill` | arg0 equals helper tgid captured at filter installation in its PID namespace |
| `clone` | Primary mode only: exact reviewed pthread flags `CLONE_VM \| CLONE_FS \| CLONE_FILES \| CLONE_SIGHAND \| CLONE_THREAD \| CLONE_SYSVSEM \| CLONE_SETTLS \| CLONE_PARENT_SETTID \| CLONE_CHILD_CLEARTID`, no exit signal; degraded mode denies |
| `clone3` | Sole `SECCOMP_RET_ERRNO(ENOSYS)` exception, to reach checked legacy clone; degraded mode still kills legacy clone |
| `set_robust_list`, `rseq` | Reviewed libc per-thread registration |
| `exit`, `exit_group` | Allowed |
| `kill`, `tkill`, `pidfd_send_signal`, `rt_sigqueueinfo`, `rt_tgsigqueueinfo` | Denied, including brute-forced external pids |
| `open`, `openat`, `openat2`, `newfstatat`, `statx` | Denied |
| `dup`, `dup2`, `dup3`, `fcntl`, `pidfd_getfd` | Denied, including every fd duplication route |
| `memfd_create`, `userfaultfd`, `personality`, `ioctl`, `pkey_alloc` | Denied |
| `prctl` | Only `PR_SET_DUMPABLE` with arg1=0 for the final monotonic drop; all other requests denied |
| `execve`, `execveat`, `fork`, `vfork`, `unshare`, `setns`, `seccomp` | Denied after bootstrap |
| `socket`, `socketpair`, `connect`, `bind`, `listen`, `accept`, `accept4`, `sendmsg`, `recvmsg` | Denied for all families |
| `ptrace`, `process_vm_readv`, `process_vm_writev`, `mount`, `bpf`, `keyctl`, `perf_event_open`, `io_uring_setup`, `io_uring_enter`, `io_uring_register` | Denied |
| Filesystem mutations and all remaining syscalls | Denied by default |

Slice-1a rulings 32 and 33 permit guard advice 102/103 and narrowly scoped
native-runtime additions. An added syscall or argument must act only on the
helper's own address space, threads, or signal state, touch no fd, path,
process, namespace, socket, credential, or external kernel object, and leave
confinement unchanged. Each addition needs a cited table row, a positive native
witness, and a report entry. All other policy changes require the lead's ruling.

Shared futex waits are required even for process-local pthreads.
[glibc's join path](https://codebrowser.dev/glibc/glibc/nptl/pthread_join_common.c.html#102)
waits for `CLONE_CHILD_CLEARTID` using `LLL_SHARED`.
Its [absolute-wait implementation](https://codebrowser.dev/glibc/glibc/nptl/futex-internal.c.html#83)
uses `FUTEX_WAIT_BITSET`, with the realtime flag for `CLOCK_REALTIME`. Hence a
private-only policy breaks `pthread_join` when the worker is still running.
Qualify the exact set against the packaged glibc, with a positive witness that
enters join before releasing a still-running worker. Decoder close and cancel
must also complete under the final filter. Unsupported futex operations remain
process-killing denials, not a generic futex allow.

Each named denial and each argument predicate gets its own negative witness,
including high argument bits, source/command/response fd swaps, ring/source mmap,
exec permission changes, wrong-tgid `tgkill`, and alternate clone flags. Signals
also attack synthetic parent, supervisor, and sibling processes with known and
brute-forced pids. Verify unchanged canaries and actual native refusal. Seccomp
denials assert whole-process `SIGSYS` termination; the separate clone3 witness
asserts `ENOSYS` and the checked legacy fallback.

## Windows confinement

Use the documented `CreateProcessW` LPAC launch with an unnamed Job and zero
capabilities. Register a profile once per user under the stable moniker chosen
for the media helper. Derive the expected SID with
`DeriveAppContainerSidFromAppContainerName` and check the SID returned at
registration. Grant that SID read/execute on the packaged helper and DLL
directory once at install or first run in a user-owned install. For an
admin-owned install, the installer grants access. Never grant the source
directory. Use `SECURITY_CAPABILITIES` with
`PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES` and
`PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY` set to
`PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT`.
[Microsoft's LPAC launch contract](https://learn.microsoft.com/en-us/windows/win32/secauthz/implementing-an-appcontainer).

Preparation can run again at app startup. Remove only known empty profile
directories with nonrecursive directory removes. Preserve unexpected content.
Set protected DACLs on `Packages\<name>`, any retained `AC` tree and its
contents, and the profile registry storage key. Remove every ACE for the
container SID, ALL APPLICATION PACKAGES, and ALL RESTRICTED APPLICATION
PACKAGES. Open the registry key by moniker under the current user's classes
hive at `Local Settings\Software\Microsoft\Windows\CurrentVersion\AppContainer\Storage`.
Production code does not impersonate a thread. Before each launch, recheck
registration, SID, and the protected, grant-free storage DACLs with a fixed
walk cap. An unreadable or over-cap tree refuses launch. Launch performs no
profile repair. Uninstall calls `DeleteAppContainerProfile`.

Use `STARTUPINFOEXW` with
`EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT`,
an explicit executable path, and the install directory as cwd. Pass
`SystemRoot` and a fresh host-owned empty directory as `LOCALAPPDATA`.
Windows rewrites the child environment for the profile. Detach from the
console and inherit only the listed handles. Do not request elevation.
Before `ResumeThread`, check `TokenIsAppContainer`, the expected AppContainer
SID, zero `TokenCapabilities`, and low integrity. The suspended token must
pass `AccessCheck` for read through its own package SID and fail the same
check through ALL APPLICATION PACKAGES. Successful creation alone proves
none of these facts.

If an additional restricted-token layer is necessary, compose a lowbox token
from the restricted base via `NtCreateLowBoxToken`, as in Chromium's reviewed
composition; its restricting-SID list includes the AppContainer SID so the
second access check can grant the install-directory read/execute right.
Do not substitute `CreateProcessAsUserW(restricted_token)` for verified LPAC.
[Chromium composition witness](https://chromium.googlesource.com/chromium/src/+/71104fd3/sandbox/win/src/restricted_token_unittest.cc)
and [restricting-SID access checks](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-createrestrictedtoken).

The host uses `CreateFileW` for the source and `GetFileInformationByHandleEx`
for identity/type checks. Inherit only narrowed read-only source, pipe, and
section handles through `DuplicateHandle`, `SetHandleInformation`, and
`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`. No source path or parent process, token,
Job, project, clipboard, GPU, or network handle crosses. Use
`InitializeProcThreadAttributeList`, `UpdateProcThreadAttribute`, and
`DeleteProcThreadAttributeList` for all creation attributes.
[Creation attributes](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute).

Use `CreateJobObjectW` and `SetInformationJobObject` before launch. Set
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, `JOB_OBJECT_LIMIT_ACTIVE_PROCESS=1`,
`JOB_OBJECT_LIMIT_PROCESS_MEMORY`, and `JOB_OBJECT_LIMIT_JOB_MEMORY` to the
admitted private budget. Allow no breakaway. Attach atomically with
`PROC_THREAD_ATTRIBUTE_JOB_LIST`, verify with `QueryInformationJobObject`,
and associate a completion port for memory/exit events. Resume only after
membership, handles, token, and limits pass host checks, using `ResumeThread`.
Set `JobObjectCpuRateControlInformation` with
`JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP`.
Reserve the CPU share before launch; provisionally cap each helper at
min(100%, 200% / logical-CPU-count) of machine capacity for video and
min(100%, 100% / logical-CPU-count) for audio/probe, rounded to API units.
Host inspection verifies the actual hard-cap rate alongside memory/process limits.
Use `TerminateJobObject` on timeout, then wait with `WaitForSingleObject` and
verify emptiness. Reserve shared sections outside the Job commit limit.
[Job memory limits](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_extended_limit_information).
[Job CPU rate contract](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_cpu_rate_control_information).

Set creation-time `PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY` for DEP, mandatory
ASLR, strict handle checks, extension-point disable, Win32k system-call disable,
prohibit dynamic code, and no remote/low-integrity image loading. Set
`PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY` to
`PROCESS_CREATION_CHILD_PROCESS_RESTRICTED`. Build with CFG and verify the
packaged MS1 DLLs. Use `GetProcessMitigationPolicy` for host inspection and
`SetProcessMitigationPolicy` for any bootstrap-only additions before parsing.
Do not require Microsoft-only signatures, which would reject our FFmpeg DLLs.
[Mitigation policies](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-setprocessmitigationpolicy).

Windows reads OS and runtime images. The prepared profile must give the
helper no writable private disk or registry storage. The helper's own
AppContainer named-object directory is accepted. Its objects are transient
and the Job admits one process. Native canaries must prove creation, growth,
and mutation are denied in reachable private, temp, profile, and registry
locations. If a runtime write is unavoidable, enforce and prove a hard quota
before admitting media. Expose no write broker. Also test named pipes, ALPC
endpoints, and `OpenProcess` against the host and a sibling helper. Stable SID
reuse grants no sibling IPC authority. Unsupported LPAC, mitigation, or
nested Job setup refuses launch.

Hosted runs 44512875269, 44523085254, and 44525476876 establish the launch
requirements and storage behavior on `windows-2025`. Creation without
registration returned Win32 2. Passing only `SystemRoot` with a registered
profile returned 203. Token class 46 returned Win32 87 and native status
`0xc0000003`. Added deny ACEs left the `AC` tree writable. Removing the
package grants made its files and directories refuse writes. The storage
registry key accepted a direct write as created and refused a direct write
after its package grant was removed. The child started with the `AC` tree
absent. The ordinary AppContainer could read an ALL APPLICATION PACKAGES
file. LPAC received access denied. All this evidence comes from an elevated
administrator account. A standard account and Windows 10 and 11 client
builds remain unproven (MS-T8).


## Hardware decode and software cost

At this baseline video opens `.decoder().video()` in `decode.rs:2059`, with
software conversion at `:2097`. No production `av_hwdevice_ctx_create`, hw-frame
context, VAAPI, CUDA, D3D11VA, DXVA, QSV, or NVDEC setup was found in `crates/`.
`scripts/pf1-probe-replay.sh:32` records historical CLI hwaccel timings, not an
app hardware-decode path. GPU compositing is a separate existing operation.

MS2 grants no GPU/device access. Hardware decode inside a process sandbox is
possible with device and driver access, but would expand the kernel/driver
boundary and invalidate this fd-only policy. Software decode is the baseline,
so MS2 does not remove an existing hwaccel speedup. Its incremental costs are
process startup, IPC, bounded copies, and reduced decoder concurrency.
4K60 HEVC/AV1 may still be CPU-limited. Use lower-resolution previews or create
local proxies through sandboxed decode followed by host encoding. Full-quality
export can run slower than real time. Never enable in-process/hardware decode
as an automatic performance fallback. Device-enabled decode needs another
security design and driver-specific evidence.

## Failure, recovery, and existing children

Introduce typed `MediaSandboxError` variants `SandboxUnavailable`, `Crashed`,
`TimedOut`, `ResourceLimit`, `ProtocolViolation`, `UnsupportedMedia`, and
`Cancelled`, with host-owned source/clip/request identity. Classify OOM from
native cgroup/Job evidence when available; otherwise report a crash, not guessed
OOM. A failed helper affects all clips sharing that source lease, not the app.
Preserve the last validated preview or show an unavailable-media placeholder.
Block export completion on an affected source and retain the prior output.

UI and agent status use the same structured failure. For example, show
"Media decoder stopped for clip A. Project edits are safe. Retry or relink the
source." Offer Retry, Relink, and Remove clip through ordinary operations.
Never delete the source or clip automatically. Discard unfinished analysis and
cache entries; publish completed entries atomically from the host only. Helper
failures must leave Save, undo, recovery, and editing unrelated clips usable.

Allow one automatic restart per source in five minutes for an unexpected crash,
only after verified cleanup. After a second crash, OOM, or protocol
violation, quarantine that source for the session and require explicit Retry or
Relink. A slow source's `TimedOut` remains retryable at its checkpoint and does
not count as hang quarantine. Repeated no-progress failures with independently
established hang evidence require explicit Retry, preserving the slow-decode
distinction. No automatic retry for malformed output or sandbox failure. Explicit
retries have a one-second cooldown and at most three launches per minute.
The global launch circuit breaker (ruling 35) spends a token only on an
unhealthy launch outcome across all sources: crash, OOM, protocol violation,
`TimedOut`, sandbox failure, malformed output, or a typed malformed-input
refusal. Healthy results, cancellation and authority loss spend nothing. The
rates are at most two/s, burst four, and 20 per rolling minute. On exhaustion,
admission pauses: the queue is cleared and every queued request is refused with
the 60 s cooldown, and so is every new request during it. The first request
after the cooldown resumes admission; there is no timer drain and no resume
control. The queue is bounded (32 requests; over the bound a typed retryable
`QueueFull`), and a queued request waits at most 120 s (`QueueWait`). Its
deadline starts at launch, never at enqueue. Refusal text says what happened and
when to try again. A project of hundreds of hostile sources therefore pauses
after a handful of failures, never drains endlessly.
The app's supervision thread retains the reservation until the process is
reaped and native membership is empty. Cleanup failure retains ownership and
blocks replacement.
On Linux, app loss triggers the app → shim → payload PDEATHSIG chain, independent
of the scope supervisor and cooperative watchdog. On Windows, app loss closes
the Job handle and triggers kill-on-close.

Reuse process ownership and native cleanup in `kinewright-job-sys`, especially
`src/linux.rs:121` and `src/windows.rs:261`. Extend that crate with an explicit
media launch profile, passed handles, and limits. Existing `Job::verify` at
`src/windows.rs:295` currently expects only kill-on-close; media needs its own
validated configuration, not a weakened check. Do not add a second Job/cgroup
owner or make media depend on `kinewright-agent`.

The guardian transport at
`crates/kinewright-agent/src/orchestration/guardian.rs:139` uses a Linux user
service and keeps cleanup proof at `:478`. Its Windows tree calls suspended
Job launch in `crates/kinewright-agent/src/job_guardian/windows.rs:20`.
Orchestrator task sessions go through `ContainedDriver::start_session` in
`crates/kinewright-agent/src/orchestration/adapters.rs:126`; task-child state is
tracked at `crates/kinewright-agent/src/orchestration/tasks.rs:502`.
Reuse the contained-before-release and reap-before-slot-release discipline.
Provider admission, credentials, execution claims, and task accounting do not
belong in the media protocol. Existing namespace setup at
`crates/kinewright-job-sys/src/namespace.rs:44` does not add a network namespace
or this syscall policy. Existing child containment alone is not MS2 confinement.

## Crate choices

Split helper-only decoder/conversion code from the current media crate so the
helper does not link wgpu, cpal, Whisper, reqwest, or an agent runtime. Add a
small IPC crate containing wire types and host-side validation without FFmpeg.
Keep narrow OS operations in job-sys. No Python/Node runtime ships with MS2.

| Option | Decision and reason |
| --- | --- |
| [`landlock`](https://docs.rs/landlock/latest/landlock/) | Add a pinned direct Linux dependency. Require enforcement of selected known rights when available; log absent ABI/policy as defence in depth unavailable. Namespace/seccomp remain load-bearing. |
| [`seccompiler`](https://docs.rs/seccompiler/latest/seccompiler/) | Add a pinned Linux dependency, with JSON policy loading disabled. Typed argument rules compile the reviewed policy to BPF; the policy remains our responsibility. |
| [`zbus` 5.19.0](https://docs.rs/zbus/5.19.0/zbus/blocking/) | Use the blocking API for MS2 only in the separate Linux supervisor binary, with `default-features = false` and `async-io`, `blocking-api` enabled. It supplies D-Bus authentication, framing, signatures, and typed systemd calls. Keep MS2's lifecycle code and runtime use in that binary. A hand-rolled D-Bus client adds protocol/security risk without a confinement benefit and is rejected. |
| App's transitive `zbus` through eframe/AccessKit/AT-SPI | accessibility only, pre-existing, not an MS2 client. Keep app features unchanged. Ruling 34 permits the supervisor's pin to update this transitive dependency; Cargo.lock has one zbus version. |
| [`extrasafe`](https://docs.rs/extrasafe/latest/extrasafe/) | Decline. It combines syscall, Landlock, and isolation helpers, but a second policy abstraction obscures our exact descriptor/argument rules. |
| [`birdcage`](https://docs.rs/birdcage/latest/birdcage/) | Decline. Its Linux/macOS process sandbox does not supply Windows confinement and duplicates launch ownership. |
| [`nix`](https://docs.rs/crate/nix/latest) | Decline as a new dependency. Existing job-sys uses rustix for these Unix operations. |
| [`windows`](https://microsoft.github.io/windows-docs-rs/doc/windows/) | Useful typed Win32 bindings, but extend the existing pinned `windows-sys` features in job-sys instead of introducing another binding layer. |

The selected zbus dependency tree has the following branches. Every direct
Linux dependency is listed; repeated dependencies and lower-level runtime and
proc-macro leaves are collapsed. Slice 1a records the complete qualified Linux
normal/build tree and the license/checksum inventory in
`docs/reports/ms2-s1a-evidence/supervisor-locked-tree.txt` and
`supervisor-dependencies.json`. The latter records all 99 packages and their
resolved features; it verifies no FFmpeg in the supervisor and no zbus in the helper.

```text
kinewright-media-supervisor (Linux binary crate, no FFmpeg)
└── zbus 5.19.0 [async-io, blocking-api]
    ├── zbus_macros 5.19.0 (build-time proc macro)
    │   ├── proc-macro-crate, proc-macro2, quote, syn
    │   └── zbus_names, zvariant, zvariant_utils
    ├── zbus_names 4.3.5
    │   └── serde, winnow, zvariant
    ├── zvariant 5.15.1 [enumflags2]
    │   ├── endi, enumflags2, serde, winnow, zcheapstr
    │   └── zvariant_derive, zvariant_utils (shared proc-macro dependencies)
    ├── async-broadcast, async-executor, async-io, async-lock, async-process
    ├── async-recursion, async-task, async-trait, blocking
    │   └── shared runtime branches: async-channel, async-signal, concurrent-queue,
    │       crossbeam-utils, event-listener-strategy, fastrand, futures-io,
    │       parking, pin-project-lite, piper, polling, signal-hook-registry, slab
    ├── enumflags2, event-listener, futures-core, futures-lite, ordered-stream
    ├── serde, serde_repr, hex, tracing, uuid, winnow
    └── libc, rustix (Linux OS bindings, including linux-raw-sys and bitflags)
```

This inventory comes from the
[published zbus 5.19.0 manifest and release lockfile](https://crates.io/api/v1/crates/zbus/5.19.0/download).
`async-fs` is a feature name here, not an additional crate dependency. Disable
Tokio, vsock, p2p/bus-implementation, and optional wire-format integrations.
The blocking API still drives an async backend; budget that runtime in the
supervisor allowance. In slice 1, record the complete locked normal/build tree
for these features. The helper has no zbus dependency. Ruling 34 permits the
app's pre-existing accessibility dependency; MS2's systemd manager client
exists only in the supervisor. CI runs `scripts/ms2_client_boundary.py` and
its red witnesses: no other workspace crate may declare a direct zbus
dependency, app source may not mention `org.freedesktop.systemd1`, and the
lockfile contains one zbus version. Do not change app accessibility features.

Review dependency licenses, release provenance, feature sets, and the locked
transitive graph in slice 1. FFmpeg/OS FFI and shared mappings require justified
unsafe wrappers. No unsafe, clone, or Arc solely to satisfy the compiler.

## Proof gates and measured budgets

Run these later, on both native platforms. None has been run for this design.

- A deliberately malicious helper uses the real launcher and tries to read
  another file, reopen a source path, write/truncate the source, write outside
  its response pipe/ring, reach Internet and loopback, create AF_UNIX/VSOCK/
  NETLINK sockets, exec, fork, access parent memory, and duplicate hidden handles.
  On Windows also attack profile storage boundaries, registry, clipboard,
  remote image loading, and nested Job breakaway. Use synthetic canary files
  and listeners. Assert native denial and unchanged canaries, not just exit.
  Give each mechanism an isolated denial witness: seccomp with masking namespace/
  Landlock protections absent, Landlock alone where applicable, PID namespace
  signal isolation, and independent memory/CPU/job/handle restrictions. Removing
  only that mechanism must change its specific refusal, not leave another guard
  masking the result. Separately run full-stack attacks and negative controls
  with deliberate combinations removed. No real credentials or changes to the
  desktop's host policy; the Ubuntu CI profile install is the explicit setup exception.
- Probe bootstrap ordering with a helper that attempts each attack before its
  first request. Inspect every inherited descriptor/handle and token/namespace
  property independently. Missing user namespaces/seccomp, invalid required
  mitigations, and corrupted bundles must refuse before parsing. Absent Landlock
  logs reduced defence in depth; missing controllers/user manager must exercise
  degraded rlimits/RSS enforcement successfully. Gate both the stock Arch
  desktop and Ubuntu CI runner with its installed AppArmor profile. Removing
  the profile on the runner must produce `SandboxUnavailable` before parsing.
  Native Windows storage writes must be denied or hit a proven hard
  quota before any source is read; there is no unbounded private-storage exception.
- Fuzz the host's actual framing/parser/state validator with cargo-fuzz/libFuzzer
  and sanitizers. Include truncation, huge lengths, recursion, duplicate fields,
  integer wrap, stale IDs, NaN/Inf/subnormal PCM, non-finite summary metadata,
  wrong positions/formats, nested duplicate JSON keys, out-of-order messages,
  floods, ring overwrite during copy, and fake progress. Require no panic,
  out-of-bounds access, unbounded allocation, or project mutation.
- Inject access violations/SIGSEGV, abort, CPU spin, memory exhaustion, pipe EOF,
  parent/supervisor loss, and cancellation at each phase. Assert typed UI and
  agent errors, responsive Save/undo, identical saved project content after
  reopen, unchanged previous export, empty native membership, and bounded
  respawn. Leave a deliberately surviving child to prove cleanup blocks reuse.
- With the helper watchdog disabled, kill the app before readiness and during
  decode, and kill the supervisor before readiness and during decode. Run all
  four cases in both primary scope and degraded resource modes. Assert shim
  and payload death and empty scopes in primary mode. Where the app survives,
  assert reaping, refused launches until supervisor restart, and reservation
  recovery only after cleanup.
  Add a barrier before payload PDEATHSIG arming, kill the shim there, and assert
  that the inherited pidfd closes the PID-0 race. Assert the supervisor's cgroup
  path is outside every helper scope. These are slice-1a gates, not watchdog tests.
- Under the final seccomp filter, join a worker that is still running and
  exercise decoder close and cancel. Assert success, then test forbidden futex
  operations and the kill-by-default action independently.
- In slice 1a, pass valid video and still IX admission end to end, including
  bounded first-frame decode, still orientation, and host interpretation checks.
  Keep malformed-input refusals too. Record probe PSS and address-space peaks
  for 1080p, UHD, and stills before freezing the probe's resource limits.
- Compare colour/rotation/timestamp/sample-boundary output with baseline golden
  fixtures. Exercise all call sites above, including project admission, mixed
  timelines, exported-artifact checks, cache hits, and background analyses.

`LoudnessMeter` at `crates/kinewright-media/src/loudness.rs:381` retains per-channel
energies plus ends, blocks, and short-term series. It is not constant-memory.
In slice 4 use a charged 32 MiB history/snapshot/sort-work cap within the audio
private reservation, checked before every growth/allocation. Exceeding it returns
typed `ResourceLimit(LoudnessHistoryLimit)` with no partial final loudness result.
Qualify accepted-length results against the existing meter and EBU fixtures.
An exact bounded algorithm can replace this only after equivalent qualification;
do not describe today's incremental update as bounded retained memory.

Use release binaries named by commit and SHA-256, the same MS1 FFmpeg artifact,
and local hashed H.264, HEVC 10-bit, AV1, still, malformed, and multi-hour audio
fixtures. Pair at equal thread counts/frame sizes to isolate transport overhead,
and separately against the shipping `default_threads` configuration (frame
threading up to 16 at `crates/kinewright-media/src/decode.rs:1786`).
Do not wait for a quiet desktop. Record other load and run no build alongside
timing. Report cold startup separately from warm requests, p50/p95 over at least
30 launches/seeks, and five paired 60-second throughput lanes per video case.

| Gate | Proposed number to hit |
| --- | --- |
| Sustained decode plus transport, equal threads | At least 90% of baseline fps; separate from shipping-thread gate |
| Absolute shipping-config H.264 4K60 gate | Sandbox sustains 60 fps when shipping frame-threaded baseline reaches 60; otherwise report measured absolute fps and capacity failure |
| Copy p95 | Measure same-size two-copy memcpy ceiling on this machine, both hot/cold and two-source contention; freeze a p95 threshold at 1.5× that measured two-copy p95 before implementation; no assumed 4 ms |
| First frame, cold source | Include probe + reap + fresh decode launch; added p95 at most 150 ms, absolute p95 at most 500 ms on reference H.264 |
| Warm random seek | Added p95 at most 25 ms; absolute p95 at most 250 ms for GOP 60 and 1 s for GOP 250 |
| Waveform / admitted loudness | At least 90% baseline samples/s; waveform bounded, loudness obeys charged history cap with typed refusal |
| Three-/four-angle multicam | Three and four distinct 1080p30 sources; sustain selected output at 30 fps with switch p95 ≤250 ms, two helpers only; also record 4K capacity |
| Two distant playheads, one source | Alternate GOP-60/GOP-250 positions; p95 ≤250 ms / 1 s respectively; no extra helper or unbounded pending work |
| Source-switch churn | 100 switches; record cold/warm p95, launch-breaker pauses, queue/cancellation stability, and reservation recovery |
| Resource use | Provisional private peaks 96 MiB audio / 384 MiB video; rings separate; primary kernel totals hold, degraded RSS overshoot measured explicitly |
| Cancellation and kill | Acknowledge or terminate within 250 ms; native emptiness and reap within a further 2 s |
| Idle/leak test | After 100 open/seek/cancel/close cycles, host retained bytes within 8 MiB of start; zero helpers after idle expiry |

For a packed frame of size S and measured memcpy payload throughput B, two
copies cost at least 2S/B; account for four S of memory reads+writes when the
ceiling is expressed as bus bandwidth. Publish that floor beside measured p95.
Measure worst-case compliant codec/DPB streams before freezing reservation rows.
For multicam shortfalls, scale reservations by raster before considering any
increase above two helpers. Record failed targets and review the design or
limits before shipping; do not relabel an unmeasured estimate as evidence.
Publish CPU time, copy bandwidth, PSS/commit, and two-source contention as well as fps.

## Build slices

| Slice | Required deliverable and gate |
| --- | --- |
| 1a, Linux confinement + probe | Implement source admission, blocked shim scope join/degraded limits, namespaces/seccomp, app-parented shim/payload PDEATHSIG, the separate zbus scope supervisor and typed lifecycle contract, and isolated/full-stack attack witnesses end to end. Migrate all Linux import/project probes, including `crates/kinewright-project/src/owned_media.rs:415`. Include bounded first-frame decode, admission-required still inspection, and the replacement allocation sub-budget now. Return metadata only, no PCM/pixel rings. Measure 1080p/UHD/still probe PSS and address-space peaks before freezing probe limits. Pass video and still IX admission success fixtures plus malformed refusals, final-filter join/close/cancel, and watchdog-disabled app/supervisor-loss witnesses in both modes. Assert supervisor cgroup separation. Ship the install-time AppArmor profile, install it on Ubuntu CI, and qualify stock Arch plus Ubuntu availability and missing-profile refusal. |
| 1b, Windows confinement + probe | Prepare a stable per-user LPAC profile with an install read/execute grant and protected, grant-free private storage. Verify the profile before each launch and the actual token, Job, CPU cap, mitigations, and handle inheritance before resuming. Exercise storage, network, named-pipe, ALPC, and process refusals before end-to-end probe protocol and IX admission. Hosted launcher evidence covers elevated `windows-2025`. Qualify standard accounts and Windows 10 and 11 client builds in MS-T8 before making a Windows safety claim. |
| 2, waveform | First streaming consumer on both platforms. A validated probe precedes a fresh source-bound measurement helper. Stream bounded peak summaries through credited binary `DataChunk` frames. Add cancellation/checkpoint/launch-breaker witnesses and long-audio memory qualification. No loudness yet. |
| 3, frames | Add agreed-geometry video and PCM rings for preview, playback, timeline rendering, and export source reads. Freeze ±64.0 PCM validation, preserve sample/colour/rotation contracts, and refuse mid-stream geometry changes. Gate snapshot-copy/parser fuzzing, shipping-config absolute fps, copy ceiling, two playheads, multicam, and churn. |
| 4, loudness/thumbnails/rest | Add charged-cap loudness, source thumbnails, still pixel output beyond admission inspection, recognition extraction, scene/beat/silence analysis, room tone, and cache inspection. Thumbnail pixels use bounded binary chunks, ≤256 KiB total per thumbnail, never encoded images for host decode. Delete all direct input/decode host APIs. Complete all-callers audit, native escape tests, affected/dependent-crate gates, memory soak, and security review. |

Each slice has its own security gate. Until slice 4, unmigrated operations retain
their existing exposure; do not advertise complete media isolation. Partial
rollout adds no legacy mode or bypass switch. MS1 remains required. Linux IX
admission is confined at 1a, Windows at 1b; arbitrary subsequent media use is not
fully isolated until the remaining consumers migrate.

## Decisions and remaining qualification

Availability is decided. Missing user namespaces or seccomp fails closed;
missing cgroups uses degraded rlimits/RSS supervision, and absent Landlock logs
reduced defence in depth. Restricted Ubuntu needs the packaged install-time
AppArmor profile, also installed by CI. Probe memory numbers remain provisional
until the slice-1a PSS/address-space measurements; the admission first-frame and
still work and replacement sub-budget are part of that slice.
The native Windows spike comes first in 1b. The PCM wire limit is ±64.0 before
slice 3. Start with two helpers; measure three-/four-angle multicam and scale
reservations by raster before raising the process cap. These are decisions and
proof obligations, not unanswered product choices.

Round-1 technical corrections are explicit above. Stateless seccomp cannot
enforce a numeric thread count (ruling 3), and a systemd scope
does not automatically follow its creator's lifetime (ruling 5). The fixed
thread set and app-parented shim/payload PDEATHSIG chain provide the missing
mechanisms.
Ruling 19's UHD reservation is corrected to 3,649 MiB / 3.563 GiB.
Round 2 adopts the lifecycle-only zbus supervisor, shared futex forms,
kill-by-default seccomp, and host-owned byte-identity guarantee. Ruling 24's
payload `getppid` race check is corrected with an inherited shim pidfd because
namespace init sees an outer parent as PID 0; the platform evidence is above.
No new product question is opened by this document revision.
