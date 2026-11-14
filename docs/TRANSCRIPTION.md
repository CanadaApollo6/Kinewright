# Local recognition

Asset transcription and voice notes use NVIDIA NeMo-Speech.cpp 0.3.0 on the CPU.
The native runtime is built separately from pinned source; `source
scripts/setup-ffmpeg.sh` (Linux) or `scripts/setup-ffmpeg.ps1` (Windows) provisions
it before cargo. See [BUILDING.md](BUILDING.md). AVX2, FMA, F16C, BMI2 and SSE4.2
are required. Unsupported CPUs receive a typed error before model creation.

## Model and licence

No language hint, an empty hint, or English (`en`, `eng`, `english`, including regional tags) uses
**NVIDIA Nemotron Speech Streaming EN 0.6B**. First-use asset
transcription downloads its 699,872,960-byte Q8 GGUF from NVIDIA's Hugging Face
repository, revision `ebe59e5a817142986528bbbee5dba8db7b38ed50`.
SHA-256: `d9a01898d2a611c8764e23a1c2f45e70bbd5a425dc4de93692ac951dd603812d`.
The authoritative file URL, size, digest, licence and notice live in
`recognizer::ENGLISH_MODEL` in `kinewright-media`.

Licensed by NVIDIA Corporation under the
[NVIDIA Open Model License](https://www.nvidia.com/en-us/agreements/enterprise-software/nvidia-open-model-license/).
Runtime Apache-2.0 and third-party notices are in `LICENSES/nemo-speech/`;
model notices and the AMI test-clip attribution are in `LICENSES/`.

A supported non-English language hint uses **NVIDIA Parakeet TDT 0.6B v3**, offline.
Its [pinned model card](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3/blob/541d1f99c6b0c3cd0b11a95167540bb8edefd82b/README.md)
lists `en`, `es`, `fr`, `de`, `bg`, `hr`, `cs`, `da`, `nl`, `et`, `fi`, `el`, `hu`,
`it`, `lv`, `lt`, `mt`, `pl`, `pt`, `ro`, `sk`, `sl`, `sv`, `ru` and `uk`.
The primary subtag is compared without ASCII case after splitting on `-` or `_`.
Other separators and unsupported codes return `UnsupportedLanguage` before any
download or inference. No language detection runs. Its 713,975,456-byte Q8 GGUF is pinned to NVIDIA revision
`541d1f99c6b0c3cd0b11a95167540bb8edefd82b`, SHA-256
`e3880d0aaaaf2c308ea2c35016b2b895c423eb3fda924c1b463d1c19b7f4d32e`.
The file, attribution and [CC-BY-4.0 licence](https://creativecommons.org/licenses/by/4.0/)
are described in `recognizer::PARAKEET_MODEL` and `LICENSES/NVIDIA-models-NOTICE.txt`.

Models, media caches, recordings, investigator settings, recovery journals and
feedback captures share one root. Linux data lives in `$XDG_DATA_HOME/kinewright`
when `XDG_DATA_HOME` is absolute,
otherwise `$HOME/.local/share/kinewright`. The root is owned by the current user
and created with mode 0700. A linked or foreign-owned root is refused. Windows
uses `%LOCALAPPDATA%/Kinewright`. Missing home configuration is a typed error;
there is no temporary-directory fallback or migration from old temporary data.

Weights are installed under the selected data directory's
`models/<model-id>/`. Installation exclusively creates an unpredictable partial file,
checks its size and SHA-256, then renames it atomically. Failed and cancelled
installs remove their partial file. A successful verification receipt records
file identity, length, modification time and native change time. An unchanged
file reuses that receipt without hashing 700 MB on every start. Changed metadata
forces a new hash; missing, short or altered weights are refused before native
loading and asset jobs download a verified replacement. Windows retains a handle
that excludes writes and deletion while the weights are loaded. Linux retains
the verified inode and checks its stamp and pathname before each session; the
private model cache must not be edited during inference.

## Sessions, clocks and memory

The public `Recognizer` creates a borrowed `Session`: feed finite mono 16 kHz
samples, step, commit at the fed source-clock position, then finish and drain.
Feeds contain at most 2,560 samples (160 ms). EN uses explicit lookahead 13,
verbatim output, punctuation and at most four CPU threads. Parakeet exposes
its offline capability through the same session interface: it retains one piece,
then decodes on the step after commit or finish. It has no streaming lookahead.
Both adapters use the same configured feed size, pause policy and source clock.

Asset decoding retains about 65 seconds of PCM. A private pause finder measures
10 ms mean-square levels and chooses the quietest 300 ms span within five seconds
of the next 60-second boundary. One streaming session continues across these
commits. Parakeet decodes each bounded piece separately. The streaming adapter
normalises native stream-global times to window-relative times; the offline
adapter returns piece-relative times. The session adds the window epoch exactly once. A commit never resets the
source clock. Word times and audio progress are integer microseconds. Recognition
is tagged `TokenDurationsLagged`: token-duration starts lag sound by about
30–250 ms and scatter. **These times are not measured cut edges.** No confidence
or speaker information is invented by the recogniser.

A process-wide serial worker serves assets and voice notes. Queued work reuses
one resident recogniser. Switching models drops the old model before loading
the next one. Same-model reuse also requires the same identity and a valid file.
When the queue is empty, it destroys its stream and model before delivering the last result, then waits without
loaded weights. A recognition job panic returns a typed failure, discards the model and
allows the next queued job to run. The transcript job worker also fails a panicking
job and continues with its queued successor. The
process retains its idle worker, transcript words, allocator capacity and native
thread-pool state; sample and model mappings are released. Cancellation and media
authority revocation are checked between steps and close the stream. A native
step already in progress completes before cancellation can be observed. For
Parakeet this can be the entire roughly 60-second piece; cancellation is checked
again before its result is published.

## Cache and voice notes

Exact microsecond recognition and its timing source are stored under
`transcripts/v4` and `voice-transcripts/v4`. Keys include source SHA-256, frame
rate, duration, language request and the full recogniser identity: model id and
file hash, runtime version, native build key, threads, lookahead, feed size,
punctuation/verbatim settings and pause policy. The identity records the fixed
four-thread configuration; only the native call clamps it to machine capacity.
Changing the requested model replaces a ready result and cancels an older request.
Live request entries are retained until the worker finishes them. Only completed
statuses count toward the 4,096-entry eviction cap; unknown tokens cannot publish.
Earlier derived transcripts are
recomputed. Only the editor adapter converts words to source frames (floor start,
ceil end). Original spelling is displayed; comparisons normalise case and
punctuation through `normalize_transcript_word`.

Voice notes use the same worker and cache recipe. They remain bounded by the
existing recording limits. Silence needs no model. Missing or altered weights
return `AwaitingModel`; explicit preparation uses the same installer. Inference
does not download a model while handling a voice note. Native word intervals may
overlap; voice validation requires ordered starts and ends and valid bounds.

## Real-model verification

The fast tier runs scripted sessions, queue ordering, cache and provisioning
witnesses without downloading weights. Explicit real tests are ignored by
default and registered in `ci/ignored-tests.txt`. Set `KINEWRIGHT_ASR_MODEL_DIR`
to the directory containing the pinned GGUF, then run:

```sh
cargo test -p kinewright-media --lib recognizer::real::dq1_real_ami_short_keeps_um_and_known_words -- --exact --ignored --nocapture
```

The committed 12-second AMI clip must keep known words and “um”; selected starts
allow 800 ms across builds. The slow CI recognition job runs it and a 72-second repetition that crosses a
commit on Linux and Windows, with hash-checked, cached EN weights. CI keeps the
second model out of the 30-minute job. The matching Parakeet short-clip witness
uses `KINEWRIGHT_PARAKEET_MODEL_DIR` and is explicitly invoked on demand. Additional on-demand tests take
`KINEWRIGHT_ASR_AMI8` and `KINEWRIGHT_ASR_AMI30` and compare whole versus committed
recognition within one build, measure steps, and check cancellation and release.

For Parakeet, run `recognizer::real::dq1_real_parakeet_ami_short_keeps_um_and_known_words`
with `--exact --ignored --nocapture`. Its AMI8 and AMI30 measurement tests use the
same fixture variables above. These tests always use explicit model and fixture
paths and temporary verification receipts.

Links above the chosen data or explicit model root are resolved through its parent;
the root itself and entries below it must be real entries, never links or reparse points.
The containing directory is trusted; Kinewright defends the root and its children.
On Linux, missing parents are created 0700 and existing parents keep their modes;
the application data root and its child directories must belong to the user and are
made 0700, including existing directories.

Recovery journals use `recovery/`, recordings use `recordings/<project>/`, and
investigator settings use `investigator.json` under that root. The app reuses the
media engine's resolved root. Missing storage stops a recording with a readable
error; unavailable investigator storage keeps settings in memory with a notice.
Cache reads, writes, inventory and clear check their original root and every
created directory component again, including after a family directory is replaced.
No data is read or migrated from the former temporary locations.

## Acoustic evidence

For media with a completed content identity, the transcript background job also
builds a Core `SoundProfile`. It streams every source-rate sample and channel
through `ProfileBuilder` for 10 ms peak and RMS hops, before downmixing. Earshot
1.2.2 receives the decoder's same 16 kHz mono chunks as recognition, in 256-sample
frames. A score greater than 0.5 marks that frame as voiced; every overlapping
profile hop is voiced. Only the detector's final frame is zero-padded, and its
interval ends at the real recording duration. An unavailable or failed detector
is `NoVoiceInformation`, never a profile claiming no voice. Job cancellation,
panics and status use the transcript worker's existing boundaries.

Profiles live under `derived-analysis/v1/sound-profiles/v1/` in the application
data root. The existing derived-analysis inventory and scoped clear include them. Their keys
include the content SHA-256 and byte length, selected audio stream, source-level
measurement version, Core hop, detector version, rate, frame size and threshold.
They contain no recogniser identity. A transcript cache hit fills a missing
profile without loading a model; switching recognition models reuses the profile.
Media without a completed content hash is not measured. The stream selector is
shared with recognition, export and the live mixer.

The implementation holds hop output, voice intervals, one decoded frame and a
256-sample detector carry, rather than the recording's PCM. Earshot's network is
built into its pinned Rust crate, with no separate model download. Its MIT licence
is included in [LICENSES/earshot-MIT.txt](../LICENSES/earshot-MIT.txt).
