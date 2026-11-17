# MS2 media sandbox

Status: proposed design for security-tier review. Source baseline is
`origin/main` at `266f8890e259fb20d4c8f10d778b5b5fed6b3257`.
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
| `crates/kinewright-project/src/owned_media.rs:416`, `:451`, `:457`, `:471` | Replace certified admission/probe/decode calls with helper requests. Keep pinning, interpretation validation, and byte identity in the host. |
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
| `crates/kinewright-media/src/frame.rs:47` | Keep safe Rust conversion of validated RGBA64 pixels into the existing scene-linear working frame. |

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

The host supervisor admits at most two helpers across preview, export, and
analysis. Playback has priority. Background work yields at chunk boundaries.
Reserve capacity before launch. Keep at most 32 pending requests globally and
one active plus one replaceable pending request per consumer. Coalesce obsolete
seeks. Evict an idle helper after five seconds or immediately under pressure.
A different source always requires a fresh process. No warm cross-file reuse.

Proposed budgets are targets and admission limits, not measured facts.

| Reservation | Per helper | Global bound |
| --- | --- | --- |
| Audio/measurement helper, including runtime | 128 MiB hard private/charged budget | Two helpers, 256 MiB |
| Video helper including native buffers and its ring | 512 MiB hard reservation | Two helpers, 1,024 MiB |
| Video ring, included in the preceding row | Two slots of at most 64 MiB | 256 MiB, counted once |
| Host snapshots/conversion scratch | At most 128 MiB per active video consumer | 256 MiB |
| Accepted source-frame cache | Shared byte ledger, no per-clip duplication | 128 MiB |
| Bootstrap, IPC, and supervisor allowance | Account before spawning | 128 MiB |

The maximum MS2 reservation is 1,536 MiB, including host buffers and bootstrap.
This excludes existing project data, Whisper models, compositor VRAM, and output
encode buffers, which retain their own budgets. Merge source caches into the
existing media ledger so this is not an additional uncounted cache allowance.
Target actual peak audio RSS at 96 MiB and video RSS at 384 MiB including rings.
Use one helper on systems that cannot reserve the two-helper maximum. Refuse new
work with `ResourceLimit` if even one reservation cannot fit. Do not retry OOM
with a larger automatic allowance. Large reference sets and full-resolution
frames above 64 MiB may be refused. Preview may explicitly request a smaller
frame; export must refuse rather than silently reduce resolution or bit depth.

Start with two FFmpeg decode threads per helper, one for measurement. Admit no
more than four decode threads globally. Keep reference frames and conversion
buffers inside the reservation. Reconcile `owned_io.rs:27`'s existing 24-buffer
bound and reservations with this limit; never charge both ledgers independently
or remove the conservative bound without allocation evidence.

Measure private bytes, shared mappings, PSS, and peak RSS separately on Linux.
Record cgroup `memory.current`, `memory.peak`, and `memory.events` as well.
On Windows record private commit, working set, mapped-section sizes, and Job
peaks. Job commit is not RSS and may exclude shared sections. Reserve every
parent-created section separately. Check two distinct 4K sources, 100 clips of
one source, long audio, cancellation churn, and repeated source replacement.

## IPC treats the helper as compromised

Use two inherited anonymous pipes for control, one in each direction. No TCP,
localhost server, named rendezvous, descriptor-passing command, or path-bearing
request exists after bootstrap. Reuse the bounded framing discipline in
`crates/kinewright-agent/src/job_guardian/protocol.rs:163`, in a dependency-light
IPC crate. Do not depend on the agent crate from media.

Each control frame has a fixed header containing magic, exact build protocol
identifier, message tag, payload length, request ID, and source generation.
The payload is strict typed JSON using existing serde dependencies. Cap a
control frame at 64 KiB before allocation, nesting at eight, streams at 16,
waveform peaks at 2,048, text at 256 bytes, and outstanding responses at two.
Reject unknown/duplicate fields, unknown enum tags, trailing data, invalid
UTF-8, arithmetic overflow, and a different build protocol. There is one
current protocol, no version ladder or mixed-build negotiation.

