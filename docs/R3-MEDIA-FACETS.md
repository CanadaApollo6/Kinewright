# R3 MediaEngine facet split

R3 resolves audit findings F2 and F3. The R2 `main` contract had grown to 20
`MediaEngine` methods since the audit's 17-method count, plus three visual-asset
methods available only on `FfmpegMediaEngine`. R3 replaces that surface with
three core-owned facets and moves the pure visual result types into
`kinewright-core`.

## Facet contracts

```rust
pub trait Playback: Send + Sync {
    fn set_document(&self, doc: Arc<Document>);
    fn request_frame(&self, t: TimeCode);
    fn frames(&self) -> Receiver<(TimeCode, FrameTexture)>;
    fn events(&self) -> Receiver<MediaEvent>;
    fn play(&self, from: TimeCode);
    fn pause(&self);
    fn seek(&self, to: TimeCode);
    fn position(&self) -> TimeCode;
}

pub trait Analysis: Send + Sync {
    fn probe(&self, path: &Path) -> Result<MediaAsset, MediaError>;
    fn thumbnail_at(&self, t: TimeCode, max_w: u32) -> Result<RgbaImage, MediaError>;
    fn request_transcription(&self, asset: MediaAsset);
    fn transcript_status(&self, asset: &MediaAsset) -> TranscriptStatus;
    fn timeline_transcript(
        &self,
        document: &Document,
        range: Option<std::ops::Range<TimeCode>>,
    ) -> Result<Vec<TimelineTranscriptWord>, MediaError>;
    fn request_silence_detection(&self, asset: MediaAsset);
    fn silence_status(&self, asset: &MediaAsset) -> SilenceStatus;
    fn timeline_silences(
        &self,
        document: &Document,
        range: Option<std::ops::Range<TimeCode>>,
        minimum_source_frames: TimeCode,
    ) -> Result<Vec<TimelineSilenceSpan>, MediaError>;
    fn request_scene_detection(&self, asset: MediaAsset);
    fn scene_status(&self, asset: &MediaAsset) -> SceneStatus;
    fn timeline_scene_changes(
        &self,
        document: &Document,
        range: Option<std::ops::Range<TimeCode>>,
        minimum_confidence_basis_points: u16,
    ) -> Result<Vec<TimelineSceneChange>, MediaError>;
    fn request_waveform(&self, asset: MediaAsset) -> bool;
    fn request_thumbnail(
        &self,
        asset: MediaAsset,
        source_at: TimeCode,
        max_width: u32,
    ) -> bool;
    fn visual_asset_results(&self) -> Receiver<VisualAssetResult>;
}

pub trait Export: Send + Sync {
    fn export(
        &self,
        out: &Path,
        settings: ExportSettings,
        progress: ProgressSink,
    ) -> Result<(), MediaError>;
}
```

`request_waveform` and `request_thumbnail` join `visual_asset_results` on
`Analysis`. All three were concrete-only, and leaving either request method on
`FfmpegMediaEngine` would keep timeline and media-bin consumers concrete.

There is no `MediaEngine` umbrella trait. The composition root already coerces
one `Arc<FfmpegMediaEngine>` into the three facet objects once. An umbrella
would add no wiring value and would preserve an attractive path back to the fat
dependency.

## Consumer narrowing

| Consumer | Dependency after R3 | Actual use |
|---|---|---|
| App composition root | Concrete once, then `Arc<dyn Playback>`, `Arc<dyn Analysis>`, `Arc<dyn Export>` | Constructs FFmpeg and distributes facets |
| Playback, transport, keys | `Playback` | Frames, events, transport, seek, position, document updates |
| Media bin, transcript UI, timeline visuals | `Analysis` | Probe, derived analysis, waveforms, thumbnails |
| Export dialog worker | `Export` | Export only |
| MCP server | `Playback` + `Analysis` | `set_document` after edits; probe, frame inspection, and derived analysis |
| Media test helpers | `Analysis`, `Playback`, or `Export` as used | No helper parameter requires the concrete engine |

The app no longer stores `Arc<FfmpegMediaEngine>`. Its only concrete reference
is the private composition-root constructor parameter.

## Signature changes

- Removed `kinewright_core::MediaEngine` and its single 20-method implementation
  requirement. Added `kinewright_core::{Playback, Analysis, Export}`.
- `FfmpegMediaEngine` now implements all three facets separately.
- Removed the inherent `FfmpegMediaEngine::{request_waveform,
  request_thumbnail, visual_asset_results}` methods. Their signatures are
  unchanged on `Analysis`, so callers now import that trait.
- Moved `WaveformPeak`, `WaveformData`, `ThumbnailKey`, `ThumbnailFrame`,
  `VisualRequestKind`, and `VisualAssetResult` from `kinewright-media` ownership
  to `kinewright-core`. `kinewright-media` re-exports them for source compatibility.
- Changed `McpServer::start(core, media: Arc<dyn MediaEngine>)` to
  `McpServer::start(core, playback: Arc<dyn Playback>, analysis: Arc<dyn Analysis>)`.
- Changed `kinewright_media::test_support::wait_for_transcript(
  &FfmpegMediaEngine, AssetId, bool)` to
  `wait_for_transcript(&dyn Analysis, AssetId, bool)`.

The MCP `NoopMedia` test double no longer implements the unused `export`
method or imports `ExportSettings` and `ProgressSink`. It implements only the
two facets accepted by the server. No export test double is needed.
