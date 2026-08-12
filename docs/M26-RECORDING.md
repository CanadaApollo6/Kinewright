# M26 — In-Editor Recording

Record the screen, a camera, or a voiceover without leaving OpenReel, and
have the result land on the timeline ready for agentic editing. This closes
the Descript loop: record → transcribe → edit by text — all in one place.

## How it works

Capture runs the **bundled FFmpeg CLI as a subprocess** — the same
drive-the-installed-tool pattern the agent harnesses use. That choice buys:

- **Crash isolation.** A capture failure can never take the editor (or an
  in-flight edit session) down. FFmpeg dying unexpectedly is detected, the
  error points at a per-recording log file, and whatever was written is
  salvaged into the import pipeline.
- **A graceful stop.** Stopping writes FFmpeg's interactive `q` to stdin, so
  the container gets a valid trailer; a kill is only the timeout fallback.
- **No new linkage.** The media engine's library surface is untouched;
  recording is app-level workflow around a subprocess.

Sources (v1):

| Source | FFmpeg inputs | Output |
| --- | --- | --- |
| Screen | `gdigrab` desktop at 30 fps, optional dshow microphone | `Recording N.mp4` |
| Camera | one dshow input binding `video=…:audio=…` | `Recording N.mp4` |
| Voice | dshow microphone only | `Recording N.m4a` |

Video encodes with `libx264 -preset ultrafast` (capture must never contend
with the machine being recorded) plus an even-dimension crop guard for
`yuv420p`. Device names come from FFmpeg's own `-list_devices` output,
parsed from stderr (where DirectShow reports it, by design).

Recordings save to `Videos\OpenReel` under the user profile — user-visible,
ordinary files. Names count upward (`Recording 1.mp4`, `Recording 2.mp4`);
no timestamps to squint at.

## One gesture, end to end

Stopping a recording feeds the finished file to the exact pipeline imports
use: probe → media pool → **auto-add to the timeline** → monitor cues to
the first frame → transcription starts. By the time you've watched your
take back, the transcript is arriving and the words are cuttable.

## Trust boundaries

- **The agent has no capture tool.** Recording is a human act; the MCP tool
  surface gains nothing from this milestone. An agent can edit a recording
  the moment it exists, but can never cause one.
- Everything is local files; nothing leaves the machine.

## Packaging

`ffmpeg.exe` now stages beside `OpenReel.exe` and ships in the installer
(the GPL FFmpeg build was already bundled as DLLs; recording needs the CLI
too). At runtime the CLI resolves: `OPENREEL_FFMPEG` override → beside the
executable (installed layout) → `third_party\ffmpeg\bin` above the
executable (dev checkouts) → PATH.

## Deferred

- Single-monitor / window / region capture — `gdigrab desktop` grabs the
  whole desktop, which spans all monitors on multi-display setups.
- System-audio (loopback) capture — Windows needs a WASAPI loopback route
  that dshow alone does not offer cleanly.
- Simultaneous screen + camera (picture-in-picture) capture.
- A camera preview inside the record dialog.
- Pause/resume within one recording.