Requests are `Probe`, `MeasureAudio`, `Thumbnail`, `ReadVideo`, `ReadAudio`,
`Seek`, `Cancel`, `ReleaseSlot`, and `Close`. Responses are fixed-state variants
such as `Ready`, `ProbeResult`, `SummaryChunk`, `FrameReady`, `AudioReady`,
`Complete`, and `Failed`. Decoder/filter names are host-owned enums. No helper
message can request a file open, command, allocation, or project edit.
`Ready` is a sequencing message, never sufficient evidence of confinement.

The host validates IDs, owner authority, state transitions, stream selection,
positive rational denominators, checked timestamps, dimensions, colour enums,
counts, and response cardinality before constructing domain values. It supplies
asset IDs and cache keys itself. Never trust a helper's hash as source identity.
Budget cumulative summary count/bytes per requested range and publish pages
incrementally; a 64 KiB per-message limit alone does not bound a long analysis.
Rate-limit progress to four messages per second. A flood is a protocol error.

### Frame and audio transport

Use a host-created shared-memory ring with two fixed slots per video helper.
The app chooses dimensions, slot sizes, and format after bounded probing.
Transport packed RGBA64LE, one plane, for managed source video, preserving the
existing conversion into `WorkingFrame`. Transport RGBA8, one plane, only for
explicit display thumbnails. No NV12/P010 or native decoder planes in MS2.
Those save bandwidth but would move colour conversion or more FFmpeg into the
host. Keep existing colour refusals and golden-frame results unchanged.

Dimensions are nonzero, at most 8,192 per axis, and subject to the 64 MiB slot
limit. Compute packed strides and lengths with checked arithmetic in the host.
`FrameReady` carries a slot index, sequence, timestamp, and agreed format ID,
never a pointer or helper-selected offset/stride. Each slot is single-use until
`ReleaseSlot`. With no credits, the helper stops producing; it cannot allocate
an overflow queue. The host owns all counters outside shared memory.

