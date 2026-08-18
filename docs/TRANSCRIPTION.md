# Local transcription assets

Kinewright M6 uses `whisper-rs` 0.16.0 with its default CPU-only whisper.cpp
build. It does not enable CUDA, Vulkan, Metal, or other GPU inference features.

On the first transcription, Kinewright downloads this pinned multilingual model:

- Model: OpenAI Whisper `small`, converted to GGML as `ggml-small.bin`
- Revision: `90a64d80ea254cf67575b41a5971f972c79f7b45`
- URL: `https://huggingface.co/ggerganov/whisper.cpp/resolve/90a64d80ea254cf67575b41a5971f972c79f7b45/ggml-small.bin?download=true`
- SHA-256: `1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b`
- Download size: approximately 488 MB
- Model license: MIT, as declared by the `ggerganov/whisper.cpp` model repository
- `whisper-rs` license: Unlicense

The model is verified before installation under
`%LOCALAPPDATA%\Kinewright\models\whisper`. Transcript cache records live under
`%LOCALAPPDATA%\Kinewright\transcripts\v2` and are keyed by the source file's
SHA-256. A cache record also pins the model SHA-256 and source frame rate, so a
model change invalidates old derived data without touching the project file or
operation log.

After the first verified model download, transcription and cache reuse are
offline. Only explicit media import/project-open scheduling can start a
transcription job; ordinary playback and inspector calls do not download a
model unless they request a missing transcript.

Callers that know the source language can provide an explicit language hint.
The editorial benchmark uses `en` so independent output verification does not
spend work on language detection; ordinary application transcription keeps
Whisper's multilingual auto-detection.

Kinewright does not install `whisper-rs` 0.16's safe abort callback. Its erased
closure wrapper can be invoked through the wrong concrete pointer type and was
observed to abort healthy Windows inference. Cancellation remains checked
before and after synchronous inference and before cache publication. A request
made during inference therefore becomes terminal at the next safe boundary.
