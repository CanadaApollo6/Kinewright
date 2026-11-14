# Local recognition

Asset transcription and voice notes use NVIDIA NeMo-Speech.cpp 0.3.0 on the CPU.
The native runtime is built separately from pinned source; `source
scripts/setup-ffmpeg.sh` (Linux) or `scripts/setup-ffmpeg.ps1` (Windows) provisions
it before cargo. See [BUILDING.md](BUILDING.md). AVX2, FMA, F16C, BMI2 and SSE4.2
are required. Unsupported CPUs receive a typed error before model creation.

## Model and licence

DQ1 S2 uses **NVIDIA Nemotron Speech Streaming EN 0.6B**. First-use asset
transcription downloads its 699,872,960-byte Q8 GGUF from NVIDIA's Hugging Face
repository, revision `ebe59e5a817142986528bbbee5dba8db7b38ed50`.
SHA-256: `d9a01898d2a611c8764e23a1c2f45e70bbd5a425dc4de93692ac951dd603812d`.
The authoritative file URL, size, digest, licence and notice live in
`recognizer::ENGLISH_MODEL` in `kinewright-media`.

Licensed by NVIDIA Corporation under the
[NVIDIA Open Model License](https://www.nvidia.com/en-us/agreements/enterprise-software/nvidia-open-model-license/).
Runtime Apache-2.0 and third-party notices are in `LICENSES/nemo-speech/`;
model notices and the AMI test-clip attribution are in `LICENSES/`.

Weights are installed under the selected data directory's
`models/nemotron-speech-streaming-en-0.6b/`. Installation writes a partial file,
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
verbatim output, punctuation and at most four CPU threads. The offline-only
Parakeet model is described as data and a capability; its adapter and language
routing are DQ1 S3. S2 accepts no hint or `en`; it does not detect language.

Asset decoding retains about 65 seconds of PCM. A private pause finder measures
10 ms mean-square levels and chooses the quietest 300 ms span within five seconds
of the next 60-second boundary. One streaming session continues across these
commits. The adapter normalises native stream-global times to window-relative
times; the session adds the window epoch exactly once. A commit never resets the
source clock. Word times and audio progress are integer microseconds. Recognition
is tagged `TokenDurationsLagged`: token-duration starts lag sound by about
30–250 ms and scatter. **These times are not measured cut edges.** No confidence
or speaker information is invented by the recogniser.

A process-wide serial worker serves assets and voice notes. Queued work reuses
one loaded model. When the queue is empty, it destroys the stream and model
before delivering the last result, then waits without loaded weights. The
process retains its idle worker, transcript words, allocator capacity and native
thread-pool state; sample and model mappings are released. Cancellation and media
authority revocation are checked between steps and close the stream. A native
step already in progress completes before cancellation can be observed.

## Cache and voice notes

Exact microsecond recognition and its timing source are stored under
`transcripts/v4` and `voice-transcripts/v4`. Keys include source SHA-256, frame
rate, duration, language request and the full recogniser identity: model id and
file hash, runtime version, native build key, threads, lookahead, feed size,
punctuation/verbatim settings and pause policy. Earlier derived transcripts are
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
allow 800 ms across builds. The slow CI recognition job runs it on Linux and
Windows, with hash-checked, cached weights. Additional on-demand tests take
`KINEWRIGHT_ASR_AMI8` and `KINEWRIGHT_ASR_AMI30` and compare whole versus committed
recognition within one build, measure steps, and check cancellation and release.