On Linux use `memfd_create`, size it in the host, and apply
`F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_SEAL`. The helper maps writable data, then
closes the section fd before lockdown. The app maps read-only. Do not claim
`F_SEAL_WRITE` while a writable ring is required. These seals prevent resize,
not malicious pixel modification. [memfd semantics](https://man7.org/linux/man-pages/man2/memfd_create.2.html).
On Windows use unnamed `CreateFileMappingW(INVALID_HANDLE_VALUE, ...)` sections
and `MapViewOfFile`, with minimal duplicate-handle rights, no executable
mapping, and no resizing authority. [Mapping API](https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-createfilemappingw).

A compromised helper can overwrite a credited slot at any moment. Copy its
fixed byte range once into host-owned storage, then validate and consume that
snapshot only. Do not lend Rust references into writable shared memory or let
wgpu/DSP read it directly. Confine the necessary raw mapping/copy operations
to an audited OS wrapper with a memory-safety argument. Slot acknowledgements
provide back-pressure, not trust or immutable ownership.

Count two transport copies for an RGBA64 frame: converted helper plane into
the ring, then ring into the host snapshot. Conversion into the working frame
and GPU upload are additional operations. At 3840 × 2160 × 8 bytes × 60 fps,
one stream carries 3,981,312,000 bytes/s, about 3.71 GiB/s. Two transport copies
move 7.96 GB/s of payload and roughly 15.93 GB/s of memory reads plus writes.
A same-size GPU upload adds about 3.98 GB/s, before working-frame conversion.
These are arithmetic estimates, not measured bandwidth. RGBA8 halves these
figures but is unsuitable as a replacement for the managed colour path.

PCM uses two slots of at most 256 KiB, little-endian interleaved f32, at an
explicit requested rate of 16 or 48 kHz, at most eight channels and 8,192 sample
frames per slot. Check exact lengths, sample positions, finiteness, and an
explicit amplitude bound before DSP. Reject unsupported amplitudes rather than
silently clamp. Whisper uses 16 kHz mono windows with bounded overlap; it never
accumulates the entire recording. Pure measurements return summaries without a
PCM ring. Cache writes and transcript merging remain host operations.

### Seeking, cancellation, and deadlines

A seek advances a host-owned generation, flushes pending slots and codec state,
and carries an integer source-frame target and rate. Decode from a keyframe
inside, returning the actual source time. Preserve existing integer-frame,
rotation, colour, and sample-boundary semantics. Reject stale generations even
when the old result is structurally valid. Source/owner revocation cancels the
lease, kills its helper, and invalidates outstanding results and cache grants.

Allow five seconds for startup/initial probe, ten seconds for first frame or
seek, and two seconds without requested streaming progress. Whole-file scans
use a separate explicit analysis request, with a 30-second chunk deadline and
checkpoint, never an unlimited extension of a probe. App deadlines count useful
validated output, not heartbeats. Suspend stream deadlines while the host has
withheld all credits. Cancellation has a 250 ms cooperative grace through the
AVIO interrupt flag. Then terminate, reap, and discard all partial results.
A separate supervisor thread enforces deadlines even inside a stuck FFmpeg call.

## Linux confinement

Use a single-threaded trusted bootstrap after exec. Load only the packaged
helper and MS1 libraries, eagerly resolve imports, clear loader/plugin/proxy
variables, and initialize required runtime state without reading media. Resolve
the executable from the package, never PATH or the project's working directory.
The host opens one regular local file `O_RDONLY | O_CLOEXEC`, rejects devices,
FIFOs and sockets, and passes the descriptor, not a filename. Preserve the host's
pinned snapshot, digest, selected-stream, and revocable-authority checks from
`crates/kinewright-project/src/owned_media.rs:691` and `owned_io.rs:33`.
The helper can verify chunks, but its own verification cannot authorize host
publication. Writable external sources require host admission to an immutable
snapshot first. Do not mmap media files where truncation can SIGBUS the app.

Before parsing, perform the following ordered bootstrap.

1. Join a host-retained cgroup v2 leaf before release. Require working memory
   and pids controllers. Set `memory.max` to the admitted budget,
   `memory.swap.max=0`, `memory.oom.group=1`, and `pids.max=8`.
   The app stays outside. Verify controls and cleanup ownership. Existing
   user-service delegation can supply the subtree; absence fails closed.
   [cgroup controls](https://docs.kernel.org/admin-guide/cgroup-v2.html).
2. `unshare(CLONE_NEWUSER)`, deny setgroups, and map only the caller UID/GID.
   Enter private mount and network namespaces with `unshare(CLONE_NEWNS |
   CLONE_NEWNET)`. Make mounts private, pivot into an empty root, detach the old
   root, and set cwd to `/`. Mount no procfs, sysfs, devices, home, or project.
   Leave loopback down. Drop all capabilities after namespace setup.
3. Put only command-read, response-write, lifetime-read, and source-read fds
   into fixed slots. Map rings, close their fds and every other inherited fd,
   including stdio, sockets, directory fds, and cgroup controls. Use
   `close_range` over the complement and verify the inventory before removing
   procfs access. [Descriptor closure](https://man7.org/linux/man-pages/man2/close_range.2.html).
4. Set `PR_SET_NO_NEW_PRIVS=1`, `PR_SET_DUMPABLE=0`, and `RLIMIT_CORE=0`,
   `RLIMIT_FSIZE=0`, `RLIMIT_NOFILE=16`, `RLIMIT_STACK=8 MiB`, and
   `RLIMIT_AS=1 GiB` for video or 256 MiB for audio. A 300 CPU-second
   `RLIMIT_CPU` ceiling retires a busy helper at a checkpoint; rotate before
   that ceiling. The host's wall deadlines handle hangs independently.
   `RLIMIT_RSS` is not a Linux memory limit and is not used as one.
5. Install a Landlock ruleset that handles filesystem rights supported by the
   required policy and grants no path access. No rule grants the source path:
   its already-open read-only fd is the capability. Opened fds bypass Landlock
   path checks, so the inventory and access modes are essential. Require
   enforcement; never accept the crate's default best-effort success as proof.
   [Landlock semantics](https://www.kernel.org/doc/html/latest/userspace-api/landlock.html).
6. Install seccomp-bpf before creating decode threads. Default-deny syscalls,
   reject foreign audit architectures and x32, and allow only bounded runtime
   operations. Allow reads/preads/seeks on the source and control fds, writes
   only on the response pipe, non-executable anonymous memory, clocks, futexes,
   signals, and exit. Deny open/openat/openat2, filesystem mutation, ioctl,
   execve/execveat, fork/vfork, sockets of every family, connect, ptrace,
   process_vm access, pidfd_getfd, mount, bpf, keyctl, perf, and io_uring.
   Permit legacy clone only with the exact reviewed pthread flags including
   CLONE_THREAD; return ENOSYS for clone3 so libc uses that checked path.
   Filter full argument widths. All new threads inherit restrictions.
   [seccomp semantics](https://man7.org/linux/man-pages/man2/seccomp.2.html).

Only then acknowledge bootstrap and accept a request. Keep the syscall policy
explicit and based on traces plus negative tests, not a broad FFmpeg preset.
Landlock ABI absence, disabled user namespaces, seccomp failure, or unavailable
resource enforcement yields `SandboxUnavailable`. No unsandboxed fallback,
sudo setup, kernel-policy change, or in-process retry is permitted. This is a
platform support requirement, not a backward-compatibility implementation.

## Windows confinement

Require a less-privileged AppContainer (LPAC), layered with a restricted primary
token and an unnamed Job. Do not fall back to a restricted token alone.
Use `CreateAppContainerProfile` with a per-launch identity, zero capabilities,
and `DeriveAppContainerSidFromAppContainerName` only to recover that identity.
`DeleteAppContainerProfile` follows proven process cleanup. Grant read/execute
only to the packaged helper and its DLLs using `SetNamedSecurityInfoW`, never
to the source directory. Use explicit `SECURITY_CAPABILITIES` with
`PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES` and
`PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY` set to
`PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT`.
[Microsoft's LPAC launch contract](https://learn.microsoft.com/en-us/windows/win32/secauthz/implementing-an-appcontainer).

Use `OpenProcessToken`, `CreateRestrictedToken(DISABLE_MAX_PRIVILEGE, ...)`,
disable administrative groups, and remove remaining unnecessary privileges.
Set a low integrity label with `SetTokenInformation(TokenIntegrityLevel)`.
Use `CreateProcessAsUserW` with that primary token, `STARTUPINFOEXW`,
`EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT`,
a minimal environment, and an explicit executable path. Do not request elevation.
[Restricted-token API](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-createrestrictedtoken).

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
Use `TerminateJobObject` on timeout, then wait with `WaitForSingleObject` and
verify emptiness. Reserve shared sections outside the Job commit limit.
[Job memory limits](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_extended_limit_information).

Set creation-time `PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY` for DEP, mandatory
ASLR, strict handle checks, extension-point disable, Win32k system-call disable,
prohibit dynamic code, and no remote/low-integrity image loading. Set
`PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY` to
`PROCESS_CREATION_CHILD_PROCESS_RESTRICTED`. Build with CFG and verify the
packaged MS1 DLLs. Use `GetProcessMitigationPolicy` for host inspection and
`SetProcessMitigationPolicy` for any bootstrap-only additions before parsing.
Do not require Microsoft-only signatures, which would reject our FFmpeg DLLs.
[Mitigation policies](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-setprocessmitigationpolicy).

LPAC is not Linux's fd-only filesystem model. Windows must read OS/runtime
images and permits private AppContainer storage. Keep that storage ephemeral,
quota/account it, and expose no broker for host file or registry writes. Test
that other user files, registry state, network, and sibling processes are
inaccessible. Do not promise that a helper cannot write its own private storage.
Unsupported LPAC, mitigations, token composition, or nested Job setup fails
closed. Prove this exact combined launch on native Windows before shipping.

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
only after verified cleanup. After a second crash, timeout, OOM, or protocol
violation, quarantine that source for the session and require explicit Retry or
Relink. No automatic retry for malformed output or sandbox failure. Explicit
retries have a one-second cooldown and at most three launches per minute.
The supervisor retains the reservation until the process is reaped and native
membership is empty. Cleanup failure retains ownership and blocks replacement.
Parent loss closes the lifetime pipe; the native supervisor kills the helper.

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
| [`landlock`](https://docs.rs/landlock/latest/landlock/) | Add a pinned direct Linux dependency. Safe ruleset construction avoids new handwritten syscall layouts. Require enforced policy, not best-effort degradation. |
| [`seccompiler`](https://docs.rs/seccompiler/latest/seccompiler/) | Add a pinned Linux dependency, with JSON policy loading disabled. Typed argument rules compile the reviewed policy to BPF; the policy remains our responsibility. |
| [`extrasafe`](https://docs.rs/extrasafe/latest/extrasafe/) | Decline. It combines syscall, Landlock, and isolation helpers, but a second policy abstraction obscures our exact descriptor/argument rules. |
| [`birdcage`](https://docs.rs/birdcage/latest/birdcage/) | Decline. Its Linux/macOS process sandbox does not supply Windows confinement and duplicates launch ownership. |
| [`nix`](https://docs.rs/crate/nix/latest) | Decline as a new dependency. Existing job-sys uses rustix for these Unix operations. |
| [`windows`](https://microsoft.github.io/windows-docs-rs/doc/windows/) | Useful typed Win32 bindings, but extend the existing pinned `windows-sys` features in job-sys instead of introducing another binding layer. |

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
  Remove each protection in a disposable negative-control build and require
  its targeted witness to fail. No real credentials or host policy changes.
- Probe bootstrap ordering with a helper that attempts each attack before its
  first request. Inspect every inherited descriptor/handle and token/namespace
  property independently. Test denied namespaces, absent Landlock, missing
  controllers, invalid mitigations, and corrupted helper/library bundles. All
  must refuse before parsing. AppContainer-local writes are an allowed exception
  to the generic write test, bounded and removed on cleanup.
- Fuzz the host's actual framing/parser/state validator with cargo-fuzz/libFuzzer
  and sanitizers. Include truncation, huge lengths, recursion, duplicate fields,
  integer wrap, stale IDs, NaN/Inf PCM, wrong formats, out-of-order messages,
  floods, ring overwrite during copy, and fake progress. Require no panic,
  out-of-bounds access, unbounded allocation, or project mutation.
- Inject access violations/SIGSEGV, abort, CPU spin, memory exhaustion, pipe EOF,
  parent/supervisor loss, and cancellation at each phase. Assert typed UI and
  agent errors, responsive Save/undo, identical saved project content after
  reopen, unchanged previous export, empty native membership, and bounded
  respawn. Leave a deliberately surviving child to prove cleanup blocks reuse.
- Compare colour/rotation/timestamp/sample-boundary output with baseline golden
  fixtures. Exercise all call sites above, including project admission, mixed
  timelines, exported-artifact checks, cache hits, and background analyses.

Use release binaries named by commit and SHA-256, the same MS1 FFmpeg artifact,
and local hashed H.264, HEVC 10-bit, AV1, still, malformed, and multi-hour audio
fixtures. Pair baseline versus sandbox at equal thread counts and frame sizes.
Do not wait for a quiet desktop. Record other load and run no build alongside
timing. Report cold startup separately from warm requests, p50/p95 over at least
30 launches/seeks, and five paired 60-second throughput lanes per video case.

| Gate | Proposed number to hit |
| --- | --- |
| Sustained decode plus transport, compared with identical software baseline | At least 90% of baseline fps; IPC copy p95 at most 4 ms/frame at 4K |
| Reference H.264 4K60, when equal-thread baseline reaches 60 fps | Sustain 60 fps; otherwise report baseline capacity separately from MS2 overhead |
| First frame, cold launch | Added p95 at most 150 ms; absolute p95 at most 500 ms on the reference H.264 fixture |
| Warm random seek | Added p95 at most 25 ms; absolute p95 at most 250 ms for GOP 60 and 1 s for GOP 250 |
| Waveform/loudness | At least 90% baseline samples/s; memory independent of recording duration |
| Resource use | Audio peak RSS at most 96 MiB; video at most 384 MiB; hard reservations and totals above always hold |
| Cancellation and kill | Acknowledge or terminate within 250 ms; native emptiness and reap within a further 2 s |
| Idle/leak test | After 100 open/seek/cancel/close cycles, host retained bytes within 8 MiB of start; zero helpers after idle expiry |

These targets may fail. Record failures and review the design or limits before
shipping; do not relabel an unmeasured estimate as evidence. Publish CPU time,
copy bandwidth, PSS/commit, and two-source contention as well as fps.

## Four build slices

1. Ship source waveform/loudness measurement through the helper on Linux and
   Windows. Implement the typed summary protocol, both confinement profiles,
   source-handle admission, budgets, supervision, and malicious-helper/crash
   witnesses. Stream measurements internally, return capped summaries only.
   No frame or PCM ring yet. No insecure fallback in this migrated operation.
2. Move every import/project probe and source thumbnail/still inspection into
   the helper. Return thumbnails as bounded raw-pixel chunks over control,
   total at most 256 KiB per thumbnail. Preserve source authority and colour
   refusals. Gate new-project and IX admission with hostile files and verify
   that these paths no longer invoke FFmpeg input APIs in the host.
3. Add shared-memory video and PCM rings. Move preview, source audio playback,
   timeline rendering, and export input reads. Preserve compositor/encode
   ownership, sample timing, cancellation, and output byte/colour contracts.
   Pass native failure tests, ring/parser fuzzing, and paired overhead gates.
4. Migrate recognition extraction, scene/beat/silence analysis, room tone,
   cache inspection, and all remaining decode callers. Bound long audio and
   cache retention. Delete direct input/decode APIs from the host dependency
   boundary. Complete the all-callers audit, two-platform escape tests, full
   affected-crate and dependent-crate gates, memory soak, and security review.

Each slice is a usable operation with its own security gate. Until slice 4,
unmigrated operations retain their existing exposure; do not advertise complete
media isolation. Partial rollout adds no runtime legacy mode or bypass switch.
MS1 remains required for the final package; IX must not assume that a merged
MS2 design or an early slice makes arbitrary interchange media safe.

## Open questions for review

1. Is fail-closed media support acceptable where Landlock, user namespaces,
   or delegated memory/pids controllers are unavailable? This design requires it.
2. Do measured 4K reference sets fit 512 MiB per helper and 1,536 MiB total,
   including current certified-buffer reservations, without unacceptable refusals?
3. Does native Windows prove restricted-token LPAC launch, inherited source
   handles, required mitigations, and per-launch profile cleanup together?
4. What explicit PCM amplitude limit preserves current supported recordings
   while preventing unsafe DSP arithmetic? Fix it in the wire contract before PCM.
5. Does two source-bound helpers with serialized same-source cursors meet real
   multicam/transition latency targets? Measure before increasing concurrency.
